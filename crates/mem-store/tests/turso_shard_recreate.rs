//! Shard recreate battery (blueprint W1.2 step 8): incompatible/corrupt
//! shards REJECT writes — the original database is never moved or emptied
//! (upstream getWriteShard guard + tests ported from the guarded flows).

mod common;

use std::sync::Arc;

use mem_store::db::{BatchMode, Statement};
use mem_store::error::StoreError;
use mem_store::shard_manager::{ShardManager, get_shard_path};
use mem_store::vector::{MemoryInsert, count_all_vectors, insert_vector};

use common::real_components;
use mem_store::scope::Scope;

fn scope_hash(tag: u8) -> String {
    format!("a1b2c3d4e5f60{tag:03x}")
}

async fn seed_one_memory(
    shard: &mem_store::shard_manager::ShardManager,
    full_path: &std::path::Path,
) {
    // open through the manager and insert one memory row directly
    let db = shard.open_shard(full_path).await.expect("open shard");
    insert_vector(
        db.as_ref(),
        &MemoryInsert {
            id: "mem1".into(),
            content: "seed".into(),
            vector: vec![1.0f32, 0.0],
            tags_vector: None,
            container_tag: "opencode_project_a1b2c3d4e5f60001".into(),
            tags: None,
            memory_type: None,
            created_at: 1,
            updated_at: 1,
            metadata: None,
            session_id: None,
            display_name: None,
            user_name: None,
            user_email: None,
            project_path: None,
            project_name: None,
            git_repo_url: None,
        },
    )
    .await
    .expect("seed insert");
}

async fn expect_incompatible(manager: &ShardManager, hash: &str) -> StoreError {
    let scope = Scope::Project;
    manager
        .get_write_shard(scope, hash)
        .await
        .expect_err("incompatible shard rejects writes")
}

// ---------------------------------------------------------------------------
// 1/5
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_rejects_writes_on_dimension_mismatch_without_moving_or_emptying_shard() {
    let config = common::test_store_config("shard-dims-mismatch", 2, 100);
    let (_config, _connections, manager) = real_components(config).await;
    let hash = scope_hash(1);
    let shard = manager
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect("first shard");
    assert_eq!(shard.shard_index, 0);
    let full = get_shard_path(
        &_config.storage_path,
        Scope::Project,
        &hash,
        shard.shard_index,
    )
    .expect("path");
    seed_one_memory(&manager, &full).await;

    // Corrupt the shard's dims metadata: claims 3 where config says 2.
    let db = manager.open_shard(&full).await.expect("re-open");
    db.execute(
        "INSERT OR REPLACE INTO shard_metadata (key, value) VALUES ('embedding_dimensions', '3')",
        Vec::new(),
    )
    .await
    .expect("dims tamper");
    drop(db);

    let error = expect_incompatible(&manager, &hash).await;
    assert!(
        matches!(error, StoreError::ShardIncompatible { .. }),
        "typed error: {error}"
    );
    // The shard stays untouched: same row count, no `.invalid.bak`.
    let db = manager.open_shard(&full).await.expect("re-open");
    assert_eq!(count_all_vectors(db.as_ref()).await, 1, "data intact");
    let baks = std::fs::read_dir(&_config.storage_path)
        .expect("storage dir")
        .filter_map(|entry| entry.ok().map(|e| e.path().to_string_lossy().to_string()))
        .filter(|path| path.contains(".bak"))
        .collect::<Vec<_>>();
    assert!(baks.is_empty(), "no backup junk created: {baks:?}");
}

