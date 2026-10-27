//! Issue 44 — the `profile inject` counter: exactly one event per RENDERED
//! block, never on a disabled/unwired/refused path (ADR-0014 D-2).
//!
//! This lives in its own integration-test binary so the process-global log sink
//! only ever sees this file's injections: the sibling `inject_profile.rs` has
//! six `Rendered` assembles that would race a shared-sink delta.
//!
//! Fixtures are real, as in `inject_profile.rs`: a real `ProfileStore`, a real
//! `EpisodicStore`, `FakeEmbedder` and `TestDir`.

mod common;

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};

use common::TestDir;
use mem_bridge::inject::{InjectParams, InjectService, ProfileInjection};
use mem_bridge::scope_resolver::ProjectScopeResolver;
use mem_core::logging::LogSink;
use mem_episodic::store::EpisodicStore;
use mem_profile::profile_context::{ProfileInjectPolicy, ProfileRenderCaps};
use mem_profile::profile_store::{ProfileConfig, ProfileData, ProfileItem, ProfileStore};

/// The process-global sink, installed exactly once.
fn sink() -> LogSink {
    static ONCE: OnceLock<LogSink> = OnceLock::new();
    ONCE.get_or_init(|| {
        let dir = common::temp_dir("inject-profile-counter");
        let sink = LogSink::new(dir.join("opencode-mem.log"));
        mem_core::logging::init_process_logging(sink.clone())
            .expect("the per-binary sink installs");
        sink
    })
    .clone()
}

/// Rendered lines carrying the counter's message (`PROFILE_INJECT`).
fn counter_lines() -> usize {
    std::fs::read_to_string(sink().path())
        .map(|text| {
            text.lines()
                .filter(|line| line.contains(mem_core::logging::signatures::PROFILE_INJECT))
                .count()
        })
        .unwrap_or(0)
}

fn policy(enabled: bool) -> ProfileInjectPolicy {
    ProfileInjectPolicy {
        enabled,
        caps: ProfileRenderCaps {
            preferences: 5,
            patterns: 5,
            workflows: 3,
        },
    }
}

fn params(user_id: &str) -> InjectParams {
    InjectParams {
        project_id: Some("/srv/proj".to_string()),
        agent_id: Some("agent-1".to_string()),
        token_budget: Some(2_000),
        include: None,
        user_id: Some(user_id.to_string()),
    }
}

fn service(
    dir: &TestDir,
    profile: Option<(Arc<ProfileStore>, ProfileInjectPolicy)>,
) -> InjectService {
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    let shards = Arc::new(common::shard_manager(dir.store_config()));
    let resolver: Arc<dyn ProjectScopeResolver> = Arc::new(common::FixedScopeResolver::default());
    let service = InjectService::new(
        store,
        shards,
        resolver,
        Arc::new(AtomicBool::new(false)),
        Arc::new(common::FakeEmbedder::with_vector(common::unit_vector(0))),
        Arc::new(mem_store::platform::SystemProcessProbe),
    );
    match profile {
        Some((store, policy)) => service.with_profile(store, policy),
        None => service,
    }
}

/// One rendered block emits exactly one counter line; `Disabled` and `NotWired`
/// emit none, and the `Rendered` arm fires once per `assemble` (not once per
/// `profile_tier` call site).
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inject_profile_counter_emits_once_per_rendered_block() {
    let dir = TestDir::new("inject-profile-counter-run");
    let profile = Arc::new(ProfileStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
        ProfileConfig::default(),
    ));
    profile
        .create_profile(
            "probe@example.com",
            "Test User",
            "testuser",
            "probe@example.com",
            &ProfileData {
                preferences: vec![ProfileItem {
                    description: "pref counted".to_string(),
                    category: Some("tooling".to_string()),
                    confidence: Some(0.9),
                    frequency: Some(3),
                    ..Default::default()
                }],
                ..Default::default()
            },
            1,
        )
        .await
        .expect("the profile row is created");

    let disabled = service(&dir, Some((profile.clone(), policy(false))));
    let unwired = service(&dir, None);
    let rendered = service(&dir, Some((profile.clone(), policy(true))));

    let base = counter_lines();
    let disabled_result = disabled
        .assemble(&params("probe@example.com"))
        .await
        .expect("assemble stays Ok");
    assert_eq!(disabled_result.profile, ProfileInjection::Disabled);
    let after_disabled = counter_lines();
    assert_eq!(
        after_disabled - base,
        0,
        "a disabled tier must not emit the counter"
    );

    let unwired_result = unwired
        .assemble(&params("probe@example.com"))
        .await
        .expect("assemble stays Ok");
    assert_eq!(unwired_result.profile, ProfileInjection::NotWired);
    let after_unwired = counter_lines();
    assert_eq!(
        after_unwired - after_disabled,
        0,
        "an unwired tier must not emit the counter"
    );

    let rendered_result = rendered
        .assemble(&params("probe@example.com"))
        .await
        .expect("assemble stays Ok");
    assert_eq!(rendered_result.profile, ProfileInjection::Rendered);
    let after_rendered = counter_lines();
    assert_eq!(
        after_rendered - after_unwired,
        1,
        "exactly one counter line per rendered block"
    );
}
