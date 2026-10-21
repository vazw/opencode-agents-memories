//! W5.3 Step 8 — the engine's runtime composition root: everything the
//! dispatch path and the Q8 scheduler need, built ONCE from the resolved
//! `mem_core::Config` and the storage root.
//!
//! Composition contract (blueprint [DEPENDENCIES]):
//! - `src/main.rs` loads the resolved `mem_core::Config` synchronously and
//!   passes it into `run_in_runtime`; the runtime and the scheduler are
//!   NEVER in `src/main.rs`.
//! - Every tier is addressed through the SAME resolved scope family.
//! - Provider wiring: the capture summarizer and the consolidation
//!   summarizer compose the OpenCode host provider (default base_url
//!   `127.0.0.1:4096`, model `inherit`) — a host that is not answering
//!   makes capture-summarization fail closed (retry/dead-letter), never
//!   hang the request path.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mem_core::config::Config;
use mem_episodic::store::EpisodicStore;
use mem_profile::profile_store::{ProfileConfig, ProfileStore};
use mem_profile::prompt::UserPromptManager;
use mem_store::connection::ConnectionManager;
use mem_store::platform::{Platform, SystemProcessProbe};
use mem_store::shard_manager::ShardManager;

use crate::capture::{CaptureService, CaptureWriterDeps, DefaultTier1Writer};
use crate::inject::InjectService;

/// The composed runtime (shared with [`EngineRuntime`]).
pub struct ComposedRuntime {
    pub capture: Arc<CaptureService>,
    pub inject: Arc<InjectService>,
    pub store: Arc<EpisodicStore>,
    pub shards: Arc<ShardManager>,
    /// The user-prompt store the mem-web surface reads (W6.7). Nothing in
    /// mem-bridge or the root opened it before this wave; it is constructed
    /// exactly once here so the root composition root can hand it to
    /// `ApiServices::production(...)`.
    pub prompts: Arc<UserPromptManager>,
    /// The user-profile store the mem-web surface reads (W6.7), constructed
    /// here for the same reason.
    pub profile: Arc<ProfileStore>,
    pub embedder: Arc<dyn mem_embed::embedder::Embedder>,
    pub episodic_config: mem_core::config::Episodic,
    /// The shared in-flight consolidation gate (set by the scheduler's RAII
    /// guard; read by the injector to skip only the tier-1 leg).
    pub consolidation_active: Arc<AtomicBool>,
    /// The per-project consolidation runner behind the scheduler.
    pub consolidation_runner: Arc<dyn crate::engine::ConsolidationRunner>,
    /// The shared project-scope resolver (capture, inject and the scheduler
    /// all derive the SAME tag/hash).
    pub scope_resolver: Arc<dyn crate::scope_resolver::ProjectScopeResolver>,
    /// Whether an LLM-backed capture summarizer was wirable.
    pub summarizer_available: bool,
}

