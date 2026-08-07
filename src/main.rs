mod config;
mod export;
mod fetch;
mod index;
mod metadata;
mod model;
mod storage;
mod theme;
mod tui;
mod validate;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};

use config::Config;

#[derive(Parser)]
#[command(name = "grimoire", version, about = "A fast TUI reference manager")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Search query (pre-fills TUI filter)
    #[arg(global = false)]
    query: Vec<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Import one or more PDFs, DOIs, arXiv IDs, or URLs into the library
    Add {
        /// Paths to PDF files, DOIs, arXiv IDs, or URLs
        #[arg(required = true)]
        paths: Vec<String>,
        /// Import even if an entry with the same DOI or title already exists
        #[arg(short, long)]
        force: bool,
    },
    /// Pick a reference and output its citation key
    Cite {
        /// Output format: plain (default), latex, typst
        #[arg(short, long, default_value = "plain")]
        format: String,
    },
    /// Export references (yaml, json, bibtex, or hayagriva) to stdout or a file
    Export {
        /// Output format: yaml, json, bibtex, hayagriva
        #[arg(short, long)]
        format: String,
        /// Write to a file instead of stdout
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Only export references carrying this tag (repeatable; matches any)
        #[arg(short, long)]
        tag: Vec<String>,
    },
    /// Rebuild the search index from filesystem
    Reindex,
    /// Validate library integrity (missing PDFs, junk files, temp names)
    Validate {
        /// Automatically fix issues (rename temp files, remove non-PDFs)
        #[arg(short, long)]
        fix: bool,
    },
    /// Generate a shell completion script (bash, zsh, fish, elvish, powershell)
    Completions {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
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
        Some(Command::Add { paths, force }) => cmd_add_many(&library, &paths, force),
        Some(Command::Cite { format }) => tui::cite(&config, &library, &format),
        Some(Command::Export {
            format,
            output,
            tag,
        }) => export::run(&library, &format, output.as_deref(), &tag),
        Some(Command::Reindex) => cmd_reindex(&library),
        Some(Command::Validate { fix }) => validate::run(&library, fix),
        Some(Command::Completions { shell }) => {
            let mut cmd = Cli::command();
            let name = cmd.get_name().to_string();
            clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
            Ok(())
        }
    }
}

/// Import several inputs in one invocation. A failure on one input is reported
/// but does not abort the rest; the command exits non-zero if any failed.
pub fn cmd_add_many(library: &Path, inputs: &[String], force: bool) -> Result<()> {
    let mut failures = 0;
    for input in inputs {
        if let Err(e) = cmd_add(library, input, force) {
            eprintln!("error: failed to add {input}: {e}");
            failures += 1;
        }
    }
    if failures > 0 {
        anyhow::bail!("{failures} of {} input(s) failed", inputs.len());
    }
    Ok(())
}

pub fn cmd_add(library: &Path, input: &str, force: bool) -> Result<()> {
    std::fs::create_dir_all(library)?;

    let path = PathBuf::from(input);
    if path.exists() {
        return add_from_file(library, input, force);
    }

    if let Some(arxiv_id) = fetch::detect_arxiv_id(input) {
        return add_from_arxiv(library, &arxiv_id, force);
    }

    if let Some(pmc_id) = fetch::detect_pmc_id(input) {
        return add_from_pmc(library, &pmc_id, force);
    }

    if let Some(pmid) = fetch::detect_pmid(input) {
        return add_from_pubmed(library, &pmid, force);
    }

    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(doi) = fetch::detect_doi_url(input) {
            return add_from_doi(library, &doi, force);
        }
        return add_from_web_url(library, input, force);
    }

    if let Some(doi) = fetch::detect_doi(input) {
        return add_from_doi(library, &doi, force);
    }

    anyhow::bail!("Not a file, URL, arXiv ID, or DOI: {}", input)
}

/// Import from an arbitrary web URL that isn't a recognized arXiv/PMC/doi.org
/// link: try a DOI embedded in the URL, then the landing page's citation meta
/// tags, and finally fall back to treating the URL as a direct PDF.
fn add_from_web_url(library: &Path, url: &str, force: bool) -> Result<()> {
    if let Some(doi) = fetch::detect_doi_in_url(url) {
        return add_from_doi(library, &doi, force);
    }

    if let Ok(info) = fetch::resolve_landing_page(url) {
        if let Some(doi) = info.doi {
            println!("Resolved DOI from page: {doi}");
            return add_from_doi_with_pdf(library, &doi, info.pdf_url.as_deref(), force);
        }
        if let Some(pdf_url) = info.pdf_url {
            println!("Resolved PDF from page: {pdf_url}");
            return add_from_url(library, &pdf_url, force);
        }
    }

    add_from_url(library, url, force)
}

