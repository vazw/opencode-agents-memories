//! Repo-level guard: no crate outside `mem-vault` may persist a vault
//! `content_hash` (ADR-0012 D-6: rotation rewrites every id).

use std::path::{Path, PathBuf};

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn test_no_crate_outside_mem_vault_persists_content_hash() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let crates_dir = manifest_dir.join("crates");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&crates_dir).expect("read crates") {
        let entry = entry.expect("entry");
        let name = entry.file_name();
        if name == "mem-vault" {
            continue;
        }
        if entry.path().is_dir() {
            collect_rs(&entry.path(), &mut files);
        }
    }

    for path in &files {
        let relative = path
            .strip_prefix(manifest_dir)
            .expect("under root")
            .to_string_lossy()
            .replace('\\', "/");
        let source = std::fs::read_to_string(path).expect("read");
        for (index, line) in source.lines().enumerate() {
            if line.contains("content_hash") {
                panic!(
                    "{relative}:{} contains `content_hash` outside mem-vault: {line}",
                    index + 1
                );
            }
        }
    }
}
