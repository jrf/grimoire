use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::metadata;
use crate::model::{Reference, ReferenceKind};
use crate::storage;

/// A reference paired with its citation key (the library directory name),
/// flattened so the key sits alongside the reference fields in the output.
#[derive(Serialize)]
struct ExportEntry {
    key: String,
    #[serde(flatten)]
    reference: Reference,
}

pub fn run(library: &Path, format: &str, output: Option<&Path>, tags: &[String]) -> Result<()> {
    let dirs = storage::list_ref_dirs(library)?;

    let mut entries = Vec::new();
    for dir in &dirs {
        let key = dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        match metadata::read_info(dir) {
            Ok(reference) => entries.push(ExportEntry { key, reference }),
            Err(e) => eprintln!("warning: skipping {}: {}", dir.display(), e),
        }
    }

    // Keep only references carrying at least one of the requested tags.
    if !tags.is_empty() {
        entries.retain(|e| e.reference.tags.iter().any(|t| tags.contains(t)));
    }

    let rendered = match format.to_lowercase().as_str() {
        "yaml" | "yml" => serde_norway::to_string(&entries)?,
        "json" => serde_json::to_string_pretty(&entries)?,
        "bibtex" | "bib" => entries
            .iter()
            .map(|e| to_bibtex(&e.key, &e.reference))
            .collect::<Vec<_>>()
            .join("\n\n"),
        "hayagriva" => {
            let library: BTreeMap<String, HayagrivaEntry> = entries
                .iter()
                .map(|e| (e.key.clone(), to_hayagriva(&e.reference)))
                .collect();
            serde_norway::to_string(&library)?
        }
        other => anyhow::bail!(
            "Unknown export format: {other} (expected yaml, json, bibtex, or hayagriva)"
        ),
    };

    match output {
        Some(path) => {
            std::fs::write(path, format!("{rendered}\n"))
                .with_context(|| format!("Failed to write {}", path.display()))?;
            eprintln!(
                "Exported {} reference(s) → {}",
                entries.len(),
                path.display()
            );
        }
        None => println!("{rendered}"),
    }
    Ok(())
}

/// Normalize an author name to `Family, Given` order so citation processors
/// (Hayagriva, BibTeX) can identify the surname. A name that already contains a
/// comma is assumed to be in `Family, Given` order and is left as-is, aside from
/// trimming stray trailing commas/whitespace. Single-token names are returned
/// unchanged.
fn family_given(author: &str) -> String {
    let author = author.trim();
    if author.contains(',') {
        return author.trim_end_matches(',').trim().to_string();
    }
    let tokens: Vec<&str> = author.split_whitespace().collect();
    match tokens.split_last() {
        Some((surname, given)) if !given.is_empty() => format!("{surname}, {}", given.join(" ")),
        _ => author.to_string(),
    }
}

/// Render a single reference as a BibTeX entry.
pub fn to_bibtex(cite_key: &str, r: &Reference) -> String {
    let entry_type = match r.kind {
        ReferenceKind::Paper => "article",
        ReferenceKind::Book => "book",
    };
    let mut bib = format!("@{entry_type}{{{cite_key},\n");
    bib.push_str(&format!("  title = {{{}}},\n", r.title));
    let authors = r
        .authors
        .iter()
        .map(|a| family_given(a))
        .collect::<Vec<_>>()
        .join(" and ");
    if !authors.is_empty() {
        bib.push_str(&format!("  author = {{{authors}}},\n"));
    }
    if let Some(year) = r.year {
        bib.push_str(&format!("  year = {{{year}}},\n"));
    }
    if let Some(ref journal) = r.journal {
        bib.push_str(&format!("  journal = {{{journal}}},\n"));
    }
    if let Some(ref publisher) = r.publisher {
        bib.push_str(&format!("  publisher = {{{publisher}}},\n"));
    }
    if let Some(ref edition) = r.edition {
        bib.push_str(&format!("  edition = {{{edition}}},\n"));
    }
    if let Some(ref series) = r.series {
        bib.push_str(&format!("  series = {{{series}}},\n"));
    }
    if !r.isbn.is_empty() {
        bib.push_str(&format!("  isbn = {{{}}},\n", r.isbn.join(", ")));
    }
    if let Some(ref doi) = r.doi {
        bib.push_str(&format!("  doi = {{{doi}}},\n"));
    }
    if let Some(ref arxiv) = r.arxiv {
        bib.push_str(&format!("  eprint = {{{arxiv}}},\n"));
        bib.push_str("  archiveprefix = {arXiv},\n");
    }
    bib.push('}');
    bib
}

/// A single entry in a Hayagriva bibliography (Typst's YAML citation format).
/// The library is a mapping of citation key -> entry.
#[derive(Serialize)]
struct HayagrivaEntry {
    #[serde(rename = "type")]
    entry_type: String,
    title: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    author: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    date: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent: Option<HayagrivaParent>,
    #[serde(rename = "serial-number", skip_serializing_if = "Option::is_none")]
    serial_number: Option<HayagrivaSerial>,
    #[serde(rename = "abstract", skip_serializing_if = "Option::is_none")]
    r#abstract: Option<String>,
}

/// The containing work (e.g. the journal a paper appears in).
#[derive(Serialize)]
struct HayagrivaParent {
    #[serde(rename = "type")]
    entry_type: String,
    title: String,
}

/// Identifiers Hayagriva groups under `serial-number`.
#[derive(Serialize)]
struct HayagrivaSerial {
    #[serde(skip_serializing_if = "Option::is_none")]
    doi: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    arxiv: Option<String>,
}

fn to_hayagriva(r: &Reference) -> HayagrivaEntry {
    let serial = if r.doi.is_some() || r.arxiv.is_some() {
        Some(HayagrivaSerial {
            doi: r.doi.clone(),
            arxiv: r.arxiv.clone(),
        })
    } else {
        None
    };

    HayagrivaEntry {
        entry_type: match r.kind {
            ReferenceKind::Paper => "article",
            ReferenceKind::Book => "book",
        }
        .to_string(),
        title: r.title.clone(),
        author: r.authors.iter().map(|a| family_given(a)).collect(),
        date: r.year,
        parent: r.journal.clone().map(|title| HayagrivaParent {
            entry_type: "periodical".to_string(),
            title,
        }),
        serial_number: serial,
        r#abstract: r.r#abstract.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::to_bibtex;
    use crate::model::Reference;

    #[test]
    fn exports_books_as_bibtex_books() {
        let reference: Reference = toml::from_str(
            r#"
            kind = "book"
            title = "Synthetic Analysis"
            authors = ["Ada Example"]
            year = 2026
            edition = "2"
            publisher = "Example Press"
            series = "Synthetic Mathematics"
            isbn = ["978-0-00-000000-0"]
            "#,
        )
        .unwrap();

        let bibtex = to_bibtex("example-2026-synthetic", &reference);
        assert!(bibtex.starts_with("@book{example-2026-synthetic,"));
        assert!(bibtex.contains("publisher = {Example Press}"));
        assert!(bibtex.contains("edition = {2}"));
        assert!(bibtex.contains("isbn = {978-0-00-000000-0}"));
    }
}
