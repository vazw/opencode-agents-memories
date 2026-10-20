//! W4.3 lane — consolidation age threshold (AC-2).
//!
//! The seams under test are the store's injected clock
//! (`EpisodicStore::with_clock`), the [`ConsolidationPolicy`] cutoff and
//! `Consolidator::run`. Every case runs against a real turso database in a
//! temp dir; time comes from the injected `FixedClock`, never from the wall
//! clock.

mod common;

use std::sync::Arc;

use common::{ConsolidationFixture, DAY_MILLIS, FixedClock, TestDir};
use mem_episodic::consolidation::ConsolidationPolicy;
use mem_episodic::record::NewAgentSession;
use mem_episodic::store::EpisodicStore;

/// A fixed "now" the whole file shares (2026-10-20T00:00:00Z).
const NOW: i64 = 1_789_516_800_000;

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_with_clock_stamps_created_at_and_updated_at() {
    let dir = TestDir::new("consolidation-clock");
    let clock = FixedClock::new(NOW);
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config()).with_clock(clock.clock());

    let embedder = common::FakeEmbedder::with_vector(vec![0.1_f32; 8]);
    let new = NewAgentSession {
        project_id: "project-a".to_string(),
        session_id: "session-a".to_string(),
        agent_id: "agent-a".to_string(),
        summary: "a summary".to_string(),
        ..Default::default()
    };
    let id = store
        .insert_session(new, &embedder)
        .await
        .expect("insert session");

    let row = store
        .get_session(&id)
        .await
        .expect("get session")
        .expect("session is visible");
    assert_eq!(row.created_at, NOW, "created_at follows the injected clock");
    assert_eq!(row.updated_at, NOW, "updated_at follows the injected clock");

    // The clock seam is the only time source: advancing it moves the stamp.
    clock.set(NOW + 1_000);
    let embedder = common::FakeEmbedder::with_vector(vec![0.1_f32; 8]);
    let new = NewAgentSession {
        project_id: "project-a".to_string(),
        session_id: "session-b".to_string(),
        agent_id: "agent-a".to_string(),
        summary: "another summary".to_string(),
        ..Default::default()
    };
    let second = store
        .insert_session(new, &embedder)
        .await
        .expect("insert second session");
    let row = store
        .get_session(&second)
        .await
        .expect("get second session")
        .expect("second session is visible");
    assert_eq!(row.created_at, NOW + 1_000);
    assert_eq!(row.updated_at, NOW + 1_000);
}

// ---------------------------------------------------------------------------
// The age threshold (AC-2)
// ---------------------------------------------------------------------------

