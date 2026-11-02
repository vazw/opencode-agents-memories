//! Issue 90 / slice 4 #95 — the profile confidence decay lifecycle.
//!
//! `ProfileStore::sweep_decay_daily` had no production caller, so stored
//! confidence never decayed. `spawn_profile_decay_lifecycle` is its caller: one
//! tracked task, a startup tick behind the handshake, then a wake-up loop. The
//! 24 h gate is the STORE's own (`decay::SWEEP_INTERVAL_MILLIS`); the lifecycle
//! cadence is only how often the gate is asked.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::TestDir;
use mem_bridge::engine::{
    DecaySweeper, Engine, EngineConfig, PROFILE_DECAY_TICK_SECS, spawn_profile_decay_lifecycle,
};
use mem_embed::embedder::BoxFuture;
use mem_episodic::store::Clock;
use mem_profile::error::ProfileError;
use mem_profile::profile_store::{
    DecaySweepReport, ProfileConfig, ProfileData, ProfileItem, ProfileStore,
};

const DAY_MS: i64 = 86_400_000;
/// A fixed "now" far from the wall clock, so a lifecycle that read the real
/// clock instead of the injected one cannot produce the asserted numbers.
const NOW: i64 = 1_900_000_000_000;

/// A sweeper that counts calls and records the `now` it was handed.
#[derive(Default)]
struct CountingSweeper {
    calls: AtomicUsize,
    last_now: AtomicI64,
}

impl DecaySweeper for CountingSweeper {
    fn sweep<'a>(&'a self, now: i64) -> BoxFuture<'a, Result<DecaySweepReport, ProfileError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.last_now.store(now, Ordering::SeqCst);
        Box::pin(async {
            Ok(DecaySweepReport {
                profiles_scanned: 0,
                profiles_updated: 0,
            })
        })
    }
}

fn fixed_clock(millis: i64) -> Clock {
    Arc::new(move || millis)
}

/// Polls `condition` for at most 2 s at 5 ms steps (a scheduling wait).
async fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn profile_store(dir: &TestDir) -> Arc<ProfileStore> {
    Arc::new(ProfileStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
        ProfileConfig::default(),
    ))
}

/// One preference: confidence 0.8, frequency 1, three pieces of evidence, a
/// 5:1 Beta posterior, last seen 90 days before `now`.
fn aged_profile(now: i64) -> ProfileData {
    ProfileData {
        preferences: vec![ProfileItem {
            description: "decay me".to_string(),
            category: Some("style".to_string()),
            confidence: Some(0.8),
            frequency: Some(1),
            evidence: Some(vec!["e0".into(), "e1".into(), "e2".into()]),
            last_seen: Some(now - 90 * DAY_MS),
            alpha: Some(5.0),
            beta: Some(1.0),
            ..ProfileItem::default()
        }],
        patterns: Vec::new(),
        workflows: Vec::new(),
        learning_paths: None,
    }
}

async fn seed(store: &ProfileStore, now: i64) -> String {
    store
        .create_profile(
            "decay@example.com",
            "Unknown",
            "unknown",
            "decay@example.com",
            &aged_profile(now),
            7,
        )
        .await
        .expect("create profile")
}

async fn stored(store: &ProfileStore, id: &str) -> (i64, f64) {
    let row = store
        .get_profile_by_id(id)
        .await
        .expect("read profile")
        .expect("profile exists");
    let data: ProfileData = mem_store::serde_json::from_str(&row.profile_data).expect("parse");
    let confidence = data.preferences[0].confidence.expect("confidence stored");
    (row.version, confidence)
}

