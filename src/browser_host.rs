//! Browser integration via the WebExtension native-messaging protocol.
//!
//! Browser extensions run in a sandbox and cannot spawn `grimoire` directly.
//! The only supported bridge is native messaging: the browser launches a host
//! process (this module, via `grimoire browser-host`) and exchanges messages
//! over stdio. Each message is a little-endian `u32` length prefix followed by
//! that many bytes of UTF-8 JSON.
//!
//! Request (from the extension):
//! ```json
//! { "action": "add", "input": "https://arxiv.org/abs/1706.03762", "force": false }
//! { "action": "ping" }
//! ```
//! Response (to the extension):
//! ```json
//! { "ok": true, "action": "add", "status": "added", "keys": ["vaswani-2017-..."] }
//! { "ok": false, "error": "…" }
//! ```

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AddOptions;
use crate::config::Config;

/// The native-messaging host name. Extensions connect with
/// `chrome.runtime.connectNative("com.grimoire.host")` (or the `browser.*`
/// equivalent). The manifest file is named `<host>.json`.
pub const HOST_NAME: &str = "com.grimoire.host";

/// Chrome/Firefox cap a single native message at 1 MiB coming from the
/// extension. Reject anything larger rather than trying to allocate it.
const MAX_MESSAGE_LEN: u32 = 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
enum Request {
    /// Liveness/handshake check used by the extensions on startup.
    Ping,
    /// Import a DOI, arXiv ID, PubMed ID, or URL into the library.
    Add {
        input: String,
        #[serde(default)]
        force: bool,
    },
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum Response {
    Ping {
        ok: bool,
        action: &'static str,
        version: &'static str,
    },
    Add {
        ok: bool,
        action: &'static str,
        input: String,
        status: &'static str,
        keys: Vec<String>,
    },
    Error {
        ok: bool,
        error: String,
    },
}

impl Response {
    fn error(message: impl Into<String>) -> Self {
        Response::Error {
            ok: false,
            error: message.into(),
        }
    }
}

/// Run the native-messaging host loop until the browser closes the pipe (EOF).
pub fn run(config: &Config) -> Result<()> {
    let library = config.library_dir();
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();

    while let Some(message) = read_message(&mut reader)? {
        let response = handle(&library, &message);
        write_message(&mut writer, &response)?;
    }
    Ok(())
}

/// Dispatch one decoded message. Parse and processing failures become an error
/// response rather than tearing down the whole host connection.
fn handle(library: &Path, message: &[u8]) -> Response {
    let request: Request = match serde_json::from_slice(message) {
        Ok(request) => request,
        Err(error) => {
            // Surface the raw action name when possible to aid debugging.
            let action = serde_json::from_slice::<Value>(message)
                .ok()
                .and_then(|value| {
                    value
                        .get("action")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
            return Response::error(match action {
                Some(action) => format!("Unsupported or malformed request '{action}': {error}"),
                None => format!("Malformed request: {error}"),
            });
        }
    };

    match request {
        Request::Ping => Response::Ping {
            ok: true,
            action: "ping",
            version: env!("CARGO_PKG_VERSION"),
        },
        Request::Add { input, force } => add(library, &input, force),
    }
}

fn add(library: &Path, input: &str, force: bool) -> Response {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Response::error("No input provided");
    }
    match crate::cmd_add_capture(library, trimmed, force, &AddOptions::default()) {
        Ok(keys) => Response::Add {
            ok: true,
            action: "add",
            input: trimmed.to_string(),
            status: if keys.is_empty() { "skipped" } else { "added" },
            keys,
        },
        Err(error) => Response::error(format!("{error:#}")),
    }
}

/// Read one length-prefixed message. Returns `Ok(None)` on a clean EOF (the
/// browser closed the port), which ends the host loop normally.
fn read_message(reader: &mut impl Read) -> Result<Option<Vec<u8>>> {
    let mut length = [0u8; 4];
    match reader.read_exact(&mut length) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error).context("Failed to read native message length"),
    }
    let length = u32::from_le_bytes(length);
    anyhow::ensure!(
        length <= MAX_MESSAGE_LEN,
        "Native message length {length} exceeds the {MAX_MESSAGE_LEN}-byte limit"
    );
    let mut buffer = vec![0u8; length as usize];
    reader
        .read_exact(&mut buffer)
        .context("Failed to read native message body")?;
    Ok(Some(buffer))
}

/// Write one length-prefixed JSON response and flush so the browser sees it.
fn write_message(writer: &mut impl Write, response: &Response) -> Result<()> {
    let payload = serde_json::to_vec(response)?;
    let length = u32::try_from(payload.len())
        .context("Response exceeds the maximum native message length")?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

/// A browser family that supports native messaging, with the per-user directory
/// where its host manifests live and the allow-list key it expects.
struct Target {
    /// Directory (relative to the user's home) holding `NativeMessagingHosts`.
    dirs: Vec<PathBuf>,
    /// `true` for Chromium-family browsers (`allowed_origins`, IDs formatted as
    /// `chrome-extension://<id>/`); `false` for Firefox (`allowed_extensions`).
    chromium: bool,
}

/// Native-messaging manifest. Chromium browsers read `allowed_origins`; Firefox
/// reads `allowed_extensions`. Only one is serialized per target.
#[derive(Serialize)]
struct Manifest<'a> {
    name: &'a str,
    description: &'a str,
    path: String,
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    allowed_origins: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    allowed_extensions: Option<Vec<String>>,
}

