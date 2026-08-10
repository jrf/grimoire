mod backfill;
mod cli;
mod config;
mod dedup;
mod enrich;
mod export;
mod fetch;
mod index;
mod metadata;
mod model;
mod semantic;
mod storage;
mod theme;
mod tui;
mod validate;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use serde::Serialize;

use config::Config;

#[derive(Parser)]
#[command(name = "grimoire", version, about = "A fast TUI reference manager")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Emit a stable machine-readable response
    #[arg(long, global = true)]
    json: bool,

    /// Search query (pre-fills TUI filter)
    #[arg(global = false)]
    query: Vec<String>,
}

#[derive(Subcommand)]
enum Command {
    /// List references with optional text and tag filters
    List {
        /// Case-insensitive substring filter over key and metadata
        #[arg(short, long)]
        query: Option<String>,
        /// Only include references carrying one of these tags
        #[arg(short, long)]
        tag: Vec<String>,
        /// Maximum number of references to return
        #[arg(short, long)]
        limit: Option<usize>,
    },
    /// Show one reference by its library key
    Show { key: String },
    /// Search the full-text reference index
    Search {
        #[arg(required = true)]
        query: Vec<String>,
        #[arg(short, long)]
        limit: Option<usize>,
    },
    /// Print the local PDF path for a reference
    Path { key: String },
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
        /// Reference key; omit to use the interactive picker
        key: Option<String>,
        /// Output format: plain (default), latex, typst
        #[arg(short, long, default_value = "plain")]
        format: String,
    },
    /// Fill missing metadata for exact reference keys
    Enrich {
        /// Reference keys to enrich
        keys: Vec<String>,
        /// Enrich every reference in the library
        #[arg(long, conflicts_with = "keys")]
        all: bool,
        /// Write the proposed changes to info.toml
        #[arg(long)]
        apply: bool,
    },
    /// Update metadata for one exact reference key
    Update {
        key: String,
        #[arg(long)]
        title: Option<String>,
        /// Replace the author list (repeatable)
        #[arg(long = "author")]
        authors: Vec<String>,
        #[arg(long, conflicts_with = "authors")]
        clear_authors: bool,
        #[arg(long)]
        year: Option<u16>,
        #[arg(long, conflicts_with = "year")]
        clear_year: bool,
        #[arg(long)]
        doi: Option<String>,
        #[arg(long, conflicts_with = "doi")]
        clear_doi: bool,
        #[arg(long)]
        arxiv: Option<String>,
        #[arg(long, conflicts_with = "arxiv")]
        clear_arxiv: bool,
        #[arg(long)]
        journal: Option<String>,
        #[arg(long, conflicts_with = "journal")]
        clear_journal: bool,
        #[arg(long = "abstract")]
        abstract_text: Option<String>,
        #[arg(long, conflicts_with = "abstract_text")]
        clear_abstract: bool,
        #[arg(long = "add-tag")]
        add_tags: Vec<String>,
        #[arg(long = "remove-tag")]
        remove_tags: Vec<String>,
        /// Write the proposed changes to info.toml
        #[arg(long)]
        apply: bool,
    },
    /// Find duplicate groups, or move explicitly rejected entries to .trash
    Dedup {
        /// Key to retain from a duplicate group (repeatable; requires --apply)
        #[arg(long)]
        keep: Vec<String>,
        /// Move every other entry in selected groups to .trash
        #[arg(long)]
        apply: bool,
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
    /// Fetch missing PDFs and abstracts for existing entries (open-access only)
    Backfill {
        /// Only download missing PDFs (skip abstract/metadata fetches)
        #[arg(long, conflicts_with = "abstracts_only")]
        pdfs_only: bool,
        /// Only fetch missing abstracts (skip PDF downloads)
        #[arg(long)]
        abstracts_only: bool,
        /// Report what would be attempted without changing anything
        #[arg(long)]
        check: bool,
    },
    /// Rebuild the search index from filesystem
    Reindex,
    /// Build a local vector index from JSONL files under each paper's derived directory
    SemanticIndex {
        /// Re-embed every passage even when its source is unchanged
        #[arg(long)]
        force: bool,
    },
    /// Search indexed passages by semantic similarity
    Semantic {
        /// Natural-language search query
        #[arg(required = true)]
        query: Vec<String>,
        /// Number of passages to return (defaults to 100)
        #[arg(short, long, conflicts_with = "all")]
        limit: Option<usize>,
        /// Zero-based result offset
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// Return every result from the offset onward
        #[arg(long)]
        all: bool,
    },
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

fn main() {
    let cli = Cli::parse();
    let json = cli.json;
    if let Err(error) = run(cli) {
        if json {
            cli::print_json_error(&error);
        } else {
            eprintln!("Error: {error:#}");
        }
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let config = Config::load()?;
    let library = config.library_dir();

    match cli.command {
        None => {
            anyhow::ensure!(!cli.json, "--json requires a CLI command");
            let initial = if cli.query.is_empty() {
                None
            } else {
                Some(cli.query.join(" "))
            };
            tui::browse(&config, &library, initial.as_deref())
        }
        Some(Command::List { query, tag, limit }) => {
            let (records, warnings) = cli::list(&library, query.as_deref(), &tag, limit)?;
            if cli.json {
                cli::print_json_with_warnings(records, warnings)
            } else {
                for warning in warnings {
                    eprintln!("warning: {warning}");
                }
                for record in records {
                    println!(
                        "{}\t{}\t{}",
                        record.key,
                        record
                            .reference
                            .year
                            .map(|year| year.to_string())
                            .unwrap_or_default(),
                        record.reference.title
                    );
                }
                Ok(())
            }
        }
        Some(Command::Show { key }) => {
            let record = cli::show(&library, &key)?;
            if cli.json {
                cli::print_json(record)
            } else {
                println!("{}", toml::to_string_pretty(&record.reference)?);
                Ok(())
            }
        }
        Some(Command::Search { query, limit }) => {
            let index = index::Index::open(&library)?;
            let hits = index.search_with_limit(&query.join(" "), limit)?;
            if cli.json {
                cli::print_json(hits)
            } else {
                for hit in hits {
                    println!(
                        "{}\t{}\t{}",
                        hit.dir_name,
                        hit.year.map(|year| year.to_string()).unwrap_or_default(),
                        hit.title
                    );
                }
                Ok(())
            }
        }
        Some(Command::Path { key }) => {
            let record = cli::show(&library, &key)?;
            let pdf = record.pdf.context(format!("No PDF available for {key}"))?;
            if cli.json {
                cli::print_json(serde_json::json!({"key": key, "pdf": pdf}))
            } else {
                println!("{}", pdf.display());
                Ok(())
            }
        }
        Some(Command::Add { paths, force }) => {
            let report = cmd_add_many(&library, &paths, force)?;
            if cli.json {
                cli::print_json(report)
            } else {
                Ok(())
            }
        }
        Some(Command::Cite { key, format }) => {
            if let Some(key) = key {
                cli::reference_dir(&library, &key)?;
                let citation = cli::citation(&key, &format)?;
                if cli.json {
                    cli::print_json(
                        serde_json::json!({"key": key, "format": format, "citation": citation}),
                    )
                } else {
                    println!("{citation}");
                    Ok(())
                }
            } else {
                anyhow::ensure!(!cli.json, "cite --json requires a reference key");
                tui::cite(&config, &library, &format)
            }
        }
        Some(Command::Enrich { keys, all, apply }) => {
            anyhow::ensure!(
                all || !keys.is_empty(),
                "Provide reference keys or use --all"
            );
            let (selected, warnings) = if all {
                let (records, warnings) = cli::records(&library)?;
                (
                    records.into_iter().map(|record| record.key).collect(),
                    warnings,
                )
            } else {
                (keys, Vec::new())
            };
            let mut results = Vec::new();
            for key in selected {
                let record = cli::show(&library, &key)?;
                let updated = enrich::enrich_entry(&record.directory, &record.reference)?
                    .unwrap_or_else(|| record.reference.clone());
                let changes = cli::changes(&record.reference, &updated)?;
                if apply && !changes.is_empty() {
                    metadata::write_info(&record.directory, &updated)?;
                    index_reference(&library, &record.directory, &updated)?;
                }
                results.push(cli::MutationRecord {
                    key,
                    changed: !changes.is_empty(),
                    applied: apply && !changes.is_empty(),
                    changes,
                    reference: updated,
                });
            }
            if cli.json {
                cli::print_json_with_warnings(results, warnings)
            } else {
                for warning in warnings {
                    eprintln!("warning: {warning}");
                }
                print_mutations(&results, apply);
                Ok(())
            }
        }
        Some(Command::Update {
            key,
            title,
            authors,
            clear_authors,
            year,
            clear_year,
            doi,
            clear_doi,
            arxiv,
            clear_arxiv,
            journal,
            clear_journal,
            abstract_text,
            clear_abstract,
            add_tags,
            remove_tags,
            apply,
        }) => {
            let record = cli::show(&library, &key)?;
            let mut updated = record.reference.clone();
            let requested = title.is_some()
                || !authors.is_empty()
                || clear_authors
                || year.is_some()
                || clear_year
                || doi.is_some()
                || clear_doi
                || arxiv.is_some()
                || clear_arxiv
                || journal.is_some()
                || clear_journal
                || abstract_text.is_some()
                || clear_abstract
                || !add_tags.is_empty()
                || !remove_tags.is_empty();
            anyhow::ensure!(requested, "No metadata changes were requested");
            if let Some(value) = title {
                updated.title = value;
            }
            if !authors.is_empty() {
                updated.authors = authors;
            } else if clear_authors {
                updated.authors.clear();
            }
            if let Some(value) = year {
                updated.year = Some(value);
            } else if clear_year {
                updated.year = None;
            }
            if let Some(value) = doi {
                updated.doi = Some(value);
            } else if clear_doi {
                updated.doi = None;
            }
            if let Some(value) = arxiv {
                updated.arxiv = Some(value);
            } else if clear_arxiv {
                updated.arxiv = None;
            }
            if let Some(value) = journal {
                updated.journal = Some(value);
            } else if clear_journal {
                updated.journal = None;
            }
            if let Some(value) = abstract_text {
                updated.r#abstract = Some(value);
            } else if clear_abstract {
                updated.r#abstract = None;
            }
            updated.tags.retain(|tag| {
                !remove_tags
                    .iter()
                    .any(|removed| removed.eq_ignore_ascii_case(tag))
            });
            for tag in add_tags {
                if !updated
                    .tags
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(&tag))
                {
                    updated.tags.push(tag);
                }
            }
            let changes = cli::changes(&record.reference, &updated)?;
            let changed = !changes.is_empty();
            if apply && changed {
                metadata::write_info(&record.directory, &updated)?;
                index_reference(&library, &record.directory, &updated)?;
            }
            let result = cli::MutationRecord {
                key,
                changed,
                applied: apply && changed,
                changes,
                reference: updated,
            };
            if cli.json {
                cli::print_json(result)
            } else {
                print_mutations(std::slice::from_ref(&result), apply);
                Ok(())
            }
        }
        Some(Command::Dedup { keep, apply }) => {
            let report = dedup::run(&library, &keep, apply)?;
            if apply && !report.removed.is_empty() {
                index::Index::open(&library)?.reindex(&library)?;
            }
            if cli.json {
                cli::print_json(report)
            } else {
                println!("{} duplicate group(s)", report.groups.len());
                for group in &report.groups {
                    println!(
                        "  {}",
                        group
                            .candidates
                            .iter()
                            .map(|item| item.key.as_str())
                            .collect::<Vec<_>>()
                            .join(" | ")
                    );
                }
                if report.removed.is_empty() {
                    println!("No changes applied.");
                } else {
                    println!("Moved {} reference(s) to .trash.", report.removed.len());
                }
                Ok(())
            }
        }
        Some(Command::Export {
            format,
            output,
            tag,
        }) => {
            anyhow::ensure!(
                !cli.json,
                "Global --json is not valid for export; use `export --format json`"
            );
            export::run(&library, &format, output.as_deref(), &tag)
        }
        Some(Command::Backfill {
            pdfs_only,
            abstracts_only,
            check,
        }) => {
            let report = backfill::run(
                &library,
                &backfill::Options {
                    pdfs: !abstracts_only,
                    abstracts: !pdfs_only,
                    check,
                    quiet: cli.json,
                },
            )?;
            if cli.json {
                cli::print_json(report)
            } else {
                Ok(())
            }
        }
        Some(Command::Reindex) => {
            let count = index::Index::open(&library)?.reindex(&library)?;
            if cli.json {
                cli::print_json(serde_json::json!({"indexed": count}))
            } else {
                println!("Indexed {count} references.");
                Ok(())
            }
        }
        Some(Command::SemanticIndex { force }) => {
            let summary = semantic::build(&library, &config.embedding, force, cli.json)?;
            if cli.json {
                cli::print_json(summary)
            } else {
                Ok(())
            }
        }
        Some(Command::Semantic {
            query,
            limit,
            offset,
            all,
        }) => {
            let query = query.join(" ");
            let ranking = semantic::rank(&library, &query, &config.embedding)?;
            anyhow::ensure!(
                offset <= ranking.total(),
                "Semantic search offset {offset} exceeds {} results",
                ranking.total()
            );
            let page_limit = if all {
                ranking.total().saturating_sub(offset).max(1)
            } else {
                limit.unwrap_or(semantic::DEFAULT_PAGE_SIZE)
            };
            let page = ranking.page(&library, offset, page_limit)?;
            if cli.json {
                cli::print_json(page)
            } else {
                semantic::print_page(&page);
                Ok(())
            }
        }
        Some(Command::Validate { fix }) => {
            if cli.json {
                cli::print_json(validate::validate(&library, fix)?)
            } else {
                validate::run(&library, fix)
            }
        }
        Some(Command::Completions { shell }) => {
            anyhow::ensure!(!cli.json, "--json is not valid for completions");
            let mut cmd = Cli::command();
            let name = cmd.get_name().to_string();
            clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
            Ok(())
        }
    }
}

fn print_mutations(records: &[cli::MutationRecord], apply: bool) {
    for record in records {
        let state = if record.changed {
            if record.applied {
                "updated"
            } else {
                "would update"
            }
        } else {
            "unchanged"
        };
        println!("{}: {state}", record.key);
        for change in &record.changes {
            println!("  {}: {} -> {}", change.field, change.before, change.after);
        }
    }
    if !apply && records.iter().any(|record| record.changed) {
        println!("Dry run only; repeat with --apply to write changes.");
    }
}

/// Import several inputs in one invocation. A failure on one input is reported
/// but does not abort the rest; the command exits non-zero if any failed.
#[derive(Debug, Serialize)]
pub struct AddReport {
    inputs: Vec<AddResult>,
}

#[derive(Debug, Serialize)]
struct AddResult {
    input: String,
    status: &'static str,
    keys: Vec<String>,
}

pub fn cmd_add_many(library: &Path, inputs: &[String], force: bool) -> Result<AddReport> {
    let mut failures = 0;
    let mut results = Vec::new();
    for input in inputs {
        let before = storage::list_ref_dirs(library)?;
        match cmd_add(library, input, force) {
            Ok(()) => {
                let after = storage::list_ref_dirs(library)?;
                let mut keys = after
                    .iter()
                    .filter(|path| !before.contains(path))
                    .filter_map(|path| path.file_name())
                    .map(|name| name.to_string_lossy().to_string())
                    .collect::<Vec<_>>();
                keys.sort();
                results.push(AddResult {
                    input: input.clone(),
                    status: if keys.is_empty() { "skipped" } else { "added" },
                    keys,
                });
            }
            Err(e) => {
                eprintln!("error: failed to add {input}: {e}");
                failures += 1;
            }
        }
    }
    if failures > 0 {
        anyhow::bail!("{failures} of {} input(s) failed", inputs.len());
    }
    Ok(AddReport { inputs: results })
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
            eprintln!("Resolved DOI from page: {doi}");
            return add_from_doi_with_pdf(library, &doi, info.pdf_url.as_deref(), force);
        }
        if let Some(pdf_url) = info.pdf_url {
            eprintln!("Resolved PDF from page: {pdf_url}");
            return add_from_url(library, &pdf_url, force);
        }
    }

    // The page yielded no DOI and no PDF link (often a JavaScript-rendered or
    // bot-protected publisher page). Try the URL as a direct PDF, but if that
    // fails, point the user at the reliable path rather than a raw error.
    add_from_url(library, url, force).map_err(|e| {
        anyhow::anyhow!(
            "couldn't import {url}: {e}\n  \
             No DOI or PDF link was found on the page — it may be \
             JavaScript-rendered or bot-protected.\n  \
             Try adding by DOI instead, e.g. `grimoire add 10.1234/...`."
        )
    })
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

