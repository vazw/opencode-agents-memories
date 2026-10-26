//! The portability round-trip oracle (upstream
//! `tests/memory-portability.test.ts` + the blueprint's W6.4 acceptance
//! criteria).
//!
//! The seam is the real one: a [`PortabilityService`] over a fake
//! [`FactStore`], a REAL [`ProfileStore`], a REAL [`EpisodicStore`] and a
//! fixed-vector fake embedder (upstream monkey-patches the embedding
//! service the same way).
//!
//! | Rust test | acceptance criterion |
//! |---|---|
//! | `bundle_schema_roundtrip` | AC1 — the wire shape |
//! | `roundtrip_durable_tiers` | AC2 — the per-field fidelity contract |
//! | `staging_roundtrip_preserves_sessions` | AC3 — the raw staging opt-in |
//! | `staging_opt_in_includes_sessions` | AC3 — the opt-in flag |
//! | `roundtrip_without_staging_restores_milestones` | AC3 — the milestone tier without staging |
//! | `import_missing_tiers_ok` | AC4 — a missing tier never deletes |
//! | `import_extra_tiers_preserved` | AC4 — an extra tier is preserved |
//! | `import_aborts_on_duplicate_ids` | upstream parity — zero writes |
//! | `import_dry_run_writes_nothing` | the dry-run mode |
//! | `import_replace_mode_replaces_the_row` | the replace mode |
//! | `import_rejects_unsupported_schema_version` | upstream's version guard |
//! | `write_bundle_is_private_and_atomic` | the writer rules |
//! | `scratch_base_is_absolute_under_the_target_dir` | issue 59 — the scratch base is cargo's test-data dir under the target dir |

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mem_embed::embedder::{BoxFuture, Embedder};
use mem_embed::error::EmbedError;
use mem_episodic::milestone::{Contributor, Milestone};
use mem_episodic::record::{AgentSession, NewAgentSession};
use mem_episodic::store::EpisodicStore;
use mem_portability::bundle::{Bundle, ExportedMemory, PluginInfo, SourceInfo};
use mem_portability::facts::FactStore;
use mem_portability::{
    ExportOptions, PortabilityError, PortabilityService, ReplaceMode, read_bundle, write_bundle,
};
use mem_profile::profile_store::{ProfileConfig, ProfileStore};
use mem_store::config::StoreConfig;

/// The embedding dimension the fake embedder and the stores agree on.
const DIMENSIONS: u32 = 8;

/// The container-tag prefix the services use.
const TAG_PREFIX: &str = "test";

// ---------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------

/// One stored fact: the container tag, the memory, the metadata JSON and
/// the two vectors.
#[derive(Debug, Clone)]
struct StoredFact {
    tag: String,
    memory: ExportedMemory,
    metadata_json: String,
    embedding: Vec<f32>,
    tags_embedding: Option<Vec<f32>>,
}

/// The in-memory durable-fact store.
#[derive(Debug, Default)]
struct FakeFacts {
    rows: Mutex<Vec<StoredFact>>,
}

impl FakeFacts {
    fn new() -> Self {
        Self::default()
    }

    fn seed(&self, tag: &str, memory: ExportedMemory) {
        self.rows.lock().expect("rows lock").push(StoredFact {
            tag: tag.to_string(),
            memory,
            metadata_json: "{}".to_string(),
            embedding: Vec::new(),
            tags_embedding: None,
        });
    }

    fn with_tag(&self, tag: &str) -> Vec<StoredFact> {
        self.rows
            .lock()
            .expect("rows lock")
            .iter()
            .filter(|row| row.tag == tag)
            .cloned()
            .collect()
    }

    fn count(&self) -> usize {
        self.rows.lock().expect("rows lock").len()
    }
}

#[async_trait::async_trait]
impl FactStore for FakeFacts {
    async fn list_memories(
        &self,
        container_tag: &str,
    ) -> Result<Vec<ExportedMemory>, PortabilityError> {
        Ok(self
            .with_tag(container_tag)
            .into_iter()
            .map(|row| row.memory)
            .collect())
    }

    async fn memory_exists(&self, id: &str, container_tag: &str) -> Result<bool, PortabilityError> {
        Ok(self
            .with_tag(container_tag)
            .into_iter()
            .any(|row| row.memory.id == id))
    }

    async fn insert_memory(
        &self,
        target_tag: &str,
        memory: ExportedMemory,
        embedding: Vec<f32>,
        tags_embedding: Option<Vec<f32>>,
        metadata_json: String,
    ) -> Result<(), PortabilityError> {
        self.rows.lock().expect("rows lock").push(StoredFact {
            tag: target_tag.to_string(),
            memory,
            metadata_json,
            embedding,
            tags_embedding,
        });
        Ok(())
    }

    async fn delete_memory(&self, id: &str, container_tag: &str) -> Result<(), PortabilityError> {
        let mut rows = self.rows.lock().expect("rows lock");
        let before = rows.len();
        rows.retain(|row| !(row.tag == container_tag && row.memory.id == id));
        if rows.len() == before {
            return Err(PortabilityError::Store(format!(
                "memory {id} not found in {container_tag}"
            )));
        }
        Ok(())
    }
}

/// The fixed-vector fake embedder (upstream's monkey-patched
/// `embedWithTimeout`).
struct FixedEmbedder;

impl Embedder for FixedEmbedder {
    fn warmup(&self) -> BoxFuture<'_, Result<(), EmbedError>> {
        Box::pin(async { Ok(()) })
    }

    fn embed(&self, _text: &str) -> BoxFuture<'_, Result<Vec<f32>, EmbedError>> {
        Box::pin(async { Ok(vec![0.25; DIMENSIONS as usize]) })
    }
}

/// The embedder that fails on a MARKED text (the prepare-phase proof).
///
/// The failure is triggered by the CONTENT, so the FIRST row embeds fine
/// and the SECOND row fails: a write path that embeds inside the insert
/// loop leaves the first row behind, while a prepare phase that computes
/// every embedding first never writes anything.
struct FailingEmbedder;

/// The text the failing embedder refuses.
const FAIL_MARKER: &str = "the second row";

impl Embedder for FailingEmbedder {
    fn warmup(&self) -> BoxFuture<'_, Result<(), EmbedError>> {
        Box::pin(async { Ok(()) })
    }

    fn embed(&self, text: &str) -> BoxFuture<'_, Result<Vec<f32>, EmbedError>> {
        if text.contains(FAIL_MARKER) {
            Box::pin(async {
                Err(EmbedError::Inference {
                    message: "the embedder is down".to_string(),
                })
            })
        } else {
            Box::pin(async { Ok(vec![0.25; DIMENSIONS as usize]) })
        }
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The base cargo reserves for this test binary's data: `<target>/tmp`.
///
/// Cargo creates it and resolves a relative `CARGO_TARGET_DIR` itself, so it is
/// absolute whatever the working directory or the variable's form. `env!` is
/// only defined for integration-test targets — inside `src/` it is a compile
/// error — so this helper must never leave `tests/`.
fn scratch_base() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
}

