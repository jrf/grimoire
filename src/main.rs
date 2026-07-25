mod config;
mod export;
mod fetch;
mod import_polaris;
mod index;
mod metadata;
mod model;
mod storage;
mod theme;
mod tui;
mod validate;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use config::Config;

#[derive(Parser)]
#[command(name = "grimoire", about = "A fast TUI reference manager")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Search query (pre-fills TUI filter)
    #[arg(global = false)]
    query: Vec<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Import a PDF, DOI, arXiv ID, or URL into the library
    Add {
        /// Path to PDF file, DOI, arXiv ID, or URL
        path: String,
    },
    /// Pick a reference and output its citation key
    Cite {
        /// Output format: plain (default), latex, typst
        #[arg(short, long, default_value = "plain")]
        format: String,
    },
    /// Export all references to stdout (yaml, json, bibtex, or hayagriva)
    Export {
        /// Output format: yaml, json, bibtex, hayagriva
        #[arg(short, long)]
        format: String,
    },
    /// Rebuild the search index from filesystem
    Reindex,
    /// Import papers from a Polaris library
    ImportPolaris {
        /// Overwrite metadata for existing entries
        #[arg(short, long)]
        force: bool,
    },
    /// Validate library integrity (missing PDFs, junk files, temp names)
    Validate {
        /// Automatically fix issues (rename temp files, remove non-PDFs)
        #[arg(short, long)]
        fix: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load()?;
    let library = config.library_dir();

    match cli.command {
        None => {
            let initial = if cli.query.is_empty() {
                None
            } else {
                Some(cli.query.join(" "))
            };
            tui::browse(&config, &library, initial.as_deref())
        }
        Some(Command::Add { path }) => cmd_add(&library, &path),
        Some(Command::Cite { format }) => tui::cite(&config, &library, &format),
        Some(Command::Export { format }) => export::run(&library, &format),
        Some(Command::Reindex) => cmd_reindex(&library),
        Some(Command::ImportPolaris { force }) => import_polaris::run(&library, force),
        Some(Command::Validate { fix }) => validate::run(&library, fix),
    }
}

pub fn cmd_add(library: &Path, input: &str) -> Result<()> {
    std::fs::create_dir_all(library)?;

    let path = PathBuf::from(input);
    if path.exists() {
        return add_from_file(library, input);
    }

    if let Some(arxiv_id) = fetch::detect_arxiv_id(input) {
        return add_from_arxiv(library, &arxiv_id);
    }

    if let Some(doi) = fetch::detect_doi(input) {
        return add_from_doi(library, &doi);
    }

    if input.starts_with("http://") || input.starts_with("https://") {
        return add_from_url(library, input);
    }

    anyhow::bail!("Not a file, URL, arXiv ID, or DOI: {}", input)
}

pub fn index_reference(library: &Path, ref_dir: &Path, reference: &crate::model::Reference) {
    if let Ok(idx) = index::Index::open(library) {
        let dir_name = ref_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let pdf_path = reference.files.first().map(|f| ref_dir.join(f));
        let fulltext = pdf_path
            .as_ref()
            .filter(|p| p.exists())
            .and_then(|p| metadata::extract_pdf_text(p));
        let _ = idx.upsert_with_fulltext(&dir_name, reference, fulltext.as_deref());
    }
}

fn add_from_arxiv(library: &Path, arxiv_id: &str) -> Result<()> {
    println!("Fetching metadata from arXiv: {}", arxiv_id);
    let mut reference = fetch::fetch_arxiv(arxiv_id)?;

    let ref_dir = storage::create_ref_dir(library, &reference)?;
    let pdf_filename = format!("{}.pdf", arxiv_id);
    let pdf_path = ref_dir.join(&pdf_filename);

    println!("Downloading PDF...");
    fetch::download_arxiv_pdf(arxiv_id, &pdf_path)?;

    reference.files = vec![pdf_filename];
    metadata::write_info(&ref_dir, &reference)?;
    index_reference(library, &ref_dir, &reference);

    println!("Added: {}", reference.title);
    println!("  → {}", ref_dir.display());
    Ok(())
}

fn add_from_doi(library: &Path, doi: &str) -> Result<()> {
    println!("Fetching metadata from CrossRef: {}", doi);
    let reference = fetch::fetch_crossref(doi)?;

    let ref_dir = storage::create_ref_dir(library, &reference)?;
    metadata::write_info(&ref_dir, &reference)?;
    index_reference(library, &ref_dir, &reference);

    println!("Added: {}", reference.title);
    println!("  → {}", ref_dir.display());
    println!("  (no PDF — add one manually to the directory)");
    Ok(())
}

fn add_from_file(library: &Path, path: &str) -> Result<()> {
    let path = PathBuf::from(path)
        .canonicalize()
        .with_context(|| format!("File not found: {}", path))?;

    anyhow::ensure!(
        path.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf")),
        "Not a PDF file and not a recognized arXiv ID or DOI"
    );

    let mut reference = metadata::extract_from_pdf(&path)?;

    let arxiv_id = path
        .file_stem()
        .and_then(|s| fetch::detect_arxiv_id(&s.to_string_lossy()));
    if let Some(ref id) = arxiv_id {
        println!("Detected arXiv ID: {} — fetching metadata...", id);
        if let Ok(fetched) = fetch::fetch_arxiv(id) {
            reference.title = fetched.title;
            reference.authors = fetched.authors;
            reference.year = fetched.year;
            reference.doi = fetched.doi;
            reference.arxiv = fetched.arxiv;
            reference.r#abstract = fetched.r#abstract;
        }
    }

    let ref_dir = storage::create_ref_dir(library, &reference)?;
    let filename = storage::copy_pdf(&path, &ref_dir)?;
    reference.files = vec![filename];
    metadata::write_info(&ref_dir, &reference)?;
    index_reference(library, &ref_dir, &reference);

    println!("Added: {}", reference.title);
    println!("  → {}", ref_dir.display());
    Ok(())
}

fn add_from_url(library: &Path, url: &str) -> Result<()> {
    println!("Downloading PDF from URL...");

    let response =
        reqwest::blocking::get(url).with_context(|| format!("Failed to download: {}", url))?;

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !content_type.contains("pdf") && !url.ends_with(".pdf") {
        eprintln!(
            "Warning: URL may not be a PDF (content-type: {})",
            content_type
        );
    }

    let filename = url
        .rsplit('/')
        .next()
        .unwrap_or("download")
        .split('?')
        .next()
        .unwrap_or("download");
    let filename = if filename.ends_with(".pdf") {
        filename.to_string()
    } else {
        format!("{}.pdf", filename)
    };

    let tmp_dir = tempfile::tempdir()?;
    let tmp_path = tmp_dir.path().join(&filename);
    let bytes = response.bytes()?;
    std::fs::write(&tmp_path, &bytes)?;

    add_from_file(library, tmp_path.to_str().unwrap())
}

fn cmd_reindex(library: &Path) -> Result<()> {
    let idx = index::Index::open(library)?;
    let count = idx.reindex(library)?;
    println!("Indexed {} references.", count);
    Ok(())
}
