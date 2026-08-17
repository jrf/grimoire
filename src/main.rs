mod backfill;
mod cli;
mod config;
mod dedup;
mod docling;
mod enrich;
mod export;
mod fetch;
mod formula;
mod index;
mod kitty;
mod metadata;
mod model;
mod semantic;
mod storage;
mod theme;
mod tui;
mod validate;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use config::Config;
use model::ReferenceKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum SemanticGroup {
    Papers,
    Passages,
}

#[derive(Parser)]
#[command(name = "grimoire", version, about = "A fast scholarly library")]
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
        /// Type of local work being imported
        #[arg(long, value_enum, default_value_t = ReferenceKind::Paper)]
        kind: ReferenceKind,
        /// Override the PDF title (single input only)
        #[arg(long)]
        title: Option<String>,
        /// Override the author list (repeatable; single input only)
        #[arg(long = "author")]
        authors: Vec<String>,
        /// Override the publication year (single input only)
        #[arg(long)]
        year: Option<u16>,
        /// Book edition (single input only)
        #[arg(long)]
        edition: Option<String>,
        /// Book publisher (single input only)
        #[arg(long)]
        publisher: Option<String>,
        /// Book series (single input only)
        #[arg(long)]
        series: Option<String>,
        /// Book ISBN (repeatable; single input only)
        #[arg(long)]
        isbn: Vec<String>,
        /// Override the DOI (single input only)
        #[arg(long)]
        doi: Option<String>,
    },
    /// Import Docling JSON as structured passages for an existing work
    ImportDerived {
        /// Existing library key
        key: String,
        /// Docling JSON document to preserve and convert to passages
        #[arg(long)]
        docling: PathBuf,
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
    /// Build a local vector index from JSONL files under each work's derived directory
    SemanticIndex {
        /// Re-embed every passage even when its source is unchanged
        #[arg(long)]
        force: bool,
    },
    /// Search indexed works or passages by semantic similarity
    Semantic {
        /// Natural-language search query
        #[arg(required = true)]
        query: Vec<String>,
        /// Number of works or passages to return (defaults to 100)
        #[arg(short, long, conflicts_with = "all")]
        limit: Option<usize>,
        /// Zero-based result offset
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// Return every result from the offset onward
        #[arg(long)]
        all: bool,
        /// Group results by work (`papers`, retained for compatibility), or return passages
        #[arg(long, value_enum, default_value = "papers")]
        group: SemanticGroup,
        /// Number of ranked passages to include with each work result
        #[arg(long)]
        per_paper: Option<usize>,
        /// Require every exact query term, then rank matches by similarity
        #[arg(long)]
        exact: bool,
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
        Some(Command::Add {
            paths,
            force,
            kind,
            title,
            authors,
            year,
            edition,
            publisher,
            series,
            isbn,
            doi,
        }) => {
            let options = AddOptions {
                kind,
                title,
                authors,
                year,
                edition,
                publisher,
                series,
                isbn,
                doi,
            };
            let report = cmd_add_many(&library, &paths, force, &options)?;
            if cli.json {
                cli::print_json(report)
            } else {
                Ok(())
            }
        }
        Some(Command::ImportDerived { key, docling }) => {
            let directory = cli::reference_dir(&library, &key)?;
            let report = docling::import(&directory, &docling)?;
            if cli.json {
                cli::print_json(report)
            } else {
                println!(
                    "Imported {} passages from {} body blocks ({} formulas, {} pages).",
                    report.passages, report.body_blocks, report.formulas, report.pages
                );
                println!("  → {}", report.passages_path.display());
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
            group,
            per_paper,
            exact,
        }) => {
            let query = query.join(" ");
            anyhow::ensure!(
                group == SemanticGroup::Papers || per_paper.is_none(),
                "--per-paper can only be used with --group papers"
            );
            let ranking = if exact {
                semantic::rank_exact(&library, &query, &config.embedding)?
            } else {
                semantic::rank(&library, &query, &config.embedding)?
            };
            match group {
                SemanticGroup::Papers => {
                    let per_paper = per_paper.unwrap_or(1);
                    let page_limit = if all {
                        ranking.paper_total().saturating_sub(offset).max(1)
                    } else {
                        limit.unwrap_or(semantic::DEFAULT_PAGE_SIZE)
                    };
                    let page = ranking.paper_page(&library, offset, page_limit, per_paper)?;
                    if cli.json {
                        cli::print_json(page)
                    } else {
                        semantic::print_paper_page(&page);
                        Ok(())
                    }
                }
                SemanticGroup::Passages => {
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddOptions {
    kind: ReferenceKind,
    title: Option<String>,
    authors: Vec<String>,
    year: Option<u16>,
    edition: Option<String>,
    publisher: Option<String>,
    series: Option<String>,
    isbn: Vec<String>,
    doi: Option<String>,
}

impl AddOptions {
    fn apply(&self, reference: &mut crate::model::Reference) -> Result<()> {
        anyhow::ensure!(
            self.kind == ReferenceKind::Book
                || (self.edition.is_none()
                    && self.publisher.is_none()
                    && self.series.is_none()
                    && self.isbn.is_empty()),
            "Book metadata options require `--kind book`"
        );
        reference.kind = self.kind;
        if let Some(title) = &self.title {
            anyhow::ensure!(!title.trim().is_empty(), "Title cannot be empty");
            reference.title = title.trim().to_string();
        }
        if !self.authors.is_empty() {
            anyhow::ensure!(
                self.authors.iter().all(|author| !author.trim().is_empty()),
                "Authors cannot be empty"
            );
            reference.authors = self
                .authors
                .iter()
                .map(|author| author.trim().to_string())
                .collect();
        }
        if let Some(year) = self.year {
            reference.year = Some(year);
        }
        reference.edition = self.edition.as_deref().map(str::trim).map(str::to_string);
        reference.publisher = self.publisher.as_deref().map(str::trim).map(str::to_string);
        reference.series = self.series.as_deref().map(str::trim).map(str::to_string);
        if !self.isbn.is_empty() {
            reference.isbn = self
                .isbn
                .iter()
                .map(|isbn| isbn.trim().to_string())
                .filter(|isbn| !isbn.is_empty())
                .collect();
            anyhow::ensure!(!reference.isbn.is_empty(), "ISBN cannot be empty");
        }
        if let Some(doi) = &self.doi {
            anyhow::ensure!(!doi.trim().is_empty(), "DOI cannot be empty");
            reference.doi = Some(doi.trim().to_string());
        }
        Ok(())
    }

    fn has_overrides(&self) -> bool {
        self != &Self::default()
    }
}

pub fn cmd_add_many(
    library: &Path,
    inputs: &[String],
    force: bool,
    options: &AddOptions,
) -> Result<AddReport> {
    anyhow::ensure!(
        inputs.len() == 1 || !options.has_overrides(),
        "Metadata overrides can only be used when adding one PDF"
    );
    let mut failures = 0;
    let mut results = Vec::new();
    for input in inputs {
        let before = storage::list_ref_dirs(library)?;
        match cmd_add(library, input, force, options) {
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

pub fn cmd_add(library: &Path, input: &str, force: bool, options: &AddOptions) -> Result<()> {
    std::fs::create_dir_all(library)?;

    let path = PathBuf::from(input);
    if path.exists() {
        return add_from_file(library, input, force, options);
    }

    anyhow::ensure!(
        !options.has_overrides(),
        "`--kind` and metadata overrides currently require a local PDF"
    );

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
    eprintln!("Fetching metadata for DOI: {}", doi);
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

fn add_from_file(library: &Path, path: &str, force: bool, options: &AddOptions) -> Result<()> {
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

    options.apply(&mut reference)?;

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

    add_from_file(
        library,
        tmp_path.to_str().unwrap(),
        force,
        &AddOptions::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command, SemanticGroup};
    use crate::model::ReferenceKind;
    use clap::Parser;

    #[test]
    fn semantic_cli_defaults_to_first_page_and_supports_explicit_paging() {
        let cli = Cli::try_parse_from(["grimoire", "semantic", "synthetic query"]).unwrap();
        let Some(Command::Semantic {
            limit,
            offset,
            all,
            group,
            per_paper,
            ..
        }) = cli.command
        else {
            panic!("semantic command was not parsed");
        };
        assert_eq!(limit, None);
        assert_eq!(offset, 0);
        assert!(!all);
        assert_eq!(group, SemanticGroup::Papers);
        assert_eq!(per_paper, None);

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

        let cli = Cli::try_parse_from([
            "grimoire",
            "semantic",
            "synthetic query",
            "--group",
            "passages",
        ])
        .unwrap();
        let Some(Command::Semantic { group, .. }) = cli.command else {
            panic!("semantic command was not parsed");
        };
        assert_eq!(group, SemanticGroup::Passages);

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

    #[test]
    fn book_and_docling_commands_parse_explicit_metadata() {
        let cli = Cli::try_parse_from([
            "grimoire",
            "add",
            "--kind",
            "book",
            "--title",
            "Synthetic Analysis",
            "--author",
            "Ada Example",
            "--year",
            "2026",
            "--edition",
            "2",
            "synthetic.pdf",
        ])
        .unwrap();
        let Some(Command::Add {
            kind,
            title,
            authors,
            year,
            edition,
            ..
        }) = cli.command
        else {
            panic!("add command was not parsed");
        };
        assert_eq!(kind, ReferenceKind::Book);
        assert_eq!(title.as_deref(), Some("Synthetic Analysis"));
        assert_eq!(authors, ["Ada Example"]);
        assert_eq!(year, Some(2026));
        assert_eq!(edition.as_deref(), Some("2"));

        let cli = Cli::try_parse_from([
            "grimoire",
            "import-derived",
            "example-2026-synthetic",
            "--docling",
            "synthetic.json",
        ])
        .unwrap();
        assert!(matches!(cli.command, Some(Command::ImportDerived { .. })));

        let cli = Cli::try_parse_from(["grimoire", "semantic", "monotone convergence", "--exact"])
            .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Semantic { exact: true, .. })
        ));
    }
}
