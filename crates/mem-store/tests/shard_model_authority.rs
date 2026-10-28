//! W3 — model authority: the pure judgement over the stored identity facts,
//! the typed refusal's Display, the write-path refusal, the dimension/model
//! distinction, and the read-cost measurement harness.

mod common;

use mem_core::config::EmbeddingPooling;
use mem_core::embedding::EmbeddingIdentity;
use mem_store::db::Value;
use mem_store::error::{ShardModelMismatch, ShardModelMismatchEntry, StoreError};
use mem_store::shard_facts::{self, ModelAuthority, ModelFacts};

/// AC1 — the Display names every shard (including an absent stored model) and
/// the exact operator command; the boxed payload keeps `StoreError` inside its
/// measured high-water mark.
#[test]
fn test_mismatch_display_names_every_shard_and_the_command() {
    let remedy =
        "opencode-agents-memory --mode=reembed-scope --scope opencode_project_a1b2c3d4e5f67890";
    let report = ShardModelMismatch {
        shards: vec![
            ShardModelMismatchEntry {
                id: 3,
                stored_model: Some("foreign-model".to_string()),
                expected: "Xenova/nomic-embed-text-v1".to_string(),
            },
            ShardModelMismatchEntry {
                id: 4,
                stored_model: None,
                expected: "Xenova/nomic-embed-text-v1".to_string(),
            },
        ],
        remedy: remedy.to_string(),
    };
    let display = report.to_string();
    assert!(display.contains("shard 3"), "{display}");
    assert!(display.contains("shard 4"), "{display}");
    assert!(
        display.contains("stored model 'foreign-model'"),
        "{display}"
    );
    assert!(display.contains("stored model none"), "{display}");
    assert!(
        display.contains("expected 'Xenova/nomic-embed-text-v1'"),
        "{display}"
    );
    assert!(display.contains("--mode=reembed-scope"), "{display}");
    assert!(
        display.contains("opencode_project_a1b2c3d4e5f67890"),
        "{display}"
    );
    assert_eq!(
        display,
        "embedding model mismatch: shard 3 (stored model 'foreign-model', \
         expected 'Xenova/nomic-embed-text-v1'); shard 4 (stored model none, \
         expected 'Xenova/nomic-embed-text-v1'); re-embed the scope: \
         opencode-agents-memory --mode=reembed-scope --scope \
         opencode_project_a1b2c3d4e5f67890"
    );
    assert!(
        std::mem::size_of::<StoreError>() <= 112,
        "StoreError grew past W1's measured 112-byte high-water mark: {}",
        std::mem::size_of::<StoreError>()
    );
}

/// The identity a test config describes.
fn identity(model: &str, dimensions: u32) -> EmbeddingIdentity {
    EmbeddingIdentity {
        model: model.to_string(),
        dimensions,
        pooling: EmbeddingPooling::Mean,
        dtype: None,
        use_task_prefixes: false,
        query_prefix: None,
        document_prefix: None,
    }
}

