//! Step 9 — the bridge surfaces over `Embedder::truncation_stats`.
//!
//! A fake embedder pins the payload shape and the two rate policies: the
//! process-wide `Once` for the engine, and "every call" for the CLI run.

use mem_bridge::truncation_report::{TruncationReporter, payload};
use mem_embed::embedder::{BoxFuture, Embedder, TruncationSnapshot};
use mem_embed::error::EmbedError;

struct FakeEmbedder {
    snapshot: Option<TruncationSnapshot>,
}

impl FakeEmbedder {
    fn truncating() -> Self {
        Self {
            snapshot: Some(TruncationSnapshot {
                cap_tokens: Some(6),
                truncated_inputs: 2,
                dropped_tokens: 1_996,
            }),
        }
    }
}

impl Embedder for FakeEmbedder {
    fn warmup(&self) -> BoxFuture<'_, Result<(), EmbedError>> {
        Box::pin(async { Ok(()) })
    }

    fn embed(&self, _text: &str) -> BoxFuture<'_, Result<Vec<f32>, EmbedError>> {
        Box::pin(async { Ok(vec![0.0; 4]) })
    }

    fn truncation_stats(&self) -> Option<TruncationSnapshot> {
        self.snapshot
    }
}

#[test]
fn test_payload_is_none_at_zero() {
    let quiet = TruncationSnapshot {
        cap_tokens: Some(6),
        truncated_inputs: 0,
        dropped_tokens: 0,
    };
    assert!(
        payload(&quiet).is_none(),
        "nothing was lost, nothing to say"
    );

    let defaulted = TruncationSnapshot::default();
    assert!(
        payload(&defaulted).is_none(),
        "the default snapshot is all zeros"
    );
}

#[test]
fn test_payload_carries_the_three_counters() {
    let snapshot = TruncationSnapshot {
        cap_tokens: Some(6),
        truncated_inputs: 2,
        dropped_tokens: 1_996,
    };
    assert_eq!(
        payload(&snapshot).expect("non-zero"),
        serde_json::json!({
            "cap_tokens": 6,
            "truncated_inputs": 2,
            "dropped_tokens": 1_996,
        })
    );

    let unbounded = TruncationSnapshot {
        cap_tokens: None,
        truncated_inputs: 1,
        dropped_tokens: 3,
    };
    assert_eq!(
        payload(&unbounded).expect("non-zero")["cap_tokens"],
        serde_json::Value::Null
    );
}

#[test]
fn test_first_truncation_is_reported_once() {
    let reporter = TruncationReporter::new();
    let embedder = FakeEmbedder::truncating();

    let first = reporter.report_first(&embedder, "test");
    assert!(
        first.is_some(),
        "the first truncation is announced on stderr"
    );

    let second = reporter.report_first(&embedder, "test");
    assert!(
        second.is_none(),
        "a reporter without a `Once` would report every call"
    );
}

#[test]
fn test_totals_are_reported_on_every_call() {
    let embedder = FakeEmbedder::truncating();

    let first = TruncationReporter::report_totals(&embedder, "reembed", "reembed.input_truncated");
    let second = TruncationReporter::report_totals(&embedder, "reembed", "reembed.input_truncated");

    assert_eq!(first, second, "the CLI path reuses no `Once`");
    assert_eq!(first.expect("non-zero totals")["truncated_inputs"], 2);
}

#[test]
fn test_totals_are_silent_when_nothing_truncated() {
    let embedder = FakeEmbedder { snapshot: None };
    assert!(
        TruncationReporter::report_totals(&embedder, "reembed", "reembed.input_truncated")
            .is_none()
    );
}