/// A scratch directory under the target dir (never `/tmp`).
fn scratch_root(label: &str) -> PathBuf {
    let dir = scratch_base()
        .join("scratch")
        .join("mem-portability")
        .join(label);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

// scratch base resolution (issue 59)

#[test]
fn scratch_base_is_absolute_under_the_target_dir() {
    let base = scratch_base();
    assert!(
        base.is_absolute(),
        "the scratch base must be absolute, got {:?}",
        base
    );
    let exe = std::env::current_exe().expect("current test executable");
    let target = exe
        .ancestors()
        .nth(3)
        .expect("the test executable lives at <target>/<profile>/deps/<test>");
    assert!(
        base.starts_with(target),
        "the scratch base must live under the target directory: {} is not under {}",
        base.display(),
        target.display()
    );
    assert!(
        !base.starts_with(env!("CARGO_MANIFEST_DIR")),
        "the scratch base must not live inside the checkout, got {}",
        base.display()
    );
}

/// A project directory pinned by the project marker (so the container tag
/// is deterministic and never resolves to an enclosing repository).
fn project_dir(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).expect("project dir");
    std::fs::write(dir.join(".opencode-mem-project"), "").expect("marker");
    dir
}

fn store_config(storage_path: &Path) -> StoreConfig {
    StoreConfig {
        storage_path: storage_path.to_path_buf(),
        embedding_dimensions: DIMENSIONS,
        embedding_model: "test-model".to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: TAG_PREFIX.to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
        allow_unguarded_open: false,
    }
}

/// One composed service: the facts, the profile store, the episodic store
/// and the embedder.
struct Harness {
    profiles: Arc<ProfileStore>,
    episodic: Arc<EpisodicStore>,
    service: PortabilityService,
}

fn harness(storage_root: &Path, facts: Arc<FakeFacts>) -> Harness {
    let config = store_config(storage_root);
    let profiles = Arc::new(ProfileStore::new(
        storage_root.to_path_buf(),
        config.clone(),
        ProfileConfig::default(),
    ));
    let episodic = Arc::new(EpisodicStore::new(
        storage_root.to_path_buf(),
        config.clone(),
    ));
    let service = PortabilityService::new(
        facts.clone(),
        profiles.clone(),
        episodic.clone(),
        Arc::new(FixedEmbedder),
    )
    .with_container_tag_prefix(TAG_PREFIX)
    .with_embedding_info("test-model", DIMENSIONS);
    Harness {
        profiles,
        episodic,
        service,
    }
}

fn exported_memory(id: &str, content: &str) -> ExportedMemory {
    ExportedMemory {
        id: id.to_string(),
        content: content.to_string(),
        memory_type: Some("note".to_string()),
        tags: Some(vec!["alpha".to_string(), "beta".to_string()]),
        created_at: 111,
        updated_at: Some(222),
        is_pinned: Some(true),
        metadata: Some(BTreeMap::from([(
            "keep".to_string(),
            serde_json::Value::Bool(true),
        )])),
        display_name: Some("source display".to_string()),
        user_name: Some("source user".to_string()),
        user_email: Some("source@example.com".to_string()),
        project_path: Some("/source/project".to_string()),
        project_name: Some("source-project".to_string()),
        git_repo_url: Some("https://example.com/source.git".to_string()),
    }
}

/// One agent session row for the staging tier.
///
/// The `project_id` is the episodic project key, which is the project
/// DIRECTORY's path — the same key the export lists by.
fn agent_session(project_id: &str, id: &str, session_id: &str, summary: &str) -> AgentSession {
    agent_session_with_parent(project_id, id, session_id, summary, None)
}

/// One agent session row that may carry a parent link.
///
/// The parent link is a SOURCE ROW id (the row's own `id`), never the
/// parent's `session_id`: the link is resolved against the episodic row
/// table, not against the session triple.
fn agent_session_with_parent(
    project_id: &str,
    id: &str,
    session_id: &str,
    summary: &str,
    parent_row_id: Option<String>,
) -> AgentSession {
    AgentSession {
        id: id.to_string(),
        project_id: project_id.to_string(),
        session_id: session_id.to_string(),
        agent_id: "agent-1".to_string(),
        parent_session_id: parent_row_id,
        summary: summary.to_string(),
        metadata: mem_episodic::payload::SessionMetadata {
            timestamp: 1_700_000_000_000,
            agent_name: "Agent One".to_string(),
            git_branch: Some("main".to_string()),
            git_commit: Some("abc1234".to_string()),
            files_touched: vec!["src/main.rs".to_string()],
            completed_work: vec![mem_episodic::record::WorkItem {
                kind: mem_episodic::record::WorkKind::Feature,
                description: "shipped the thing".to_string(),
            }],
            decisions: vec![mem_episodic::record::Decision {
                description: "use the seam".to_string(),
            }],
            open_threads: vec!["follow up".to_string()],
            extra: BTreeMap::new(),
        },
        embedding: Vec::new(),
        created_at: 1_700_000_000_000,
        updated_at: 1_700_000_000_000,
        deleted_at: None,
        capture_id: Some(format!("cap-{session_id}")),
        orphaned: false,
    }
}

/// One milestone with two contributors.
fn milestone(id: &str) -> Milestone {
    Milestone {
        id: id.to_string(),
        project_id: "source-project-id".to_string(),
        agent_id: "agent-1".to_string(),
        summary: "the consolidated summary".to_string(),
        contributors: vec![
            Contributor {
                session_row_id: "row-1".to_string(),
                session_id: "sess-1".to_string(),
                parent_session_id: None,
                agent_id: "agent-1".to_string(),
                agent_name: "Agent One".to_string(),
                created_at: 1_700_000_000_000,
                summary: "the first source summary".to_string(),
            },
            Contributor {
                session_row_id: "row-2".to_string(),
                session_id: "sess-2".to_string(),
                parent_session_id: Some("row-1".to_string()),
                agent_id: "agent-1".to_string(),
                agent_name: "Agent One".to_string(),
                created_at: 1_700_000_100_000,
                summary: "the second source summary".to_string(),
            },
        ],
        source_count: 2,
        window_start: 1_700_000_000_000,
        window_end: 1_700_000_100_000,
        embedding: Vec::new(),
        created_at: 1_700_000_200_000,
    }
}

/// Seeds a session row into the episodic store.
async fn seed_session(store: &EpisodicStore, session: &AgentSession) {
    let new = NewAgentSession {
        project_id: session.project_id.clone(),
        session_id: session.session_id.clone(),
        agent_id: session.agent_id.clone(),
        parent_session_id: session.parent_session_id.clone(),
        summary: session.summary.clone(),
        agent_name: session.metadata.agent_name.clone(),
        git_branch: session.metadata.git_branch.clone(),
        git_commit: session.metadata.git_commit.clone(),
        files_touched: session.metadata.files_touched.clone(),
        completed_work: session.metadata.completed_work.clone(),
        decisions: session.metadata.decisions.clone(),
        open_threads: session.metadata.open_threads.clone(),
        extra: session.metadata.extra.clone(),
        timestamp: Some(session.metadata.timestamp),
        capture_id: session.capture_id.clone(),
    };
    store
        .upsert_session(new, &FixedEmbedder)
        .await
        .expect("seed session");
}