pub fn index_reference(
    library: &Path,
    ref_dir: &Path,
    reference: &crate::model::Reference,
) -> Result<()> {
    let idx = index::Index::open(library)?;
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
    idx.upsert_with_fulltext(&dir_name, reference, fulltext.as_deref())?;
    Ok(())
}

fn add_from_arxiv(library: &Path, arxiv_id: &str, force: bool) -> Result<()> {
    eprintln!("Fetching metadata from arXiv: {}", arxiv_id);
    let mut reference = fetch::fetch_arxiv(arxiv_id)?;

    if skip_as_duplicate(library, &reference, force) {
        return Ok(());
    }

    let ref_dir = storage::create_ref_dir(library, &reference)?;
    let pdf_filename = format!("{}.pdf", arxiv_id);
    let pdf_path = ref_dir.join(&pdf_filename);

    eprintln!("Downloading PDF...");
    fetch::download_arxiv_pdf(arxiv_id, &pdf_path)?;

    reference.files = vec![pdf_filename];
    metadata::write_info(&ref_dir, &reference)?;
    index_reference(library, &ref_dir, &reference)?;

    eprintln!("Added: {}", reference.title);
    eprintln!("  → {}", ref_dir.display());
    Ok(())
}

fn add_from_doi(library: &Path, doi: &str, force: bool) -> Result<()> {
    add_from_doi_with_pdf(library, doi, None, force)
}

