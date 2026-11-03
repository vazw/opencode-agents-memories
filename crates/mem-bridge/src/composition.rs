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
use mem_profile::learning_lock::{LearningLock, LearningLockOptions};
use mem_profile::profile_context::ProfileInjectPolicy;
use mem_profile::profile_store::{ProfileConfig, ProfileStore};
use mem_profile::prompt::UserPromptManager;
use mem_store::connection::ConnectionManager;
use mem_store::platform::{Platform, SystemProcessProbe};
use mem_store::project_tags::{CoreProjectTagSource, ProjectTagSource};
use mem_store::shard_manager::ShardManager;

use crate::capture::{CaptureService, CaptureWriterDeps, DefaultTier1Writer};
use crate::compaction::CompactionPolicy;
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
    /// `ApiServices::production(...)`. Since issue 48 it carries the B2′
    /// eraser (`with_eraser`) — the seam that makes a prompt delete
    /// invalidate the profiles the prompt could have contributed to.
    pub prompts: Arc<UserPromptManager>,
    /// The user-profile store the mem-web surface reads (W6.7), constructed
    /// here for the same reason.
    pub profile: Arc<ProfileStore>,
    /// Issue 69: the skills manager the bridge's `skills` method reads. The
    /// manager opens `skills.db` lazily, so a composition that never answers
    /// `skills` creates no file.
    pub skills: Arc<mem_skills::SkillsManager>,
    pub embedder: Arc<dyn mem_embed::embedder::Embedder>,
    /// W6.6: the `facts.db` store the web milestone projection joins, or
    /// `None` when the promotion pipeline is off (the flag-off composition
    /// creates no `facts.db`). It is the SAME handle the cross-tier task
    /// writes through.
    pub facts: Option<Arc<mem_facts::store::FactsStore>>,
    pub episodic_config: mem_core::config::Episodic,
    /// The shared in-flight consolidation gate (set by the scheduler's RAII
    /// guard; read by the injector to skip only the tier-1 leg).
    pub consolidation_active: Arc<AtomicBool>,
    /// The per-project consolidation runner behind the scheduler.
    pub consolidation_runner: Arc<dyn crate::engine::ConsolidationRunner>,
    /// The shared project-scope resolver (capture, inject and the scheduler
    /// all derive the SAME tag/hash).
    pub scope_resolver: Arc<dyn crate::scope_resolver::ProjectScopeResolver>,
    /// Issue 70: the storage root the inventory/migration services address
    /// (`ShardManager` exposes `config()` but no root).
    pub storage_root: PathBuf,
    /// Issue 70: the derived store config, cloned before the shard manager
    /// consumes it (the inventory services construct their own stores from
    /// it).
    pub store_config: mem_store::config::StoreConfig,
    /// Issue 70: the connection manager `ShardInventory` opens shard files
    /// with.
    pub connections: Arc<ConnectionManager>,
    /// Issue 70: the project-tag source the inventory/migration services
    /// resolve paths with — the same prefix/overrides the resolver uses.
    pub tags: Arc<dyn ProjectTagSource>,
    /// Issue 70: the `profile` write's fail-closed lock (lazy: constructing
    /// it creates no file).
    pub learning_lock: Arc<LearningLock>,
    /// Whether an LLM-backed capture summarizer was wirable.
    pub summarizer_available: bool,
    /// The capture provider **post-filter** (D-52-4): the `FilteredStructured`
    /// handle the capture summarizer consumes, and the only provider handle
    /// capture can reach — it exposes no `AiProvider`, so no schema-free call
    /// site exists. Exposed for the same reason `prompts` and `profile` are: a
    /// guard that is constructed and never handed over protects nothing.
    ///
    /// Minted only by `build_composed_runtime`. Issue 33 must not touch this
    /// field or its initialiser. The consolidation summarizer deliberately
    /// keeps the UNWRAPPED `base` (D-52-7): a wrapped consolidation summarizer
    /// turns a legacy marker-bearing memory into a silent permanent skip.
    pub provider: Option<Arc<dyn crate::capture_provider::StructuredCapture>>,
}

