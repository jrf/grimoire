use anyhow::{Context, Result};
use lopdf::Document;
use std::path::Path;

use crate::model::{Reference, ReferenceKind};

pub fn extract_from_pdf(path: &Path) -> Result<Reference> {
    let doc = Document::load(path).context("Failed to read PDF")?;
    let info = pdf_info(&doc);

    let filename = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();

    let title = info.title.unwrap_or_else(|| {
        path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".to_string())
    });

    let authors = match info.author {
        Some(a) if !a.is_empty() => parse_authors(&a),
        _ => vec![],
    };

    Ok(Reference {
        kind: ReferenceKind::Paper,
        title,
        authors,
        year: None,
        doi: None,
        arxiv: None,
        journal: None,
        edition: None,
        publisher: None,
        series: None,
        isbn: vec![],
        tags: vec![],
        files: vec![filename],
        r#abstract: None,
    })
}

pub fn read_info(dir: &Path) -> Result<Reference> {
    let path = dir.join("info.toml");
    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    Ok(toml::from_str(&contents)?)
}

pub fn write_info(dir: &Path, reference: &Reference) -> Result<()> {
    let path = dir.join("info.toml");
    let contents = toml::to_string_pretty(reference)?;
    atomic_write(&path, contents.as_bytes())
        .with_context(|| format!("Failed to write {}", path.display()))
}

/// Write `contents` to `path` atomically: stage into a sibling temp file, flush
/// it to disk, then rename over the destination. A crash or full disk mid-write
/// leaves either the old file or the complete new one — never a truncated
/// `info.toml` that would silently drop the reference on the next read.
fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("Failed to create temp file in {}", dir.display()))?;
    tmp.write_all(contents)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

struct PdfInfo {
    title: Option<String>,
    author: Option<String>,
}

fn pdf_info(doc: &Document) -> PdfInfo {
    let mut title = None;
    let mut author = None;

    if let Ok(info_dict) = doc.trailer.get(b"Info")
        && let Ok(info_ref) = info_dict.as_reference()
        && let Ok(info_obj) = doc.get_object(info_ref)
        && let Ok(dict) = info_obj.as_dict()
    {
        title = dict_string(dict, b"Title");
        author = dict_string(dict, b"Author");
    }

    PdfInfo { title, author }
}

fn dict_string(dict: &lopdf::Dictionary, key: &[u8]) -> Option<String> {
    dict.get(key).ok().and_then(|obj| match obj {
        lopdf::Object::String(bytes, _) => {
            let s = String::from_utf8_lossy(bytes).trim().to_string();
            if s.is_empty() { None } else { Some(s) }
        }
        _ => None,
    })
}

pub fn extract_pdf_text(path: &Path) -> Option<String> {
    const MAX_FILE_SIZE: u64 = 50 * 1024 * 1024; // 50 MB
    const MAX_PAGES: u32 = 100;

    let file_size = std::fs::metadata(path).ok()?.len();
    if file_size > MAX_FILE_SIZE {
        return None;
    }

    let doc = Document::load(path).ok()?;
    let page_numbers: Vec<u32> = doc
        .get_pages()
        .keys()
        .copied()
        .take(MAX_PAGES as usize)
        .collect();
    let text = doc.extract_text(&page_numbers).ok()?;
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn parse_authors(raw: &str) -> Vec<String> {
    if raw.contains(';') {
        raw.split(';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else if raw.contains(',') && raw.matches(',').count() >= 2 {
        raw.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else if raw.contains(" and ") {
        raw.split(" and ")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        vec![raw.trim().to_string()]
    }
}
