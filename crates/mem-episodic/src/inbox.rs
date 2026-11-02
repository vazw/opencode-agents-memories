//! W5.3 Step 4 — the durable capture inbox (ADR-0007 D-48): a capture is
//! durably queued into `episodic.db` BEFORE the `{"accepted":true}` ack.
//!
//! Bounded and idempotent: at most [`INBOX_CAPACITY`] queued entries, one
//! conditional insert per enqueue (the capacity predicate lives INSIDE the
//! statement — no count-then-insert race), duplicates of an existing
//! `capture_id` are accepted as a no-op. `mark_done` purges the row in one
//! `BatchMode::Write` write (no separate `purge_done`/reaper exists — the
//! D-48 done statement is the DELETE). Dead-letter after
//! [`MAX_CAPTURE_ATTEMPTS`] consumed attempts: the terminal state is an
//! episodic-only row, logged with a structured event and NO transcript
//! payload.
//!
//! **One clock (issue 110, D-110-2).** Every stamp this file writes and the
//! `now` the drain compares against come from [`EpisodicStore::now`], the
//! store's injectable clock. There is deliberately no wall-clock read here:
//! a fake-clock test and a real row cannot disagree, and re-adding a second
//! time source needs a visible import. `not_before` is therefore an absolute
//! wall-clock instant, so a deadline survives a restart (the row is durable)
//! but does not survive a backwards clock correction.

use std::sync::Arc;

use mem_store::db::{BatchMode, Db, Statement, Value, query_one};
use mem_store::episodic_sql as sql;
use mem_store::serde_json::Value as Json;

use crate::error::EpisodicError;
use crate::store::EpisodicStore;

/// The queued-entry capacity bound (blueprint Step 4).
pub const INBOX_CAPACITY: usize = 1024;

/// A dead-letter fires after this many CONSUMED (non-lock) failed attempts.
pub const MAX_CAPTURE_ATTEMPTS: u32 = 5;

/// The per-drain dequeue batch bound.
pub const DRAIN_BATCH_LIMIT: usize = 32;

/// Why an enqueue failed.
#[derive(Debug, Clone)]
pub enum InboxError {
    /// The queued row count is at [`INBOX_CAPACITY`].
    Full,
    /// Storage failure.
    Store(EpisodicError),
}

