use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use serde::Serialize;

use crate::metadata;
use crate::storage;

pub struct Index {
    conn: Connection,
}

impl Index {
    pub fn open(library: &Path) -> Result<Self> {
        let db_path = library.join(".grimoire.db");
        let conn = Connection::open(&db_path)
            .with_context(|| format!("Failed to open index at {}", db_path.display()))?;

        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;

        let has_fulltext: bool = conn
            .prepare("PRAGMA table_info(reference)")
            .and_then(|mut stmt| {
                stmt.query_map([], |row| row.get::<_, String>(1))
                    .map(|rows| rows.flatten().any(|name| name == "fulltext"))
            })
            .unwrap_or(false);

        if !has_fulltext {
            conn.execute_batch(
                "DROP TRIGGER IF EXISTS reference_ai;
                 DROP TRIGGER IF EXISTS reference_ad;
                 DROP TRIGGER IF EXISTS reference_au;
                 DROP TABLE IF EXISTS reference_fts;
                 DROP TABLE IF EXISTS reference;",
            )?;
        }

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS reference (
                dir_name TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                authors TEXT NOT NULL DEFAULT '',
                year INTEGER,
                doi TEXT,
                arxiv TEXT,
                journal TEXT,
                tags TEXT NOT NULL DEFAULT '',
                abstract_text TEXT,
                files TEXT NOT NULL DEFAULT '',
                fulltext TEXT
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS reference_fts USING fts5(
                title, authors, abstract_text, tags, fulltext,
                content='reference',
                content_rowid='rowid'
            );

            CREATE TRIGGER IF NOT EXISTS reference_ai AFTER INSERT ON reference BEGIN
                INSERT INTO reference_fts(rowid, title, authors, abstract_text, tags, fulltext)
                VALUES (new.rowid, new.title, new.authors, new.abstract_text, new.tags, new.fulltext);
            END;

            CREATE TRIGGER IF NOT EXISTS reference_ad AFTER DELETE ON reference BEGIN
                INSERT INTO reference_fts(reference_fts, rowid, title, authors, abstract_text, tags, fulltext)
                VALUES ('delete', old.rowid, old.title, old.authors, old.abstract_text, old.tags, old.fulltext);
            END;

            CREATE TRIGGER IF NOT EXISTS reference_au AFTER UPDATE ON reference BEGIN
                INSERT INTO reference_fts(reference_fts, rowid, title, authors, abstract_text, tags, fulltext)
                VALUES ('delete', old.rowid, old.title, old.authors, old.abstract_text, old.tags, old.fulltext);
                INSERT INTO reference_fts(rowid, title, authors, abstract_text, tags, fulltext)
                VALUES (new.rowid, new.title, new.authors, new.abstract_text, new.tags, new.fulltext);
            END;"
        )?;

        Ok(Self { conn })
    }

    pub fn reindex(&self, library: &Path) -> Result<usize> {
        let dirs = storage::list_ref_dirs(library)?;

        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM reference", [])?;

        let mut count = 0;
        for dir in &dirs {
            let reference = match metadata::read_info(dir) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let dir_name = dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let fulltext = find_pdf(dir, &reference).and_then(|p| metadata::extract_pdf_text(&p));

            tx.execute(
                "INSERT INTO reference (dir_name, title, authors, year, doi, arxiv, journal, tags, abstract_text, files, fulltext)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(dir_name) DO UPDATE SET
                    title=?2, authors=?3, year=?4, doi=?5, arxiv=?6, journal=?7, tags=?8, abstract_text=?9, files=?10, fulltext=?11",
                params![
                    dir_name,
                    reference.title,
                    reference.authors.join("; "),
                    reference.year,
                    reference.doi,
                    reference.arxiv,
                    reference.journal,
                    reference.tags.join(", "),
                    reference.r#abstract,
                    reference.files.join(", "),
                    fulltext,
                ],
            )?;
            count += 1;
        }

        tx.commit()?;
        Ok(count)
    }

    #[allow(dead_code)]
    pub fn upsert(&self, dir_name: &str, r: &crate::model::Reference) -> Result<()> {
        self.upsert_with_fulltext(dir_name, r, None)
    }

    pub fn upsert_with_fulltext(
        &self,
        dir_name: &str,
        r: &crate::model::Reference,
        fulltext: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO reference (dir_name, title, authors, year, doi, arxiv, journal, tags, abstract_text, files, fulltext)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(dir_name) DO UPDATE SET
                title=?2, authors=?3, year=?4, doi=?5, arxiv=?6, journal=?7, tags=?8, abstract_text=?9, files=?10, fulltext=?11",
            params![
                dir_name,
                r.title,
                r.authors.join("; "),
                r.year,
                r.doi,
                r.arxiv,
                r.journal,
                r.tags.join(", "),
                r.r#abstract,
                r.files.join(", "),
                fulltext,
            ],
        )?;
        Ok(())
    }

    pub fn search_with_limit(&self, query: &str, limit: Option<usize>) -> Result<Vec<SearchHit>> {
        anyhow::ensure!(!query.trim().is_empty(), "Search query cannot be empty");
        let fts_query = query
            .split_whitespace()
            .map(|w| format!("\"{}\"", w.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" ");

        let mut stmt = self.conn.prepare(
            "SELECT r.dir_name, r.title, r.authors, r.year,
                    snippet(reference_fts, 0, '{{', '}}', '...', 20) as snippet
             FROM reference_fts fts
             JOIN reference r ON r.rowid = fts.rowid
             WHERE reference_fts MATCH ?1
             ORDER BY rank",
        )?;

        let hits = stmt.query_map(params![fts_query], |row| {
            Ok(SearchHit {
                dir_name: row.get(0)?,
                title: row.get(1)?,
                authors: row.get(2)?,
                year: row.get(3)?,
                snippet: row.get(4)?,
            })
        })?;

        let mut results = Vec::new();
        for hit in hits {
            results.push(hit?);
        }
        if let Some(limit) = limit {
            results.truncate(limit);
        }
        Ok(results)
    }
}

fn find_pdf(dir: &Path, r: &crate::model::Reference) -> Option<std::path::PathBuf> {
    if let Some(f) = r.files.first() {
        let p = dir.join(f);
        if p.exists() {
            return Some(p);
        }
    }
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let p = e.path();
        if p.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
        {
            Some(p)
        } else {
            None
        }
    })
}

#[derive(Debug, Serialize)]
pub struct SearchHit {
    pub dir_name: String,
    pub title: String,
    pub authors: String,
    pub year: Option<u16>,
    pub snippet: String,
}
