use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::metadata;
use crate::model::Reference;
use crate::storage;

struct PolarisRecord {
    title: String,
    author: Option<String>,
    filename: String,
    tags: Vec<String>,
}

pub fn run(library: &Path, force: bool) -> Result<()> {
    anyhow::ensure!(
        cfg!(target_os = "macos"),
        "import-polaris is only supported on macOS (Polaris is a macOS-only app)"
    );

    let polaris_dir = find_polaris_dir()?;
    let db_path = polaris_dir.join("library.db");
    let docs_dir = polaris_dir.join("Documents");

    anyhow::ensure!(
        db_path.exists(),
        "Polaris database not found at {}",
        db_path.display()
    );

    let conn = Connection::open(&db_path)?;
    let records = read_records(&conn)?;

    if records.is_empty() {
        println!("No documents found in Polaris library.");
        return Ok(());
    }

    std::fs::create_dir_all(library)?;

    let existing = storage::list_ref_dirs(library)?;
    let existing_map: std::collections::HashMap<String, PathBuf> = existing
        .iter()
        .filter_map(|dir| {
            metadata::read_info(dir)
                .ok()
                .and_then(|r| r.files.first().map(|f| (f.clone(), dir.clone())))
        })
        .collect();

    let mut imported = 0;
    let mut updated = 0;
    let mut skipped = 0;
    for record in &records {
        if let Some(existing_dir) = existing_map.get(&record.filename) {
            if force {
                let authors: Vec<String> = record
                    .author
                    .as_deref()
                    .map(|a| {
                        a.split(';')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();

                let mut reference = metadata::read_info(existing_dir)?;
                reference.title = record.title.clone();
                reference.authors = authors;
                reference.tags = record.tags.clone();
                metadata::write_info(existing_dir, &reference)?;

                println!("  Updated: {}", reference.title);
                updated += 1;
            } else {
                skipped += 1;
            }
            continue;
        }

        let pdf_path = docs_dir.join(&record.filename);
        if !pdf_path.exists() {
            eprintln!("Skipping {}: PDF not found", record.filename);
            continue;
        }

        let authors: Vec<String> = record
            .author
            .as_deref()
            .map(|a| {
                a.split(';')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let reference = Reference {
            title: record.title.clone(),
            authors,
            year: None,
            doi: None,
            arxiv: None,
            journal: None,
            tags: record.tags.clone(),
            files: vec![record.filename.clone()],
            r#abstract: None,
        };

        let ref_dir = storage::create_ref_dir(library, &reference)?;
        storage::copy_pdf(&pdf_path, &ref_dir)?;
        metadata::write_info(&ref_dir, &reference)?;

        println!("  Imported: {}", reference.title);
        imported += 1;
    }

    println!("\nImported {} papers from Polaris.", imported);
    if updated > 0 {
        println!("Updated {} existing entries.", updated);
    }
    if skipped > 0 {
        println!("Skipped {} already in library.", skipped);
    }
    Ok(())
}

fn find_polaris_dir() -> Result<PathBuf> {
    let app_support = dirs::home_dir()
        .context("Cannot determine home directory")?
        .join("Library/Application Support/Polaris");

    if app_support.join("library.db").exists() {
        return Ok(app_support);
    }

    let icloud = dirs::home_dir()
        .unwrap()
        .join("Library/Mobile Documents/com~apple~CloudDocs/Polaris");

    if icloud.join("library.db").exists() {
        return Ok(icloud);
    }

    anyhow::bail!(
        "Polaris library not found. Checked:\n  {}\n  {}",
        app_support.display(),
        icloud.display()
    )
}

fn read_records(conn: &Connection) -> Result<Vec<PolarisRecord>> {
    let mut stmt = conn.prepare("SELECT title, author, filename, tags FROM document")?;
    let rows = stmt.query_map([], |row| {
        let tags_json: Option<String> = row.get(3)?;
        let tags: Vec<String> = tags_json
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();

        Ok(PolarisRecord {
            title: row.get(0)?,
            author: row.get(1)?,
            filename: row.get(2)?,
            tags,
        })
    })?;

    let mut records = Vec::new();
    for row in rows {
        records.push(row?);
    }
    Ok(records)
}
