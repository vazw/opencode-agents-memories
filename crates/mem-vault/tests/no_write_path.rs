//! W7.2 — `mem-vault` ships no content path (ADR-0012 D-9).
//!
//! # A lexical tripwire, not a proof
//!
//! This test walks every `.rs` under `crates/mem-vault/src/**` (starting at
//! `CARGO_MANIFEST_DIR/src`, recursing directories) and fails when a file
//! contains a DML phrase or declares a write-shaped public function. It is a
//! lexical check on words, so it catches the ordinary way a writer appears —
//! the day someone adds the first statement constant or the first public
//! `insert_*` — and it cannot catch:
//!
//! - macro-generated SQL;
//! - a `#[path]` module declared outside `src/`;
//! - a statement spelled across a `concat!`;
//! - a bare method reference with no declaration line.
//!
//! The same caveat class applies to W7.1's purge barrier (M21/M26). The
//! absence of a content path is a design commitment this test makes visible,
//! not something it proves.

use std::path::{Path, PathBuf};

/// The DML phrases the scan refuses (the trailing space in `update ` keeps
/// every ordinary English sentence from tripping it).
const DML_PHRASES: [&str; 3] = ["insert into", "update ", "delete from"];

/// The public-function name stems that would mean a content path exists.
const NAME_STEMS: [&str; 7] = [
    "insert", "upsert", "put", "delete", "remove", "write", "append",
];

/// The source files the walker must see (a broken walker must not pass
/// vacuously).
const EXPECTED_SOURCES: [&str; 5] = ["lib.rs", "key.rs", "hmac.rs", "schema.rs", "store.rs"];

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
fn test_vault_ships_no_write_path() {
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
        let source = std::fs::read_to_string(path).expect("read a vault source file");
        for (index, line) in source.lines().enumerate() {
            let number = index + 1;

            let lowered = line.to_ascii_lowercase();
            for phrase in DML_PHRASES {
                assert!(
                    !lowered.contains(phrase),
                    "{relative}:{number} carries the DML phrase {phrase:?}: {line}"
                );
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
                let lowered_name = name.to_ascii_lowercase();
                for stem in NAME_STEMS {
                    assert!(
                        !lowered_name.contains(stem),
                        "{relative}:{number} declares the write-shaped public function \
                         `{name}` — this crate ships no content path (ADR-0012 D-9): {line}"
                    );
                }
            }
        }
    }
}
