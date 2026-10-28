//! A process-wide WARN capture for the AC6b fallback-warning tests.
//!
//! # Why not `set_default`
//!
//! A thread-local subscriber install can leave an already-registered callsite
//! uninterested: the callsite's first registration computes its interest from
//! the dispatchers at that moment and stores it (`DefaultCallsite::register`),
//! while a subscriber install rebuilds the same atomic from another caller
//! (`rebuild_interest_cache`); the two writes are unsynchronised. A callsite
//! another test registered as `never` before the capture exists can therefore
//! stay `never` for the rest of the process — the parallel workspace schedule
//! reproduces it as zero events.
//!
//! The capture is therefore installed ONCE per test binary as the process-wide
//! subscriber (`OnceLock` + `set_global_default`), the interest cache is
//! rebuilt after the install (and once more immediately before each window,
//! covering an in-flight first registration), the subscriber answers
//! `Interest::always()` for WARN from `register_callsite` itself, and `events()`
//! filters by the emitting thread so tests sharing the binary cannot see each
//! other's warns. The tests stay on the current-thread runtime and must not
//! `tokio::spawn` the seam (the thread filter would drop the event).
//!
//! No dev-dependency: `tracing` is a normal dependency of both crates.

use std::sync::{Arc, Mutex, OnceLock};

/// The message `shard_facts::warn_model_equality` emits (pinned here for the
/// subscriber's filter; a drift breaks the test loudly).
pub const MODEL_EQUALITY_MESSAGE: &str =
    "shard identity: no fingerprint; accepting on model equality (not proof)";

/// One captured WARN event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WarnEvent {
    /// The event's `message` field.
    pub message: String,
    /// The `shard_id` field, when present.
    pub shard_id: Option<i64>,
    /// The `stored_model` field.
    pub stored_model: Option<String>,
    /// The `configured_model` field.
    pub configured_model: Option<String>,
    /// The `authority` field (`model-equality` / `model-equality-no-identity`).
    pub authority: Option<String>,
    /// Every field as `name=value;` — the privacy assertion reads this.
    pub rendered: String,
    /// The emitting thread; `events()` shows only the caller's thread.
    pub thread: std::thread::ThreadId,
}

static INSTALLED: OnceLock<WarnCapture> = OnceLock::new();

/// Installs the capture as the process-wide subscriber (once per test binary)
/// and returns a handle. `rebuild_interest_cache` re-registers every callsite
/// that another test may have registered as `never` before this call.
pub fn install() -> WarnCapture {
    INSTALLED
        .get_or_init(|| {
            let capture = WarnCapture::default();
            tracing::subscriber::set_global_default(capture.clone())
                .expect("the capture is the only global tracing subscriber in this binary");
            tracing::callsite::rebuild_interest_cache();
            capture
        })
        .clone()
}

/// The capture subscriber. The event store is an `Arc`, so the clone handed to
/// `set_global_default` shares the same events with every test's handle (the
/// trait's required impl cannot be written for `Arc<WarnCapture>`: both the
/// trait and `Arc` are foreign, so the impl lives on the local type).
#[derive(Default, Clone)]
pub struct WarnCapture {
    events: Arc<Mutex<Vec<WarnEvent>>>,
}

impl WarnCapture {
    /// The captured events emitted by the calling thread, in order.
    pub fn events(&self) -> Vec<WarnEvent> {
        let me = std::thread::current().id();
        self.events
            .lock()
            .expect("warn capture lock")
            .iter()
            .filter(|event| event.thread == me)
            .cloned()
            .collect()
    }

    /// Drops the calling thread's events (the fixture's own warns precede the
    /// window under assertion).
    pub fn clear_current_thread(&self) {
        let me = std::thread::current().id();
        self.events
            .lock()
            .expect("warn capture lock")
            .retain(|event| event.thread != me);
    }
}

#[derive(Default)]
struct Fields {
    message: Option<String>,
    shard_id: Option<i64>,
    stored_model: Option<String>,
    configured_model: Option<String>,
    authority: Option<String>,
    rendered: String,
}

impl tracing::field::Visit for Fields {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.rendered
            .push_str(&format!("{}={value};", field.name()));
        match field.name() {
            "message" => self.message = Some(value.to_string()),
            "stored_model" => self.stored_model = Some(value.to_string()),
            "configured_model" => self.configured_model = Some(value.to_string()),
            "authority" => self.authority = Some(value.to_string()),
            _ => {}
        }
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.rendered
            .push_str(&format!("{}={value};", field.name()));
        if field.name() == "shard_id" {
            self.shard_id = Some(value);
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.record_str(field, &format!("{value:?}"));
    }
}

impl tracing::Subscriber for WarnCapture {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() == tracing::Level::WARN
    }

    fn register_callsite(
        &self,
        metadata: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        if *metadata.level() == tracing::Level::WARN {
            tracing::subscriber::Interest::always()
        } else {
            tracing::subscriber::Interest::never()
        }
    }

    fn new_span(&self, _attributes: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        if fields.message.as_deref() != Some(MODEL_EQUALITY_MESSAGE) {
            return;
        }
        self.events
            .lock()
            .expect("warn capture lock")
            .push(WarnEvent {
                message: fields.message.unwrap_or_default(),
                shard_id: fields.shard_id,
                stored_model: fields.stored_model,
                configured_model: fields.configured_model,
                authority: fields.authority,
                rendered: fields.rendered,
                thread: std::thread::current().id(),
            });
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}
