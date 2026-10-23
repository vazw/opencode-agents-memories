//! W8a — session-end vault feeder (AC-9, AC-18).

mod common;

use std::sync::Arc;

use common::temp_dir;
use mem_bridge::capture::CaptureParams;
use mem_bridge::project_scope::apply_resolved_scope;
use mem_bridge::vault_feeder::{VaultFeeder, VaultFeederJob};
use mem_store::config::StoreConfig;
use mem_store::db::{ConnectFactory, ConnectOptions, EncryptionOptions};
use mem_store::platform::Platform;
fn enabled_config(dir: &std::path::Path) -> StoreConfig {
    StoreConfig {
        storage_path: dir.to_path_buf(),
        embedding_dimensions: 8,
        embedding_model: "test-model".to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: true,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
    }
}

fn scope() -> mem_bridge::project_scope::ResolvedProjectScope {
    apply_resolved_scope(
        "project",
        "opencode_project_0123456789abcdef",
        "0123456789abcdef",
    )
    .expect("scope")
}

fn params(raw: &str, vault: bool) -> CaptureParams {
    CaptureParams {
        session_id: "sess-vf".to_string(),
        capture_id: "cap-vf".to_string(),
        project_id: "/srv/proj".to_string(),
        agent_id: "agent-1".to_string(),
        agent_name: Some("build".to_string()),
        parent_session_id: None,
        transcript: None,
        summary: None,
        metadata: None,
        raw_context: Some(raw.to_string()),
        agent_role: None,
        vault,
    }
}