/// AC2 — the seven-row truth table. Two rows are the discriminators: a
/// model-first judge accepts row 2 (equal model, differing fingerprint), and
/// a "None identity means valid" judge accepts row 3 (differing model).
#[test]
fn test_judge_model_facts_truth_table() {
    let configured = identity("test-model", 2);
    let configured_fingerprint = configured.fingerprint().expect("identity serializes");
    let foreign_fingerprint = identity("other-model", 2)
        .fingerprint()
        .expect("identity serializes");

    // Row 1: fingerprint equals the identity's -> Fingerprint.
    assert_eq!(
        shard_facts::judge_model_facts(
            &ModelFacts {
                fingerprint: Some(configured_fingerprint.clone()),
                stored_model: Some("anything".to_string()),
            },
            Some(&configured),
            "test-model",
        ),
        ModelAuthority::Fingerprint
    );

    // Row 2: fingerprint differs, model equal -> Mismatch (fingerprint wins).
    assert_eq!(
        shard_facts::judge_model_facts(
            &ModelFacts {
                fingerprint: Some(foreign_fingerprint.clone()),
                stored_model: Some("test-model".to_string()),
            },
            Some(&configured),
            "test-model",
        ),
        ModelAuthority::Mismatch {
            stored_model: Some("test-model".to_string()),
        }
    );

    // Row 3: fingerprint present, stored model differs, no identity -> Mismatch.
    assert_eq!(
        shard_facts::judge_model_facts(
            &ModelFacts {
                fingerprint: Some(configured_fingerprint.clone()),
                stored_model: Some("foreign-model".to_string()),
            },
            None,
            "test-model",
        ),
        ModelAuthority::Mismatch {
            stored_model: Some("foreign-model".to_string()),
        }
    );

    // Row 4: fingerprint present, model absent, no identity -> warned fallback.
    assert_eq!(
        shard_facts::judge_model_facts(
            &ModelFacts {
                fingerprint: Some(configured_fingerprint.clone()),
                stored_model: None,
            },
            None,
            "test-model",
        ),
        ModelAuthority::ModelEqualityNoIdentity { stored_model: None }
    );

    // Row 5: fingerprint present, model equal, no identity -> warned fallback.
    assert_eq!(
        shard_facts::judge_model_facts(
            &ModelFacts {
                fingerprint: Some(configured_fingerprint),
                stored_model: Some("test-model".to_string()),
            },
            None,
            "test-model",
        ),
        ModelAuthority::ModelEqualityNoIdentity {
            stored_model: Some("test-model".to_string()),
        }
    );

    // Row 6: neither fact -> legacy equality (nothing to compare).
    assert_eq!(
        shard_facts::judge_model_facts(
            &ModelFacts {
                fingerprint: None,
                stored_model: None,
            },
            Some(&configured),
            "test-model",
        ),
        ModelAuthority::ModelEquality { stored_model: None }
    );

    // Row 7a: absent fingerprint, equal model -> equality.
    assert_eq!(
        shard_facts::judge_model_facts(
            &ModelFacts {
                fingerprint: None,
                stored_model: Some("test-model".to_string()),
            },
            None,
            "test-model",
        ),
        ModelAuthority::ModelEquality {
            stored_model: Some("test-model".to_string()),
        }
    );

    // Row 7b: absent fingerprint, differing model -> Mismatch.
    assert_eq!(
        shard_facts::judge_model_facts(
            &ModelFacts {
                fingerprint: None,
                stored_model: Some("legacy-unknown".to_string()),
            },
            Some(&configured),
            "test-model",
        ),
        ModelAuthority::Mismatch {
            stored_model: Some("legacy-unknown".to_string()),
        }
    );
}

