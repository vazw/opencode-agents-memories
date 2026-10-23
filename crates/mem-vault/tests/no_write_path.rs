//! W8a — `mem-vault` content-addressing invariant (ADR-0012 D-6/D-9).
//!
//! # A lexical tripwire, not a proof
//!
//! This test walks every `.rs` under `crates/mem-vault/src/**` (starting at
//! `CARGO_MANIFEST_DIR/src`, recursing directories) and enforces three
//! lexical rules that make the content-addressing contract visible:
//!
//! (a) DML phrases (`insert into`, `update `, `delete from`) are allowed only
//!     in `store.rs` — any other file carrying one fails;
//! (b) a `pub fn`/`pub async fn` whose parameter list contains `content_hash`
//!     fails, except a function named `get` (the legitimate read by hash);
//! (c) a `pub fn`/`pub async fn` whose parameter list mentions `VaultRecord`
//!     fails.
//!
//! It is a lexical check on words, so it catches the ordinary way a writer
//! appears — and it cannot catch macro-generated SQL, `#[path]` modules
//! declared outside `src/`, a statement spelled across a `concat!`, or a
//! bare method reference with no declaration line. The compile-fail doctest on
//! `VaultEntry` (AC-1) is the stronger guarantee: `VaultEntry` carries no
//! `content_hash` field, so an id can never be caller-supplied. This tripwire
//! is the ordinary-way detector.

use std::path::{Path, PathBuf};

/// The DML phrases the scan refuses outside `store.rs` (the trailing space in
/// `update ` keeps every ordinary English sentence from tripping it).
const DML_PHRASES: [&str; 3] = ["insert into", "update ", "delete from"];

/// The source files the walker must see (a broken walker must not pass
/// vacuously).
const EXPECTED_SOURCES: [&str; 7] = [
    "lib.rs",
    "key.rs",
    "hmac.rs",
    "schema.rs",
    "store.rs",
    "entry.rs",
    "ids.rs",
];

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn test_vault_content_addressing_invariant() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect_rs(&manifest_dir.join("src"), &mut files);

    assert!(
        files.len() >= EXPECTED_SOURCES.len(),
        "the walker saw only {} source files; a broken walker must not pass",
        files.len()
    );
    for expected in EXPECTED_SOURCES {
        assert!(
            files
                .iter()
                .any(|path| path.file_name().and_then(|name| name.to_str()) == Some(expected)),
            "the walker must see src/{expected}"
        );
    }

    for path in &files {
        let relative = path
            .strip_prefix(manifest_dir)
            .expect("a source path under the crate root")
            .to_string_lossy()
            .replace('\\', "/");
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let is_store = file_name == "store.rs";
        let source = std::fs::read_to_string(path).expect("read a vault source file");
        for (index, line) in source.lines().enumerate() {
            let number = index + 1;

            let lowered = line.to_ascii_lowercase();
            if !is_store {
                for phrase in DML_PHRASES {
                    assert!(
                        !lowered.contains(phrase),
                        "{relative}:{number} carries the DML phrase {phrase:?}: {line}"
                    );
                }
            }

            let trimmed = line.trim_start();
            let declared = trimmed
                .strip_prefix("pub fn ")
                .or_else(|| trimmed.strip_prefix("pub async fn "));
            if let Some(rest) = declared {
                let name: String = rest
                    .chars()
                    .take_while(|character| character.is_alphanumeric() || *character == '_')
                    .collect();
                // Extract the parameter list on this declaration line.
                let params = if let Some(start) = rest.find('(') {
                    if let Some(end) = rest[start..].find(')') {
                        &rest[start..start + end + 1]
                    } else {
                        ""
                    }
                } else {
                    ""
                };
                let lowered_params = params.to_ascii_lowercase();

                // (b) content_hash in a parameter list is forbidden except `get`.
                if lowered_params.contains("content_hash") && name != "get" {
                    assert!(
                        false,
                        "{relative}:{number} declares `{name}` with a `content_hash` \
                         parameter outside `get`: {line}"
                    );
                }

                // (c) VaultRecord in a parameter list is forbidden.
                if lowered_params.contains("vaultrecord") {
                    assert!(
                        false,
                        "{relative}:{number} declares `{name}` taking a `VaultRecord`: {line}"
                    );
                }
            }
        }
    }
}
