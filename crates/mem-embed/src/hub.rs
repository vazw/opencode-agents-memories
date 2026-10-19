//! Minimal HF-hub model acquisition: transport seam + atomic tmp+rename
//! writer.
//!
//! Scope (recorded divergence, a subset of the upstream downloader): three
//! file kinds (`config.json`, `tokenizer.json`, `onnx/model{suffix}.onnx`),
//! `resolve/main` URLs, reqwest's default redirect following (HF resolve URLs
//! redirect to the CDN), no auth, one file at a time, never a partial write.
//!
//! Path wall: every relative path handed to [`ensure_model_files`] is
//! validated *before* any fetch — a config-supplied path can never escape the
//! model directory.

use std::path::{Component, Path, PathBuf};

use crate::embedder::BoxFuture;
use crate::error::EmbedError;

/// HTTP seam for hub downloads (fake-injected in tests). `Debug` is required
/// so option structs holding `Option<Arc<dyn HubTransport>>` can derive it.
pub trait HubTransport: std::fmt::Debug + Send + Sync {
    fn get(&self, url: &str) -> BoxFuture<'_, Result<Vec<u8>, EmbedError>>;
}

/// Real reqwest-backed transport (redirects followed by default).
#[derive(Debug)]
pub struct ReqwestHubTransport {
    client: reqwest::Client,
}

impl ReqwestHubTransport {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl Default for ReqwestHubTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HubTransport for ReqwestHubTransport {
    fn get(&self, url: &str) -> BoxFuture<'_, Result<Vec<u8>, EmbedError>> {
        // Own the URL first: the returned future is bounded by `&self`, so it
        // must not borrow the `url` parameter.
        let url = url.to_owned();
        Box::pin(async move {
            let response =
                self.client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|error| EmbedError::HubTransport {
                        url: url.clone(),
                        message: error.to_string(),
                    })?;
            let status = response.status();
            if !status.is_success() {
                return Err(EmbedError::HubHttp {
                    url: url.clone(),
                    status: status.as_u16(),
                });
            }
            let bytes = response
                .bytes()
                .await
                .map_err(|error| EmbedError::HubTransport {
                    url: url.clone(),
                    message: error.to_string(),
                })?;
            Ok(bytes.to_vec())
        })
    }
}

/// Canonical HF resolve URL.
pub fn huggingface_file_url(model: &str, relative_path: &str) -> String {
    format!("https://huggingface.co/{model}/resolve/main/{relative_path}")
}

/// Fetch every missing `files` entry into `model_dir`.
///
/// Contract:
/// - every `file` must be relative and free of `..` components, else
///   [`EmbedError::HubPathEscape`] (checked for the whole list before the
///   first fetch);
/// - an existing destination file is never rewritten;
/// - each download is `{dir}/{file}.tmp` + `rename`, so a failure at any step
///   leaves the destination untouched and no tmp file behind.
pub async fn ensure_model_files(
    model: &str,
    model_dir: &Path,
    files: &[&str],
    transport: &dyn HubTransport,
) -> Result<(), EmbedError> {
    for file in files {
        validate_relative_path(file)?;
    }

    for file in files {
        let destination = model_dir.join(file);
        if destination.exists() {
            continue;
        }
        let url = huggingface_file_url(model, file);
        // Fetch first: a failed fetch must not leave a tmp file behind.
        let bytes = transport.get(&url).await?;
        write_atomic(&destination, &bytes).map_err(|message| EmbedError::HubTransport {
            url: url.clone(),
            message,
        })?;
    }
    Ok(())
}

/// The `{destination}.tmp` sibling path.
fn tmp_path(destination: &Path) -> PathBuf {
    let mut name = destination
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    destination.with_file_name(name)
}

/// Write `bytes` to `destination` atomically (tmp + rename).
fn write_atomic(destination: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let tmp = tmp_path(destination);
    std::fs::write(&tmp, bytes).map_err(|error| error.to_string())?;
    match std::fs::rename(&tmp, destination) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&tmp);
            Err(error.to_string())
        }
    }
}

/// Refuse absolute paths and any `..` component.
fn validate_relative_path(path: &str) -> Result<(), EmbedError> {
    let parsed = Path::new(path);
    let escapes = path.is_empty()
        || parsed.is_absolute()
        || parsed.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        });
    if escapes {
        return Err(EmbedError::HubPathEscape {
            path: path.to_string(),
        });
    }
    Ok(())
}
