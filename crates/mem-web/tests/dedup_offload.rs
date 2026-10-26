//! The near-duplicate scan leaves the runtime thread — and the run guard
//! survives the request that started it.
//!
//! Centrepiece (AC2): on a **single-threaded** tokio runtime, once the fake
//! store's `memories()` handshake has fired, the spawned deduplication request
//! must still be unfinished while a concurrent `GET /api/health` through the
//! real `build_router` answers. At base the scan runs inline on the runtime
//! thread, so the handshake cannot even be observed until the scan has
//! returned and `dedup.is_finished()` is already true.
//!
//! Guard lease (AC3): a request future dropped mid-scan must not release the
//! run guard, because the detached scan it queued is still burning a blocking
//! thread. The test owns the first future itself (`timeout` on `service.run()`,
//! never a spawned `JoinHandle`: dropping a handle does not cancel the task, so
//! the async guard alone would hold the pass and the lease would go untested).
//! At base the first run finishes in a single poll — there is no await between
//! `memories()` and the end of the run for a fixture with no exact duplicates —
//! so the timeout never fires and the second run is admitted: the observed
//! base failure is *the guard died with the completed request*, not
//! "cancelled before any scan".
//!
//! No sleeps, no `Instant` assertions: the ordering of the observations is the
//! proof, and the release poll is a bounded `yield_now` loop whose exhaustion
//! is an explicit failure.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use common::{api_request, app_state_with_api};
use mem_web::api::deduplication::{
    DedupMemory, DedupService, DedupShard, DedupStore, ProductionDeduplication, RUNNING_MESSAGE,
};
use mem_web::api::ports::{ApiServices, PortError};
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;
use serde_json::Value;
use tokio::sync::Notify;

const TOKEN: &str = "test-token";
const TAG: &str = "tag_offload";
const DIMENSIONS: usize = 768;

/// The centrepiece's survivor count.
///
/// Two jobs, both from `.tmp/measure/20261026-issue-54b-scan-ladder.txt` (the
/// Step 1 ladder, measured in the **debug** profile this suite runs in):
///
/// (a) `n(n−1)/2 = 75_855 < MAX_NEAR_SCAN_PAIRS`, so the fixture can never
///     truncate and `pairsScanned` is the whole grid;
/// (b) comfortably more than 0.1 s of scan. The ladder's recommended n for
///     0.1 s is 130 at its measured c_iter; this fixture is 3x that, about
///     0.9 s of scan in debug, which is what makes the "still unfinished after
///     the health response" assertion deterministic instead of timing-fragile.
///
/// The literal is baked in on purpose: the test never re-derives it from a
/// machine measurement at run time.
const CENTREPIECE_N: usize = 390;

/// The AC3 survivor count: `n >= 2000` per the test plan.
const LEASE_N: usize = 3_000;

/// How long the first run may go before its future is dropped. `<= 50 ms` per
/// the test plan; the fixture's scan is far longer than this (tiny vectors
/// still cost well over 50 ms over ~4.5M comparisons in debug).
const LEASE_DROP_AFTER: Duration = Duration::from_millis(20);

/// The bounded `yield_now` poll's iteration cap. A few thousand yields would
/// exhaust long before a ~1 s scan clears its lease; 20 million cannot.
const MAX_RELEASE_YIELDS: usize = 20_000_000;

/// Distinct content, one-hot vectors: distinct lanes give a cosine of exactly
/// `0.0` (below the threshold) and, past `768` rows, a repeated lane gives
/// exactly `1.0` (excluded by the kernel's `< 1.0` rule). Nothing is ever
/// claimed, so no outer iteration is skipped and the scan is exactly the whole
/// ordered pair grid — which is what `pairsScanned == n(n-1)/2` asserts.
fn one_hot_rows(n: usize) -> Vec<DedupMemory> {
    (0..n)
        .map(|index| {
            let mut vector = vec![0.0_f32; DIMENSIONS];
            vector[index % DIMENSIONS] = 1.0;
            DedupMemory {
                id: format!("mem_offload_{index}"),
                content: format!("offload content {index}"),
                container_tag: TAG.to_string(),
                created_at: index as i64,
                vector: Some(vector),
            }
        })
        .collect()
}