/// Builds the runtime over `storage_root` from the resolved config.
pub async fn build_composed_runtime(
    storage_root: &std::path::Path,
    config: &Config,
    embedder: Arc<dyn mem_embed::embedder::Embedder>,
    provider: Option<Arc<dyn mem_ai::provider::AiProvider>>,
) -> Result<ComposedRuntime, String> {
    // SECURITY (mandate 5): the store config is DERIVED from the resolved
    // `mem_core::Config` — `from_core` copies the configured encryption
    // switch, cipher and key material. A hand-built literal here would
    // silently default encryption to off and write every production row
    // (episodic staging, the inbox transcript payload, tier-1 shards) in
    // plaintext. The ONLY field this root owns is the storage path.
    let mut store_config = mem_store::config::from_core(config);
    store_config.storage_path = storage_root.to_path_buf();

    let store = Arc::new(EpisodicStore::new(
        storage_root.to_path_buf(),
        store_config.clone(),
    ));
    store
        .ready()
        .await
        .map_err(|error| format!("episodic store init failed: {error}"))?;

    // W6.7: the two profile-tier stores the mem-web surface needs. Both take
    // their own database files (`user-prompts.db` / `user-profiles.db`) under
    // the SAME storage root and the SAME derived store config, and both open
    // lazily — so constructing them here costs handles, not I/O. They are
    // built BEFORE `store_config` is moved into the shard manager.
    let prompts = Arc::new(UserPromptManager::new(
        storage_root.to_path_buf(),
        store_config.clone(),
        3,
    ));
    let profile = Arc::new(ProfileStore::new(
        storage_root.to_path_buf(),
        store_config.clone(),
        ProfileConfig::default(),
    ));

    let probe: Arc<dyn mem_store::platform::ProcessProbe> = Arc::new(SystemProcessProbe);
    let connections = Arc::new(ConnectionManager::new(
        storage_root.to_path_buf(),
        store_config.clone(),
        Platform::current(),
        Arc::new(mem_store::db::turso_connect_factory()),
    ));
    let shards = Arc::new(ShardManager::new(
        storage_root.to_path_buf(),
        store_config,
        connections,
        probe.clone(),
    ));

    // The capture summarizer: provider-backed when a provider reaches the
    // composition root (the OpenCode host transport is composed there). With
    // no injected provider, the root builds the OpenCode host adapter over
    // the documented default base URL (`127.0.0.1:4096`, model `inherit`);
    // a host that is not answering fails closed (retry/dead-letter) — it
    // never hangs the request path.
    let provider = match provider {
        Some(provider) => Some(provider),
        None => {
            let host = mem_ai::provider::opencode_host::OpenCodeHostConfig {
                provider_id: config
                    .opencode_provider
                    .clone()
                    .unwrap_or_else(|| "opencode".to_string()),
                model: config
                    .opencode_model
                    .clone()
                    .unwrap_or_else(|| "inherit".to_string()),
                variant: config.opencode_variant.clone(),
                ..Default::default()
            };
            Some(
                Arc::new(mem_ai::provider::opencode_host::OpenCodeHostProvider::new(
                    host,
                    Arc::new(mem_ai::provider::opencode_host::ReqwestHostTransport::new()),
                )) as Arc<dyn mem_ai::provider::AiProvider>,
            )
        }
    };
    let summarizer_probe = provider
        .clone()
        .map(|provider| {
            let model = config
                .opencode_model
                .clone()
                .unwrap_or_else(|| "inherit".to_string());
            Arc::new(crate::capture_prompt::ProviderCaptureSummarizer::new(
                provider,
                model,
                config.auto_capture_max_context_bytes.max(1024) as usize,
            )) as Arc<dyn crate::capture_prompt::CaptureSummarizer>
        })
        .ok_or_else(|| "no provider".to_string())
        .ok();
    let summarizer = summarizer_probe.clone();

    let resolver: Arc<dyn crate::scope_resolver::ProjectScopeResolver> = Arc::new(EnvResolver {
        container_tag_prefix: config.container_tag_prefix.clone(),
        user_email_override: config.user_email_override.clone(),
        user_name_override: config.user_name_override.clone(),
    });

    let capture = CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store.clone()),
        Arc::new(DefaultTier1Writer::new(CaptureWriterDeps {
            shards: shards.clone(),
            embedder: embedder.clone(),
        })),
        embedder.clone(),
        summarizer,
        resolver.clone(),
        config.auto_capture_max_context_bytes.max(1) as usize,
    );

    let consolidation_active = Arc::new(AtomicBool::new(false));
    let inject = InjectService::new(
        store.clone(),
        shards.clone(),
        resolver.clone(),
        consolidation_active.clone(),
        embedder.clone(),
        probe.clone(),
    );

    // Q8: the consolidator behind the scheduler. Same provider, same
    // embedder, same probe, the tree's `DEFAULT_BATCH_LIMIT` (rows per agent
    // batch — NOT the scheduler's 64-project scan bound), and a real
    // promotion sink over the project shards.
    let consolidation_summarizer: Arc<dyn mem_episodic::summarizer::MilestoneSummarizer> =
        match provider.as_ref() {
            Some(provider) => Arc::new(mem_episodic::summarizer::ProviderSummarizer::new(
                provider.clone(),
                config
                    .opencode_model
                    .clone()
                    .unwrap_or_else(|| "inherit".to_string()),
            )),
            None => Arc::new(UnavailableMilestoneSummarizer),
        };
    let policy = mem_episodic::consolidation::ConsolidationPolicy::from_episodic(&config.episodic)
        .map_err(|error| format!("consolidation policy invalid: {error}"))?;
    let deps = mem_episodic::consolidation::ConsolidationDeps {
        summarizer: consolidation_summarizer,
        embedder: embedder.clone(),
        sink: Arc::new(mem_episodic::promotion::ShardPermanentMemorySink::new(
            shards.clone(),
            config.embedding_dimensions,
        )),
        batch_writer: Arc::new(mem_episodic::consolidation::StoreBatchWriter::new(
            store.clone(),
        )),
        prefix_options: mem_embed::prefix::PrefixOptions::default(),
        batch_limit: mem_episodic::consolidation::DEFAULT_BATCH_LIMIT,
        probe: probe.clone(),
    };
    let consolidator = mem_episodic::consolidation::Consolidator::new(store.clone(), policy, deps);
    let consolidation_runner: Arc<dyn crate::engine::ConsolidationRunner> =
        Arc::new(crate::engine::DefaultConsolidationRunner::new(consolidator));

    Ok(ComposedRuntime {
        capture: Arc::new(capture),
        inject: Arc::new(inject),
        store,
        shards,
        prompts,
        profile,
        embedder,
        episodic_config: config.episodic,
        consolidation_active,
        consolidation_runner,
        scope_resolver: resolver,
        summarizer_available: summarizer_probe.is_some(),
    })
}

