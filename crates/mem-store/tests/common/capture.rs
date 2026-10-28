//! A thread-local WARN capture for the AC6b fallback-warning test.
//!
//! No dev-dependency: `tracing::subscriber::set_default` ships with the
//! `tracing` dependency both crates already have. The capture records every
//! field into `rendered` so the privacy assertion can prove a path, a session
//! id or content never rides on the warning.

use std::sync::{Arc, Mutex};

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
}

/// The capture subscriber. The event store is an `Arc`, so a clone handed to
/// `set_default` shares the same events with the test's handle (the trait's
/// REQUIRED impl cannot be written for `Arc<WarnCapture>`: both the trait and
/// `Arc` are foreign, so the impl lives on the local type).
#[derive(Default, Clone)]
pub struct WarnCapture {
    events: Arc<Mutex<Vec<WarnEvent>>>,
}

impl WarnCapture {
    /// Every captured event, in order.
    pub fn events(&self) -> Vec<WarnEvent> {
        self.events.lock().expect("warn capture lock").clone()
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
            });
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}
