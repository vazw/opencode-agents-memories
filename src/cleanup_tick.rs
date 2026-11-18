//! Issue 41 — the capture-triggered retention cleanup tick.
//!
//! One task, one permit, no timer. Every accepted capture fires a
//! [`CaptureSignal`] wake; the task asks the maintenance port whether the
//! configured interval elapsed and runs at most one sweep per wake.
//!
//! # Why here and not in `mem-web`
//!
//! Only the root sees both halves: the port (`mem-web`) and the structured
//! stderr channel (`mem-bridge`). The tick therefore composes them without
//! either crate learning about the other.
//!
//! # Why it is safe to detach
//!
//! `run_in_runtime` drops the runtime after `shutdown_pipeline`, so an in-flight
//! tick is cancelled silently and its stamp stays unwritten — the next start
//! retries. A wake that arrives after shutdown consumes its permit, reads
//! `cancel`, and exits without touching the port.
//!
//! # What each outcome logs
//!
//! `cleanup.tick` (info) for a completed sweep — this is the line the latency
//! instrument parses for `duration_ms`; `cleanup.busy` (warn) when another
//! sweep held the guard; `cleanup.tick_failed` (warn) with a CLOSED `kind`
//! (never `Display`, which can carry paths) when the port failed.
//! `Disabled` and `NotDue` are silent by decision: the idle path runs per turn,
//! so anything unlatched is unbounded stderr.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use mem_bridge::engine::{CaptureSignal, structured_stderr};
use mem_store::serde_json::json;
use mem_web::api::ports::{CleanupTick, MaintenancePort, PortError};

/// The tick loop: wake, check the interval, sweep at most once.
pub struct CleanupTrigger {
    signal: CaptureSignal,
}

impl CleanupTrigger {
    pub fn new(signal: CaptureSignal) -> Self {
        Self { signal }
    }