/// The default window: 30 days.
const DEFAULT_AGE_DAYS: u32 = 30;

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_claims_sessions_older_than_threshold() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-old",
            "agent-a",
            "an old session",
            &["a durable decision"],
        )
        .await;

    let report = fixture
        .real_consolidator(DEFAULT_AGE_DAYS)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert_eq!(report.agents_considered, 1);
    assert_eq!(report.milestones_written, 1);
    assert_eq!(report.sessions_consolidated, 1);
    // The summarizer received exactly the old row.
    let transcripts = fixture.summarizer.transcripts();
    assert_eq!(transcripts.len(), 1);
    assert!(
        transcripts[0].contains("session-old"),
        "the old row reached the summarizer: {}",
        transcripts[0]
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_leaves_sessions_younger_than_threshold_untouched() {
    let fixture = ConsolidationFixture::new();
    let row = fixture
        .insert_at(
            -10 * DAY_MILLIS,
            "project-a",
            "session-young",
            "agent-a",
            "a young session",
            &[],
        )
        .await;

    let report = fixture
        .real_consolidator(DEFAULT_AGE_DAYS)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert_eq!(report.agents_considered, 0);
    assert_eq!(fixture.summarizer.call_count(), 0);
    assert_eq!(report.milestones_written, 0);
    assert!(
        fixture
            .store
            .get_session(&row)
            .await
            .expect("get")
            .is_some(),
        "the young row is untouched"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_boundary_session_exactly_at_cutoff_is_not_eligible() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -(DEFAULT_AGE_DAYS as i64) * DAY_MILLIS,
            "project-a",
            "session-boundary",
            "agent-a",
            "exactly at the cutoff",
            &[],
        )
        .await;

    let report = fixture
        .real_consolidator(DEFAULT_AGE_DAYS)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    // The eligibility predicate is `created_at < cutoff`: the boundary row
    // is NOT eligible.
    assert_eq!(report.agents_considered, 0);
    assert_eq!(fixture.summarizer.call_count(), 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_threshold_comes_from_episodic_config_not_constant() {
    let episodic = mem_core::config::Episodic {
        consolidation_age_days: 7,
    };
    let policy = ConsolidationPolicy::from_episodic(&episodic).expect("policy");
    assert_eq!(policy.age_days(), 7);

    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -8 * DAY_MILLIS,
            "project-a",
            "session-8d",
            "agent-a",
            "eight days old",
            &[],
        )
        .await;

    // The 7-day window claims the 8-day-old row ...
    let report = fixture
        .real_consolidator(policy.age_days())
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");
    assert_eq!(report.agents_considered, 1);
    assert_eq!(report.milestones_written, 1);

    // ... while the default 30-day window does not.
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -8 * DAY_MILLIS,
            "project-a",
            "session-8d",
            "agent-a",
            "eight days old",
            &[],
        )
        .await;
    let report = fixture
        .real_consolidator(DEFAULT_AGE_DAYS)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");
    assert_eq!(report.agents_considered, 0);
}

#[test]
fn test_policy_rejects_zero_age_days() {
    let error = ConsolidationPolicy::new(0).expect_err("zero days is rejected");
    assert!(
        matches!(
            &error,
            mem_episodic::EpisodicError::Validation { field, .. }
                if field == "consolidation_age_days"
        ),
        "unexpected error: {error}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_future_dated_sessions_are_never_eligible() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            10 * DAY_MILLIS,
            "project-a",
            "session-future",
            "agent-a",
            "a future session",
            &[],
        )
        .await;

    let report = fixture
        .real_consolidator(DEFAULT_AGE_DAYS)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");
    assert_eq!(report.agents_considered, 0);
    assert_eq!(fixture.summarizer.call_count(), 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_eligible_sessions_are_capped_per_agent_per_run() {
    let fixture = ConsolidationFixture::new();
    for index in 0..3 {
        fixture
            .insert_at(
                -(40 - index) * DAY_MILLIS,
                "project-a",
                &format!("session-{index}"),
                "agent-a",
                "an old session",
                &[],
            )
            .await;
    }

    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> =
        fixture.recorder.clone();
    let report = fixture
        .consolidator_with_limit(DEFAULT_AGE_DAYS, writer, 1)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert_eq!(report.agents_considered, 1);
    assert_eq!(report.sessions_consolidated, 1);
    let transcripts = fixture.summarizer.transcripts();
    assert_eq!(transcripts.len(), 1);
    assert!(
        transcripts[0].contains("session-0"),
        "the oldest row is claimed first: {}",
        transcripts[0]
    );
    assert!(!transcripts[0].contains("session-1"));
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_consolidation_is_scoped_per_project_and_agent() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "project a agent a",
            &[],
        )
        .await;
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-b",
            "agent-b",
            "project a agent b",
            &[],
        )
        .await;
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-b",
            "session-c",
            "agent-a",
            "project b agent a",
            &[],
        )
        .await;

    let report = fixture
        .real_consolidator(DEFAULT_AGE_DAYS)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    // Only the two agents of project-a are considered.
    assert_eq!(report.agents_considered, 2);
    let transcripts = fixture.summarizer.transcripts();
    assert_eq!(transcripts.len(), 2);
    let joined = transcripts.join("\n");
    assert!(joined.contains("session-a"), "{joined}");
    assert!(joined.contains("session-b"), "{joined}");
    assert!(
        !joined.contains("session-c"),
        "the other project is untouched"
    );
}

/// A zero claim limit is REJECTED, never coerced.
///
/// A silent no-op would count the agent as "considered" while neither
/// consolidating nor skipping it — the run would look successful and
/// consolidate nothing.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_zero_batch_limit_is_rejected() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the source summary",
            &["a durable decision"],
        )
        .await;

    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> =
        fixture.recorder.clone();
    let error = fixture
        .consolidator_with_limit(DEFAULT_AGE_DAYS, writer, 0)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect_err("a zero batch limit is rejected");

    match error {
        mem_episodic::EpisodicError::Validation { field, .. } => {
            assert_eq!(field, "batch_limit", "the offending field is named");
        }
        other => panic!("unexpected error: {other}"),
    }

    // Nothing was written.
    assert_eq!(fixture.summarizer.call_count(), 0, "zero summarizer calls");
    assert_eq!(fixture.sink.call_count(), 0, "zero sink calls");
    assert_eq!(fixture.recorder.call_count(), 0, "zero batch writes");
}
