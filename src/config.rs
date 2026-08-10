use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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
    pub theme_catalog: Option<String>,
    pub layout: Option<String>,
    pub semantic_results: Option<usize>,
    #[serde(default)]
    pub embedding: EmbeddingConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct EmbeddingConfig {
    pub repo: String,
    pub revision: String,
    pub model_file: String,
    pub external_files: Vec<String>,
    pub tokenizer_file: String,
    pub config_file: String,
    pub special_tokens_map_file: String,
    pub tokenizer_config_file: String,
    pub pooling: String,
    pub output: Option<EmbeddingOutput>,
    pub query_template: String,
    pub document_template: String,
    pub max_length: usize,
    pub batch_size: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum EmbeddingOutput {
    Name(String),
    Index(usize),
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            repo: "onnx-community/embeddinggemma-300m-ONNX".to_string(),
            revision: "5090578d9565bb06545b4552f76e6bc2c93e4a66".to_string(),
            model_file: "onnx/model_q4.onnx".to_string(),
            external_files: vec!["onnx/model_q4.onnx_data".to_string()],
            tokenizer_file: "tokenizer.json".to_string(),
            config_file: "config.json".to_string(),
            special_tokens_map_file: "special_tokens_map.json".to_string(),
            tokenizer_config_file: "tokenizer_config.json".to_string(),
            pooling: "mean".to_string(),
            output: Some(EmbeddingOutput::Name("sentence_embedding".to_string())),
            query_template: "task: search result | query: {query}".to_string(),
            document_template: "title: {title} | text: {text}".to_string(),
            max_length: 2048,
            batch_size: 32,
        }
    }
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
                theme_catalog: None,
                layout: None,
                semantic_results: None,
                embedding: EmbeddingConfig::default(),
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

    pub fn semantic_results(&self) -> usize {
        self.semantic_results.unwrap_or(10).max(1)
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
    use super::{Config, EmbeddingOutput, ExternalCommand, default_opener};
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

    #[test]
    fn semantic_result_count_defaults_and_never_reaches_zero() {
        let default: Config = toml::from_str("").unwrap();
        let configured: Config = toml::from_str("semantic_results = 25").unwrap();
        let zero: Config = toml::from_str("semantic_results = 0").unwrap();

        assert_eq!(default.semantic_results(), 10);
        assert_eq!(configured.semantic_results(), 25);
        assert_eq!(zero.semantic_results(), 1);
    }

    #[test]
    fn embedding_profile_can_override_model_and_output_index() {
        let config: Config = toml::from_str(
            r#"
            [embedding]
            repo = "synthetic/model"
            revision = "synthetic-revision"
            model_file = "model.onnx"
            output = 2
            "#,
        )
        .unwrap();

        assert_eq!(config.embedding.repo, "synthetic/model");
        assert_eq!(config.embedding.model_file, "model.onnx");
        assert_eq!(config.embedding.output, Some(EmbeddingOutput::Index(2)));
        assert_eq!(config.embedding.pooling, "mean");
    }
}