/// The `metadata` object the import stamped.
fn metadata_of(fact: &StoredFact) -> serde_json::Value {
    serde_json::from_str(&fact.metadata_json).expect("metadata json")
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

#[test]
fn bundle_schema_roundtrip() {
    let root = scratch_root("schema");
    let mut bundle = Bundle::empty();
    bundle.schema_version = mem_portability::bundle::SchemaVersion::CURRENT;
    bundle.exported_at = "2026-10-22T00:00:00.000Z".to_string();
    bundle.plugin = PluginInfo {
        package: "opencode-agents-memory".to_string(),
        version: "0.1.0".to_string(),
    };
    bundle.source = SourceInfo {
        container_tag: "test_project_0123456789abcdef".to_string(),
        scope: mem_portability::bundle::BundleScope::Project,
        scope_hash: "0123456789abcdef".to_string(),
        project_path: Some("/tmp/project".to_string()),
        project_name: Some("project".to_string()),
        git_repo_url: None,
    };
    bundle.embedding = mem_portability::bundle::EmbeddingInfo {
        model: "test-model".to_string(),
        dimensions: DIMENSIONS,
    };
    bundle.memories = vec![exported_memory("m-1", "the content")];

    let text = serde_json::to_string(&bundle).expect("serialize");
    let parsed: Bundle = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(parsed, bundle);

    // The wire names are upstream's.
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(value["schemaVersion"], serde_json::json!(1));
    assert_eq!(
        value["exportedAt"],
        serde_json::json!("2026-10-22T00:00:00.000Z")
    );
    assert_eq!(value["source"]["scope"], serde_json::json!("project"));
    assert_eq!(
        value["source"]["scopeHash"],
        serde_json::json!("0123456789abcdef")
    );
    assert_eq!(value["memories"][0]["id"], serde_json::json!("m-1"));
    assert_eq!(value["memories"][0]["createdAt"], serde_json::json!(111));
    assert_eq!(value["memories"][0]["isPinned"], serde_json::json!(true));
    assert_eq!(
        value["memories"][0]["tags"],
        serde_json::json!(["alpha", "beta"])
    );
    // The memory type travels under upstream's `type` key — never under a
    // camelCase `memoryType` (an upstream bundle's `type` would be
    // dropped on import).
    assert_eq!(value["memories"][0]["type"], serde_json::json!("note"));
    assert!(value["memories"][0].get("memoryType").is_none());
    // The vector never travels.
    assert!(value["memories"][0].get("vector").is_none());
    // The optional tiers are absent when they are absent.
    assert!(value.get("staging").is_none());
    assert!(value.get("profiles").is_none());
    assert!(value.get("milestones").is_none());
    let _ = root;
}

/// The bundle reads an UPSTREAM-shaped document: upstream's memory type
/// key is `type`, and the port must not drop it.
#[test]
fn the_bundle_reads_the_upstream_type_key() {
    let text = r#"{
      "schemaVersion": 1,
      "exportedAt": "2026-10-22T00:00:00.000Z",
      "plugin": {"package": "opencode-mem", "version": "2.31.3"},
      "source": {
        "containerTag": "opencode_project_0123456789abcdef",
        "scope": "project",
        "scopeHash": "0123456789abcdef"
      },
      "embedding": {"model": "bge-m3", "dimensions": 8},
      "memories": [
        {"id": "m-1", "content": "the content", "type": "decision", "createdAt": 111}
      ]
    }"#;
    let bundle: Bundle = serde_json::from_str(text).expect("the upstream shape deserializes");
    assert_eq!(bundle.memories.len(), 1);
    assert_eq!(bundle.memories[0].memory_type.as_deref(), Some("decision"));
    // And it is re-emitted under the SAME key.
    let value: serde_json::Value = serde_json::to_value(&bundle).expect("re-serialize");
    assert_eq!(value["memories"][0]["type"], serde_json::json!("decision"));
    assert!(value["memories"][0].get("memoryType").is_none());
}

