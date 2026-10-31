//! Step 10 — maintenance measurement: does the stored corpus fit the cap?
//!
//! `#[ignore]`d and env-gated. It opens a **copy** of a storage root
//! (`MEM_CORPUS_AUDIT_ROOT`, e.g. `.tmp/scratch/corpus-audit/data`), reads
//! every memory's `content` from every shard, encodes each with the cached
//! nomic tokenizer and prints the maximum id count beside the resolved cap.
//!
//! This is the only artifact that can settle "no stored vector changes for
//! nomic": a character count cannot (byte-level tokenizers can emit more than
//! one token per character). It is a maintenance step, **not a CI gate**, and
//! it must never point at the live root — a live sqlite3 open creates a
//! `-shm` there.
//!
//! Copy recipe (recorded in the ledger):
//! `cp -a --reflink=auto ~/.opencode-mem/data "$R/data"` of
//! `{db,-wal,-shm,-tshm}`, then `rm -f "$R/data/.engine.lock"` (a copied lock
//! names the live holder and the probe would refuse to start).

use std::path::{Path, PathBuf};

use mem_embed::local::model_cache::{effective_max_tokens, read_declared_position_limit};
use mem_embed::local::tokenize::{encode_text, load_tokenizer};
use mem_store::config::StoreConfig;
use mem_store::db::{ConnectFactory, ConnectOptions, turso_connect_factory};
use mem_store::platform::Platform;
use mem_store::sql;

/// The model whose cached tokenizer the audit uses.
const MODEL: &str = "Xenova/nomic-embed-text-v1";

fn store_config(root: &Path) -> StoreConfig {
    StoreConfig {
        storage_path: root.to_path_buf(),
        embedding_dimensions: 768,
        embedding_model: MODEL.to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
        allow_unguarded_open: false,
    }
}

/// Every `*_shard_*.db` under `root` (shards live in `{scope}s/`).
fn shard_paths(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let is_shard = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("_shard_") && name.ends_with(".db"));
            if is_shard {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
#[ignore = "maintenance measurement: set MEM_CORPUS_AUDIT_ROOT to a COPY of a storage root"]
async fn test_corpus_rows_fit_the_nomic_cap() {
    let root = match std::env::var("MEM_CORPUS_AUDIT_ROOT") {
        Ok(root) => PathBuf::from(root),
        Err(_) => {
            println!("MEM_CORPUS_AUDIT_ROOT is unset; nothing to audit");
            return;
        }
    };

    let cache_root = root.join(".cache");
    let tokenizer = load_tokenizer(&cache_root.join(MODEL).join("tokenizer.json"))
        .expect("the cached nomic tokenizer");
    let special_tokens = encode_text(&tokenizer, "", usize::MAX)
        .expect("the special-token probe")
        .ids
        .len();
    let declared = read_declared_position_limit(MODEL, &cache_root);
    let cap = effective_max_tokens(declared, special_tokens, None).expect("the cap resolves");
    println!("declared={declared:?} special_tokens={special_tokens} cap={cap:?}");

    let config = store_config(&root);
    let options = {
        let mut options = ConnectOptions::for_config(&config, Platform::current());
        // Read-only: the copy has no live holder, and the audit must not write.
        options.read_only = true;
        options
    };

    let mut shard_count = 0_usize;
    let mut row_count = 0_usize;
    let mut max_ids = 0_usize;
    let mut max_chars = 0_usize;

    for path in shard_paths(&root) {
        let db = turso_connect_factory()
            .connect(path.clone(), options.clone())
            .await
            .unwrap_or_else(|error| panic!("open {}: {error}", path.display()));
        let rows = db
            .query(sql::GET_ALL_MEMORIES, Vec::new())
            .await
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        shard_count += 1;
        for row in rows {
            let Some(content) = row.text("content") else {
                continue;
            };
            row_count += 1;
            max_chars = max_chars.max(content.chars().count());
            let ids = encode_text(&tokenizer, &content, usize::MAX)
                .expect("encode a stored row")
                .ids
                .len();
            max_ids = max_ids.max(ids);
        }
    }

    println!(
        "corpus: shards={shard_count} rows={row_count} max_chars={max_chars} max_ids={max_ids} cap={cap:?}"
    );

    if let Some(cap) = cap {
        assert!(
            max_ids <= cap,
            "a stored row needs {max_ids} ids but the cap is {cap}: its vector would change"
        );
    }
}
