use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use slug::slugify;

use crate::model::Reference;

pub fn create_ref_dir(library: &Path, reference: &Reference) -> Result<PathBuf> {
    let dir_name = make_dir_name(reference);
    let mut dir = library.join(&dir_name);

    if dir.exists() {
        let mut n = 2;
        loop {
            let candidate = library.join(format!("{}-{}", dir_name, n));
            if !candidate.exists() {
                dir = candidate;
                break;
            }
            n += 1;
        }
    }

    std::fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    Ok(dir)
}

pub fn copy_pdf(source: &Path, dest_dir: &Path) -> Result<String> {
    let filename = source
        .file_name()
        .context("Source has no filename")?
        .to_string_lossy()
        .to_string();

    let dest = dest_dir.join(&filename);
    std::fs::copy(source, &dest)
        .with_context(|| format!("Failed to copy PDF to {}", dest.display()))?;

    Ok(filename)
}

/// Find an existing reference that duplicates `reference`, matching on DOI
/// first (most reliable) and then on normalized title. Returns the matching
/// directory and which key matched ("doi" or "title"), or `None`.
pub fn find_duplicate(
    library: &Path,
    reference: &Reference,
) -> Result<Option<(PathBuf, &'static str)>> {
    let want_doi = reference
        .doi
        .as_deref()
        .map(|d| d.trim().to_lowercase())
        .filter(|d| !d.is_empty());
    let title = reference.title.trim().to_lowercase();
    let want_title = (!title.is_empty()).then_some(title);

    if want_doi.is_none() && want_title.is_none() {
        return Ok(None);
    }

    for dir in list_ref_dirs(library)? {
        let existing = match crate::metadata::read_info(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if let Some(ref doi) = want_doi {
            let existing_doi = existing.doi.as_deref().map(|d| d.trim().to_lowercase());
            if existing_doi.as_deref() == Some(doi.as_str()) {
                return Ok(Some((dir, "doi")));
            }
        }
        if let Some(ref title) = want_title
            && existing.title.trim().to_lowercase() == *title
        {
            return Ok(Some((dir, "title")));
        }
    }

    Ok(None)
}

pub fn list_ref_dirs(library: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    if !library.exists() {
        return Ok(dirs);
    }
    for entry in std::fs::read_dir(library)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() && path.join("info.toml").exists() {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn make_dir_name(reference: &Reference) -> String {
    let author = reference
        .authors
        .first()
        .map(|a| last_name(a))
        .unwrap_or_else(|| "unknown".to_string());

    let year = reference
        .year
        .map(|y| y.to_string())
        .unwrap_or_else(|| "0000".to_string());

    let title_word = reference
        .title
        .split_whitespace()
        .find(|w| {
            let lower = w.to_lowercase();
            !matches!(
                lower.as_str(),
                "a" | "an" | "the" | "on" | "of" | "for" | "in" | "to" | "and" | "with"
            )
        })
        .unwrap_or("untitled");

    slugify(format!("{}-{}-{}", author, year, title_word))
}

fn last_name(author: &str) -> String {
    if let Some((last, _)) = author.rsplit_once(',') {
        return last.trim().to_string();
    }
    author
        .split_whitespace()
        .last()
        .unwrap_or(author)
        .to_string()
}