#[tokio::test]
async fn roundtrip_durable_tiers() {
    let root = scratch_root("durable");
    let source_dir = project_dir(&root, "source-project");
    let target_dir = project_dir(&root, "target-project");
    let source_store = root.join("source-store");
    let target_store = root.join("target-store");
    std::fs::create_dir_all(&source_store).expect("source store");
    std::fs::create_dir_all(&target_store).expect("target store");

    let source_facts = Arc::new(FakeFacts::new());
    let target_facts = Arc::new(FakeFacts::new());
    let source = harness(&source_store, source_facts.clone());
    let target = harness(&target_store, target_facts.clone());

    // The source's project facts.
    let source_scope = source
        .service
        .resolve_scope(&source_dir)
        .expect("source scope");
    source_facts.seed(
        &source_scope.container_tag,
        exported_memory("m-1", "first fact"),
    );
    source_facts.seed(
        &source_scope.container_tag,
        exported_memory("m-2", "second fact"),
    );
    // The source's global (user-scope) fact.
    source_facts.seed(
        &source_scope.user_container_tag,
        exported_memory("g-1", "global fact"),
    );

    // The source's profile.
    let profile_id = source
        .profiles
        .create_profile(
            "user-1",
            "Display",
            "user",
            "user@example.com",
            &mem_profile::profile_store::ProfileData::default(),
            7,
        )
        .await
        .expect("create profile");

    // The source's milestone and its two session rows.
    let source_sessions = vec![
        agent_session(
            &source_dir.to_string_lossy(),
            "row-1",
            "sess-1",
            "the first session",
        ),
        agent_session(
            &source_dir.to_string_lossy(),
            "row-2",
            "sess-2",
            "the second session",
        ),
    ];
    for session in &source_sessions {
        seed_session(&source.episodic, session).await;
    }
    let milestone_row = milestone("mile-1");
    source
        .episodic
        .write_batch(vec![mem_store::db::Statement::new(
            mem_store::episodic_sql::INSERT_MILESTONE,
            vec![
                mem_store::db::Value::Text(milestone_row.id.clone()),
                mem_store::db::Value::Text(source_dir.to_string_lossy().to_string()),
                mem_store::db::Value::Text(milestone_row.agent_id.clone()),
                mem_store::db::Value::Text(milestone_row.summary.clone()),
                mem_store::db::Value::Text(
                    mem_episodic::milestone::encode_contributors(&milestone_row.contributors)
                        .expect("contributors"),
                ),
                mem_store::db::Value::Integer(milestone_row.source_count),
                mem_store::db::Value::Integer(milestone_row.window_start),
                mem_store::db::Value::Integer(milestone_row.window_end),
                mem_store::db::Value::Text("[0.25,0.25,0.25,0.25,0.25,0.25,0.25,0.25]".to_string()),
                mem_store::db::Value::Integer(milestone_row.created_at),
            ],
        )])
        .await
        .expect("seed milestone");

    // --- Export.
    let bundle = source
        .service
        .export(
            &source_dir,
            ExportOptions {
                include_staging: true,
            },
        )
        .await
        .expect("export");
    assert_eq!(bundle.schema_version.as_u32(), 1);
    assert_eq!(bundle.source.container_tag, source_scope.container_tag);
    assert_eq!(bundle.source.scope_hash, source_scope.scope_hash);
    assert_eq!(bundle.memories.len(), 2);
    assert_eq!(
        bundle.global_memories.as_ref().map(|rows| rows.len()),
        Some(1)
    );
    assert_eq!(bundle.profiles.as_ref().map(|rows| rows.len()), Some(1));
    assert_eq!(bundle.milestones.as_ref().map(|rows| rows.len()), Some(1));
    assert_eq!(
        bundle.staging.as_ref().map(|tier| tier.sessions.len()),
        Some(2)
    );

    // --- Import into the fresh target.
    let report = target
        .service
        .import(&target_dir, bundle.clone(), ReplaceMode::Merge, false)
        .await
        .expect("import");
    // The report counts every imported row: the two project facts plus the
    // one global fact.
    assert_eq!(report.imported, 3);
    assert!(report.skipped.is_empty());
    assert!(report.rejected.is_empty());
    let target_scope = target
        .service
        .resolve_scope(&target_dir)
        .expect("target scope");
    assert_eq!(report.container_tag, target_scope.container_tag);
    assert_ne!(target_scope.container_tag, source_scope.container_tag);

    // --- The per-field fidelity contract.
    let imported = target_facts.with_tag(&target_scope.container_tag);
    assert_eq!(imported.len(), 2);
    let source_memories = source_facts.with_tag(&source_scope.container_tag);
    for original in &source_memories {
        let stored = imported
            .iter()
            .find(|row| row.memory.id == original.memory.id)
            .expect("imported memory");
        let rewritten = &stored.memory;
        // Every field outside the rewrite list is byte-equal.
        assert_eq!(rewritten.id, original.memory.id);
        assert_eq!(rewritten.content, original.memory.content);
        assert_eq!(rewritten.memory_type, original.memory.memory_type);
        assert_eq!(rewritten.tags, original.memory.tags);
        assert_eq!(rewritten.created_at, original.memory.created_at);
        assert_eq!(rewritten.updated_at, original.memory.updated_at);
        assert_eq!(rewritten.is_pinned, original.memory.is_pinned);
        assert_eq!(rewritten.user_name, original.memory.user_name);
        assert_eq!(rewritten.user_email, original.memory.user_email);
        assert_eq!(rewritten.git_repo_url, original.memory.git_repo_url);
        // The rewrite list: the target-bound fields.
        assert_ne!(rewritten.project_path, original.memory.project_path);
        assert_ne!(rewritten.project_name, original.memory.project_name);
        assert_ne!(rewritten.display_name, original.memory.display_name);
        // The metadata: the source's keys survive and the provenance is
        // stamped.
        let metadata = metadata_of(stored);
        assert_eq!(metadata["keep"], serde_json::json!(true));
        assert_eq!(metadata["source"], serde_json::json!("import"));
        assert_eq!(metadata["exportSchemaVersion"], serde_json::json!(1));
        assert_eq!(
            metadata["originalContainerTag"],
            serde_json::json!(source_scope.container_tag.as_str())
        );
        assert!(metadata.get("importedAt").is_some());
        // The vector was re-embedded, not carried over.
        assert_eq!(stored.embedding.len(), DIMENSIONS as usize);
        assert!(stored.tags_embedding.is_some());
    }

    // The global tier landed in the target's user container.
    let globals = target_facts.with_tag(&target_scope.user_container_tag);
    assert_eq!(globals.len(), 1);
    assert_eq!(globals[0].memory.id, "g-1");

    // The profile tier: the id, the version and the timestamps survive.
    let profiles = target
        .profiles
        .get_all_active_profiles()
        .await
        .expect("target profiles");
    let imported_profile = profiles
        .iter()
        .find(|profile| profile.id == profile_id)
        .expect("imported profile");
    assert_eq!(imported_profile.user_id, "user-1");
    assert_eq!(imported_profile.total_prompts_analyzed, 7);
    let source_profiles = source
        .profiles
        .get_all_active_profiles()
        .await
        .expect("source profiles");
    let original_profile = source_profiles
        .iter()
        .find(|profile| profile.id == profile_id)
        .expect("source profile");
    assert_eq!(imported_profile.version, original_profile.version);
    assert_eq!(imported_profile.created_at, original_profile.created_at);
    assert_eq!(
        imported_profile.last_analyzed_at,
        original_profile.last_analyzed_at
    );

    // The milestone tier: the project id is rewritten, the id and the
    // contributors are preserved.
    let milestones = target
        .episodic
        .list_milestones(&target_dir.to_string_lossy(), 100)
        .await
        .expect("target milestones");
    assert_eq!(milestones.len(), 1);
    assert_eq!(milestones[0].id, "mile-1");
    assert_eq!(milestones[0].project_id, target_dir.to_string_lossy());
    assert_eq!(milestones[0].contributors.len(), 2);
    assert_eq!(milestones[0].contributors[0].session_id, "sess-1");
    assert_eq!(milestones[0].contributors[1].session_id, "sess-2");
    assert_eq!(milestones[0].summary, "the consolidated summary");
}

#[tokio::test]
async fn staging_roundtrip_preserves_sessions() {
    let root = scratch_root("staging");
    let source_dir = project_dir(&root, "source-project");
    let target_dir = project_dir(&root, "target-project");
    let source_store = root.join("source-store");
    let target_store = root.join("target-store");
    std::fs::create_dir_all(&source_store).expect("source store");
    std::fs::create_dir_all(&target_store).expect("target store");

    let source = harness(&source_store, Arc::new(FakeFacts::new()));
    let target = harness(&target_store, Arc::new(FakeFacts::new()));

    for session in [
        agent_session(
            &source_dir.to_string_lossy(),
            "row-1",
            "sess-1",
            "the first session",
        ),
        agent_session(
            &source_dir.to_string_lossy(),
            "row-2",
            "sess-2",
            "the second session",
        ),
    ] {
        seed_session(&source.episodic, &session).await;
    }
    let source_rows = source
        .episodic
        .list_sessions(&source_dir.to_string_lossy(), 100)
        .await
        .expect("source sessions");
    assert_eq!(source_rows.len(), 2);

    let bundle = source
        .service
        .export(
            &source_dir,
            ExportOptions {
                include_staging: true,
            },
        )
        .await
        .expect("export");
    let staging = bundle.staging.as_ref().expect("staging tier");
    assert_eq!(staging.sessions.len(), 2);

    target
        .service
        .import(&target_dir, bundle, ReplaceMode::Merge, false)
        .await
        .expect("import");

    // The row count is equal — no silent drop.
    let target_rows = target
        .episodic
        .list_sessions(&target_dir.to_string_lossy(), 100)
        .await
        .expect("target sessions");
    assert_eq!(target_rows.len(), source_rows.len());
    assert_eq!(target_rows.len(), 2);
    // The mapping table holds.
    let imported = target_rows
        .iter()
        .find(|row| row.session_id == "sess-1")
        .expect("imported session");
    assert_eq!(imported.summary, "the first session");
    assert_eq!(imported.agent_id, "agent-1");
    assert_eq!(imported.metadata.agent_name, "Agent One");
    assert_eq!(imported.metadata.git_branch.as_deref(), Some("main"));
    assert_eq!(imported.metadata.git_commit.as_deref(), Some("abc1234"));
    assert_eq!(
        imported.metadata.files_touched,
        vec!["src/main.rs".to_string()]
    );
    assert_eq!(imported.metadata.completed_work.len(), 1);
    assert_eq!(imported.metadata.decisions.len(), 1);
    assert_eq!(
        imported.metadata.open_threads,
        vec!["follow up".to_string()]
    );
    assert_eq!(imported.metadata.timestamp, 1_700_000_000_000);
    assert_eq!(imported.capture_id.as_deref(), Some("cap-sess-1"));
    // The project id is the TARGET's.
    assert_eq!(imported.project_id, target_dir.to_string_lossy());
}

