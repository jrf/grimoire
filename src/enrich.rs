use std::path::Path;

use anyhow::Result;

use crate::fetch;
use crate::model::Reference;

/// Fetch fresh metadata for an entry and return a copy with any *missing* fields
/// filled in. Purely additive: existing values are never overwritten. Returns
/// `None` when no source (arXiv ID, DOI, dir-name arXiv ID, or title) resolves.
///
/// The lookup order mirrors how the entry was most likely imported: an explicit
/// arXiv ID, then a DOI, then an arXiv ID embedded in the directory name, and
/// finally a CrossRef title search as a last resort.
pub fn enrich_entry(dir: &Path, r: &Reference) -> Result<Option<Reference>> {
    let arxiv_id = r
        .arxiv
        .clone()
        .or_else(|| r.files.iter().find_map(|f| fetch::detect_arxiv_id(f)));

    let fetched = if let Some(ref id) = arxiv_id {
        fetch::fetch_arxiv(id).ok()
    } else if let Some(ref doi) = r.doi {
        fetch::fetch_crossref(doi).ok()
    } else {
        // Try to detect arXiv ID from directory name
        let dir_name = dir.file_name().unwrap_or_default().to_string_lossy();
        if let Some(id) = fetch::detect_arxiv_id(&dir_name) {
            fetch::fetch_arxiv(&id).ok()
        } else if !r.title.is_empty() {
            fetch::search_crossref_by_title(&r.title).ok()
        } else {
            return Ok(None);
        }
    };

    let fetched = match fetched {
        Some(f) => f,
        None => return Ok(None),
    };

    let mut updated = r.clone();

    if updated.year.is_none() || updated.year == Some(0) {
        updated.year = fetched.year;
    }
    if updated.authors.is_empty() {
        updated.authors = fetched.authors;
    }
    if updated.r#abstract.is_none() {
        updated.r#abstract = fetched.r#abstract;
    }
    if updated.doi.is_none() {
        updated.doi = fetched.doi;
    }
    if updated.arxiv.is_none() {
        updated.arxiv = fetched.arxiv;
    }
    if updated.journal.is_none() {
        updated.journal = fetched.journal;
    }

    Ok(Some(updated))
}
