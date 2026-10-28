//! AC9 (issue 43 W2): the production composition carries the embedding
//! identity, and a shard created by it is stamped with the fingerprint.
//!
//! `build_composed_runtime` is the only production `ShardManager::new` site,
//! so this binary proves the wiring end to end: the accessor and the claim on
//! `create_shard`.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use mem_bridge::composition::build_composed_runtime;
use mem_store::db::{Value, query_one};
use mem_store::scope::Scope;
use mem_store::sql;

struct NoopEmbedder;

impl mem_embed::embedder::Embedder for NoopEmbedder {
    fn warmup(
        &self,
    ) -> mem_embed::embedder::BoxFuture<'_, Result<(), mem_embed::error::EmbedError>> {
        Box::pin(async { Ok(()) })
    }

    fn embed(
        &self,
        _text: &str,
    ) -> mem_embed::embedder::BoxFuture<'_, Result<Vec<f32>, mem_embed::error::EmbedError>> {
        Box::pin(async { Ok(vec![0.0; 8]) })
    }
}

fn load_config(root: &std::path::Path, home: &std::path::Path) -> mem_core::config::Config {
    let env: BTreeMap<String, String> = [("HOME".to_string(), home.to_string_lossy().to_string())]
        .into_iter()
        .collect();
    let mut config =
        mem_core::config::Config::load(&mem_core::config::ConfigPaths::standard(home), &env)
            .expect("the default config loads");
    config.storage_path = root.to_path_buf();
    config.embedding_dimensions = 8;
    config
}

#[tokio::test]
async fn test_composition_wires_the_embedding_identity() {
    let root = common::temp_dir("composition-identity");
    let home = common::temp_dir("composition-identity-home");
    let config = load_config(&root, &home);

    let runtime = build_composed_runtime(&root, &config, Arc::new(NoopEmbedder), None)
        .await
        .expect("the runtime composes");

    // (a) The accessor carries the config's identity.
    let identity = runtime
        .shards
        .embedding_identity()
        .expect("the production composition must carry the embedding identity");
    assert_eq!(identity.model, config.embedding_model);
    assert_eq!(identity.dimensions, config.embedding_dimensions);

    // (b) A created shard is stamped by it (wiring AND claim on create).
    let shard = runtime
        .shards
        .create_shard(Scope::Project, "0123456789abcdef", 0)
        .await
        .expect("create a shard");
    let db = runtime
        .shards
        .open_shard(&shard.db_path)
        .await
        .expect("open the created shard");
    let row = query_one(
        db.as_ref(),
        sql::GET_SHARD_METADATA_VALUE,
        vec![Value::Text("embedding_fingerprint".to_string())],
    )
    .await
    .expect("fingerprint read")
    .expect("the fingerprint row must exist");
    assert_eq!(
        row.text("value").as_deref(),
        Some(identity.fingerprint().expect("serializes").as_str()),
        "the created shard carries the configured fingerprint"
    );
    drop(db);
    drop(runtime);
    common::cleanup_temp_dir(&root);
    common::cleanup_temp_dir(&home);
}