/// The no-provider milestone summarizer: every run fails closed (logged by
/// the scheduler, never fatal to capture/inject).
struct UnavailableMilestoneSummarizer;

impl mem_episodic::summarizer::MilestoneSummarizer for UnavailableMilestoneSummarizer {
    fn summarize<'a>(
        &'a self,
        _transcript: &'a str,
    ) -> mem_embed::embedder::BoxFuture<'a, Result<String, mem_episodic::error::EpisodicError>>
    {
        Box::pin(async {
            Err(mem_episodic::error::EpisodicError::Summary {
                message: "no milestone summarizer provider composed".to_string(),
            })
        })
    }
}

/// The tag-resolution resolver over the resolved config (the same
/// `TagResolver::project_tag_info` derivation as Step 2).
pub struct EnvResolver {
    pub container_tag_prefix: String,
    pub user_email_override: Option<String>,
    pub user_name_override: Option<String>,
}

impl crate::scope_resolver::ProjectScopeResolver for EnvResolver {
    fn resolve(
        &self,
        project_id: &str,
    ) -> Result<crate::project_scope::ResolvedProjectScope, crate::project_scope::ProjectScopeError>
    {
        let env = mem_core::env::ProcessEnv;
        let git = mem_core::tags::probe::SystemGitProbe::new(&env);
        let resolver = mem_core::tags::TagResolver::new(
            self.container_tag_prefix.as_str(),
            self.user_email_override.as_deref(),
            self.user_name_override.as_deref(),
            &env,
            &git,
        );
        crate::project_scope::resolve_project_scope(project_id, &resolver)
    }
}

