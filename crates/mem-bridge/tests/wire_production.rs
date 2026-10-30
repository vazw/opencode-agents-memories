//! Issue 70 wave 1 — the production assembly seam (T38).
//!
//! This is the ONLY oracle that goes through `Engine::from_composed`: the
//! engine wires every service from the composed runtime, consults the
//! portability factory exactly once, and the composed learning lock is the
//! one the profile write contends on.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use common::{FakeEmbedder, TestDir, unit_vector};
use mem_bridge::composition::build_composed_runtime;
use mem_bridge::engine::{Engine, EngineConfig};
use mem_bridge::jsonrpc::{Response, parse_line};
use mem_bridge::tools::portability::{
    ExportOutcome, ExportRequest, ImportOutcome, ImportRequest, PortabilityBackend,
    PortabilityDeps, PortabilityFactory, PortabilityFault,
};
use mem_core::config::{Config, ConfigPaths};
use mem_profile::learning_lock::{LearningLock, LearningLockOptions};
use mem_store::project_tags::{CoreProjectTagSource, ProjectTagSource};
use serde_json::{Value, json};

/// What the factory saw when the seam consulted it.
#[derive(Debug, Clone, PartialEq)]
struct RecordedDeps {
    container_tag_prefix: String,
    embedding_model: String,
    embedding_dimensions: u32,
}

/// The recording factory: one build, one backend.
#[derive(Default)]
struct RecordingFactory {
    builds: Mutex<Vec<RecordedDeps>>,
    backend: Arc<RecordingBackend>,
}

#[derive(Default)]
struct RecordingBackend {
    exports: Mutex<Vec<(String, PathBuf)>>,
}

impl PortabilityFactory for RecordingFactory {
    fn build(&self, deps: &PortabilityDeps) -> Option<Arc<dyn PortabilityBackend>> {
        self.builds.lock().expect("builds").push(RecordedDeps {
            container_tag_prefix: deps.container_tag_prefix.clone(),
            embedding_model: deps.embedding_model.clone(),
            embedding_dimensions: deps.embedding_dimensions,
        });
        Some(self.backend.clone())
    }
}

impl PortabilityBackend for RecordingBackend {
    fn export<'a>(
        &'a self,
        request: &'a ExportRequest,
    ) -> mem_embed::embedder::BoxFuture<'a, Result<ExportOutcome, PortabilityFault>> {
        self.exports
            .lock()
            .expect("exports")
            .push((request.project_path.clone(), request.output_path.clone()));
        Box::pin(async {
            Ok(ExportOutcome {
                container_tag: "opencode_project_0000000000000000".to_string(),
                count: 0,
                scope_hash: "0000000000000000".to_string(),
            })
        })
    }

    fn import<'a>(
        &'a self,
        _request: &'a ImportRequest,
    ) -> mem_embed::embedder::BoxFuture<'a, Result<ImportOutcome, PortabilityFault>> {
        Box::pin(async {
            Ok(ImportOutcome {
                container_tag: "opencode_project_0000000000000000".to_string(),
                dry_run: true,
                imported: 0,
                skipped: Vec::new(),
                rejected: Vec::new(),
            })
        })
    }
}

async fn dispatch(engine: &Engine, method: &str, params: Value) -> Response {
    let request = parse_line(
        &json!({"id": "t38", "jsonrpc": "2.0", "method": method, "params": params}).to_string(),
    )
    .expect("production request parses");
    engine.dispatch(&request).await
}

fn error_code(response: &Response) -> i32 {
    response
        .error
        .as_ref()
        .map(|error| error.code)
        .expect("error envelope")
}

fn data(response: &Response) -> &Value {
    response
        .error
        .as_ref()
        .and_then(|error| error.data.as_ref())
        .expect("advisory data")
}

fn result(response: &Response) -> &Value {
    response.result.as_ref().expect("result envelope")
}