/// AC3 — a session with a PARENT link survives the round trip.
///
/// The source's parent link is a SOURCE ROW id and the target mints its
/// own row ids, so the link can only be resolved after every row exists:
/// the import inserts the rows first and links them afterwards. (The
/// export lists the rows `created_at DESC`, so the CHILD — whose parent
/// was captured first — is listed first: the order can never be relied
/// on.)
#[tokio::test]
async fn staging_roundtrip_preserves_parent_links() {
    let root = scratch_root("staging-parent");
    let source_dir = project_dir(&root, "source-project");
    let target_dir = project_dir(&root, "target-project");
    let source_store = root.join("source-store");
    let target_store = root.join("target-store");
    std::fs::create_dir_all(&source_store).expect("source store");
    std::fs::create_dir_all(&target_store).expect("target store");

    let source = harness(&source_store, Arc::new(FakeFacts::new()));
    let target = harness(&target_store, Arc::new(FakeFacts::new()));

    // The parent row.
    seed_session(
        &source.episodic,
        &agent_session(
            &source_dir.to_string_lossy(),
            "row-parent",
            "sess-parent",
            "the parent session",
        ),
    )
    .await;
    // The parent's SOURCE row id.
    let source_rows = source
        .episodic
        .list_sessions(&source_dir.to_string_lossy(), 100)
        .await
        .expect("source sessions");
    let source_parent_row_id = source_rows
        .iter()
        .find(|row| row.session_id == "sess-parent")
        .expect("the parent row")
        .id
        .clone();

    // The child row: its parent link is the SOURCE's row id.
    seed_session(
        &source.episodic,
        &agent_session_with_parent(
            &source_dir.to_string_lossy(),
            "row-child",
            "sess-child",
            "the child session",
            Some(source_parent_row_id.clone()),
        ),
    )
    .await;

    let bundle = source
        .service
        .export(
            &source_dir,
            ExportOptions {
                include_staging: true,
            },
        )
        .await
        .expect("export");
    let staging = bundle.staging.as_ref().expect("staging tier");
    assert_eq!(staging.sessions.len(), 2);
    // The child carries the source's parent link.
    let child = staging
        .sessions
        .iter()
        .find(|row| row.session_id == "sess-child")
        .expect("the child row");
    assert_eq!(
        child.parent_session_id.as_deref(),
        Some(source_parent_row_id.as_str())
    );

    target
        .service
        .import(&target_dir, bundle, ReplaceMode::Merge, false)
        .await
        .expect("import");

    // Both rows landed.
    let target_rows = target
        .episodic
        .list_sessions(&target_dir.to_string_lossy(), 100)
        .await
        .expect("target sessions");
    assert_eq!(target_rows.len(), 2);
    let target_parent = target_rows
        .iter()
        .find(|row| row.session_id == "sess-parent")
        .expect("the imported parent");
    let target_child = target_rows
        .iter()
        .find(|row| row.session_id == "sess-child")
        .expect("the imported child");
    // The child's parent points at the TARGET's parent row id — the
    // target mints its own row ids, so the source's id cannot resolve.
    assert_eq!(
        target_child.parent_session_id.as_deref(),
        Some(target_parent.id.as_str()),
        "the child's parent must be the target's parent row"
    );
    assert_ne!(target_parent.id, source_parent_row_id);
}

/// AC3 — a parent link whose parent row is NOT in the bundle is RECORDED,
/// never silently dropped.
#[tokio::test]
async fn staging_records_an_unresolvable_parent_link() {
    let root = scratch_root("staging-missing-parent");
    let target_dir = project_dir(&root, "target-project");
    let target_store = root.join("target-store");
    std::fs::create_dir_all(&target_store).expect("target store");
    let target = harness(&target_store, Arc::new(FakeFacts::new()));

    let mut bundle = Bundle::empty();
    bundle.schema_version = mem_portability::bundle::SchemaVersion::CURRENT;
    bundle.exported_at = "2026-10-22T00:00:00.000Z".to_string();
    bundle.source = SourceInfo {
        container_tag: "test_project_0123456789abcdef".to_string(),
        scope: mem_portability::bundle::BundleScope::Project,
        scope_hash: "0123456789abcdef".to_string(),
        project_path: None,
        project_name: None,
        git_repo_url: None,
    };
    bundle.memories = Vec::new();
    bundle.staging = Some(mem_portability::bundle::StagingTier {
        sessions: vec![agent_session_with_parent(
            "/source/project",
            "row-child",
            "sess-child",
            "the orphan child",
            Some("row-that-is-not-here".to_string()),
        )],
    });

    let report = target
        .service
        .import(&target_dir, bundle, ReplaceMode::Merge, false)
        .await
        .expect("import");
    // The row itself landed.
    let rows = target
        .episodic
        .list_sessions(&target_dir.to_string_lossy(), 100)
        .await
        .expect("target sessions");
    assert_eq!(rows.len(), 1);
    // The link is recorded, not dropped.
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].id, "row-child");
    assert!(
        report.skipped[0].reason.contains("row-that-is-not-here"),
        "{:?}",
        report.skipped[0].reason
    );
    assert!(rows[0].parent_session_id.is_none());
}

#[tokio::test]
async fn staging_opt_in_includes_sessions() {
    let root = scratch_root("staging-opt-in");
    let source_dir = project_dir(&root, "source-project");
    let source_store = root.join("source-store");
    std::fs::create_dir_all(&source_store).expect("source store");
    let source = harness(&source_store, Arc::new(FakeFacts::new()));

    seed_session(
        &source.episodic,
        &agent_session(
            &source_dir.to_string_lossy(),
            "row-1",
            "sess-1",
            "the first session",
        ),
    )
    .await;

    let without = source
        .service
        .export(
            &source_dir,
            ExportOptions {
                include_staging: false,
            },
        )
        .await
        .expect("export without staging");
    assert!(without.staging.is_none());

    let with = source
        .service
        .export(
            &source_dir,
            ExportOptions {
                include_staging: true,
            },
        )
        .await
        .expect("export with staging");
    let staging = with.staging.as_ref().expect("staging tier");
    assert_eq!(staging.sessions.len(), 1);
    assert_eq!(staging.sessions[0].session_id, "sess-1");
}

