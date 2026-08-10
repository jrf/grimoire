use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::model::Reference;
use crate::storage;

#[derive(Serialize)]
pub struct ValidateResult {
    pub total: usize,
    pub fixed: usize,
    pub issues: Vec<String>,
}

impl ValidateResult {
    pub fn summary(&self) -> String {
        if self.issues.is_empty() && self.fixed == 0 {
            format!("Library OK — {} references", self.total)
        } else if self.fixed > 0 && self.issues.is_empty() {
            format!("Fixed {} issues", self.fixed)
        } else if self.fixed > 0 {
            format!(
                "{} issues, fixed {}",
                self.issues.len() + self.fixed,
                self.fixed
            )
        } else {
            format!("{} issues found", self.issues.len())
        }
    }
}

pub fn validate(library: &Path, fix: bool) -> Result<ValidateResult> {
    let dirs = storage::list_ref_dirs(library)?;
    let total = dirs.len();
    let mut issues: Vec<String> = Vec::new();
    let mut fixed = 0u32;

    for ref_dir in &dirs {
        let dir_name = ref_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let toml_path = ref_dir.join("info.toml");
        // A single unreadable/malformed info.toml must not abort validation of
        // the rest of the library — report it as an issue and move on. (The tool
        // whose job is finding integrity problems has to survive one.)
        let content = match std::fs::read_to_string(&toml_path) {
            Ok(c) => c,
            Err(e) => {
                issues.push(format!("unreadable info.toml: {dir_name} ({e})"));
                continue;
            }
        };
        let reference: Reference = match toml::from_str(&content) {
            Ok(r) => r,
            Err(e) => {
                issues.push(format!("malformed info.toml: {dir_name} ({e})"));
                continue;
            }
        };

        if reference.title.is_empty() {
            issues.push(format!("missing title: {}", dir_name));
        }
        if reference.authors.is_empty() {
            issues.push(format!("missing authors: {}", dir_name));
        }
        if reference.year.is_none() {
            issues.push(format!("missing year: {}", dir_name));
        }

        for listed_file in &reference.files {
            let file_path = ref_dir.join(listed_file);
            if !file_path.exists() {
                issues.push(format!("listed but missing: {}/{}", dir_name, listed_file));
            } else if !is_pdf(&file_path) {
                if fix {
                    std::fs::remove_file(&file_path)?;
                    let new_content = content.replace(&format!("\"{}\"", listed_file), "");
                    let new_content = new_content.replace("files = [\n    \n]", "files = []");
                    std::fs::write(&toml_path, &new_content)?;
                    fixed += 1;
                } else {
                    issues.push(format!("not a PDF: {}/{}", dir_name, listed_file));
                }
            }
        }

        let has_pdf = std::fs::read_dir(ref_dir)?.filter_map(|e| e.ok()).any(|e| {
            let p = e.path();
            p.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
                && is_pdf(&p)
        });

        if !has_pdf && reference.files.is_empty() {
            issues.push(format!("no PDF: {}", dir_name));
        }

        for entry in std::fs::read_dir(ref_dir)?.filter_map(|e| e.ok()) {
            let fname = entry.file_name().to_string_lossy().to_string();
            if fname.starts_with("tmp") && fname != "info.toml" {
                if fix && fname.ends_with(".pdf") && is_pdf(&entry.path()) {
                    let proper = format!("{}.pdf", dir_name);
                    let new_path = ref_dir.join(&proper);
                    std::fs::rename(entry.path(), &new_path)?;
                    let new_content =
                        content.replace(&format!("\"{}\"", fname), &format!("\"{}\"", proper));
                    std::fs::write(&toml_path, &new_content)?;
                    fixed += 1;
                } else {
                    issues.push(format!("temp name: {}/{}", dir_name, fname));
                }
            }
        }
    }

    Ok(ValidateResult {
        total,
        fixed: fixed as usize,
        issues,
    })
}

pub fn run(library: &Path, fix: bool) -> Result<()> {
    let result = validate(library, fix)?;
    if result.issues.is_empty() {
        println!("{}", result.summary());
    } else {
        println!("{}", result.summary());
        for issue in &result.issues {
            println!("  {}", issue);
        }
    }
    Ok(())
}

fn is_pdf(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 4];
    use std::io::Read;
    if file.read_exact(&mut buf).is_err() {
        return false;
    }
    &buf == b"%PDF"
}
