use std::path::Path;

use anyhow::{Context, Result};

use crate::enrich;
use crate::fetch;
use crate::metadata;
use crate::model::Reference;
use crate::storage;

pub struct Options {
    /// Try to download a PDF for entries that have none.
    pub pdfs: bool,
    /// Fetch metadata (abstract, and any other missing fields) for entries
    /// missing an abstract.
    pub abstracts: bool,
    /// Report what would be attempted without touching the network or disk.
    pub check: bool,
}

/// Fill in missing PDFs and abstracts for existing library entries. Purely
/// additive — an entry that already has the thing is never touched, and
/// metadata merges only fill empty fields (see [`enrich::enrich_entry`]).
pub fn run(library: &Path, opts: &Options) -> Result<()> {
    let dirs = storage::list_ref_dirs(library)?;

    if opts.check {
        return report_check(&dirs);
    }

    let mut pdfs_added = 0usize;
    let mut abstracts_added = 0usize;
    let mut unresolved = 0usize;
    let mut considered = 0usize;

    for dir in &dirs {
        let name = dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let content = match std::fs::read_to_string(dir.join("info.toml")) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let mut reference: Reference = match toml::from_str(&content) {
            Ok(r) => r,
            Err(_) => continue,
        };

        let want_abstract = opts.abstracts && reference.r#abstract.is_none();
        let want_pdf = opts.pdfs && !has_pdf(dir);
        if !want_abstract && !want_pdf {
            continue;
        }
        considered += 1;

        let mut changed = false;
        let mut notes: Vec<String> = Vec::new();

        // Metadata pass first: enrich fills the abstract (and any other empty
        // fields, including a missing DOI the PDF lookup can then use).
        if want_abstract {
            match enrich::enrich_entry(dir, &reference) {
                Ok(Some(updated)) if updated != reference => {
                    if reference.r#abstract.is_none() && updated.r#abstract.is_some() {
                        abstracts_added += 1;
                        notes.push("abstract".into());
                    }
                    reference = updated;
                    changed = true;
                }
                _ => {}
            }
        }

        // PDF pass: needs a DOI (possibly just filled in above).
        if want_pdf {
            match reference.doi.clone() {
                Some(doi) => match fetch::fetch_pdf_for_doi(&doi) {
                    Some((bytes, source)) => {
                        let filename = format!("{name}.pdf");
                        match std::fs::write(dir.join(&filename), &bytes) {
                            Ok(()) => {
                                reference.files = vec![filename];
                                changed = true;
                                pdfs_added += 1;
                                notes.push(format!("pdf {} via {source}", human_size(bytes.len())));
                            }
                            Err(e) => {
                                unresolved += 1;
                                notes.push(format!("pdf save failed: {e}"));
                            }
                        }
                    }
                    None => {
                        unresolved += 1;
                        notes.push("no OA pdf".into());
                    }
                },
                None => {
                    unresolved += 1;
                    notes.push("no DOI for pdf".into());
                }
            }
        }

        if changed {
            metadata::write_info(dir, &reference)
                .with_context(|| format!("Failed to update {name}"))?;
            crate::index_reference(library, dir, &reference);
        }

        let status = if notes.is_empty() {
            "nothing found".to_string()
        } else {
            notes.join(", ")
        };
        println!("  {:<44} {}", truncate(&name, 44), status);
    }

    println!(
        "\nBackfill complete: {pdfs_added} PDF(s) downloaded, {abstracts_added} abstract(s) filled, \
         {unresolved} unresolved  ({considered} of {} entries had gaps)",
        dirs.len()
    );
    Ok(())
}

/// Local-only preview: count holes without hitting the network.
fn report_check(dirs: &[std::path::PathBuf]) -> Result<()> {
    let mut missing_pdf = 0usize;
    let mut missing_pdf_with_doi = 0usize;
    let mut missing_abstract = 0usize;

    for dir in dirs {
        let content = match std::fs::read_to_string(dir.join("info.toml")) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let reference: Reference = match toml::from_str(&content) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if !has_pdf(dir) {
            missing_pdf += 1;
            if reference.doi.is_some() {
                missing_pdf_with_doi += 1;
            }
        }
        if reference.r#abstract.is_none() {
            missing_abstract += 1;
        }
    }

    println!("Scanned {} entries:", dirs.len());
    println!("  missing PDF:                {missing_pdf}");
    println!("  missing PDF, has DOI:       {missing_pdf_with_doi}  (backfill will try these)");
    println!("  missing abstract:           {missing_abstract}");
    Ok(())
}

/// True if the directory contains at least one `.pdf` file on disk. Checks the
/// filesystem rather than the `files` list so a dangling reference (listed but
/// deleted) is treated as missing and re-fetched.
fn has_pdf(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(|e| e.ok()).any(|e| {
        e.path()
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    })
}

fn human_size(bytes: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = 1024 * 1024;
    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else {
        format!("{} KB", bytes.max(1).div_ceil(KB))
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..s.floor_char_boundary(max)]
    }
}