#[tokio::test]
async fn roundtrip_without_staging_restores_milestones() {
    let root = scratch_root("milestones");
    let source_dir = project_dir(&root, "source-project");
    let target_dir = project_dir(&root, "target-project");
    let source_store = root.join("source-store");
    let target_store = root.join("target-store");
    std::fs::create_dir_all(&source_store).expect("source store");
    std::fs::create_dir_all(&target_store).expect("target store");

    let source = harness(&source_store, Arc::new(FakeFacts::new()));
    let target = harness(&target_store, Arc::new(FakeFacts::new()));

    // A milestone whose contributor session ids are DANGLING: the source
    // session rows do not exist (and are never created).
    let row = milestone("mile-dangling");
    source
        .episodic
        .write_batch(vec![mem_store::db::Statement::new(
            mem_store::episodic_sql::INSERT_MILESTONE,
            vec![
                mem_store::db::Value::Text(row.id.clone()),
                mem_store::db::Value::Text(source_dir.to_string_lossy().to_string()),
                mem_store::db::Value::Text(row.agent_id.clone()),
                mem_store::db::Value::Text(row.summary.clone()),
                mem_store::db::Value::Text(
                    mem_episodic::milestone::encode_contributors(&row.contributors)
                        .expect("contributors"),
                ),
                mem_store::db::Value::Integer(row.source_count),
                mem_store::db::Value::Integer(row.window_start),
                mem_store::db::Value::Integer(row.window_end),
                mem_store::db::Value::Text("[0.25,0.25,0.25,0.25,0.25,0.25,0.25,0.25]".to_string()),
                mem_store::db::Value::Integer(row.created_at),
            ],
        )])
        .await
        .expect("seed milestone");

    let bundle = source
        .service
        .export(
            &source_dir,
            ExportOptions {
                include_staging: false,
            },
        )
        .await
        .expect("export");
    assert!(bundle.staging.is_none());
    assert_eq!(bundle.milestones.as_ref().map(|rows| rows.len()), Some(1));

    target
        .service
        .import(&target_dir, bundle, ReplaceMode::Merge, false)
        .await
        .expect("import");

    let milestones = target
        .episodic
        .list_milestones(&target_dir.to_string_lossy(), 100)
        .await
        .expect("target milestones");
    assert_eq!(milestones.len(), 1);
    assert_eq!(milestones[0].contributors.len(), 2);
    assert_eq!(milestones[0].contributors[0].session_id, "sess-1");
    assert_eq!(milestones[0].contributors[1].session_id, "sess-2");
    // The dangling contributor session ids are tolerated.
    assert_eq!(milestones[0].contributors[0].session_row_id, "row-1");
    // The staging tier was absent, so no session rows were created.
    let sessions = target
        .episodic
        .list_sessions(&target_dir.to_string_lossy(), 100)
        .await
        .expect("target sessions");
    assert!(sessions.is_empty());
}

#[tokio::test]
async fn import_missing_tiers_ok() {
    let root = scratch_root("missing-tiers");
    let target_dir = project_dir(&root, "target-project");
    let target_store = root.join("target-store");
    std::fs::create_dir_all(&target_store).expect("target store");
    let target_facts = Arc::new(FakeFacts::new());
    let target = harness(&target_store, target_facts.clone());

    // The target already holds a row.
    let scope = target.service.resolve_scope(&target_dir).expect("scope");
    target_facts.seed(
        &scope.container_tag,
        exported_memory("existing", "pre-existing row"),
    );

    // A bundle with ONLY the primary tier.
    let mut bundle = Bundle::empty();
    bundle.schema_version = mem_portability::bundle::SchemaVersion::CURRENT;
    bundle.exported_at = "2026-10-22T00:00:00.000Z".to_string();
    bundle.source = SourceInfo {
        container_tag: "test_project_0123456789abcdef".to_string(),
        scope: mem_portability::bundle::BundleScope::Project,
        scope_hash: "0123456789abcdef".to_string(),
        project_path: None,
        project_name: None,
        git_repo_url: None,
    };
    bundle.memories = vec![exported_memory("m-1", "the new row")];
    // No global tier, no profiles, no milestones, no staging.

    let report = target
        .service
        .import(&target_dir, bundle, ReplaceMode::Merge, false)
        .await
        .expect("import");
    assert_eq!(report.imported, 1);

    // The pre-existing row is untouched, and the new row is there.
    let rows = target_facts.with_tag(&scope.container_tag);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|row| row.memory.id == "existing"));
    assert!(rows.iter().any(|row| row.memory.id == "m-1"));
    // No milestone was created.
    let milestones = target
        .episodic
        .list_milestones(&target_dir.to_string_lossy(), 100)
        .await
        .expect("milestones");
    assert!(milestones.is_empty());
    // No profile was created.
    let profiles = target
        .profiles
        .get_all_active_profiles()
        .await
        .expect("profiles");
    assert!(profiles.is_empty());
}

#[tokio::test]
async fn import_extra_tiers_preserved() {
    let root = scratch_root("extra-tiers");
    let target_dir = project_dir(&root, "target-project");
    let target_store = root.join("target-store");
    std::fs::create_dir_all(&target_store).expect("target store");
    let target_facts = Arc::new(FakeFacts::new());
    let target = harness(&target_store, target_facts.clone());

    // A bundle carrying an UNKNOWN top-level tier.
    let mut bundle = Bundle::empty();
    bundle.schema_version = mem_portability::bundle::SchemaVersion::CURRENT;
    bundle.exported_at = "2026-10-22T00:00:00.000Z".to_string();
    bundle.source = SourceInfo {
        container_tag: "test_project_0123456789abcdef".to_string(),
        scope: mem_portability::bundle::BundleScope::Project,
        scope_hash: "0123456789abcdef".to_string(),
        project_path: None,
        project_name: None,
        git_repo_url: None,
    };
    bundle.memories = vec![exported_memory("m-1", "the new row")];
    bundle
        .extra
        .insert("futureTier".to_string(), serde_json::json!({"a": 1}));

    // The round trip preserves the unknown tier.
    let text = serde_json::to_string(&bundle).expect("serialize");
    let parsed: Bundle = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(
        parsed.extra.get("futureTier"),
        Some(&serde_json::json!({"a": 1}))
    );
    let re_emitted = serde_json::to_string(&parsed).expect("re-serialize");
    let re_emitted_value: serde_json::Value = serde_json::from_str(&re_emitted).expect("re-parse");
    assert_eq!(re_emitted_value["futureTier"], serde_json::json!({"a": 1}));

    // The import accepts the bundle and applies the known tier.
    let report = target
        .service
        .import(&target_dir, parsed, ReplaceMode::Merge, false)
        .await
        .expect("import");
    assert_eq!(report.imported, 1);
    let scope = target.service.resolve_scope(&target_dir).expect("scope");
    assert_eq!(target_facts.with_tag(&scope.container_tag).len(), 1);
}

#[tokio::test]
async fn import_aborts_on_duplicate_ids() {
    let root = scratch_root("duplicates");
    let target_dir = project_dir(&root, "target-project");
    let target_store = root.join("target-store");
    std::fs::create_dir_all(&target_store).expect("target store");
    let target_facts = Arc::new(FakeFacts::new());
    let target = harness(&target_store, target_facts.clone());

    let scope = target.service.resolve_scope(&target_dir).expect("scope");
    target_facts.seed(
        &scope.container_tag,
        exported_memory("m-1", "already there"),
    );

    let mut bundle = Bundle::empty();
    bundle.schema_version = mem_portability::bundle::SchemaVersion::CURRENT;
    bundle.exported_at = "2026-10-22T00:00:00.000Z".to_string();
    bundle.source = SourceInfo {
        container_tag: "test_project_0123456789abcdef".to_string(),
        scope: mem_portability::bundle::BundleScope::Project,
        scope_hash: "0123456789abcdef".to_string(),
        project_path: None,
        project_name: None,
        git_repo_url: None,
    };
    bundle.memories = vec![
        exported_memory("m-1", "duplicate"),
        exported_memory("m-2", "fresh"),
    ];

    let error = target
        .service
        .import(&target_dir, bundle, ReplaceMode::Merge, false)
        .await
        .expect_err("the import aborts");
    match &error {
        PortabilityError::ImportAborted { count, rejected } => {
            assert_eq!(*count, 1);
            assert_eq!(rejected[0].id, "m-1");
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(
        error
            .to_string()
            .contains("Import aborted: 1 memory id(s) already exist"),
        "{error}"
    );
    // ZERO writes: the fresh row was not inserted.
    let rows = target_facts.with_tag(&scope.container_tag);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].memory.id, "m-1");
}