fn add_from_pubmed(library: &Path, pmid: &str, force: bool) -> Result<()> {
    eprintln!("Resolving PubMed {pmid} via NCBI...");
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
    eprintln!("Fetching metadata from CrossRef: {}", doi);
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
    let dir_name = ref_dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let pdf_filename = format!("{dir_name}.pdf");

    let save_pdf = |bytes: Vec<u8>| -> Result<()> {
        std::fs::write(ref_dir.join(&pdf_filename), bytes)
            .with_context(|| format!("Failed to save PDF to {}", ref_dir.display()))
    };

    // 1. A PDF URL the caller already knows (e.g. a page's citation_pdf_url).
    if let Some(url) = pdf_url {
        match fetch::download_pdf(url) {
            Ok(bytes) => {
                save_pdf(bytes)?;
                reference.files = vec![pdf_filename.clone()];
            }
            Err(e) => eprintln!("  (provided PDF URL failed: {e})"),
        }
    }

    // 2. Fall back to an open-access copy via Unpaywall, keyed by DOI.
    if reference.files.is_empty()
        && let Some(doi) = reference.doi.clone()
        && let Some(oa_url) = fetch::unpaywall_pdf_url(&doi)
    {
        match fetch::download_pdf(&oa_url) {
            Ok(bytes) => {
                save_pdf(bytes)?;
                reference.files = vec![pdf_filename.clone()];
                eprintln!("  (open-access PDF via Unpaywall)");
            }
            Err(e) => eprintln!("  (Unpaywall PDF failed: {e})"),
        }
    }

    metadata::write_info(&ref_dir, &reference)?;
    index_reference(library, &ref_dir, &reference)?;

    eprintln!("Added: {}", reference.title);
    eprintln!("  → {}", ref_dir.display());
    if reference.files.is_empty() {
        eprintln!("  (no PDF — add one manually to the directory)");
    }
    Ok(())
}

