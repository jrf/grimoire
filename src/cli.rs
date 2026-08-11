use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::metadata;
use crate::model::Reference;
use crate::storage;

#[derive(Debug, Serialize)]
pub struct ReferenceRecord {
    pub key: String,
    pub directory: PathBuf,
    pub pdf: Option<PathBuf>,
    #[serde(flatten)]
    pub reference: Reference,
}

#[derive(Debug, Serialize)]
pub struct FieldChange {
    pub field: String,
    pub before: serde_json::Value,
    pub after: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct MutationRecord {
    pub key: String,
    pub changed: bool,
    pub applied: bool,
    pub changes: Vec<FieldChange>,
    pub reference: Reference,
}

#[derive(Serialize)]
struct Response<T> {
    ok: bool,
    data: T,
    warnings: Vec<String>,
    errors: Vec<String>,
}

pub fn print_json<T: Serialize>(data: T) -> Result<()> {
    print_json_with_warnings(data, Vec::new())
}

pub fn print_json_with_warnings<T: Serialize>(data: T, warnings: Vec<String>) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&Response {
            ok: true,
            data,
            warnings,
            errors: Vec::new(),
        })?
    );
    Ok(())
}

pub fn print_json_error(error: &anyhow::Error) {
    let response = serde_json::json!({
        "ok": false,
        "data": null,
        "warnings": [],
        "errors": [format!("{error:#}")],
    });
    println!("{}", serde_json::to_string_pretty(&response).unwrap());
}

pub fn records(library: &Path) -> Result<(Vec<ReferenceRecord>, Vec<String>)> {
    let mut records = Vec::new();
    let mut warnings = Vec::new();
    for directory in storage::list_ref_dirs(library)? {
        let key = directory
            .file_name()
            .context("Reference directory has no name")?
            .to_string_lossy()
            .to_string();
        let reference = match metadata::read_info(&directory) {
            Ok(reference) => reference,
            Err(error) => {
                warnings.push(format!("Skipping {key}: {error:#}"));
                continue;
            }
        };
        let pdf = find_pdf(&directory, &reference);
        records.push(ReferenceRecord {
            key,
            directory,
            pdf,
            reference,
        });
    }
    Ok((records, warnings))
}

pub fn list(
    library: &Path,
    query: Option<&str>,
    tags: &[String],
    limit: Option<usize>,
) -> Result<(Vec<ReferenceRecord>, Vec<String>)> {
    let query = query.map(str::trim).filter(|query| !query.is_empty());
    let (mut records, warnings) = records(library)?;
    records.retain(|record| {
        let matches_query = query.is_none_or(|query| {
            let query = query.to_lowercase();
            let searchable = format!(
                "{} {} {} {} {} {} {} {} {} {}",
                record.key,
                record.reference.title,
                record.reference.authors.join(" "),
                record.reference.tags.join(" "),
                record.reference.doi.as_deref().unwrap_or_default(),
                record.reference.arxiv.as_deref().unwrap_or_default(),
                record.reference.publisher.as_deref().unwrap_or_default(),
                record.reference.series.as_deref().unwrap_or_default(),
                record.reference.edition.as_deref().unwrap_or_default(),
                record.reference.isbn.join(" "),
            )
            .to_lowercase();
            searchable.contains(&query)
        });
        let matches_tag = tags.is_empty()
            || record
                .reference
                .tags
                .iter()
                .any(|tag| tags.iter().any(|wanted| tag.eq_ignore_ascii_case(wanted)));
        matches_query && matches_tag
    });
    if let Some(limit) = limit {
        records.truncate(limit);
    }
    Ok((records, warnings))
}

pub fn show(library: &Path, key: &str) -> Result<ReferenceRecord> {
    let directory = reference_dir(library, key)?;
    let reference = metadata::read_info(&directory)?;
    let pdf = find_pdf(&directory, &reference);
    Ok(ReferenceRecord {
        key: key.to_string(),
        directory,
        pdf,
        reference,
    })
}

pub fn reference_dir(library: &Path, key: &str) -> Result<PathBuf> {
    anyhow::ensure!(
        !key.is_empty()
            && !Path::new(key).is_absolute()
            && Path::new(key).components().count() == 1,
        "Reference key must be one library directory name"
    );
    let directory = library.join(key);
    anyhow::ensure!(
        directory.is_dir() && directory.join("info.toml").is_file(),
        "Reference not found: {key}"
    );
    Ok(directory)
}

pub fn find_pdf(directory: &Path, reference: &Reference) -> Option<PathBuf> {
    if let Some(file) = reference.files.first() {
        let path = directory.join(file);
        if path.is_file() {
            return Some(path);
        }
    }
    std::fs::read_dir(directory)
        .ok()?
        .flatten()
        .find_map(|entry| {
            let path = entry.path();
            path.is_file().then_some(path).filter(|path| {
                path.extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
            })
        })
}

pub fn citation(key: &str, format: &str) -> Result<String> {
    match format {
        "plain" => Ok(key.to_string()),
        "latex" => Ok(format!("\\cite{{{key}}}")),
        "typst" => Ok(format!("@{key}")),
        _ => anyhow::bail!("Unknown citation format: {format} (expected plain, latex, or typst)"),
    }
}

pub fn changes(before: &Reference, after: &Reference) -> Result<Vec<FieldChange>> {
    let before = serde_json::to_value(before)?;
    let after = serde_json::to_value(after)?;
    let before = before
        .as_object()
        .context("Reference did not serialize as an object")?;
    let after = after
        .as_object()
        .context("Reference did not serialize as an object")?;
    Ok(before
        .iter()
        .filter_map(|(field, value)| {
            let next = after.get(field)?;
            (value != next).then(|| FieldChange {
                field: field.clone(),
                before: value.clone(),
                after: next.clone(),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{citation, list, print_json, reference_dir};

    #[test]
    fn citation_formats_are_noninteractive() {
        assert_eq!(
            citation("synthetic-2026-paper", "plain").unwrap(),
            "synthetic-2026-paper"
        );
        assert_eq!(
            citation("synthetic-2026-paper", "latex").unwrap(),
            "\\cite{synthetic-2026-paper}"
        );
        assert_eq!(
            citation("synthetic-2026-paper", "typst").unwrap(),
            "@synthetic-2026-paper"
        );
        assert!(citation("synthetic-2026-paper", "unknown").is_err());
    }

    #[test]
    fn reference_keys_cannot_escape_the_library() {
        let library = tempfile::tempdir().unwrap();
        assert!(reference_dir(library.path(), "../outside").is_err());
        assert!(reference_dir(library.path(), "/outside").is_err());
    }

    #[test]
    fn list_filters_synthetic_references_deterministically() {
        let library = tempfile::tempdir().unwrap();
        let paper = library.path().join("synthetic-2026-paper");
        std::fs::create_dir(&paper).unwrap();
        std::fs::write(
            paper.join("info.toml"),
            "title = \"Synthetic Retrieval Paper\"\nauthors = [\"Example, Alice\"]\nyear = 2026\ntags = [\"retrieval\"]\nfiles = []\n",
        )
        .unwrap();

        let (records, warnings) = list(
            library.path(),
            Some("synthetic"),
            &["retrieval".to_string()],
            None,
        )
        .unwrap();
        assert!(warnings.is_empty());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].key, "synthetic-2026-paper");
    }

    #[test]
    fn json_response_serializes() {
        print_json(serde_json::json!({"synthetic": true})).unwrap();
    }
}