#[tokio::test]
async fn import_dry_run_writes_nothing() {
    let root = scratch_root("dry-run");
    let target_dir = project_dir(&root, "target-project");
    let target_store = root.join("target-store");
    std::fs::create_dir_all(&target_store).expect("target store");
    let target_facts = Arc::new(FakeFacts::new());
    let target = harness(&target_store, target_facts.clone());

    let mut bundle = Bundle::empty();
    bundle.schema_version = mem_portability::bundle::SchemaVersion::CURRENT;
    bundle.exported_at = "2026-10-22T00:00:00.000Z".to_string();
    bundle.source = SourceInfo {
        container_tag: "test_project_0123456789abcdef".to_string(),
        scope: mem_portability::bundle::BundleScope::Project,
        scope_hash: "0123456789abcdef".to_string(),
        project_path: None,
        project_name: None,
        git_repo_url: None,
    };
    bundle.memories = vec![exported_memory("m-1", "the row")];

    let report = target
        .service
        .import(&target_dir, bundle.clone(), ReplaceMode::Merge, true)
        .await
        .expect("dry run");
    assert_eq!(report.imported, 1);
    // Nothing was written.
    assert_eq!(target_facts.count(), 0);

    // The real run writes it.
    target
        .service
        .import(&target_dir, bundle, ReplaceMode::Merge, false)
        .await
        .expect("import");
    assert_eq!(target_facts.count(), 1);
}

/// The dry run reports the counts the WRITE PATH would apply: the same
/// skip predicate, so a row the write path skips is not counted as
/// imported.
#[tokio::test]
async fn import_dry_run_counts_the_skipped_rows_like_the_write_path() {
    let root = scratch_root("dry-run-skips");
    let target_dir = project_dir(&root, "target-project");
    let target_store = root.join("target-store");
    std::fs::create_dir_all(&target_store).expect("target store");
    let target_facts = Arc::new(FakeFacts::new());
    let target = harness(&target_store, target_facts.clone());

    let mut bundle = Bundle::empty();
    bundle.schema_version = mem_portability::bundle::SchemaVersion::CURRENT;
    bundle.exported_at = "2026-10-22T00:00:00.000Z".to_string();
    bundle.source = SourceInfo {
        container_tag: "test_project_0123456789abcdef".to_string(),
        scope: mem_portability::bundle::BundleScope::Project,
        scope_hash: "0123456789abcdef".to_string(),
        project_path: None,
        project_name: None,
        git_repo_url: None,
    };
    bundle.memories = vec![
        exported_memory("m-1", "the row"),
        // The write path skips an empty content row.
        exported_memory("m-empty", "   "),
    ];

    let report = target
        .service
        .import(&target_dir, bundle.clone(), ReplaceMode::Merge, true)
        .await
        .expect("dry run");
    assert_eq!(report.imported, 1, "the skipped row is not counted");
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].id, "m-empty");
    // Nothing was written.
    assert_eq!(target_facts.count(), 0);

    // The real run agrees with the dry run.
    let report = target
        .service
        .import(&target_dir, bundle, ReplaceMode::Merge, false)
        .await
        .expect("import");
    assert_eq!(report.imported, 1);
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(target_facts.count(), 1);
}

/// The prepare phase computes EVERY embedding BEFORE the first insert: an
/// embedder failure on the SECOND row leaves the target untouched (the
/// first row is not written either).
#[tokio::test]
async fn import_embedder_failure_writes_nothing() {
    let root = scratch_root("prepare-phase");
    let target_dir = project_dir(&root, "target-project");
    let target_store = root.join("target-store");
    std::fs::create_dir_all(&target_store).expect("target store");
    let target_facts = Arc::new(FakeFacts::new());
    let target = harness(&target_store, target_facts.clone());

    // The same service, but with an embedder that always fails.
    let failing = PortabilityService::new(
        target_facts.clone(),
        target.profiles.clone(),
        target.episodic.clone(),
        Arc::new(FailingEmbedder),
    )
    .with_container_tag_prefix(TAG_PREFIX)
    .with_embedding_info("test-model", DIMENSIONS);

    let mut bundle = Bundle::empty();
    bundle.schema_version = mem_portability::bundle::SchemaVersion::CURRENT;
    bundle.exported_at = "2026-10-22T00:00:00.000Z".to_string();
    bundle.source = SourceInfo {
        container_tag: "test_project_0123456789abcdef".to_string(),
        scope: mem_portability::bundle::BundleScope::Project,
        scope_hash: "0123456789abcdef".to_string(),
        project_path: None,
        project_name: None,
        git_repo_url: None,
    };
    bundle.memories = vec![
        exported_memory("m-1", "the first row"),
        exported_memory("m-2", FAIL_MARKER),
    ];

    let error = failing
        .import(&target_dir, bundle, ReplaceMode::Merge, false)
        .await
        .expect_err("the embedder failure aborts the import");
    assert!(
        matches!(error, PortabilityError::Embed(_)),
        "unexpected error: {error:?}"
    );
    // ZERO writes: the first row (whose embedding SUCCEEDED) was not
    // written either.
    assert_eq!(target_facts.count(), 0);
}