fn add_from_pmc(library: &Path, pmc_id: &str, force: bool) -> Result<()> {
    eprintln!("Resolving PMC article and downloading PDF: {pmc_id}");
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
    index_reference(library, &ref_dir, &reference)?;

    eprintln!("Added: {}", reference.title);
    eprintln!("  → {}", ref_dir.display());
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
        eprintln!("Detected arXiv ID: {} — fetching metadata...", id);
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
    index_reference(library, &ref_dir, &reference)?;

    eprintln!("Added: {}", reference.title);
    eprintln!("  → {}", ref_dir.display());
    Ok(())
}

fn add_from_url(library: &Path, url: &str, force: bool) -> Result<()> {
    eprintln!("Downloading PDF from URL...");
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

#[cfg(test)]
mod tests {
    use super::{Cli, Command};
    use clap::Parser;

    #[test]
    fn semantic_cli_defaults_to_first_page_and_supports_explicit_paging() {
        let cli = Cli::try_parse_from(["grimoire", "semantic", "synthetic query"]).unwrap();
        let Some(Command::Semantic {
            limit, offset, all, ..
        }) = cli.command
        else {
            panic!("semantic command was not parsed");
        };
        assert_eq!(limit, None);
        assert_eq!(offset, 0);
        assert!(!all);

        let cli = Cli::try_parse_from([
            "grimoire",
            "semantic",
            "synthetic query",
            "--limit",
            "25",
            "--offset",
            "100",
        ])
        .unwrap();
        let Some(Command::Semantic { limit, offset, .. }) = cli.command else {
            panic!("semantic command was not parsed");
        };
        assert_eq!(limit, Some(25));
        assert_eq!(offset, 100);

        assert!(
            Cli::try_parse_from([
                "grimoire",
                "semantic",
                "synthetic query",
                "--limit",
                "25",
                "--all",
            ])
            .is_err()
        );
    }
}