/// Builds the runtime over `storage_root` from the resolved config.
pub async fn build_composed_runtime(
    storage_root: &std::path::Path,
    config: &Config,
    embedder: Arc<dyn mem_embed::embedder::Embedder>,
    provider: Option<crate::capture_provider::CaptureProvider>,
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
    //
    // Issue 48 (47.1-B2′): the profile store is built FIRST so the prompt
    // store can be handed the eraser. This is the ONE production
    // `with_eraser` site — forgetting it is exactly what
    // `test_composed_runtime_wires_the_profile_eraser_into_prompt_delete`
    // catches.
    let profile = Arc::new(ProfileStore::new(
        storage_root.to_path_buf(),
        store_config.clone(),
        ProfileConfig::default(),
    ));
    let eraser: Arc<dyn mem_profile::erasure::ProfileEraser> = profile.clone();
    let prompts = Arc::new(
        UserPromptManager::new(storage_root.to_path_buf(), store_config.clone(), 3)
            .with_eraser(eraser),
    );

    let probe: Arc<dyn mem_store::platform::ProcessProbe> = Arc::new(SystemProcessProbe);
    // W7.1 — the facts store is constructed ONLY when the promotion pipeline
    // flag is on. `FactsStore::new` touches no disk until `ready`, so a
    // flags-off composition creates no `facts.db`.
    let facts_store: Option<Arc<mem_facts::store::FactsStore>> =
        if config.feature_flags.promotion_pipeline {
            Some(Arc::new(mem_facts::store::FactsStore::new(
                store_config.clone(),
            )))
        } else {
            None
        };
    // W6.6: the same store the cross-tier task writes through is cloned here
    // BEFORE the move, so the web milestone projection reads the facts its
    // own composition produced (one file, two readers).
    let facts = facts_store.clone();
    let cross_tier = crate::cross_tier::cross_tier_task(
        config.feature_flags.promotion_pipeline,
        facts_store,
        config.promotion.ttl_fallback_days,
    )
    .map_err(|error| format!("cross-tier task wiring failed: {error}"))?;
    let connections = Arc::new(ConnectionManager::new(
        storage_root.to_path_buf(),
        store_config.clone(),
        Platform::current(),
        Arc::new(mem_store::db::turso_connect_factory()),
    ));
    // W8a: vault store + feeder. VaultStore opens lazily via ready; construction
    // touches no disk.
    let vault_store = Arc::new(mem_vault::VaultStore::new(store_config.clone()));
    let vault_feeder = Arc::new(crate::vault_feeder::VaultFeeder::new(vault_store));
    // Issue 69: the skills manager the bridge's `skills` method reads. It opens
    // `skills.db` lazily on the first request; the web surface keeps its own
    // manager (separate connection, same file) — see R2.
    let skills = Arc::new(mem_skills::SkillsManager::new(
        storage_root.to_path_buf(),
        store_config.clone(),
    ));
    let shards = Arc::new(ShardManager::new(
        storage_root.to_path_buf(),
        // Issue 70: the manager consumes a clone so `ComposedRuntime` keeps
        // the derived config for the inventory services.
        store_config.clone(),
        connections.clone(),
        probe.clone(),
        // The ONE production wiring (AC9): the identity is derived from the
        // resolved config, the same authority the store config came from.
        Some(mem_core::embedding::EmbeddingIdentity::from_config(config)),
    ));
    // Issue 70: the inventory/migration tag source and the profile write's
    // learning lock. Both read the SAME prefix/overrides the resolver uses,
    // so `list-shards`/`migrate` derive byte-identical tags.
    let tags: Arc<dyn ProjectTagSource> = Arc::new(CoreProjectTagSource {
        container_tag_prefix: config.container_tag_prefix.clone(),
        user_email_override: config.user_email_override.clone(),
        user_name_override: config.user_name_override.clone(),
    });
    let learning_lock = Arc::new(LearningLock::new(LearningLockOptions::new(
        storage_root.to_path_buf(),
        store_config.clone(),
    )));

    // The capture summarizer: provider-backed ONLY when a provider reaches the
    // composition root. With `provider: None` this root composes NO host
    // adapter: resolving the live host needs the env/probe/clock/transport
    // seams and the resolved `opencodeBaseUrl`, which the engine owns (it
    // passes the `LazyHostProvider` in). Fabricating one here would dial a
    // guessed port — the exact failure this wave removes — so `None` is
    // preserved and the capture path fails closed with the named
    // `capture_provider_unavailable` reason.
    //
    // Consequence, recorded: with `provider: None` consolidation falls back to
    // `UnavailableMilestoneSummarizer` (below). That is the tests / degraded
    // composition path; production always passes the lazy provider.
    // D-52-4/D-52-7: the capture summarizer's handle, POST-filter. The guard
    // is minted here and nowhere else, for the same reason `wrap` was: a layer
    // constructed and never handed over emits byte-identical traffic to no
    // layer at all. The consolidation handle below keeps the UNWRAPPED `base`
    // on purpose (a wrapped consolidation summarizer turns a legacy
    // marker-bearing memory into a silent permanent skip).
    let capture_structured = provider.as_ref().map(|capture| {
        Arc::new(crate::capture_provider::FilteredStructured::new(
            capture.structured.clone(),
            mem_ai::privacy::PrivacyConfig::from_core(config),
        )) as Arc<dyn crate::capture_provider::StructuredCapture>
    });
    let summarizer_probe = capture_structured.clone().map(|handle| {
        let model = config
            .opencode_model
            .clone()
            .unwrap_or_else(|| "inherit".to_string());
        Arc::new(crate::capture_prompt::ProviderCaptureSummarizer::new(
            handle,
            model,
            config.auto_capture_max_context_bytes.max(1024) as usize,
        )) as Arc<dyn crate::capture_prompt::CaptureSummarizer>
    });
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
    )
    .with_sound_memory(crate::capture::SoundMemorySettings {
        enabled: config.feature_flags.sound_memory,
        raw_context_required: config.sound_memory.raw_context_required,
    })
    .with_vault_feeder(Some(vault_feeder))
    .with_privacy(mem_ai::privacy::PrivacyConfig::from_core(config))
    // Issue 98 (C1/C2): the resolved `chatMessage.enabled`, chained beside the
    // other config-derived setters. This ONE call is what keeps the handshake
    // advertisement and the `capture` refusal on the same value.
    .with_chat_capture_enabled(config.chat_message.enabled);

    let consolidation_active = Arc::new(AtomicBool::new(false));
    let inject = InjectService::new(
        store.clone(),
        shards.clone(),
        resolver.clone(),
        consolidation_active.clone(),
        embedder.clone(),
        probe.clone(),
    )
    // The ONE production `.with_profile` site (issue 44, D1): forgetting it is
    // exactly what `test_composed_runtime_wires_the_profile_tier_into_inject`
    // catches, and the tier reports `NotWired` (logged) until it is here.
    .with_profile(profile.clone(), ProfileInjectPolicy::from_core(config));
    let inject = inject
        .with_compaction(CompactionPolicy::from_core(&config.compaction))
        .with_chat_policy(crate::inject::ChatInjectionPolicy::from_core(config));

    // Q8: the consolidator behind the scheduler. Same provider, same
    // embedder, same probe, the tree's `DEFAULT_BATCH_LIMIT` (rows per agent
    // batch — NOT the scheduler's 64-project scan bound), and a real
    // promotion sink over the project shards.
    let consolidation_summarizer: Arc<dyn mem_episodic::summarizer::MilestoneSummarizer> =
        match provider.as_ref() {
            Some(capture) => Arc::new(mem_episodic::summarizer::ProviderSummarizer::new(
                capture.base.clone(),
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
        cross_tier,
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
        skills,
        facts,
        embedder,
        episodic_config: config.episodic,
        consolidation_active,
        consolidation_runner,
        scope_resolver: resolver,
        storage_root: storage_root.to_path_buf(),
        store_config,
        connections,
        tags,
        learning_lock,
        summarizer_available: summarizer_probe.is_some(),
        provider: capture_structured,
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
/// `TagResolver::tags` derivation as Step 2).
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

/// The local ONNX cache root: `<storagePath>/.cache`, the same tree the
/// upstream plugin uses (`example/opencode-mem/src/services/embedding.ts:232`
/// sets `mod.env.cacheDir = join(CONFIG.storagePath, ".cache")`).
///
/// Deliberately not `std::env::temp_dir()`: on the runbook host `/tmp` is a
/// 16 GB tmpfs, so the weights would be RAM (523 MB that cannot be
/// reclaimed), and every reboot would wipe the model — every `search`/`add`
/// would answer `-32603` until the cache was re-provisioned. Under the store
/// the weights are disk-backed (reclaimable page cache) and survive a reboot.
pub fn local_cache_root(config: &Config) -> PathBuf {
    config.storage_path.join(".cache")
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
            local_load_options_from_config(config),
        )),
    }
}