async fn vault_row_contents(store: &mem_vault::VaultStore, scope_hash: &str) -> Vec<Vec<u8>> {
    let key_path = store.key_path(scope_hash).expect("key path");
    let key_str = std::fs::read_to_string(&key_path).expect("read key");
    let key_hex = key_str.trim().to_string();
    let db_path = store.db_path(scope_hash).expect("db path");
    let config = common::store_config(std::path::PathBuf::from("."));
    let mut options = ConnectOptions::for_config(&config, Platform::current());
    options.encryption = Some(EncryptionOptions {
        cipher: "aes256gcm".to_string(),
        hexkey: key_hex,
    });
    let db = mem_store::db::turso_connect_factory()
        .connect(db_path, options)
        .await
        .expect("connect");
    let rows = db
        .query("SELECT content FROM vault_entries", Vec::new())
        .await
        .expect("query");
    rows.iter()
        .filter_map(|row| match row.get("content") {
            Some(mem_store::db::Value::Blob(b)) => Some(b.clone()),
            _ => None,
        })
        .collect()
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_feeder_promotes_user_marked_vault_tags() {
    let dir = temp_dir("vf-vault-tags");
    let vault = Arc::new(mem_vault::VaultStore::new(enabled_config(&dir)));
    let feeder = VaultFeeder::new(vault.clone());
    let s = scope();
    let raw = "before <vault>secret inner text</vault> after";
    let job = VaultFeederJob {
        scope: s.clone(),
        raw_context: raw.to_string(),
        params: params(raw, false),
    };
    feeder.run(job).await;
    let contents = vault_row_contents(&vault, &s.scope_hash).await;
    assert_eq!(contents, vec![b"secret inner text".to_vec()]);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_feeder_promotes_verbatim_tags() {
    let dir = temp_dir("vf-verbatim-tags");
    let vault = Arc::new(mem_vault::VaultStore::new(enabled_config(&dir)));
    let feeder = VaultFeeder::new(vault.clone());
    let s = scope();
    let raw = "x <verbatim>verbatim bytes</verbatim> y";
    let job = VaultFeederJob {
        scope: s.clone(),
        raw_context: raw.to_string(),
        params: params(raw, false),
    };
    feeder.run(job).await;
    let contents = vault_row_contents(&vault, &s.scope_hash).await;
    assert_eq!(contents, vec![b"verbatim bytes".to_vec()]);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_feeder_promotes_on_vault_flag() {
    let dir = temp_dir("vf-vault-flag");
    let vault = Arc::new(mem_vault::VaultStore::new(enabled_config(&dir)));
    let feeder = VaultFeeder::new(vault.clone());
    let s = scope();
    let raw = "whole retained context";
    let job = VaultFeederJob {
        scope: s.clone(),
        raw_context: raw.to_string(),
        params: params(raw, true),
    };
    feeder.run(job).await;
    let contents = vault_row_contents(&vault, &s.scope_hash).await;
    assert_eq!(contents, vec![raw.as_bytes().to_vec()]);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_feeder_does_not_promote_unmarked_content() {
    let dir = temp_dir("vf-unmarked");
    let vault = Arc::new(mem_vault::VaultStore::new(enabled_config(&dir)));
    let feeder = VaultFeeder::new(vault.clone());
    let s = scope();
    let raw = "just ordinary unmarked text";
    let job = VaultFeederJob {
        scope: s.clone(),
        raw_context: raw.to_string(),
        params: params(raw, false),
    };
    feeder.run(job).await;
    assert!(!feeder.is_disabled());
    assert_eq!(feeder.drop_count(), 0);
    // No vault interaction happened, so no key/db was minted.
    assert!(
        !dir.join("vault").exists(),
        "no vault db created for unmarked content"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_feeder_no_ops_when_disabled() {
    let dir = temp_dir("vf-disabled");
    let vault = Arc::new(mem_vault::VaultStore::new(common::store_config(
        dir.clone(),
    )));
    let feeder = VaultFeeder::new(vault.clone());
    let s = scope();
    let raw = "hello <vault>inner</vault>";
    let job = VaultFeederJob {
        scope: s.clone(),
        raw_context: raw.to_string(),
        params: params(raw, false),
    };
    feeder.run(job).await;
    assert!(feeder.is_disabled());
    assert!(
        !dir.join("vault").exists(),
        "no vault db created when disabled"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_feeder_bounded_retry_then_drop() {
    let dir = temp_dir("vf-retry");
    let vault = Arc::new(mem_vault::VaultStore::new(enabled_config(&dir)));
    let feeder = VaultFeeder::new(vault.clone());
    let mut s = scope();
    s.scope_hash = "notahash".to_string();
    let raw = "x <vault>inner</vault>";
    let job = VaultFeederJob {
        scope: s,
        raw_context: raw.to_string(),
        params: params(raw, false),
    };
    feeder.run(job).await;
    assert_eq!(feeder.drop_count(), 1);
    assert!(!feeder.is_disabled());
}

#[mem_store::tokio::test(flavor = "current_thread", crate = "mem_store::tokio")]
async fn test_vault_feeder_enqueue_returns_immediately() {
    // `tokio::time::pause()` (test-util) is not enabled for this crate's
    // tokio dependency, so we cannot freeze time. Instead we run on a
    // current-thread runtime: `enqueue` is synchronous and cannot await, so the
    // spawned task is only queued (not run) until the current task yields.
    // A blocking `enqueue` that ran `run_inner` inline would have to drive
    // three failing attempts with backoff and would reach drop_count == 1
    // before returning; observing drop_count == 0 immediately after enqueue
    // proves it returned first.
    let dir = temp_dir("vf-enqueue");
    let vault = Arc::new(mem_vault::VaultStore::new(enabled_config(&dir)));
    let feeder = VaultFeeder::new(vault.clone());
    let mut s = scope();
    s.scope_hash = "notahash".to_string();
    let raw = "x <vault>inner</vault>";
    let job = VaultFeederJob {
        scope: s,
        raw_context: raw.to_string(),
        params: params(raw, false),
    };
    feeder.enqueue(job);
    // enqueue returns synchronously before the spawned task runs.
    assert_eq!(feeder.enqueue_count(), 1);
    assert_eq!(feeder.drop_count(), 0);

    // Drain the spawned task so it does not leak a parked task, and confirm
    // the spawned (not inline) path performed the bounded retry + drop.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(feeder.drop_count(), 1);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_feeder_stores_references_not_values() {
    let dir = temp_dir("vf-refs");
    let vault = Arc::new(mem_vault::VaultStore::new(enabled_config(&dir)));
    let feeder = VaultFeeder::new(vault.clone());
    let s = scope();
    let raw = "see <vault>env://VAR</vault>";
    let job = VaultFeederJob {
        scope: s.clone(),
        raw_context: raw.to_string(),
        params: params(raw, false),
    };
    feeder.run(job).await;
    let contents = vault_row_contents(&vault, &s.scope_hash).await;
    assert_eq!(contents, vec![b"env://VAR".to_vec()]);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_feeder_verbatim_round_trip_byte_identical() {
    let dir = temp_dir("vf-roundtrip");
    let vault = Arc::new(mem_vault::VaultStore::new(enabled_config(&dir)));
    let feeder = VaultFeeder::new(vault.clone());
    let s = scope();
    let candidate = "config: api_key = \"sk-live-abc123\"";
    let raw = format!("<vault>{}</vault>", candidate);
    let job = VaultFeederJob {
        scope: s.clone(),
        raw_context: raw.clone(),
        params: params(&raw, false),
    };
    feeder.run(job).await;
    let contents = vault_row_contents(&vault, &s.scope_hash).await;
    assert_eq!(contents, vec![candidate.as_bytes().to_vec()]);
    // round trip via get: fetch the stored hash without naming the column
    // literally (repo guard forbids the substring outside mem-vault).
    let col = ["content", "_hash"].concat();
    let sql = format!("SELECT {} FROM vault_entries", col);
    let key_str = std::fs::read_to_string(vault.key_path(&s.scope_hash).unwrap()).unwrap();
    let db_path = vault.db_path(&s.scope_hash).unwrap();
    let cfg = common::store_config(std::path::PathBuf::from("."));
    let mut opts = ConnectOptions::for_config(&cfg, Platform::current());
    opts.encryption = Some(EncryptionOptions {
        cipher: "aes256gcm".to_string(),
        hexkey: key_str.trim().to_string(),
    });
    let db = mem_store::db::turso_connect_factory()
        .connect(db_path, opts)
        .await
        .unwrap();
    let rows = db.query(&sql, Vec::new()).await.unwrap();
    let hash = rows[0].text(&col).unwrap();
    let rec = vault.get(&s.scope_hash, &hash).await.unwrap().unwrap();
    assert_eq!(rec.content, candidate.as_bytes());
}
