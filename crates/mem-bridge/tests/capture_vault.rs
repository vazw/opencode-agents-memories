//! W8a — capture params default + drain enqueues vault feeder (AC-10, AC-11).

mod common;

use std::sync::Arc;

use common::{FakeEmbedder, FixedScopeResolver, RecorderTier1Writer, TestDir};
use mem_bridge::capture::SoundMemorySettings;
use mem_bridge::capture::{CaptureParams, CaptureService};
use mem_bridge::vault_feeder::VaultFeeder;
use mem_episodic::store::EpisodicStore;
use mem_store::serde_json;

#[test]
fn test_capture_params_vault_defaults_false() {
    let params: CaptureParams = serde_json::from_str("{}").expect("deserialize");
    assert!(!params.vault);
    let default = CaptureParams::default();
    assert!(!default.vault);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_drain_enqueues_vault_feeder_job() {
    let dir = TestDir::new("cv-drain");
    let cfg = dir.store_config();
    let vault = Arc::new(mem_vault::VaultStore::new(cfg.clone()));
    let feeder = Arc::new(VaultFeeder::new(vault));
    let store = Arc::new(EpisodicStore::new(dir.path().to_path_buf(), cfg.clone()));
    store.ready().await.expect("store ready");
    let service = CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store.clone()),
        Arc::new(RecorderTier1Writer::new(0)),
        Arc::new(FakeEmbedder::with_vector(common::unit_vector(0))),
        None,
        Arc::new(FixedScopeResolver::default()),
        131_072,
    )
    .with_sound_memory(SoundMemorySettings {
        enabled: true,
        raw_context_required: false,
    })
    .with_vault_feeder(Some(feeder.clone()));
    let params = CaptureParams {
        session_id: "sess".to_string(),
        capture_id: "cap-1".to_string(),
        project_id: "/srv/proj".to_string(),
        agent_id: "agent-1".to_string(),
        agent_name: None,
        parent_session_id: None,
        transcript: Some("transcript".to_string()),
        summary: Some("summary".to_string()),
        metadata: None,
        raw_context: Some("raw context text".to_string()),
        agent_role: None,
        vault: false,
    };
    service.enqueue(&params).await.expect("enqueue");
    service.drain_once().await.expect("drain");
    assert!(feeder.enqueue_count() >= 1);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_composition_wires_vault_feeder() {
    use std::collections::BTreeMap;
    let root = common::temp_dir("cv-composition");
    let home = common::temp_dir("cv-composition-home");
    let env: BTreeMap<String, String> = [("HOME", home.to_string_lossy().to_string())]
        .map(|(key, value)| (key.to_string(), value))
        .into_iter()
        .collect();
    let mut config =
        mem_core::config::Config::load(&mem_core::config::ConfigPaths::standard(&home), &env)
            .expect("the default config loads");
    config.storage_path = root.clone();
    config.embedding_dimensions = 8;
    let runtime = mem_bridge::composition::build_composed_runtime(
        &root,
        &config,
        Arc::new(NoopEmbedder),
        None,
    )
    .await
    .expect("the runtime composes");
    assert!(runtime.capture.vault_feeder().is_some());
    drop(runtime);
    common::cleanup_temp_dir(&root);
    common::cleanup_temp_dir(&home);
}

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