/// The local loader options the engine composes from a resolved config.
///
/// Extracted from [`embedder_from_config`] so the config-to-loader hop (which
/// carries the token-cap override) is observable on its own.
pub fn local_load_options_from_config(config: &Config) -> mem_embed::local::LocalLoadOptions {
    mem_embed::local::LocalLoadOptions {
        model: config.embedding_model.clone(),
        cache_root: local_cache_root(config),
        dtype: config.embedding_dtype,
        pooling: config.embedding_pooling,
        intra_threads: None,
        hub: None,
        max_tokens: config.embedding_max_tokens,
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

    /// 48.6/48.11 — the seam exists AND production passes the eraser: a prompt
    /// delete through the composed runtime erases the profile it contributed
    /// to. This is the test that catches "the seam exists but the composition
    /// root forgot to call `with_eraser`".
    #[mem_store::tokio::test(crate = "mem_store::tokio")]
    async fn test_composed_runtime_wires_the_profile_eraser_into_prompt_delete() {
        use mem_profile::profile_store::{ProfileData, ProfileItem};

        let root = temp_dir("composition-erasure");
        let home = temp_dir("composition-erasure-home");
        let env: BTreeMap<String, String> = [("HOME", home.to_string_lossy().to_string())]
            .map(|(key, value)| (key.to_string(), value))
            .into_iter()
            .collect();
        let mut config = Config::load(&mem_core::config::ConfigPaths::standard(&home), &env)
            .expect("the default config loads");
        config.storage_path = root.clone();
        config.embedding_dimensions = 8;

        let runtime = build_composed_runtime(&root, &config, Arc::new(NoopEmbedder), None)
            .await
            .expect("the runtime composes");

        // Prompt first, profile second: the profile's watermark (= now) is >=
        // the prompt's created_at, i.e. the prompt is a contributing one.
        let prompt_id = runtime
            .prompts
            .save_prompt("session-48", "message-48", "/probe", "probe prompt")
            .await
            .expect("save the prompt");
        let profile_id = runtime
            .profile
            .create_profile(
                "probe@example.com",
                "Probe",
                "probe",
                "probe@example.com",
                &ProfileData {
                    preferences: vec![ProfileItem {
                        description: "probe preference".to_string(),
                        ..ProfileItem::default()
                    }],
                    ..ProfileData::default()
                },
                1,
            )
            .await
            .expect("create the profile");

        runtime
            .prompts
            .delete_prompt(&prompt_id)
            .await
            .expect("delete the prompt through the wired seam");

        assert!(
            runtime
                .profile
                .get_active_profile("probe@example.com")
                .await
                .expect("read the profile")
                .is_none(),
            "the composed runtime's prompt delete erases the contributing profile"
        );
        assert!(
            runtime
                .profile
                .get_profile_changelogs(&profile_id, 10)
                .await
                .expect("read the changelogs")
                .is_empty(),
            "the changelog snapshots are erased with the profile"
        );

        cleanup(&root);
        cleanup(&home);
    }

    /// The local ONNX cache lives under the store, not under `$TMPDIR`: the
    /// weights are disk-backed and survive a reboot, and 523 MB of tmpfs RAM
    /// is not paid for the model.
    #[test]
    fn test_local_cache_root_is_under_the_storage_path() {
        let root = temp_dir("cache-root");
        let home = temp_dir("cache-home");
        let env: BTreeMap<String, String> = [("HOME", home.to_string_lossy().to_string())]
            .map(|(key, value)| (key.to_string(), value))
            .into_iter()
            .collect();
        let mut config = Config::load(&mem_core::config::ConfigPaths::standard(&home), &env)
            .expect("the default config loads");
        config.storage_path = root.clone();

        assert_eq!(local_cache_root(&config), root.join(".cache"));
        assert_ne!(
            local_cache_root(&config),
            std::env::temp_dir().join(".opencode-mem-onnx")
        );

        cleanup(&root);
        cleanup(&home);
    }

    /// A store with no provisioned model must fail with an error that names
    /// the cache path under the storage root AND the provision command — the
    /// old `-32603` with no remedy was the failure mode this pins.
    #[mem_store::tokio::test(crate = "mem_store::tokio")]
    async fn test_missing_model_names_the_store_cache_and_the_provision_command() {
        let root = temp_dir("cache-missing");
        let home = temp_dir("cache-missing-home");
        let env: BTreeMap<String, String> = [("HOME", home.to_string_lossy().to_string())]
            .map(|(key, value)| (key.to_string(), value))
            .into_iter()
            .collect();
        let mut config = Config::load(&mem_core::config::ConfigPaths::standard(&home), &env)
            .expect("the default config loads");
        config.storage_path = root.clone();
        config.embedding_model = "Xenova/selftest-model-not-provisioned".to_string();
        config.embedding_dtype = None;

        let embedder = embedder_from_config(&config);
        let error = embedder
            .embed("x")
            .await
            .expect_err("an unprovisioned cache must not embed");
        let message = error.to_string();
        assert!(
            message.contains(
                &root
                    .join(".cache")
                    .join("Xenova/selftest-model-not-provisioned")
                    .to_string_lossy()
                    .to_string()
            ),
            "the error must name the cache path under the storage root, got: {message}"
        );
        assert!(
            message.contains("provision-onnx-cache.sh"),
            "the error must name the provision command, got: {message}"
        );

        cleanup(&root);
        cleanup(&home);
    }
}