/// Write `com.grimoire.host.json` into every browser manifest directory that
/// exists (or can be created) for the current user, returning the paths written.
pub fn install_manifests(binary: &Path, extension_ids: &[String]) -> Result<Vec<PathBuf>> {
    let binary = binary
        .canonicalize()
        .unwrap_or_else(|_| binary.to_path_buf());
    let mut written = Vec::new();
    for target in manifest_targets() {
        let allowed_origins = target.chromium.then(|| {
            extension_ids
                .iter()
                .map(|id| format!("chrome-extension://{id}/"))
                .collect::<Vec<_>>()
        });
        let allowed_extensions = (!target.chromium).then(|| extension_ids.to_vec());
        let manifest = Manifest {
            name: HOST_NAME,
            description: "Grimoire scholarly library importer",
            path: binary.to_string_lossy().to_string(),
            kind: "stdio",
            allowed_origins,
            allowed_extensions,
        };
        let body = serde_json::to_string_pretty(&manifest)?;
        for dir in &target.dirs {
            // Only install where the browser's profile root already exists, so we
            // don't scatter directories for browsers that aren't installed.
            let Some(parent) = dir.parent() else { continue };
            if !parent.exists() {
                continue;
            }
            std::fs::create_dir_all(dir)
                .with_context(|| format!("Failed to create {}", dir.display()))?;
            let path = dir.join(format!("{HOST_NAME}.json"));
            std::fs::write(&path, &body)
                .with_context(|| format!("Failed to write {}", path.display()))?;
            written.push(path);
        }
    }
    Ok(written)
}

/// Per-platform native-messaging manifest directories for the supported
/// browsers. Safari does not use this mechanism; its extension talks to grimoire
/// through the app wrapper produced by `safari-web-extension-converter` (see the
/// browser extension README).
fn manifest_targets() -> Vec<Target> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };

    #[cfg(target_os = "macos")]
    let (chromium_roots, firefox_root): (Vec<PathBuf>, PathBuf) = {
        let app = home.join("Library/Application Support");
        (
            vec![
                app.join("Google/Chrome"),
                app.join("Chromium"),
                app.join("Microsoft Edge"),
                app.join("BraveSoftware/Brave-Browser"),
                app.join("Vivaldi"),
            ],
            home.join("Library/Application Support/Mozilla"),
        )
    };

    #[cfg(not(target_os = "macos"))]
    let (chromium_roots, firefox_root): (Vec<PathBuf>, PathBuf) = {
        let config = home.join(".config");
        (
            vec![
                config.join("google-chrome"),
                config.join("chromium"),
                config.join("microsoft-edge"),
                config.join("BraveSoftware/Brave-Browser"),
                config.join("vivaldi"),
            ],
            home.join(".mozilla"),
        )
    };

    let mut targets = vec![Target {
        dirs: chromium_roots
            .into_iter()
            .map(|root| root.join("NativeMessagingHosts"))
            .collect(),
        chromium: true,
    }];
    targets.push(Target {
        dirs: vec![firefox_root.join("native-messaging-hosts")],
        chromium: false,
    });
    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn encode(json: &str) -> Vec<u8> {
        let mut buffer = (json.len() as u32).to_le_bytes().to_vec();
        buffer.extend_from_slice(json.as_bytes());
        buffer
    }

    #[test]
    fn reads_and_decodes_a_length_prefixed_message() {
        let framed = encode(r#"{"action":"ping"}"#);
        let mut cursor = Cursor::new(framed);
        let message = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(message, br#"{"action":"ping"}"#);
        assert!(read_message(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn ping_reports_version() {
        let library = Path::new("/tmp/does-not-matter");
        let response = handle(library, br#"{"action":"ping"}"#);
        let mut out = Vec::new();
        write_message(&mut out, &response).unwrap();
        let body = &out[4..];
        let value: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(value["ok"], serde_json::json!(true));
        assert_eq!(value["action"], serde_json::json!("ping"));
        assert_eq!(
            value["version"],
            serde_json::json!(env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn malformed_request_becomes_an_error_response() {
        let library = Path::new("/tmp/does-not-matter");
        let response = handle(library, br#"{"action":"teleport"}"#);
        match response {
            Response::Error { ok, error } => {
                assert!(!ok);
                assert!(error.contains("teleport"), "{error}");
            }
            other => panic!("expected error response, got {other:?}"),
        }
    }

    #[test]
    fn rejects_oversized_messages() {
        let mut framed = (MAX_MESSAGE_LEN + 1).to_le_bytes().to_vec();
        framed.push(0);
        let mut cursor = Cursor::new(framed);
        assert!(read_message(&mut cursor).is_err());
    }

    #[test]
    fn empty_add_input_is_rejected() {
        let library = Path::new("/tmp/does-not-matter");
        let response = add(library, "   ", false);
        matches!(response, Response::Error { .. })
            .then_some(())
            .expect("blank input should error");
    }
}