// ---------------------------------------------------------------------------
// 2/5
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_rejects_writes_when_dimension_metadata_is_missing_without_deleting_data() {
    let config = common::test_store_config("shard-dims-missing", 2, 100);
    let (_config, _connections, manager) = real_components(config).await;
    let hash = scope_hash(2);
    let shard = manager
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect("first shard");
    let full = get_shard_path(
        &_config.storage_path,
        Scope::Project,
        &hash,
        shard.shard_index,
    )
    .expect("path");
    seed_one_memory(&manager, &full).await;

    // WIPE the dims metadata row entirely.
    let db = manager.open_shard(&full).await.expect("re-open");
    db.execute("DELETE FROM shard_metadata", Vec::new())
        .await
        .expect("wipe dims");
    drop(db);

    let error = expect_incompatible(&manager, &hash).await;
    assert!(matches!(error, StoreError::ShardIncompatible { .. }));
    let db = manager.open_shard(&full).await.expect("re-open");
    assert_eq!(count_all_vectors(db.as_ref()).await, 1, "data intact");
}

// ---------------------------------------------------------------------------
// 3/5
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_rejects_writes_when_shard_file_missing_but_registry_row_exists() {
    let config = common::test_store_config("shard-file-missing", 2, 100);
    let (_config, connections, manager) = real_components(config).await;
    let hash = scope_hash(3);
    let shard = manager
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect("first shard");
    let full = get_shard_path(
        &_config.storage_path,
        Scope::Project,
        &hash,
        shard.shard_index,
    )
    .expect("path");

    // close the manager handle, then unlink the file
    connections.close_connection(&full).await;
    std::fs::remove_file(&full).expect("file removed");

    let error = expect_incompatible(&manager, &hash).await;
    assert!(matches!(error, StoreError::ShardIncompatible { .. }));
}

// ---------------------------------------------------------------------------
// 4/5
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_rejects_writes_when_memories_table_missing() {
    let config = common::test_store_config("shard-no-table", 2, 100);
    let (_config, _connections, manager) = real_components(config).await;
    let hash = scope_hash(4);
    let full = get_shard_path(&_config.storage_path, Scope::Project, &hash, 0).expect("path");
    // Create an EMPTY db file (open + drop via raw factory). The scope
    // dir doesn't exist yet (no create_shard call), so make it first.
    std::fs::create_dir_all(full.parent().expect("scope dir parent")).expect("scope dir");
    let opts = mem_store::db::ConnectOptions::for_config(
        &_config,
        mem_store::platform::Platform::current(),
    );
    use mem_store::db::ConnectFactory;
    let raw = mem_store::db::turso_connect_factory()
        .connect(full.clone(), opts)
        .await
        .expect("raw open");
    drop(raw);

    manager
        .register_existing_shard(Scope::Project, &hash, 0, &full, 0, true)
        .await
        .expect("registry row for empty file");

    let error = expect_incompatible(&manager, &hash).await;
    assert!(matches!(error, StoreError::ShardIncompatible { .. }));
    assert!(full.exists(), "empty file left untouched");
}

// ---------------------------------------------------------------------------
// 5/5
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_incompatible_error_names_scope_hash_index_and_path() {
    let config = common::test_store_config("shard-error-text", 2, 100);
    let (_config, _connections, manager) = real_components(config).await;
    let hash = scope_hash(5);
    let shard = manager
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect("first shard");
    let full = get_shard_path(
        &_config.storage_path,
        Scope::Project,
        &hash,
        shard.shard_index,
    )
    .expect("path");

    let db = manager.open_shard(&full).await.expect("re-open");
    db.execute("DELETE FROM shard_metadata", Vec::new())
        .await
        .expect("wipe dims");
    drop(db);

    let error = expect_incompatible(&manager, &hash).await;
    let text = error.to_string();
    assert!(
        text.contains(&format!("project/{hash}#0")),
        "scope/hash/index: {text}"
    );
    assert!(text.contains("left untouched"), "upstream wording: {text}");
    assert!(
        text.contains(full.to_string_lossy().as_ref()),
        "path shown: {text}"
    );
    let _: Option<Statement> = None;
    let _ = BatchMode::Write;
    let _ = Arc::new(0u8);
}
