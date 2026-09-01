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

/// True for a single-letter name token such as `M.`, `O.` or `R`, which is an
/// initial rather than a surname.
fn is_initial(token: &str) -> bool {
    let token = token.trim_end_matches('.');
    token.chars().count() == 1 && token.chars().all(char::is_alphabetic)
}

fn last_name(author: &str) -> String {
    if let Some((last, _)) = author.rsplit_once(',') {
        return last.trim().to_string();
    }

    let tokens: Vec<&str> = author.split_whitespace().collect();
    let Some(last_token) = tokens.last() else {
        return author.to_string();
    };

    // A trailing initial means the name is stored surname-first with the comma
    // missing ("Wahba George M."). Taking the last token would key the entry on
    // the initial, so fall back to the first non-initial token instead.
    if tokens.len() > 1
        && is_initial(last_token)
        && let Some(surname) = tokens.iter().find(|t| !is_initial(t))
    {
        return (*surname).to_string();
    }

    (*last_token).to_string()
}

#[cfg(test)]
mod last_name_tests {
    use super::last_name;

    #[test]
    fn comma_form_uses_the_text_before_the_comma() {
        assert_eq!(last_name("Bardes, Adrien"), "Bardes");
        assert_eq!(last_name("Vinsard, Daniela Guerrero"), "Vinsard");
        assert_eq!(last_name("Polat, Gorkem"), "Polat");
    }

    #[test]
    fn plain_form_uses_the_final_token() {
        assert_eq!(last_name("Kaiming He"), "He");
        assert_eq!(last_name("Sarah Bencardino"), "Bencardino");
        assert_eq!(last_name("Lorenzo Mur-Labadia"), "Mur-Labadia");
    }

    /// Comma-less inverted names used to key on the trailing initial, which
    /// produced library directories such as `m-2025-use` and
    /// `r-2025-foundation`.
    #[test]
    fn trailing_initial_does_not_become_the_surname() {
        assert_eq!(last_name("Wahba George M."), "Wahba");
        assert_eq!(last_name("Prichard David O."), "Prichard");
        assert_eq!(last_name("Phillips Hayley R."), "Phillips");
        assert_eq!(last_name("Fetzer Jeffrey R."), "Fetzer");
    }

    #[test]
    fn leading_initials_still_resolve_to_the_final_surname() {
        assert_eq!(last_name("George M. Wahba"), "Wahba");
        assert_eq!(last_name("G. Žibret"), "Žibret");
        assert_eq!(last_name("Luisa F. Sánchez-Peralta"), "Sánchez-Peralta");
    }

    #[test]
    fn diacritics_are_preserved_for_the_slugifier_to_transliterate() {
        assert_eq!(last_name("Jorge Sánchez"), "Sánchez");
    }

    #[test]
    fn single_token_and_empty_names_are_returned_unchanged() {
        assert_eq!(last_name("DeepSeek-AI"), "DeepSeek-AI");
        assert_eq!(last_name(""), "");
    }
}
