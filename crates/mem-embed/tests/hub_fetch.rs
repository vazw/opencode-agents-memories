//! `hub::ensure_model_files` + `huggingface_file_url` (blueprint step 5).
//!
//! Every case runs against an in-test `HubTransport` fake, so no test ever
//! touches the network. What is pinned here is the acquisition contract:
//! only missing files are fetched, writes are atomic (fetch → `{file}.tmp` →
//! `rename`, never a partial destination file, never a leftover tmp), every
//! path is validated as model-dir-relative before any fetch, and transport
//! errors surface with the URL that failed.

mod common;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use mem_embed::error::EmbedError;
use mem_embed::hub::{HubTransport, ensure_model_files, huggingface_file_url};

const MODEL: &str = "org/tiny-embed";

/// One scripted fetch result.
#[derive(Debug)]
enum Reply {
    Bytes(Vec<u8>),
    Fail(EmbedError),
}

/// Records every requested URL and replays scripted replies in order.
#[derive(Debug, Default)]
struct FakeHub {
    calls: Mutex<Vec<String>>,
    replies: Mutex<VecDeque<Reply>>,
}

impl FakeHub {
    fn new(replies: Vec<Reply>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            replies: Mutex::new(replies.into()),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("calls lock").clone()
    }
}

impl HubTransport for FakeHub {
    fn get(&self, url: &str) -> mem_embed::BoxFuture<'_, Result<Vec<u8>, EmbedError>> {
        let url = url.to_string();
        Box::pin(async move {
            self.calls.lock().expect("calls lock").push(url.to_string());
            match self.replies.lock().expect("replies lock").pop_front() {
                Some(Reply::Bytes(bytes)) => Ok(bytes),
                Some(Reply::Fail(error)) => Err(error),
                None => Err(EmbedError::HubTransport {
                    url: url.to_string(),
                    message: "no scripted reply".to_string(),
                }),
            }
        })
    }
}

fn model_dir(label: &str) -> common::TempDir {
    let guard = common::temp_dir(label);
    std::fs::create_dir_all(guard.path().join(MODEL)).expect("create model dir");
    guard
}

/// Every path under `root`, so a stray tmp file cannot hide.
fn all_paths(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read_dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path.clone());
            }
            out.push(path);
        }
    }
    out
}

#[tokio::test]
async fn test_ensure_files_downloads_only_missing_files() {
    let cache = model_dir("hub-fetch-missing");
    let dir = cache.path().join(MODEL);
    common::write_file(&dir, "tokenizer.json", "local");
    let hub = FakeHub::new(vec![Reply::Bytes(b"{}".to_vec())]);

    ensure_model_files(MODEL, &dir, &["config.json", "tokenizer.json"], &hub)
        .await
        .expect("ensure files");

    assert_eq!(
        hub.calls(),
        vec![huggingface_file_url(MODEL, "config.json")],
        "only the missing file is fetched"
    );
    assert_eq!(std::fs::read(dir.join("config.json")).unwrap(), b"{}");
    assert_eq!(
        std::fs::read_to_string(dir.join("tokenizer.json")).unwrap(),
        "local",
        "an existing file is never rewritten"
    );
    assert_eq!(
        huggingface_file_url("org/m", "onnx/model.onnx"),
        "https://huggingface.co/org/m/resolve/main/onnx/model.onnx"
    );
}

#[tokio::test]
async fn test_download_writes_atomically_and_leaves_no_tmp_on_failure() {
    let cache = model_dir("hub-fetch-atomic");
    let dir = cache.path().join(MODEL);
    let url = huggingface_file_url(MODEL, "b.json");
    let hub = FakeHub::new(vec![
        Reply::Bytes(b"aaa".to_vec()),
        Reply::Fail(EmbedError::HubTransport {
            url: url.clone(),
            message: "connection reset".to_string(),
        }),
    ]);

    let error = ensure_model_files(MODEL, &dir, &["a.json", "b.json"], &hub)
        .await
        .expect_err("second fetch fails");
    assert!(
        matches!(error, EmbedError::HubTransport { .. }),
        "{error:?}"
    );

    assert_eq!(std::fs::read_to_string(dir.join("a.json")).unwrap(), "aaa");
    assert!(!dir.join("b.json").exists(), "failed fetch writes nothing");
    let tmp = all_paths(&dir)
        .into_iter()
        .filter(|path| path.to_string_lossy().ends_with(".tmp"))
        .collect::<Vec<_>>();
    assert!(tmp.is_empty(), "no tmp file may survive: {tmp:?}");
}

#[tokio::test]
async fn test_http_status_error_names_url_and_status() {
    let cache = model_dir("hub-fetch-status");
    let dir = cache.path().join(MODEL);
    let url = huggingface_file_url(MODEL, "onnx/model.onnx");
    let hub = FakeHub::new(vec![Reply::Fail(EmbedError::HubHttp {
        url: url.clone(),
        status: 404,
    })]);

    let error = ensure_model_files(MODEL, &dir, &["onnx/model.onnx"], &hub)
        .await
        .expect_err("http status failure surfaces");
    let display = error.to_string();
    assert!(matches!(error, EmbedError::HubHttp { status: 404, .. }));
    assert!(display.contains(&url), "{display}");
    assert!(display.contains("404"), "{display}");
    assert!(!dir.join("onnx/model.onnx").exists());
}

#[tokio::test]
async fn test_hub_transport_error_names_url() {
    let cache = model_dir("hub-fetch-transport");
    let dir = cache.path().join(MODEL);
    let url = huggingface_file_url(MODEL, "config.json");
    let hub = FakeHub::new(vec![Reply::Fail(EmbedError::HubTransport {
        url: url.clone(),
        message: "connection reset".to_string(),
    })]);

    let error = ensure_model_files(MODEL, &dir, &["config.json"], &hub)
        .await
        .expect_err("transport failure surfaces");
    let display = error.to_string();
    assert!(matches!(error, EmbedError::HubTransport { .. }));
    assert!(display.contains(&url), "{display}");
    assert!(display.contains("connection reset"), "{display}");
}

#[tokio::test]
async fn test_ensure_files_rejects_traversal_paths() {
    let cache = model_dir("hub-fetch-traversal");
    let dir = cache.path().join(MODEL);
    let hub = FakeHub::default();

    for escape in ["../x.json", "onnx/../../x.json", "/etc/passwd"] {
        let error = ensure_model_files(MODEL, &dir, &[escape], &hub)
            .await
            .expect_err("path escapes are refused");
        assert!(
            matches!(error, EmbedError::HubPathEscape { .. }),
            "{escape}: {error:?}"
        );
    }
    assert!(hub.calls().is_empty(), "no fetch for a rejected path");
}