// ---------------------------------------------------------------------------
// T38 — one seam: every service wired, the factory called, one lock.
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_engine_from_composed_wires_every_service_and_calls_the_factory() {
    let root = TestDir::new("wire-production-root");
    let home = TestDir::new("wire-production-home");
    let env: BTreeMap<String, String> = [("HOME", home.path().to_string_lossy().to_string())]
        .map(|(key, value)| (key.to_string(), value))
        .into_iter()
        .collect();
    let mut config =
        Config::load(&ConfigPaths::standard(home.path()), &env).expect("the resolved config loads");
    config.storage_path = root.path().to_path_buf();
    config.embedding_dimensions = 8;
    config.user_email_override = Some("probe@example.com".to_string());

    let embedder = Arc::new(FakeEmbedder::with_vector(unit_vector(0)));
    let composed = build_composed_runtime(root.path(), &config, embedder, None)
        .await
        .expect("the runtime composes");
    let factory = RecordingFactory::default();
    let engine = Engine::from_composed(EngineConfig::default(), &composed, &config, Some(&factory));

    let project_dir = root.path().join("probe-project");
    std::fs::create_dir_all(&project_dir).expect("create the project directory");
    let project_dir = project_dir.to_string_lossy().to_string();

    // (a) `add` reaches the memories service through the real store.
    let added = dispatch(
        &engine,
        "add",
        json!({
            "session_id": "session-t38",
            "project_id": project_dir,
            "content": "production add",
        }),
    )
    .await;
    let id = result(&added)["id"].as_str().expect("id").to_string();
    assert!(id.starts_with("mem_"), "the memories service answered");

    // (b) `profile` reads null, then the composed resolver's email backs a write.
    let read = dispatch(
        &engine,
        "profile",
        json!({"session_id": "session-t38", "project_id": project_dir}),
    )
    .await;
    assert_eq!(result(&read), &json!({ "profile": null }));
    let written = dispatch(
        &engine,
        "profile",
        json!({
            "session_id": "session-t38",
            "project_id": project_dir,
            "content": "prefers production wiring",
        }),
    )
    .await;
    assert_eq!(result(&written), &json!({ "saved": true }));

    // (c) the composed learning lock is the consulted one: an externally held
    // lock blocks the next write, and its release unblocks it.
    let mut store_config = mem_store::config::from_core(&config);
    store_config.storage_path = root.path().to_path_buf();
    let external = LearningLock::new(LearningLockOptions::new(
        root.path().to_path_buf(),
        store_config,
    ));
    let guard = external.try_acquire(None).await.expect("hold the lock");
    let locked = dispatch(
        &engine,
        "profile",
        json!({
            "session_id": "session-t38",
            "project_id": project_dir,
            "content": "prefers the composed lock",
        }),
    )
    .await;
    assert_eq!(error_code(&locked), -32603);
    assert_eq!(
        data(&locked).get("reason").and_then(Value::as_str),
        Some("learning_locked"),
        "the composed lock is the one consulted"
    );
    guard.release().await;
    let unblocked = dispatch(
        &engine,
        "profile",
        json!({
            "session_id": "session-t38",
            "project_id": project_dir,
            "content": "prefers the composed lock",
        }),
    )
    .await;
    assert_eq!(result(&unblocked), &json!({ "saved": true }));

    // (d) `list-shards` uses the composed tag source over the same root.
    let expected_tag = CoreProjectTagSource {
        container_tag_prefix: config.container_tag_prefix.clone(),
        user_email_override: config.user_email_override.clone(),
        user_name_override: config.user_name_override.clone(),
    }
    .project_tag_info(Path::new(&project_dir))
    .tag;
    let inventory = dispatch(
        &engine,
        "list-shards",
        json!({"session_id": "session-t38", "project_path": project_dir}),
    )
    .await;
    let inventory = result(&inventory);
    assert_eq!(inventory["currentProject"]["tag"], expected_tag);
    assert_eq!(
        inventory["summary"]["total"], 1,
        "the shard `add` created is the one group"
    );

    // (e) `export` crosses the factory seam exactly once.
    let output_path = root.path().join("export.json");
    let output_path_str = output_path.to_string_lossy().to_string();
    let exported = dispatch(
        &engine,
        "export",
        json!({
            "session_id": "session-t38",
            "project_id": project_dir,
            "output_path": output_path_str,
        }),
    )
    .await;
    assert!(
        exported.result.is_some(),
        "export answered: {:?}",
        exported.error
    );

    let builds = factory.builds.lock().expect("builds");
    assert_eq!(builds.len(), 1, "the factory is consulted exactly once");
    assert_eq!(
        builds[0].container_tag_prefix, config.container_tag_prefix,
        "the deps carry the RESOLVED prefix"
    );
    assert_eq!(builds[0].embedding_dimensions, 8);
    assert_eq!(builds[0].embedding_model, config.embedding_model);
    drop(builds);
    let exports = factory.backend.exports.lock().expect("exports");
    assert_eq!(exports.len(), 1, "the fake backend saw one export");
    assert_eq!(exports[0].1, output_path);
}