fn shard(shard_id: i64) -> DedupShard {
    DedupShard {
        shard_id,
        db_path: PathBuf::from(format!("/tmp/fake-offload-shard-{shard_id}.db")),
    }
}

/// A store whose `memories()` fires `entered` once its answer is complete —
/// the handshake the centrepiece waits on — and whose `delete` is stateful
/// like the service test's fake.
struct FakeStore {
    shards: Vec<DedupShard>,
    rows: Mutex<Vec<(i64, DedupMemory)>>,
    deleted: Mutex<Vec<String>>,
    entered: Option<Arc<Notify>>,
}

impl FakeStore {
    fn new(rows: Vec<(i64, DedupMemory)>) -> Self {
        Self {
            shards: vec![shard(1)],
            rows: Mutex::new(rows),
            deleted: Mutex::new(Vec::new()),
            entered: None,
        }
    }

    fn gated(mut self, entered: Arc<Notify>) -> Self {
        self.entered = Some(entered);
        self
    }

    fn deleted(&self) -> Vec<String> {
        self.deleted
            .lock()
            .map(|deleted| deleted.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl DedupStore for FakeStore {
    async fn shards(&self) -> Result<Vec<DedupShard>, PortError> {
        Ok(self.shards.clone())
    }

    async fn memories(&self, shard: &DedupShard) -> Result<Vec<DedupMemory>, PortError> {
        let rows = self
            .rows
            .lock()
            .map(|rows| rows.clone())
            .unwrap_or_default();
        let rows: Vec<DedupMemory> = rows
            .into_iter()
            .filter(|(shard_id, _)| *shard_id == shard.shard_id)
            .map(|(_, row)| row)
            .collect();
        if let Some(entered) = &self.entered {
            entered.notify_one();
        }
        Ok(rows)
    }

    async fn delete(&self, _shard: &DedupShard, memory_id: &str) -> Result<(), PortError> {
        if let Ok(mut deleted) = self.deleted.lock() {
            deleted.push(memory_id.to_string());
        }
        if let Ok(mut rows) = self.rows.lock() {
            rows.retain(|(_, row)| row.id != memory_id);
        }
        Ok(())
    }
}

fn router_with(store: Arc<FakeStore>) -> axum::Router {
    let services = ApiServices::unavailable()
        .with_maintenance(Arc::new(ProductionDeduplication::new(store, true, 0.9)));
    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(TOKEN),
        None,
        PathBuf::from("/tmp/unused-dist"),
        common::throwaway_log_dir(),
        services,
        "unknown",
        "127.0.0.1",
    );
    build_router(Arc::new(state))
}

#[tokio::test]
async fn test_dedup_scan_is_detached_so_the_runtime_stays_responsive() {
    let entered = Arc::new(Notify::new());
    let store = Arc::new(
        FakeStore::new(
            one_hot_rows(CENTREPIECE_N)
                .into_iter()
                .map(|row| (1, row))
                .collect(),
        )
        .gated(Arc::clone(&entered)),
    );
    let router = router_with(Arc::clone(&store));

    let dedup = tokio::spawn(api_request(
        router.clone(),
        "POST",
        "/api/deduplicate",
        Some(TOKEN),
        None,
    ));

    // The handshake: `memories()` has answered, so the request is now either
    // scanning inline (base) or has queued the scan on a blocking thread.
    entered.notified().await;
    assert!(
        !dedup.is_finished(),
        "the deduplication request finished before the health probe ran: \
         the near scan is still on the runtime thread"
    );

    let health = api_request(router, "GET", "/api/health", None, None).await;
    assert_eq!(health.status, 200, "{}", health.body_string());
    assert_eq!(
        health.body_string(),
        r#"{"success":true,"status":"ok","authEnabled":false}"#,
        "the health envelope is the documented one"
    );
    // Load-bearing: a health 200 alone only proves "served while the request
    // was outstanding" — it does not prove the scan was off the thread. The
    // dedup task must still be unfinished *after* the health response.
    assert!(
        !dedup.is_finished(),
        "the health probe was served only after the scan had already finished"
    );

    let response = dedup
        .await
        .expect("the deduplication task joins")
        .body_string();
    let body: Value = serde_json::from_str(&response)
        .unwrap_or_else(|error| panic!("the dedup response is JSON: {error}: {response}"));
    assert_eq!(body["success"], Value::Bool(true), "{body}");
    // The explicit premise: no pair was claimed, so no outer iteration was
    // skipped and the scan visited the whole ordered pair grid.
    assert_eq!(
        body["data"]["nearDuplicateGroups"],
        serde_json::json!([]),
        "the one-hot fixture claims no candidate: {body}"
    );
    assert_eq!(
        body["data"]["nearDuplicateScan"]["pairsScanned"],
        serde_json::json!(CENTREPIECE_N * (CENTREPIECE_N - 1) / 2),
        "the centrepiece scanned the whole grid rather than nothing: {body}"
    );
    assert_eq!(
        body["data"]["nearDuplicateScan"]["truncated"],
        Value::Bool(false),
        "a single shard well under the cap never truncates: {body}"
    );
}

#[tokio::test]
async fn test_dedup_service_keeps_the_run_guard_while_a_detached_scan_outlives_a_cancelled_request()
{
    // Tiny vectors on purpose: this test exercises the guard/lease, not the
    // cosine cost. 4 lanes over ~4.5M comparisons is comfortably longer than
    // the 20 ms drop deadline and short enough for a bounded poll to outlast.
    let rows: Vec<(i64, DedupMemory)> = (0..LEASE_N)
        .map(|index| {
            (
                1,
                DedupMemory {
                    id: format!("mem_lease_{index}"),
                    content: format!("lease content {index}"),
                    container_tag: TAG.to_string(),
                    created_at: index as i64,
                    vector: Some(vec![1.0_f32, 0.0, 0.0, 0.0]),
                },
            )
        })
        .collect();
    assert!(LEASE_N >= 2000, "the drop must land well inside the scan");
    let store = Arc::new(FakeStore::new(rows));
    let service = DedupService::new(store.clone(), true, 0.9);

    // Own the first future: `timeout` DROPS it at the deadline, which is the
    // only way to model a client that disconnects. A spawned `JoinHandle`
    // would keep running and prove nothing about the guard.
    let first = tokio::time::timeout(LEASE_DROP_AFTER, service.run()).await;
    assert!(
        first.is_err(),
        "the first run answered before the deadline, so no scan outlived the request \
         (at base the inline scan finishes in a single poll)"
    );

    // The detached scan is still running, so the guard must still refuse.
    assert_eq!(
        service.run().await,
        Err(PortError::Invalid(RUNNING_MESSAGE.to_string())),
        "a second pass was admitted while the detached scan still holds a blocking thread"
    );
    assert!(
        store.deleted().is_empty(),
        "the refused pass deleted nothing: {:?}",
        store.deleted()
    );

    // Release, proven by a bounded `yield_now` poll (never a sleep). The scan
    // is ~4.5M comparisons; a few thousand yields would exhaust long before
    // it clears, so the cap is 20 million and exhaustion is a FAILURE.
    let mut yields = 0;
    let report = loop {
        match service.run().await {
            Ok(report) => break report,
            Err(PortError::Invalid(message)) if message == RUNNING_MESSAGE => {}
            Err(other) => panic!("the retry answered an unexpected error: {other:?}"),
        }
        yields += 1;
        assert!(
            yields < MAX_RELEASE_YIELDS,
            "the lease never released: after {MAX_RELEASE_YIELDS} yields the detached scan \
             still holds the run guard"
        );
        tokio::task::yield_now().await;
    };
    assert_eq!(
        report.exact_duplicates_deleted, 0,
        "the accepted retry runs over a store with no exact duplicates"
    );
    assert!(
        yields > 0,
        "the retry was admitted on its first attempt, so the refusal above proved nothing"
    );
}