impl std::fmt::Display for InboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => write!(f, "capture inbox full ({INBOX_CAPACITY} entries)"),
            Self::Store(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for InboxError {}

impl From<EpisodicError> for InboxError {
    fn from(error: EpisodicError) -> Self {
        Self::Store(error)
    }
}

/// A validated new inbox entry (the engine builds it from the parsed
/// `capture` params).
#[derive(Debug, Clone, PartialEq)]
pub struct InboxNewEntry {
    pub capture_id: String,
    pub project_id: String,
    pub session_id: String,
    pub agent_id: String,
    /// The raw request `metadata`/payload JSON as received.
    pub payload: Json,
}

/// One dequeued entry.
#[derive(Debug, Clone, PartialEq)]
pub struct InboxEntry {
    pub capture_id: String,
    pub project_id: String,
    pub session_id: String,
    pub agent_id: String,
    /// The raw payload JSON as received (never logged).
    pub payload: Json,
    pub created_at: i64,
    pub attempts: u32,
    pub last_attempt_at: Option<i64>,
}

/// The inbox over one `episodic.db` (the store owns migration v5).
pub struct CaptureInbox {
    store: Arc<EpisodicStore>,
}

impl CaptureInbox {
    /// The inbox over `store` (v5 must be applied; `store.ready()` does it).
    pub fn new(store: Arc<EpisodicStore>) -> Self {
        Self { store }
    }

    /// Durably enqueues one entry. Idempotent on `capture_id`: an existing
    /// row is accepted as a no-op (same payload assumed — retries of the
    /// SAME capture carry the same transcript).
    pub async fn enqueue(&self, entry: InboxNewEntry) -> Result<(), InboxError> {
        if entry.capture_id.trim().is_empty() {
            return Err(InboxError::Store(EpisodicError::validation(
                "capture_id",
                "must be a non-blank id",
            )));
        }
        let db = self.store.ready().await?;
        let payload_text = mem_store::serde_json::to_string(&entry.payload).map_err(|error| {
            InboxError::Store(EpisodicError::Json {
                message: error.to_string(),
            })
        })?;
        let now = self.store.now();
        let params = vec![
            Value::Text(entry.capture_id.clone()),
            Value::Text(entry.project_id.clone()),
            Value::Text(entry.session_id.clone()),
            Value::Text(entry.agent_id.clone()),
            Value::Text(payload_text),
            Value::Integer(now),
            // The statement's own NOT EXISTS predicate repeats the id.
            Value::Text(entry.capture_id.clone()),
            Value::Integer(clamp_capacity(INBOX_CAPACITY)),
        ];
        let result = db
            .execute(sql::INSERT_CAPTURE_INBOX_CONDITIONAL, params)
            .await
            .map_err(EpisodicError::Store)?;
        if result.changes == 0 {
            // Duplicate or full — disambiguate with one read.
            let row = query_one(
                db.as_ref(),
                sql::SELECT_CAPTURE_INBOX_COUNT_FOR_ID,
                vec![Value::Text(entry.capture_id.clone())],
            )
            .await
            .map_err(EpisodicError::Store)?;
            let present = row.and_then(|row| row.i64("count")).unwrap_or(0) > 0;
            if present {
                return Ok(()); // idempotent no-op
            }
            return Err(InboxError::Full);
        }
        Ok(())
    }

    /// Dequeues up to `limit` queued entries FIFO whose retry schedule has
    /// elapsed.
    ///
    /// `now` is read HERE, from the store's one clock (D-110-2), so no call
    /// site can smuggle in a second time source; a scheduled entry is
    /// invisible until `not_before <= now`.
    pub async fn dequeue(&self, limit: usize) -> Result<Vec<InboxEntry>, EpisodicError> {
        let db = self.store.ready().await?;
        let now = self.store.now();
        let rows = db
            .query(
                sql::SELECT_CAPTURE_INBOX_BATCH,
                vec![Value::Integer(now), Value::Integer(clamp_limit(limit))],
            )
            .await
            .map_err(EpisodicError::Store)?;
        rows.iter().map(row_to_entry).collect()
    }

    /// D-48 done path: the row is purged in one `BatchMode::Write` write;
    /// the row is absent on return. Returns whether a row was removed.
    pub async fn mark_done(&self, capture_id: &str) -> Result<bool, EpisodicError> {
        let db = self.store.ready().await?;
        let results = db
            .batch(
                vec![Statement::new(
                    sql::DELETE_CAPTURE_INBOX_BY_ID,
                    vec![Value::Text(capture_id.to_string())],
                )],
                BatchMode::Write,
            )
            .await
            .map_err(EpisodicError::Store)?;
        Ok(results.first().map(|r| r.changes > 0).unwrap_or(false))
    }

    /// Consumes one attempt on a retryable (non-lock) failure; returns the
    /// new attempt count.
    pub async fn bump_attempts(&self, capture_id: &str) -> Result<u32, EpisodicError> {
        let db = self.store.ready().await?;
        db.execute(
            sql::BUMP_CAPTURE_INBOX_ATTEMPTS,
            vec![
                Value::Integer(self.store.now()),
                Value::Text(capture_id.to_string()),
            ],
        )
        .await
        .map_err(EpisodicError::Store)?;
        let row = query_one(
            db.as_ref(),
            sql::SELECT_CAPTURE_INBOX_COUNT_FOR_ID,
            vec![Value::Text(capture_id.to_string())],
        )
        .await
        .map_err(EpisodicError::Store)?;
        let _ = row; // attempts read back by the caller's dequeue, if needed
        Ok(self.attempts_of(db.as_ref(), capture_id).await)
    }

    async fn attempts_of(&self, db: &dyn Db, capture_id: &str) -> u32 {
        let row = query_one(
            db,
            "SELECT attempts FROM capture_inbox WHERE capture_id = ?",
            vec![Value::Text(capture_id.to_string())],
        )
        .await
        .ok()
        .flatten();
        row.and_then(|row| row.i64("attempts"))
            .and_then(|attempts| u32::try_from(attempts.max(0)).ok())
            .unwrap_or(0)
    }

    /// Dead-letters the entry (terminal state; stays in the table with
    /// `status = 'dead'`).
    pub async fn dead_letter(&self, capture_id: &str) -> Result<(), EpisodicError> {
        let db = self.store.ready().await?;
        db.execute(
            sql::DEADLETTER_CAPTURE_INBOX,
            vec![
                Value::Integer(self.store.now()),
                Value::Text(capture_id.to_string()),
            ],
        )
        .await
        .map_err(EpisodicError::Store)?;
        Ok(())
    }

    /// Issue 110 — schedules the next attempt: stamps
    /// `not_before = now + delay_ms` (the store's one clock) and leaves the
    /// row `queued`, consuming NO attempt. Returns the stamped deadline.
    ///
    /// An absurd hint saturates at `i64::MAX` rather than wrapping: a wrapped
    /// negative value would make the row eligible immediately, the silent
    /// opposite of the hint's intent (D-110-8). A failed write surfaces as
    /// `Err` — the caller charges the attempt instead of swallowing it.
    pub async fn schedule_retry(
        &self,
        capture_id: &str,
        delay_ms: u64,
    ) -> Result<i64, EpisodicError> {
        let db = self.store.ready().await?;
        let now = self.store.now();
        let delay = i64::try_from(delay_ms).unwrap_or(i64::MAX);
        let not_before = now.saturating_add(delay);
        db.execute(
            sql::SCHEDULE_CAPTURE_INBOX_RETRY,
            vec![
                Value::Integer(not_before),
                Value::Integer(now),
                Value::Text(capture_id.to_string()),
            ],
        )
        .await
        .map_err(EpisodicError::Store)?;
        Ok(not_before)
    }

    /// The queued-row count.
    pub async fn queued_count(&self) -> Result<u64, EpisodicError> {
        let db = self.store.ready().await?;
        let row = query_one(db.as_ref(), sql::COUNT_CAPTURE_INBOX_QUEUED, Vec::new())
            .await
            .map_err(EpisodicError::Store)?;
        Ok(row.and_then(|row| row.i64("count")).unwrap_or(0).max(0) as u64)
    }

    /// Test/diagnostic seam: the attempts counter of one row.
    pub async fn attempts(&self, capture_id: &str) -> u32 {
        match self.store.ready().await {
            Ok(db) => self.attempts_of(db.as_ref(), capture_id).await,
            Err(_) => 0,
        }
    }

    /// Issue 110 — test/diagnostic seam: the retry schedule of one row
    /// (`None` = no schedule, i.e. immediately eligible).
    pub async fn not_before(&self, capture_id: &str) -> Option<i64> {
        let db = self.store.ready().await.ok()?;
        let row = query_one(
            db.as_ref(),
            sql::SELECT_CAPTURE_INBOX_NOT_BEFORE,
            vec![Value::Text(capture_id.to_string())],
        )
        .await
        .ok()??;
        row.i64("not_before")
    }
}

fn row_to_entry(row: &mem_store::db::Row) -> Result<InboxEntry, EpisodicError> {
    let payload_text = row.text("payload").unwrap_or_default();
    let payload = mem_store::serde_json::from_str::<Json>(&payload_text).map_err(|error| {
        EpisodicError::Json {
            message: error.to_string(),
        }
    })?;
    Ok(InboxEntry {
        capture_id: row.text("capture_id").unwrap_or_default(),
        project_id: row.text("project_id").unwrap_or_default(),
        session_id: row.text("session_id").unwrap_or_default(),
        agent_id: row.text("agent_id").unwrap_or_default(),
        payload,
        created_at: row.i64("created_at").unwrap_or(0),
        attempts: u32::try_from(row.i64("attempts").unwrap_or(0).max(0)).unwrap_or(u32::MAX),
        last_attempt_at: row.i64("last_attempt_at"),
    })
}

fn clamp_capacity(capacity: usize) -> i64 {
    i64::try_from(capacity).unwrap_or(i64::MAX)
}

fn clamp_limit(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}
