use std::path::PathBuf;

use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub library: Option<String>,
    pub editor: Option<String>,
    pub reader: Option<String>,
    pub theme: Option<String>,
    pub layout: Option<String>,
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            Ok(toml::from_str(&contents)?)
        } else {
            Ok(Self {
                library: None,
                editor: None,
                reader: None,
                theme: None,
                layout: None,
            })
        }
    }

    pub fn library_dir(&self) -> PathBuf {
        if let Ok(val) = std::env::var("GRIM_LIBRARY") {
            return PathBuf::from(val);
        }
        if let Some(ref lib) = self.library {
            let expanded = shellexpand::tilde(lib);
            return PathBuf::from(expanded.as_ref());
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Papers")
    }

    pub fn editor(&self) -> String {
        std::env::var("EDITOR")
            .ok()
            .or_else(|| self.editor.clone())
            .unwrap_or_else(|| "vi".to_string())
    }

    pub fn reader(&self) -> String {
        std::env::var("GRIM_READER")
            .ok()
            .or_else(|| self.reader.clone())
            .unwrap_or_else(default_reader)
    }

    fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("grimoire")
            .join("config.toml")
    }
}

fn default_reader() -> String {
    if cfg!(target_os = "macos") {
        "open".to_string()
    } else {
        "xdg-open".to_string()
    }
}