    /// Spawns the task and returns its handle. Dropping the handle detaches it;
    /// the process's runtime owns its lifetime.
    pub fn spawn(
        self,
        port: Arc<dyn MaintenancePort>,
        cancel: Arc<AtomicBool>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                self.signal.notified().await;
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                let started = Instant::now();
                match port.cleanup_tick(mem_profile::ids::now_millis()).await {
                    Ok(CleanupTick::Ran(report)) => structured_stderr(
                        "cleanup.tick",
                        "info",
                        "cleanup",
                        json!({
                            "deleted_count": report.deleted_count,
                            "user_count": report.user_count,
                            "project_count": report.project_count,
                            "prompts_deleted": report.prompts_deleted,
                            "protected_memories_skipped": report.protected_memories_skipped,
                            "inbox_dead_retained": report.inbox_dead_retained,
                            "duration_ms": started.elapsed().as_millis() as u64,
                        }),
                    ),
                    Ok(CleanupTick::Busy) => structured_stderr(
                        "cleanup.busy",
                        "warn",
                        "cleanup",
                        json!({ "component": "cleanup" }),
                    ),
                    // Silent by decision 9: the idle path must not narrate.
                    Ok(CleanupTick::Disabled) | Ok(CleanupTick::NotDue) => {}
                    Err(PortError::Unavailable) => {
                        // A degraded composition cannot become healthy later, so
                        // one logged attempt and out — never a spin.
                        structured_stderr(
                            "cleanup.tick_failed",
                            "warn",
                            "cleanup",
                            json!({ "kind": "unavailable" }),
                        );
                        break;
                    }
                    Err(other) => structured_stderr(
                        "cleanup.tick_failed",
                        "warn",
                        "cleanup",
                        json!({
                            "kind": match other {
                                PortError::Store(_)
                                | PortError::ModelMismatch(_)
                                | PortError::ShardFileMissing { .. }
                                | PortError::Busy => "store",
                                _ => "other",
                            }
                        }),
                    ),
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use async_trait::async_trait;
    use mem_bridge::engine::CaptureSignal;
    use mem_web::api::ports::{
        CleanupReport, CleanupTick, DedupReport, MaintenancePort, MigrationReport,
        MigrationRunReport, PortError, TagMigrationProgressView, TagMigrationReport,
    };

    use super::CleanupTrigger;

    const JOIN_BUDGET: Duration = Duration::from_secs(5);

    /// What the fake answers, and how many times it was asked.
    struct FakePort {
        answer: FakeAnswer,
        calls: AtomicUsize,
        changed: tokio::sync::Notify,
    }

    #[derive(Clone, Copy)]
    enum FakeAnswer {
        Ran,
        Unavailable,
    }

    impl FakePort {
        fn new(answer: FakeAnswer) -> Self {
            Self {
                answer,
                calls: AtomicUsize::new(0),
                changed: tokio::sync::Notify::new(),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        /// Waits until the task has asked at least `n` times.
        async fn wait_for_calls(&self, n: usize) {
            tokio::time::timeout(JOIN_BUDGET, async {
                while self.calls() < n {
                    self.changed.notified().await;
                }
            })
            .await
            .expect("the tick asked the port inside the budget");
        }
    }

    #[async_trait]
    impl MaintenancePort for FakePort {
        async fn detect_migration(&self) -> Result<MigrationReport, PortError> {
            Err(PortError::Unavailable)
        }

        async fn run_migration(&self, _strategy: &str) -> Result<MigrationRunReport, PortError> {
            Err(PortError::Unavailable)
        }

        async fn detect_tag_migration(&self) -> Result<TagMigrationReport, PortError> {
            Err(PortError::Unavailable)
        }

        async fn tag_migration_progress(&self) -> Result<TagMigrationProgressView, PortError> {
            Err(PortError::Unavailable)
        }

        async fn tag_migration_records(
            &self,
        ) -> Result<Vec<mem_core::tags::migration::MemoryRecord>, PortError> {
            Err(PortError::Unavailable)
        }

        async fn generate_tags(
            &self,
            _memory_id: &str,
            _content: &str,
        ) -> Result<Vec<String>, String> {
            Err("unavailable".to_string())
        }

        async fn run_tag_migration_batch(
            &self,
            _batch_size: usize,
            _records: Vec<mem_core::tags::migration::MemoryRecord>,
            _tags: std::collections::HashMap<String, Result<Vec<String>, String>>,
        ) -> Result<mem_core::tags::migration::BatchOutcome, PortError> {
            Err(PortError::Unavailable)
        }

        async fn deduplicate(&self) -> Result<DedupReport, PortError> {
            Err(PortError::Unavailable)
        }

        async fn cleanup_tick(&self, _now_ms: i64) -> Result<CleanupTick, PortError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.changed.notify_one();
            match self.answer {
                FakeAnswer::Ran => Ok(CleanupTick::Ran(CleanupReport::default())),
                FakeAnswer::Unavailable => Err(PortError::Unavailable),
            }
        }
    }

    /// Two wakes cost two calls — and the first one is AWAITED before the
    /// second, because two back-to-back `notify_one` calls coalesce into one
    /// permit. The mutant loops without awaiting the fake, which makes the
    /// second wake a stored permit consumed by the first iteration.
    #[tokio::test]
    async fn test_tick_task_runs_one_sweep_per_wake() {
        let signal = CaptureSignal::new();
        let cancel = Arc::new(AtomicBool::new(false));
        let port = Arc::new(FakePort::new(FakeAnswer::Ran));
        let handle = CleanupTrigger::new(signal.clone()).spawn(
            Arc::clone(&port) as Arc<dyn MaintenancePort>,
            Arc::clone(&cancel),
        );

        signal.notify_one();
        port.wait_for_calls(1).await;
        signal.notify_one();
        port.wait_for_calls(2).await;

        cancel.store(true, Ordering::SeqCst);
        signal.notify_one();
        tokio::time::timeout(JOIN_BUDGET, handle)
            .await
            .expect("the tick exits once cancelled")
            .expect("the tick task does not panic");

        assert_eq!(
            port.calls(),
            2,
            "one sweep per wake, and the cancel-time wake runs none"
        );
    }

    /// A wake that arrives AFTER the cancel flag must consume its permit and
    /// exit without touching the port. The mutant checks `cancel` only before
    /// `notified()`, so the stored permit runs one more sweep against a
    /// shutting-down runtime.
    #[tokio::test]
    async fn test_tick_task_ignores_a_wake_after_cancel() {
        let signal = CaptureSignal::new();
        let cancel = Arc::new(AtomicBool::new(true));
        let port = Arc::new(FakePort::new(FakeAnswer::Ran));
        let handle = CleanupTrigger::new(signal.clone()).spawn(
            Arc::clone(&port) as Arc<dyn MaintenancePort>,
            Arc::clone(&cancel),
        );

        signal.notify_one();
        tokio::time::timeout(JOIN_BUDGET, handle)
            .await
            .expect("the tick exits on the wake that follows the cancel")
            .expect("the tick task does not panic");

        assert_eq!(port.calls(), 0, "a wake after cancel must run no sweep");
    }

    /// `Unavailable` ends the task after one logged attempt: a degraded
    /// composition can never become healthy, so continuing would spin a warn
    /// line per capture.
    #[tokio::test]
    async fn test_tick_task_exits_on_unavailable_without_spinning() {
        let signal = CaptureSignal::new();
        let cancel = Arc::new(AtomicBool::new(false));
        let port = Arc::new(FakePort::new(FakeAnswer::Unavailable));
        let handle = CleanupTrigger::new(signal.clone()).spawn(
            Arc::clone(&port) as Arc<dyn MaintenancePort>,
            Arc::clone(&cancel),
        );

        signal.notify_one();
        tokio::time::timeout(JOIN_BUDGET, handle)
            .await
            .expect("the tick exits after one failed attempt")
            .expect("the tick task does not panic");

        assert_eq!(port.calls(), 1, "exactly one attempt, never a retry loop");
    }
}