/// AC3 — both identity keys are read by one statement; an absent key is
/// `None`, never `""`.
#[tokio::test]
async fn test_read_model_facts_reads_both_keys_in_one_statement() {
    let config = common::store_config("model-facts-read");
    let db = common::open_real_db(&config, "shard.db")
        .await
        .expect("open the shard");
    shard_facts::ensure_v3(db.as_ref(), 2)
        .await
        .expect("shard schema");

    for (key, value) in [
        ("embedding_fingerprint", "fingerprint-value"),
        ("embedding_model", "test-model"),
    ] {
        db.execute(
            "INSERT INTO shard_metadata (key, value) VALUES (?1, ?2)",
            vec![Value::Text(key.to_string()), Value::Text(value.to_string())],
        )
        .await
        .expect("seed identity fact");
    }
    assert_eq!(
        shard_facts::read_model_facts(db.as_ref())
            .await
            .expect("read both facts"),
        ModelFacts {
            fingerprint: Some("fingerprint-value".to_string()),
            stored_model: Some("test-model".to_string()),
        }
    );

    db.execute("DELETE FROM shard_metadata", Vec::new())
        .await
        .expect("clear identity facts");
    assert_eq!(
        shard_facts::read_model_facts(db.as_ref())
            .await
            .expect("read absent facts"),
        ModelFacts {
            fingerprint: None,
            stored_model: None,
        }
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

// ---------------------------------------------------------------------------
// The write-path refusal (AC4–AC7)
// ---------------------------------------------------------------------------

use std::sync::Arc;

use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::scope::Scope;
use mem_store::shard_manager::{ShardInfo, ShardManager};
use mem_store::vector::{self, MemoryInsert};

const HASH: &str = "a1b2c3d4e5f60003";
const TAG: &str = "opencode_project_a1b2c3d4e5f60003";

/// A registered project shard whose manager is wired with `Some(identity)`
/// when `wire_identity` is set (the fingerprint arm REQUIRES it).
async fn write_fixture(
    prefix: &str,
    wire_identity: bool,
) -> (StoreConfig, Arc<ConnectionManager>, ShardManager, ShardInfo) {
    let config = common::store_config(prefix);
    let manager_identity =
        wire_identity.then(|| identity(&config.embedding_model, config.embedding_dimensions));
    let (_config, connections, manager) =
        common::real_components_with_identity(config.clone(), manager_identity).await;
    let shard = manager
        .create_shard(Scope::Project, HASH, 0)
        .await
        .expect("create the project shard");
    (config, connections, manager, shard)
}

/// Overwrites one identity key of the shard's `shard_metadata`.
async fn set_metadata(manager: &ShardManager, shard: &ShardInfo, key: &str, value: &str) {
    let db = manager
        .open_shard(&shard.db_path)
        .await
        .expect("open the shard");
    db.execute(
        "INSERT OR REPLACE INTO shard_metadata (key, value) VALUES (?1, ?2)",
        vec![Value::Text(key.to_string()), Value::Text(value.to_string())],
    )
    .await
    .expect("write the identity fact");
}

/// Removes the fingerprint `create_shard` claims on an empty shard, leaving a
/// pre-W2 (legacy) identity: the model fallback is then the only authority.
async fn clear_fingerprint(manager: &ShardManager, shard: &ShardInfo) {
    let db = manager
        .open_shard(&shard.db_path)
        .await
        .expect("open the shard");
    db.execute(
        "DELETE FROM shard_metadata WHERE key = 'embedding_fingerprint'",
        Vec::new(),
    )
    .await
    .expect("clear the fingerprint");
}

/// Inserts one 2-dimension row and syncs the shard's absolute count.
async fn seed_row(manager: &ShardManager, shard: &ShardInfo, id: &str) {
    let db = manager
        .open_shard(&shard.db_path)
        .await
        .expect("open the shard");
    vector::insert_vector(
        db.as_ref(),
        &MemoryInsert {
            id: id.to_string(),
            content: "seeded content".to_string(),
            vector: vec![1.0, 0.0],
            tags_vector: None,
            container_tag: TAG.to_string(),
            tags: None,
            memory_type: None,
            created_at: 1_700_000_000_000,
            updated_at: 1_700_000_000_000,
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
    .expect("insert the seed row");
    let count = shard_facts::sync_count(db.as_ref())
        .await
        .expect("sync the seed count");
    manager
        .set_vector_count(shard.id, count)
        .await
        .expect("adopt the seed count");
}

/// AC4 — a fingerprint-mismatched shard refuses the next write and mutates
/// nothing. The manager MUST be wired `Some(identity)`; the model is equal, so
/// a model-first judge accepts it and fails this test.
#[tokio::test]
async fn test_a_fingerprint_mismatched_shard_refuses_the_next_write() {
    let (config, _connections, manager, shard) =
        write_fixture("model-refuse-fingerprint", true).await;
    seed_row(&manager, &shard, "mem_seed").await;
    set_metadata(
        &manager,
        &shard,
        "embedding_fingerprint",
        "foreign-fingerprint",
    )
    .await;

    let error = manager
        .get_write_shard(Scope::Project, HASH)
        .await
        .expect_err("a foreign fingerprint must refuse the write");
    match error {
        StoreError::ShardModelMismatch(report) => {
            assert_eq!(report.shards.len(), 1);
            assert_eq!(report.shards[0].id, shard.id);
            assert_eq!(
                report.shards[0].stored_model.as_deref(),
                Some("test-model"),
                "the model is never consulted, but its stored value is named"
            );
            assert_eq!(report.shards[0].expected, "test-model");
            assert!(
                report.remedy.contains("--mode=reembed-scope"),
                "the remedy names the command: {}",
                report.remedy
            );
            assert!(
                report.remedy.contains(TAG),
                "the remedy names the scope tag: {}",
                report.remedy
            );
        }
        other => panic!("expected ShardModelMismatch, got {other:?}"),
    }

    let db = manager
        .open_shard(&shard.db_path)
        .await
        .expect("open the shard");
    assert_eq!(
        shard_facts::read_count(db.as_ref()).await.expect("count"),
        1,
        "the refused write must not append"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

/// AC5 — a shard that names a foreign model but carries no fingerprint is
/// refused by the model fallback.
#[tokio::test]
async fn test_a_model_named_shard_without_a_fingerprint_refuses_the_next_write() {
    let (config, _connections, manager, shard) = write_fixture("model-refuse-model", true).await;
    clear_fingerprint(&manager, &shard).await;
    set_metadata(&manager, &shard, "embedding_model", "foreign-model").await;

    let error = manager
        .get_write_shard(Scope::Project, HASH)
        .await
        .expect_err("a foreign model must refuse the write");
    match error {
        StoreError::ShardModelMismatch(report) => {
            assert_eq!(report.shards.len(), 1);
            assert_eq!(report.shards[0].id, shard.id);
            assert_eq!(
                report.shards[0].stored_model.as_deref(),
                Some("foreign-model")
            );
        }
        other => panic!("expected ShardModelMismatch, got {other:?}"),
    }

    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

/// AC6a — an unfingerprinted shard whose model is equal still writes; a
/// `legacy-unknown` shard is refused (never softened to "absent").
#[tokio::test]
async fn test_a_legacy_shard_with_equal_model_still_writes() {
    let (config, _connections, manager, _shard) = write_fixture("model-legacy-equal", true).await;
    let written = manager
        .get_write_shard(Scope::Project, HASH)
        .await
        .expect("an equal model still writes");
    assert_eq!(written.vector_count, 0);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");

    let (config, _connections, manager, shard) = write_fixture("model-legacy-unknown", true).await;
    clear_fingerprint(&manager, &shard).await;
    set_metadata(&manager, &shard, "embedding_model", "legacy-unknown").await;
    let error = manager
        .get_write_shard(Scope::Project, HASH)
        .await
        .expect_err("legacy-unknown is a foreign model, not an absent one");
    assert!(
        matches!(error, StoreError::ShardModelMismatch(_)),
        "got {error:?}"
    );
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

/// AC7 — dimensions and model refusals stay distinct types. The first fixture
/// kills a `bool`-returning `is_shard_valid`; the second kills folding the
/// model into the dimensions check.
#[tokio::test]
async fn test_a_dimension_and_a_model_refusal_stay_distinct() {
    let (config, _connections, manager, shard) = write_fixture("model-dims-distinct", true).await;
    set_metadata(&manager, &shard, "embedding_dimensions", "4").await;
    let error = manager
        .get_write_shard(Scope::Project, HASH)
        .await
        .expect_err("stored dims 4 with equal model is incompatible");
    assert!(
        matches!(error, StoreError::ShardIncompatible { .. }),
        "got {error:?}"
    );
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");

    let (config, _connections, manager, shard) = write_fixture("model-dims-model", true).await;
    clear_fingerprint(&manager, &shard).await;
    set_metadata(&manager, &shard, "embedding_model", "foreign-model").await;
    let error = manager
        .get_write_shard(Scope::Project, HASH)
        .await
        .expect_err("equal dims with a foreign model is a model mismatch");
    assert!(
        matches!(error, StoreError::ShardModelMismatch(_)),
        "got {error:?}"
    );
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

/// The W3 measurement harness (created now, run only in the measurement step
/// with `--release --ignored --nocapture`).
#[ignore = "measurement: run with --release --ignored --nocapture in the measurement step"]
#[tokio::test]
async fn test_measure_shard_model_read_cost() {
    use std::time::Instant;

    let mut config = common::store_config("measure-model-read");
    config.embedding_dimensions = 768;
    let db = common::open_real_db(&config, "shard.db")
        .await
        .expect("open the shard");
    shard_facts::ensure_v3(db.as_ref(), 768)
        .await
        .expect("shard schema");
    db.execute(
        "INSERT INTO shard_metadata (key, value) VALUES ('embedding_model', 'test-model')",
        Vec::new(),
    )
    .await
    .expect("seed the model fact");

    let vector = mem_store::vector::vector_to_text(&vec![0.5_f32; 768]).expect("vector text");
    let mut rows = Vec::new();
    for index in 0..500 {
        rows.push(mem_store::db::Statement::new(
            "INSERT INTO memories (id, content, vector, container_tag, created_at, updated_at) \
             VALUES (?1, ?2, vector32(?3), 'opencode_project_measure', ?4, ?4)",
            vec![
                Value::Text(format!("mem-{index}")),
                Value::Text(format!("content {index}")),
                Value::Text(vector.clone()),
                Value::Integer(1_700_000_000_000 + index),
            ],
        ));
    }
    db.batch(rows, mem_store::db::BatchMode::Write)
        .await
        .expect("seed 500 rows");

    const MODEL_READS: u32 = 1_000;
    let started = Instant::now();
    for _ in 0..MODEL_READS {
        let _ = shard_facts::read_model_facts(db.as_ref())
            .await
            .expect("model facts");
    }
    let model_read_us_per_shard =
        started.elapsed().as_secs_f64() * 1_000_000.0 / f64::from(MODEL_READS);

    const SEARCHES: u32 = 20;
    let query_vector = vec![0.5_f32; 768];
    let started = Instant::now();
    for _ in 0..SEARCHES {
        let hits = mem_store::vector::search_in_shard(
            db.as_ref(),
            &query_vector,
            "opencode_project_measure",
            10,
            Some("content"),
        )
        .await
        .expect("search");
        assert!(!hits.is_empty(), "the measured search must return hits");
    }
    let search_us_per_query = started.elapsed().as_secs_f64() * 1_000_000.0 / f64::from(SEARCHES);

    let delta_pct = model_read_us_per_shard / search_us_per_query * 100.0;
    println!(
        "MEASURE profile=release rows=500 dims=768 model_read_us_per_shard={model_read_us_per_shard:.2} search_us_per_query={search_us_per_query:.2} delta_pct={delta_pct:.2}"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}
