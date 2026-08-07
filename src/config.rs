use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum ExternalCommand {
    Program(String),
    Args(Vec<String>),
}

impl ExternalCommand {
    fn build(&self) -> Result<Command> {
        let (program, args) = match self {
            Self::Program(program) => (program.as_str(), &[][..]),
            Self::Args(parts) => {
                let (program, args) = parts
                    .split_first()
                    .context("External command cannot be empty")?;
                (program.as_str(), args)
            }
        };
        anyhow::ensure!(!program.is_empty(), "External command cannot be empty");

        let mut command = Command::new(program);
        command.args(args);
        Ok(command)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub library: Option<String>,
    pub editor: Option<ExternalCommand>,
    pub reader: Option<ExternalCommand>,
    pub browser: Option<ExternalCommand>,
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
                browser: None,
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

    pub fn editor_command(&self) -> Result<Command> {
        self.command(&["GRIM_EDITOR", "EDITOR"], self.editor.as_ref(), "vi")
    }

    pub fn reader_command(&self) -> Result<Command> {
        self.command(&["GRIM_READER"], self.reader.as_ref(), default_opener())
    }

    pub fn browser_command(&self) -> Result<Command> {
        self.command(
            &["GRIM_BROWSER", "BROWSER"],
            self.browser.as_ref(),
            default_opener(),
        )
    }

    /// Resolve an external command: the first set `env_vars` wins (the
    /// `GRIM_`-prefixed name takes precedence over the conventional one), then
    /// the config-file value, then the built-in default.
    fn command(
        &self,
        env_vars: &[&str],
        configured: Option<&ExternalCommand>,
        default: &str,
    ) -> Result<Command> {
        for var in env_vars {
            if let Ok(program) = std::env::var(var) {
                return ExternalCommand::Program(program).build();
            }
        }
        configured
            .cloned()
            .unwrap_or_else(|| ExternalCommand::Program(default.to_string()))
            .build()
    }

    fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("grimoire")
            .join("config.toml")
    }
}

fn default_opener() -> &'static str {
    if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    }
}

#[cfg(test)]
mod tests {
    use super::{ExternalCommand, default_opener};
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct TestConfig {
        command: ExternalCommand,
    }

    #[test]
    fn external_command_accepts_program_or_argument_list() {
        let program: TestConfig = toml::from_str("command = \"open\"").unwrap();
        let command = program.command.build().unwrap();
        assert_eq!(command.get_program(), "open");
        assert_eq!(command.get_args().count(), 0);

        let args: TestConfig = toml::from_str("command = [\"open\", \"-a\", \"Preview\"]").unwrap();
        let command = args.command.build().unwrap();
        assert_eq!(command.get_program(), "open");
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["-a", "Preview"]);
    }

    #[test]
    fn external_command_rejects_empty_values() {
        assert!(ExternalCommand::Program(String::new()).build().is_err());
        assert!(ExternalCommand::Args(Vec::new()).build().is_err());
    }

    #[test]
    fn default_opener_matches_the_platform() {
        #[cfg(target_os = "macos")]
        assert_eq!(default_opener(), "open");

        #[cfg(not(target_os = "macos"))]
        assert_eq!(default_opener(), "xdg-open");
    }
}
