use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::metadata;
use crate::model::Reference;
use crate::storage;

#[derive(Debug, Serialize)]
pub struct Candidate {
    pub key: String,
    pub title: String,
    pub doi: Option<String>,
    pub metadata_score: u32,
    pub has_pdf: bool,
    pub recommended: bool,
}

#[derive(Debug, Serialize)]
pub struct DuplicateGroup {
    pub candidates: Vec<Candidate>,
}

#[derive(Debug, Serialize)]
pub struct DedupReport {
    pub groups: Vec<DuplicateGroup>,
    pub applied: bool,
    pub removed: Vec<RemovedReference>,
}

#[derive(Debug, Serialize)]
pub struct RemovedReference {
    pub key: String,
    pub kept: String,
    pub trash_path: PathBuf,
}

pub fn run(library: &Path, keep: &[String], apply: bool) -> Result<DedupReport> {
    let paths = find_duplicate_paths(library)?;
    let groups = paths
        .iter()
        .map(|group| describe(group))
        .collect::<Vec<_>>();

    if !apply {
        anyhow::ensure!(keep.is_empty(), "--keep requires --apply");
        return Ok(DedupReport {
            groups,
            applied: false,
            removed: Vec::new(),
        });
    }
    anyhow::ensure!(!keep.is_empty(), "--apply requires at least one --keep key");

    let mut selected = HashMap::new();
    for key in keep {
        let group_index = paths
            .iter()
            .position(|group| {
                group.iter().any(|path| {
                    path.file_name()
                        .is_some_and(|name| name == std::ffi::OsStr::new(key))
                })
            })
            .with_context(|| format!("{key} is not in a duplicate group"))?;
        anyhow::ensure!(
            selected.insert(group_index, key.clone()).is_none(),
            "Only one --keep key may be selected per duplicate group"
        );
    }

    let trash_dir = library.join(".trash");
    std::fs::create_dir_all(&trash_dir)?;
    let mut removed = Vec::new();
    for (group_index, kept) in selected {
        for path in &paths[group_index] {
            let key = path
                .file_name()
                .context("Reference directory has no name")?
                .to_string_lossy()
                .to_string();
            if key == kept {
                continue;
            }
            let destination = unique_trash_path(&trash_dir, &key);
            std::fs::rename(path, &destination)?;
            removed.push(RemovedReference {
                key,
                kept: kept.clone(),
                trash_path: destination,
            });
        }
    }
    removed.sort_by(|a, b| a.key.cmp(&b.key));

    Ok(DedupReport {
        groups,
        applied: true,
        removed,
    })
}

fn describe(paths: &[PathBuf]) -> DuplicateGroup {
    let mut candidates = paths
        .iter()
        .map(|path| {
            let reference = metadata::read_info(path).expect("duplicate scanner parsed info.toml");
            Candidate {
                key: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                title: reference.title.clone(),
                doi: reference.doi.clone(),
                metadata_score: metadata_score(&reference),
                has_pdf: has_pdf(path),
                recommended: false,
            }
        })
        .collect::<Vec<_>>();
    if let Some(best) = candidates
        .iter()
        .enumerate()
        .max_by_key(|(_, candidate)| (candidate.has_pdf, candidate.metadata_score))
        .map(|(index, _)| index)
    {
        candidates[best].recommended = true;
    }
    DuplicateGroup { candidates }
}

fn has_pdf(path: &Path) -> bool {
    path.read_dir()
        .map(|entries| {
            entries.flatten().any(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
            })
        })
        .unwrap_or(false)
}

pub(crate) fn metadata_score(reference: &Reference) -> u32 {
    u32::from(!reference.title.is_empty())
        + u32::from(!reference.authors.is_empty())
        + u32::from(reference.year.is_some_and(|year| year != 0))
        + u32::from(reference.doi.is_some())
        + u32::from(reference.arxiv.is_some())
        + u32::from(reference.journal.is_some())
        + u32::from(reference.publisher.is_some())
        + u32::from(reference.edition.is_some())
        + u32::from(reference.series.is_some())
        + u32::from(!reference.isbn.is_empty())
        + u32::from(!reference.tags.is_empty())
        + u32::from(!reference.files.is_empty())
        + u32::from(reference.r#abstract.is_some())
}

pub(crate) fn find_duplicate_paths(library: &Path) -> Result<Vec<Vec<PathBuf>>> {
    let dirs = storage::list_ref_dirs(library)?;
    let mut parent = (0..dirs.len()).collect::<Vec<_>>();
    let mut titles = HashMap::new();
    let mut dois = HashMap::new();

    for (index, directory) in dirs.iter().enumerate() {
        let reference = match metadata::read_info(directory) {
            Ok(reference) => reference,
            Err(_) => continue,
        };
        union_value(
            &mut parent,
            &mut titles,
            normalized(&reference.title),
            index,
        );
        if let Some(doi) = reference.doi {
            union_value(&mut parent, &mut dois, normalized(&doi), index);
        }
    }

    let mut components: HashMap<usize, Vec<PathBuf>> = HashMap::new();
    for (index, directory) in dirs.into_iter().enumerate() {
        let root = find(&mut parent, index);
        components.entry(root).or_default().push(directory);
    }
    let mut groups = components
        .into_values()
        .filter(|group| group.len() > 1)
        .collect::<Vec<_>>();
    for group in &mut groups {
        group.sort();
    }
    groups.sort();
    Ok(groups)
}

fn normalized(value: &str) -> Option<String> {
    let value = value.trim().to_lowercase();
    (!value.is_empty()).then_some(value)
}

fn union_value(
    parent: &mut [usize],
    first: &mut HashMap<String, usize>,
    value: Option<String>,
    index: usize,
) {
    let Some(value) = value else { return };
    if let Some(previous) = first.get(&value) {
        union(parent, index, *previous);
    } else {
        first.insert(value, index);
    }
}

fn find(parent: &mut [usize], mut index: usize) -> usize {
    while parent[index] != index {
        parent[index] = parent[parent[index]];
        index = parent[index];
    }
    index
}

fn union(parent: &mut [usize], left: usize, right: usize) {
    let left = find(parent, left);
    let right = find(parent, right);
    if left != right {
        parent[left] = right;
    }
}

pub(crate) fn unique_trash_path(trash: &Path, key: &str) -> PathBuf {
    let mut candidate = trash.join(key);
    let mut suffix = 1;
    while candidate.exists() {
        candidate = trash.join(format!("{key}-{suffix}"));
        suffix += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::run;

    fn write_reference(library: &std::path::Path, key: &str, title: &str, doi: &str) {
        let directory = library.join(key);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(
            directory.join("info.toml"),
            format!(
                "title = {title:?}\nauthors = [\"Example, Alice\"]\nyear = 2026\ndoi = {doi:?}\ntags = []\nfiles = []\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn scans_and_only_removes_with_apply_and_keep() {
        let library = tempfile::tempdir().unwrap();
        write_reference(
            library.path(),
            "synthetic-a",
            "Synthetic Paper",
            "10.0/synthetic",
        );
        write_reference(
            library.path(),
            "synthetic-b",
            "Synthetic Paper",
            "10.0/synthetic",
        );

        let preview = run(library.path(), &[], false).unwrap();
        assert_eq!(preview.groups.len(), 1);
        assert!(library.path().join("synthetic-b").exists());

        let report = run(library.path(), &["synthetic-a".into()], true).unwrap();
        assert_eq!(report.removed.len(), 1);
        assert!(library.path().join("synthetic-a").exists());
        assert!(!library.path().join("synthetic-b").exists());
        assert!(report.removed[0].trash_path.exists());
    }
}