/// If `reference` duplicates an existing entry and the import isn't forced,
/// print a notice and return `true` (the caller should skip the import).
fn skip_as_duplicate(library: &Path, reference: &crate::model::Reference, force: bool) -> bool {
    if force {
        return false;
    }
    match storage::find_duplicate(library, reference) {
        Ok(Some((existing, reason))) => {
            let name = existing.file_name().unwrap_or_default().to_string_lossy();
            eprintln!("! duplicate of {name} ({reason} match) — skipping");
            eprintln!("  use --force to add anyway");
            true
        }
        _ => false,
    }
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

fn add_from_arxiv(library: &Path, arxiv_id: &str, force: bool) -> Result<()> {
    println!("Fetching metadata from arXiv: {}", arxiv_id);
    let mut reference = fetch::fetch_arxiv(arxiv_id)?;

    if skip_as_duplicate(library, &reference, force) {
        return Ok(());
    }

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

fn add_from_doi(library: &Path, doi: &str, force: bool) -> Result<()> {
    add_from_doi_with_pdf(library, doi, None, force)
}

fn add_from_pubmed(library: &Path, pmid: &str, force: bool) -> Result<()> {
    println!("Resolving PubMed {pmid} via NCBI...");
    let reference = fetch::fetch_pubmed(pmid)?;
    add_reference_with_pdf(library, reference, None, force)
}

/// Add a reference from a DOI (CrossRef metadata), optionally downloading a PDF
/// from `pdf_url` (e.g. a publisher's `citation_pdf_url`). A failed PDF download
/// is non-fatal — the metadata entry is still created.
fn add_from_doi_with_pdf(
    library: &Path,
    doi: &str,
    pdf_url: Option<&str>,
    force: bool,
) -> Result<()> {
    println!("Fetching metadata from CrossRef: {}", doi);
    let reference = fetch::fetch_crossref(doi)?;
    add_reference_with_pdf(library, reference, pdf_url, force)
}

fn add_reference_with_pdf(
    library: &Path,
    mut reference: crate::model::Reference,
    pdf_url: Option<&str>,
    force: bool,
) -> Result<()> {
    if skip_as_duplicate(library, &reference, force) {
        return Ok(());
    }

    let ref_dir = storage::create_ref_dir(library, &reference)?;

    if let Some(url) = pdf_url {
        let dir_name = ref_dir.file_name().unwrap_or_default().to_string_lossy();
        let pdf_filename = format!("{dir_name}.pdf");
        match fetch::download_pdf(url) {
            Ok(bytes) => {
                std::fs::write(ref_dir.join(&pdf_filename), bytes)
                    .with_context(|| format!("Failed to save PDF to {}", ref_dir.display()))?;
                reference.files = vec![pdf_filename];
            }
            Err(e) => eprintln!("  (metadata added; PDF download failed: {e})"),
        }
    }

    metadata::write_info(&ref_dir, &reference)?;
    index_reference(library, &ref_dir, &reference);

    println!("Added: {}", reference.title);
    println!("  → {}", ref_dir.display());
    if reference.files.is_empty() {
        println!("  (no PDF — add one manually to the directory)");
    }
    Ok(())
}

fn add_from_pmc(library: &Path, pmc_id: &str, force: bool) -> Result<()> {
    println!("Resolving PMC article and downloading PDF: {pmc_id}");
    let (mut reference, bytes) = fetch::fetch_pmc(pmc_id)?;

    if skip_as_duplicate(library, &reference, force) {
        return Ok(());
    }

    let ref_dir = storage::create_ref_dir(library, &reference)?;
    let pdf_filename = format!("{pmc_id}.pdf");
    std::fs::write(ref_dir.join(&pdf_filename), bytes)
        .with_context(|| format!("Failed to save PDF to {}", ref_dir.display()))?;

    reference.files = vec![pdf_filename];
    metadata::write_info(&ref_dir, &reference)?;
    index_reference(library, &ref_dir, &reference);

    println!("Added: {}", reference.title);
    println!("  → {}", ref_dir.display());
    Ok(())
}

fn add_from_file(library: &Path, path: &str, force: bool) -> Result<()> {
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

    if skip_as_duplicate(library, &reference, force) {
        return Ok(());
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

fn add_from_url(library: &Path, url: &str, force: bool) -> Result<()> {
    println!("Downloading PDF from URL...");
    let bytes = fetch::download_pdf(url)?;
    let filename = reqwest::Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed
                .path_segments()?
                .rev()
                .find(|segment| !segment.is_empty())
                .map(str::to_string)
        })
        .filter(|name| name.to_ascii_lowercase().ends_with(".pdf"))
        .unwrap_or_else(|| "download.pdf".to_string());

    let tmp_dir = tempfile::tempdir()?;
    let tmp_path = tmp_dir.path().join(&filename);
    std::fs::write(&tmp_path, bytes)?;

    add_from_file(library, tmp_path.to_str().unwrap(), force)
}

fn cmd_reindex(library: &Path) -> Result<()> {
    let idx = index::Index::open(library)?;
    let count = idx.reindex(library)?;
    println!("Indexed {} references.", count);
    Ok(())
}
