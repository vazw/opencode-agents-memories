//! W5.4 — `recall`: the project-scoped session read.
//!
//! One wire method, two accepted parameter shapes (D3):
//!
//! * **canonical** — the W4.2 [`RecallProjectSessionsArgs`], flattened at the
//!   top level (camelCase, `deny_unknown_fields`), discriminated by the
//!   presence of `projectId`;
//! * **legacy** — the frozen `{query, scope?, tags?, limit?}` shape published
//!   by `docs/bridge-schema.md`; it resolves no project and reads no rows, so
//!   it answers one logged empty page (D4).
//!
//! The envelope `session_id` is stripped from a CLONE before parsing: every
//! method carries it, while the W4.2 filter key is the different `sessionId`
//! (camelCase) and must survive untouched (D3).
//!
//! The wire page is minted in exactly one place — [`RecallService::page`],
//! from the W4.2 result enum, `count = items.len()` (D11).

use std::sync::Arc;

use mem_embed::embedder::Embedder;
use mem_episodic::error::EpisodicError;
use mem_episodic::store::EpisodicStore;
use mem_episodic::tool_schema::{
    RecallMode, RecallProjectSessionsArgs, RecallProjectSessionsResult,
};
use serde_json::{Map, Value, json};

use crate::engine::structured_stderr;

/// The complete key set of the frozen legacy shape (envelope excluded).
const LEGACY_KEYS: [&str; 4] = ["query", "scope", "tags", "limit"];

/// The two accepted parameter shapes of the `recall` method.
#[derive(Debug, Clone, PartialEq)]
pub enum RecallRequest {
    /// The canonical shape: the W4.2 tool arguments.
    Canonical(RecallProjectSessionsArgs),
    /// The frozen legacy shape: an empty page, never a read.
    Legacy,
}

impl RecallRequest {
    /// Classifies one request's `params` object.
    ///
    /// The envelope's `session_id` is stripped first; the decision is
    /// structural, not positional: `projectId` present ⇒ canonical; otherwise
    /// the remaining key set must be a subset of [`LEGACY_KEYS`] **and**
    /// `query` must be a string; anything else is `-32602`.
    pub fn from_params(params: &Value) -> Result<Self, RecallError> {
        let object = params.as_object().ok_or(RecallError::InvalidParams)?;
        let mut shape: Map<String, Value> = object.clone();
        shape.remove("session_id");
        if shape.contains_key("projectId") {
            let args: RecallProjectSessionsArgs = serde_json::from_value(Value::Object(shape))
                .map_err(|_| RecallError::InvalidParams)?;
            return Ok(Self::Canonical(args));
        }
        let legacy = shape.keys().all(|key| LEGACY_KEYS.contains(&key.as_str()))
            && shape.get("query").is_some_and(Value::is_string);
        if legacy {
            Ok(Self::Legacy)
        } else {
            Err(RecallError::InvalidParams)
        }
    }
}

/// The `recall` failure classes: a caller fault versus an engine fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecallError {
    /// The parameters are neither accepted shape, or the canonical arguments
    /// fail validation (`-32602`).
    InvalidParams,
    /// The store or embedder failed (`-32603`).
    Internal,
}

impl From<EpisodicError> for RecallError {
    fn from(error: EpisodicError) -> Self {
        match error {
            EpisodicError::Validation { .. } => Self::InvalidParams,
            _ => Self::Internal,
        }
    }
}

/// The canonical read behind `recall`.
pub struct RecallService {
    store: Arc<EpisodicStore>,
    embedder: Arc<dyn Embedder>,
}

impl RecallService {
    /// Builds the service over the composed store and embedder.
    pub fn new(store: Arc<EpisodicStore>, embedder: Arc<dyn Embedder>) -> Self {
        Self { store, embedder }
    }

    /// The frozen legacy shape's answer: one logged empty page and no read
    /// (D4). The extras object is a constant empty object, so no
    /// `session_id`, query or project path can travel through the log line.
    pub fn legacy_page() -> Value {
        structured_stderr("recall.legacy_shape_unscoped", "warn", "recall", json!({}));
        Self::page(RecallProjectSessionsResult::Summaries(Vec::new()))
    }

    /// Serves one canonical request.
    ///
    /// The args are validated (and their mode resolved) BEFORE any store
    /// call, so a blank project or an invalid mode/filter never reaches SQL.
    pub async fn recall(&self, args: &RecallProjectSessionsArgs) -> Result<Value, RecallError> {
        args.validate()?;
        let mode = args.resolved_mode()?;
        let summaries = match mode {
            RecallMode::Chronological | RecallMode::Filtered => {
                self.store
                    .timeline(&args.project_id, &args.to_timeline_filter())
                    .await?
            }
            RecallMode::Semantic => {
                // `resolved_mode` guarantees a query here; refuse rather
                // than panic if that invariant is ever broken.
                let Some(query) = args.query.as_deref() else {
                    return Err(RecallError::InvalidParams);
                };
                self.store
                    .semantic_timeline(
                        &args.project_id,
                        query,
                        self.embedder.as_ref(),
                        &args.to_timeline_filter(),
                    )
                    .await?
            }
        };
        let result = if args.include_full_sessions {
            let mut sessions = Vec::with_capacity(summaries.len());
            for summary in &summaries {
                // A row deleted between the listing and this read is
                // invisible (D-11), exactly as it would have been had the
                // delete landed first.
                if let Some(session) = self
                    .store
                    .session_detail(&summary.project_id, &summary.id)
                    .await?
                {
                    sessions.push(session);
                }
            }
            RecallProjectSessionsResult::FullSessions(sessions)
        } else {
            RecallProjectSessionsResult::Summaries(summaries)
        };
        Ok(Self::page(result))
    }

    /// Mints the wire page from the W4.2 result vocabulary; `count` is always
    /// the item count, never a second source of truth (D11).
    pub fn page(result: RecallProjectSessionsResult) -> Value {
        match result {
            RecallProjectSessionsResult::Summaries(items) => {
                json!({ "count": items.len(), "items": items })
            }
            RecallProjectSessionsResult::FullSessions(items) => {
                json!({ "count": items.len(), "items": items })
            }
        }
    }
}