/// The default embedding loader: Local ONNX unless a remote url+key pair
/// is set (matching `EmbeddingService`'s branch, but handing the raw
/// `Arc<dyn Embedder>` every W5.3 seam expects).
pub fn embedder_from_config(config: &Config) -> Arc<dyn mem_embed::embedder::Embedder> {
    match (
        config.embedding_api_url.clone(),
        config.embedding_api_key.clone(),
    ) {
        (Some(url), Some(key)) => Arc::new(mem_embed::remote::RemoteEmbedder::new(
            mem_embed::remote::RemoteEmbedOptions {
                url,
                api_key: key,
                model: config.embedding_model.clone(),
                retry: Default::default(),
            },
            Arc::new(mem_embed::remote::ReqwestTransport::new()),
            mem_embed::remote::retry::system_sleep(),
        )),
        _ => Arc::new(mem_embed::local::LocalOnnxEmbedder::new(
            mem_embed::local::LocalLoadOptions {
                model: config.embedding_model.clone(),
                cache_root: std::env::temp_dir().join(".opencode-mem-onnx"),
                dtype: config.embedding_dtype,
                pooling: config.embedding_pooling,
                intra_threads: None,
                hub: None,
            },
        )),
    }
}

/// The capture summarizer's provider request is bounded; the timeout is
/// upstream's own; the default re-export keeps the constant visible.
pub const PROVIDER_TIMEOUT: Duration = Duration::from_secs(90);

/// The engine runtime holder used by `run_in_runtime`.
pub use crate::capture::CaptureParams as _CaptureParamsAlias;

/// Suppresses dead-code warnings the tests use across fixture paths.
#[allow(dead_code)]
fn _visited_root_marker(_root: PathBuf) {}

// ---------------------------------------------------------------------------
// r1 rework — the composition root must honour the RESOLVED encryption config
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::Path;

    /// A no-I/O embedder (the composition root only stores it).
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
        ) -> mem_embed::embedder::BoxFuture<'_, Result<Vec<f32>, mem_embed::error::EmbedError>>
        {
            Box::pin(async { Ok(vec![0.0; 8]) })
        }
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.subsec_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "mem-bridge-w53-{prefix}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    /// SECURITY mandate 5: with encryption enabled in the resolved config, the
    /// composed runtime must open its databases with the configured key — a
    /// KEYLESS open of `episodic.db` must fail.
    #[mem_store::tokio::test(crate = "mem_store::tokio")]
    async fn test_composition_root_honours_configured_encryption_at_rest() {
        let root = temp_dir("composition-enc");
        let home = temp_dir("composition-home");
        let env: BTreeMap<String, String> = [("HOME", home.to_string_lossy().to_string())]
            .map(|(key, value)| (key.to_string(), value))
            .into_iter()
            .collect();
        let mut config = Config::load(&mem_core::config::ConfigPaths::standard(&home), &env)
            .expect("the default config loads");
        config.storage_path = root.clone();
        config.embedding_dimensions = 8;
        config.database_encryption_enabled = true;
        config.database_encryption_key = Some("a".repeat(64));

        let runtime = build_composed_runtime(&root, &config, Arc::new(NoopEmbedder), None)
            .await
            .expect("the runtime composes with encryption enabled");
        drop(runtime);

        // A KEYLESS open (ConnectOptions::for_config never adds key material)
        // of the same file must be refused — proof the bytes really are
        // encrypted at rest.
        let store_config = mem_store::config::from_core(&config);
        use mem_store::db::ConnectFactory as _;
        let keyless = mem_store::db::turso_connect_factory()
            .connect(
                root.join("episodic.db"),
                mem_store::db::ConnectOptions::for_config(
                    &store_config,
                    mem_store::platform::Platform::current(),
                ),
            )
            .await;
        assert!(
            keyless.is_err(),
            "episodic.db must be encrypted at rest when the config enables it"
        );

        // And the context that opened it must be the resolved config, not a
        // hand-built literal that silently drops the security fields.
        assert!(store_config.database_encryption_enabled);
        assert_eq!(store_config.database_encryption_cipher, "aes256gcm");
        assert_eq!(
            store_config.database_encryption_key.as_deref(),
            Some("a".repeat(64).as_str())
        );

        cleanup(&root);
        cleanup(&home);
    }
}