/// The milestone id pre-scan runs BEFORE the first write: a milestone id
/// that already exists aborts the whole import (the same contract the
/// fact tiers have) and the fact row is not written.
#[tokio::test]
async fn import_aborts_on_a_duplicate_milestone_id() {
    let root = scratch_root("milestone-collision");
    let target_dir = project_dir(&root, "target-project");
    let target_store = root.join("target-store");
    std::fs::create_dir_all(&target_store).expect("target store");
    let target_facts = Arc::new(FakeFacts::new());
    let target = harness(&target_store, target_facts.clone());

    // The target already holds the milestone.
    let existing = milestone("mile-1");
    target
        .episodic
        .write_batch(vec![mem_store::db::Statement::new(
            mem_store::episodic_sql::INSERT_MILESTONE,
            vec![
                mem_store::db::Value::Text(existing.id.clone()),
                mem_store::db::Value::Text(target_dir.to_string_lossy().to_string()),
                mem_store::db::Value::Text(existing.agent_id.clone()),
                mem_store::db::Value::Text(existing.summary.clone()),
                mem_store::db::Value::Text(
                    mem_episodic::milestone::encode_contributors(&existing.contributors)
                        .expect("contributors"),
                ),
                mem_store::db::Value::Integer(existing.source_count),
                mem_store::db::Value::Integer(existing.window_start),
                mem_store::db::Value::Integer(existing.window_end),
                mem_store::db::Value::Text("[0.25,0.25,0.25,0.25,0.25,0.25,0.25,0.25]".to_string()),
                mem_store::db::Value::Integer(existing.created_at),
            ],
        )])
        .await
        .expect("seed milestone");

    let mut bundle = Bundle::empty();
    bundle.schema_version = mem_portability::bundle::SchemaVersion::CURRENT;
    bundle.exported_at = "2026-10-22T00:00:00.000Z".to_string();
    bundle.source = SourceInfo {
        container_tag: "test_project_0123456789abcdef".to_string(),
        scope: mem_portability::bundle::BundleScope::Project,
        scope_hash: "0123456789abcdef".to_string(),
        project_path: None,
        project_name: None,
        git_repo_url: None,
    };
    bundle.memories = vec![exported_memory("m-1", "the row")];
    bundle.milestones = Some(vec![milestone("mile-1")]);

    let error = target
        .service
        .import(&target_dir, bundle, ReplaceMode::Merge, false)
        .await
        .expect_err("the milestone collision aborts the import");
    match &error {
        PortabilityError::ImportAbortedMilestones { count } => assert_eq!(*count, 1),
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(
        error
            .to_string()
            .contains("Import aborted: 1 milestone id(s) already exist"),
        "{error}"
    );
    // ZERO writes: the fact row was not inserted.
    assert_eq!(target_facts.count(), 0);
}

#[tokio::test]
async fn import_replace_mode_replaces_the_row() {
    let root = scratch_root("replace");
    let target_dir = project_dir(&root, "target-project");
    let target_store = root.join("target-store");
    std::fs::create_dir_all(&target_store).expect("target store");
    let target_facts = Arc::new(FakeFacts::new());
    let target = harness(&target_store, target_facts.clone());

    let scope = target.service.resolve_scope(&target_dir).expect("scope");
    let mut existing = exported_memory("m-1", "the old content");
    existing.content = "the old content".to_string();
    target_facts.seed(&scope.container_tag, existing);

    let mut bundle = Bundle::empty();
    bundle.schema_version = mem_portability::bundle::SchemaVersion::CURRENT;
    bundle.exported_at = "2026-10-22T00:00:00.000Z".to_string();
    bundle.source = SourceInfo {
        container_tag: "test_project_0123456789abcdef".to_string(),
        scope: mem_portability::bundle::BundleScope::Project,
        scope_hash: "0123456789abcdef".to_string(),
        project_path: None,
        project_name: None,
        git_repo_url: None,
    };
    let mut incoming = exported_memory("m-1", "the new content");
    incoming.content = "the new content".to_string();
    bundle.memories = vec![incoming];

    // The replace mode replaces instead of aborting.
    let report = target
        .service
        .import(&target_dir, bundle, ReplaceMode::Overwrite, false)
        .await
        .expect("import");
    assert_eq!(report.imported, 1);
    let rows = target_facts.with_tag(&scope.container_tag);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].memory.content, "the new content");
}

#[tokio::test]
async fn import_rejects_unsupported_schema_version() {
    let root = scratch_root("version");
    let target_dir = project_dir(&root, "target-project");
    let target_store = root.join("target-store");
    std::fs::create_dir_all(&target_store).expect("target store");
    let target = harness(&target_store, Arc::new(FakeFacts::new()));

    let mut bundle = Bundle::empty();
    bundle.schema_version = mem_portability::bundle::SchemaVersion(99);
    bundle.exported_at = "2026-10-22T00:00:00.000Z".to_string();
    bundle.source = SourceInfo {
        container_tag: "test_project_0123456789abcdef".to_string(),
        scope: mem_portability::bundle::BundleScope::Project,
        scope_hash: "0123456789abcdef".to_string(),
        project_path: None,
        project_name: None,
        git_repo_url: None,
    };

    let error = target
        .service
        .import(&target_dir, bundle, ReplaceMode::Merge, false)
        .await
        .expect_err("the version is refused");
    assert!(
        error
            .to_string()
            .contains("Unsupported export schemaVersion 99"),
        "{error}"
    );

    // The file-level reader refuses it too, with the same message.
    let path = root.join("bad.json");
    std::fs::write(
        &path,
        r#"{"schemaVersion":99,"exportedAt":"x","plugin":{"package":"p","version":"v"},"source":{"containerTag":"t","scope":"project","scopeHash":"0123456789abcdef"},"embedding":{"model":"m","dimensions":8},"memories":[]}"#,
    )
    .expect("write");
    let error = read_bundle(&path).expect_err("the file is refused");
    assert!(
        error
            .to_string()
            .contains("Unsupported export schemaVersion 99"),
        "{error}"
    );
}

#[tokio::test]
async fn write_bundle_is_private_and_atomic() {
    let root = scratch_root("writer");
    let path = root.join("nested").join("bundle.json");

    let mut bundle = Bundle::empty();
    bundle.schema_version = mem_portability::bundle::SchemaVersion::CURRENT;
    bundle.exported_at = "2026-10-22T00:00:00.000Z".to_string();
    bundle.source = SourceInfo {
        container_tag: "test_project_0123456789abcdef".to_string(),
        scope: mem_portability::bundle::BundleScope::Project,
        scope_hash: "0123456789abcdef".to_string(),
        project_path: None,
        project_name: None,
        git_repo_url: None,
    };
    bundle.memories = vec![exported_memory("m-1", "the row")];

    write_bundle(&path, &bundle).expect("write");
    assert!(path.exists());
    // The file is private (0600) on unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the bundle must be private");
    }
    // No temp file is left behind.
    let leftovers: Vec<PathBuf> = std::fs::read_dir(path.parent().expect("parent"))
        .expect("read dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|entry| {
            entry
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.contains(".tmp-"))
                .unwrap_or(false)
        })
        .collect();
    assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");

    // The round trip through the file reader is lossless.
    let read = read_bundle(&path).expect("read");
    assert_eq!(read.memories.len(), 1);
    assert_eq!(read.memories[0].id, "m-1");
    assert_eq!(read.source.scope_hash, "0123456789abcdef");
}

#[tokio::test]
async fn read_bundle_reports_a_missing_file() {
    let root = scratch_root("missing-file");
    let error = read_bundle(&root.join("nope.json")).expect_err("missing file");
    assert!(
        error.to_string().contains("Import file not found"),
        "{error}"
    );
}

#[tokio::test]
async fn import_validates_the_source_scope_hash() {
    let root = scratch_root("scope-hash");
    let target_dir = project_dir(&root, "target-project");
    let target_store = root.join("target-store");
    std::fs::create_dir_all(&target_store).expect("target store");
    let target = harness(&target_store, Arc::new(FakeFacts::new()));

    let mut bundle = Bundle::empty();
    bundle.schema_version = mem_portability::bundle::SchemaVersion::CURRENT;
    bundle.exported_at = "2026-10-22T00:00:00.000Z".to_string();
    bundle.source = SourceInfo {
        container_tag: "test_project_0123456789abcdef".to_string(),
        scope: mem_portability::bundle::BundleScope::Project,
        scope_hash: "NOT-A-HASH".to_string(),
        project_path: None,
        project_name: None,
        git_repo_url: None,
    };

    let error = target
        .service
        .import(&target_dir, bundle, ReplaceMode::Merge, false)
        .await
        .expect_err("the scope hash is refused");
    assert!(
        error.to_string().contains("Invalid export document"),
        "{error}"
    );
}