// ---------------------------------------------------------------------------
// Step 1 — the startup tick
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_startup_tick_is_scheduled_not_awaited() {
    let engine = Engine::new(EngineConfig::default());
    let sweeper = Arc::new(CountingSweeper::default());

    spawn_profile_decay_lifecycle(
        &engine,
        sweeper.clone() as Arc<dyn DecaySweeper>,
        Duration::from_secs(PROFILE_DECAY_TICK_SECS),
        fixed_clock(NOW),
    );

    // Before any `await`: a tick executed inline (the W5.3 defect-8 shape)
    // would already have counted.
    assert_eq!(sweeper.calls.load(Ordering::SeqCst), 0, "not awaited");
    assert_eq!(
        engine.live_tasks().load(Ordering::SeqCst),
        1,
        "the tick lives inside one tracked task"
    );
    wait_until("the startup tick", || {
        sweeper.calls.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(sweeper.last_now.load(Ordering::SeqCst), NOW);
}

#[tokio::test]
async fn test_startup_tick_persists_the_decayed_confidence() {
    let dir = TestDir::new("decay-startup");
    let store = profile_store(&dir);
    let id = seed(&store, NOW).await;
    let engine = Engine::new(EngineConfig::default());

    spawn_profile_decay_lifecycle(
        &engine,
        store.clone() as Arc<dyn DecaySweeper>,
        Duration::from_secs(PROFILE_DECAY_TICK_SECS),
        fixed_clock(NOW),
    );

    let started = Instant::now();
    loop {
        let (version, confidence) = stored(&store, &id).await;
        if version == 2 {
            // 0.8 -> 0.26675 (the sweep derives twice; 90 d age, 0.5 factor).
            assert!(
                (confidence - 0.26675).abs() < 1e-6,
                "decayed with the injected now, got {confidence}"
            );
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the startup tick never persisted (version {version}, confidence {confidence})"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

// ---------------------------------------------------------------------------
// Step 2 — the wake-up loop
// ---------------------------------------------------------------------------

/// A clock the test can move.
fn movable_clock(millis: &Arc<AtomicI64>) -> Clock {
    let millis = Arc::clone(millis);
    Arc::new(move || millis.load(Ordering::SeqCst))
}

#[tokio::test]
async fn test_periodic_tick_waits_for_the_gate_then_fires_on_the_injected_clock() {
    // The cadence constant is reviewed: a silent change is a red line.
    assert_eq!(PROFILE_DECAY_TICK_SECS, 3600);

    let dir = TestDir::new("decay-periodic");
    let store = profile_store(&dir);
    let id = seed(&store, NOW).await;
    let now = Arc::new(AtomicI64::new(NOW));
    let engine = Engine::new(EngineConfig::default());

    spawn_profile_decay_lifecycle(
        &engine,
        store.clone() as Arc<dyn DecaySweeper>,
        Duration::from_millis(50),
        movable_clock(&now),
    );

    // The startup tick sweeps once (version 1 -> 2).
    let started = Instant::now();
    while stored(&store, &id).await.0 != 2 {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "startup sweep never landed"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let after_first = stored(&store, &id).await;

    // Phase 1: the clock stays at NOW for >= 3 wake-ups (200 ms): every one is
    // a not-due tick, so nothing is written.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        stored(&store, &id).await,
        after_first,
        "a not-due wake-up writes nothing"
    );

    // Phase 2: advance the INJECTED clock past the store's 24 h gate. Every tick
    // re-reads it, so the next wake-up sweeps again.
    let later = NOW + 2 * mem_profile::decay::SWEEP_INTERVAL_MILLIS;
    now.store(later, Ordering::SeqCst);
    let started = Instant::now();
    while stored(&store, &id).await.0 != 3 {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the second sweep never fired on the advanced clock"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // Reference: the same two sweeps run directly on an identical store.
    let ref_dir = TestDir::new("decay-periodic-ref");
    let reference = profile_store(&ref_dir);
    let ref_id = seed(&reference, NOW).await;
    reference.sweep_decay_daily(NOW).await.expect("ref sweep 1");
    reference
        .sweep_decay_daily(later)
        .await
        .expect("ref sweep 2");
    let expected = stored(&reference, &ref_id).await;
    let actual = stored(&store, &id).await;
    assert_eq!(actual.0, expected.0);
    assert!(
        (actual.1 - expected.1).abs() < 1e-9,
        "the lifecycle swept at the injected clock: got {}, reference {}",
        actual.1,
        expected.1
    );
}

#[tokio::test]
async fn test_decay_lifecycle_ends_on_shutdown() {
    let engine = Engine::new(EngineConfig::default());
    let sweeper = Arc::new(CountingSweeper::default());
    spawn_profile_decay_lifecycle(
        &engine,
        sweeper.clone() as Arc<dyn DecaySweeper>,
        Duration::from_secs(PROFILE_DECAY_TICK_SECS),
        fixed_clock(NOW),
    );
    wait_until("the startup tick", || {
        sweeper.calls.load(Ordering::SeqCst) == 1
    })
    .await;
    let live = engine.live_tasks();
    assert_eq!(live.load(Ordering::SeqCst), 1, "one task: the loop");

    let request = mem_bridge::jsonrpc::parse_line(
        r#"{"jsonrpc":"2.0","id":"s","method":"shutdown","params":{"session_id":"sess-decay"}}"#,
    )
    .expect("shutdown parses");
    let response = engine.dispatch(&request).await;
    assert!(response.error.is_none(), "shutdown is acked: {response:?}");

    wait_until("the loop to end after shutdown", || {
        live.load(Ordering::SeqCst) == 0
    })
    .await;
}

// ---------------------------------------------------------------------------
// Step 3 — the failure leg
// ---------------------------------------------------------------------------

/// A sweeper whose stamp is always poisoned.
#[derive(Default)]
struct PoisonedSweeper {
    calls: AtomicUsize,
}

impl DecaySweeper for PoisonedSweeper {
    fn sweep<'a>(&'a self, _now: i64) -> BoxFuture<'a, Result<DecaySweepReport, ProfileError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Err(ProfileError::LockPoisoned {
                lock: "last_decay_sweep",
                operation: "sweep_decay_daily",
            })
        })
    }
}

/// The child half: the loop must call the sweeper again after a failure.
#[tokio::test]
#[ignore = "runs only as the child of test_poisoned_stamp_is_named_and_the_loop_survives"]
async fn profile_decay_poisoned_child() {
    let engine = Engine::new(EngineConfig::default());
    let sweeper = Arc::new(PoisonedSweeper::default());
    spawn_profile_decay_lifecycle(
        &engine,
        sweeper.clone() as Arc<dyn DecaySweeper>,
        Duration::from_millis(50),
        fixed_clock(NOW),
    );
    wait_until("a second tick after the first failure", || {
        sweeper.calls.load(Ordering::SeqCst) >= 2
    })
    .await;
}

#[test]
fn test_poisoned_stamp_is_named_and_the_loop_survives() {
    let dir = TestDir::new("decay-poisoned");
    let text = common::run_captured("profile_decay_poisoned_child", &dir);
    for needle in [
        "\"event\":\"profile.decay_failed\"",
        "\"kind\":\"lock_poisoned\"",
        "\"lock\":\"last_decay_sweep\"",
    ] {
        assert!(
            text.contains(needle),
            "{needle} missing from child stderr:\n{text}"
        );
    }
}
