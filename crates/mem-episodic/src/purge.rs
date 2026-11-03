//! Episodic erasure (W7.1, D6) — the ONE home of `purge_session` (M21).
//!
//! # The invariant (verbatim, ADR-0010 D-1 as amended in effect)
//!
//! Raw context is never destroyed by consolidation or by any lifecycle stage;
//! it is destroyed only by an explicit, owner-initiated purge
//! (`purge_session`).
//!
//! Consolidation's hard delete of an `agent_sessions` ROW is a row delete on
//! a derived table and never touches the retention tables — that is what the
//! no-FK v6 schema encodes. Erasure is a SEPARATE operation and is never
//! automatic: no timer, no sweep, no flag, no consolidation stage calls this
//! module (proved by the bridge's lexical source invariant).
//!
//! # What a purge erases
//!
//! The session rows of one `(project_id, session_id, agent_id)` triple, its
//! raw context and derived segments, its inbox payloads, and the contributor's
//! own retained summary text **inside** every milestone whose contributor set
//! names the triple. The milestone row itself survives — its merged summary,
//! its embedding and its window are a separate durable artifact (owner
//! residual 1, 2026-10-21) — and is named in the report's `not_erased` tier
//! `milestones-recap`. Children of the erased parent are ORPHANED (the rule
//! consolidation uses), never deleted.
//!
//! # What it deliberately does not erase
//!
//! The legacy tier-1 `memories` shard (upstream-golden SQL, deduplicated
//! across sessions with no per-session provenance) and the merged milestone
//! recap with its embedding. Every purge report names both (`not_erased`,
//! built by the orchestration layer).
//!
//! # Ordering and convergence
//!
//! The milestone pre-scan is read-only and returns only the rows that still
//! NEED a rewrite (a matching contributor with a non-empty summary), so a
//! retry converges: the second call returns all zeros. The scrub is part of
//! the ONE write batch. Every predicate is the bound triple or a pre-scanned
//! id, so a retry over a partially erased scope deletes the remainder and
//! over-deletes nothing.

use mem_store::db::{BatchMode, Statement, Value, query_one};
use mem_store::episodic_sql as sql;

use crate::error::EpisodicError;
use crate::milestone::{decode_contributors, encode_contributors};
use crate::store::EpisodicStore;
use crate::validation::{MAX_ID_LEN, check_id};

/// The erasure scope: the session's logical identity `(project_id,
/// session_id, agent_id)`.
///
/// The triple — not `(project, session)` — is the key because
/// `agent_sessions`' uniqueness and both retention tables' primary keys use
/// it: a two-argument purge would either guess an agent or erase every
/// agent's rows sharing a session id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionScope {
    /// Project scope.
    pub project_id: String,
    /// Session scope.
    pub session_id: String,
    /// Agent scope.
    pub agent_id: String,
}

impl SessionScope {
    /// Validates and trims the three components.
    ///
    /// Empty/blank/NUL-bearing components are rejected BEFORE anything is
    /// opened, locked or read. The error names the offending field and never
    /// echoes the value.
    pub fn new(project_id: &str, session_id: &str, agent_id: &str) -> Result<Self, EpisodicError> {
        Ok(Self {
            project_id: validate_part("project_id", project_id)?,
            session_id: validate_part("session_id", session_id)?,
            agent_id: validate_part("agent_id", agent_id)?,
        })
    }
}

fn validate_part(field: &'static str, value: &str) -> Result<String, EpisodicError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(EpisodicError::validation(
            field,
            "must not be empty or whitespace-only",
        ));
    }
    if trimmed.contains('\u{0}') {
        return Err(EpisodicError::validation(field, "must not contain NUL"));
    }
    Ok(trimmed.to_string())
}

/// What one episodic purge erased (counts only — never content or paths).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EpisodicPurgeReport {
    /// Session rows removed (live and soft-deleted alike).
    pub session_rows: u64,
    /// Authoritative raw-context rows removed.
    pub raw_context: u64,
    /// Derived segment rows removed.
    pub segments: u64,
    /// Inbox payload rows removed.
    pub inbox_rows: u64,
    /// Milestones **rewritten** because the triple is a contributor: the
    /// contributor's own summary text is emptied, the row survives. Never
    /// counts a deletion — the merged recap and its embedding are retained
    /// (and reported as the `milestones-recap` survivor tier).
    pub milestones_scrubbed: u64,
}

impl EpisodicPurgeReport {
    /// Whether nothing was erased (the second-call / unknown-scope shape).
    pub fn is_empty(&self) -> bool {
        self.session_rows == 0
            && self.raw_context == 0
            && self.segments == 0
            && self.inbox_rows == 0
            && self.milestones_scrubbed == 0
    }
}

/// One milestone row the purge must rewrite: the id and its already-scrubbed
/// contributors JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MilestoneScrub {
    /// The milestone row id.
    pub id: String,
    /// The re-encoded contributor list with the triple's summary emptied.
    pub contributors: String,
}

fn triple_params(scope: &SessionScope) -> Vec<Value> {
    vec![
        Value::Text(scope.project_id.clone()),
        Value::Text(scope.session_id.clone()),
        Value::Text(scope.agent_id.clone()),
    ]
}

impl EpisodicStore {
    /// The read-only milestone pre-scan: every milestone of the scope's
    /// `(project_id, agent_id)` whose decoded contributor set contains the
    /// triple.
    ///
    /// Bounded by the project's milestone count; the contributors JSON is
    /// decoded in Rust (no JSON SQL). A corrupt contributors row is skipped
    /// with the list path's fail-soft rule — it cannot name the triple, so it
    /// is not the triple's milestone.
    pub async fn milestone_ids_for_session(
        &self,
        scope: &SessionScope,
    ) -> Result<Vec<String>, EpisodicError> {
        let db = self.ready().await?;
        let rows = db
            .query(
                sql::LIST_MILESTONES_FOR_PURGE,
                vec![
                    Value::Text(scope.project_id.clone()),
                    Value::Text(scope.agent_id.clone()),
                ],
            )
            .await?;
        let mut ids = Vec::new();
        for row in &rows {
            let Some(id) = row.text("id") else {
                continue;
            };
            let contributors_json = row.text("contributors").unwrap_or_default();
            let Ok(contributors) = decode_contributors(&contributors_json) else {
                continue;
            };
            if contributors.iter().any(|contributor| {
                contributor.session_id == scope.session_id && contributor.agent_id == scope.agent_id
            }) {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    /// The erasure pre-scan: every milestone of the scope's `(project_id,
    /// agent_id)` that STILL needs a rewrite — a contributor matching the
    /// triple whose summary text is non-empty — returned with the re-encoded
    /// contributor list.
    ///
    /// Convergence (D4): a row whose matches are already empty is skipped, so
    /// a second purge writes nothing and the report claims no erasure on a
    /// retry.
    ///
    /// Fail-closed (D7): an undecodable `contributors` row REFUSES the purge.
    /// A corrupt row may or may not name the triple, so skipping it would let
    /// the report claim an erasure the code cannot verify. (The read-only
    /// [`Self::milestone_ids_for_session`] keeps the list path's fail-soft
    /// rule — it answers "who names the triple", not "erase this".)
    pub async fn milestone_scrubs_for_session(
        &self,
        scope: &SessionScope,
    ) -> Result<Vec<MilestoneScrub>, EpisodicError> {
        let db = self.ready().await?;
        let rows = db
            .query(
                sql::LIST_MILESTONES_FOR_PURGE,
                vec![
                    Value::Text(scope.project_id.clone()),
                    Value::Text(scope.agent_id.clone()),
                ],
            )
            .await?;
        let mut scrubs = Vec::new();
        for row in &rows {
            let Some(id) = row.text("id") else {
                continue;
            };
            let contributors_json = row.text("contributors").unwrap_or_default();
            let mut contributors = decode_contributors(&contributors_json)?;
            let mut matched = false;
            let mut needs_rewrite = false;
            for contributor in contributors.iter_mut() {
                if contributor.session_id == scope.session_id
                    && contributor.agent_id == scope.agent_id
                {
                    matched = true;
                    if !contributor.summary.is_empty() {
                        contributor.summary = String::new();
                        needs_rewrite = true;
                    }
                }
            }
            if !matched || !needs_rewrite {
                continue;
            }
            scrubs.push(MilestoneScrub {
                id,
                contributors: encode_contributors(&contributors)?,
            });
        }
        Ok(scrubs)
    }

    /// Erases exactly one scope in ONE write batch.
    ///
    /// Order inside the batch: orphan the children of every session row,
    /// then delete the session rows, the raw context, the segments, the
    /// inbox rows, and finally scrub the contributor's own summary text in
    /// every pre-scanned milestone (the row survives — see
    /// [`MilestoneScrub`]). Counts come from each statement's `changes`; a
    /// second call returns all zeros (every statement is a `DELETE`/`UPDATE …
    /// WHERE`, and zero changed rows is success).
    pub async fn purge_session(
        &self,
        scope: &SessionScope,
    ) -> Result<EpisodicPurgeReport, EpisodicError> {
        let scrubs = self.milestone_scrubs_for_session(scope).await?;
        let db = self.ready().await?;
        let row_ids: Vec<String> = db
            .query(sql::SELECT_SESSION_IDS_BY_TRIPLE, triple_params(scope))
            .await?
            .iter()
            .filter_map(|row| row.text("id"))
            .collect();

        let now = self.now();
        let mut statements = Vec::new();
        for row_id in &row_ids {
            statements.push(Statement::new(
                sql::MARK_CHILDREN_ORPHANED,
                vec![Value::Integer(now), Value::Text(row_id.clone())],
            ));
        }
        statements.push(Statement::new(
            sql::PURGE_SESSIONS_BY_TRIPLE,
            triple_params(scope),
        ));
        statements.push(Statement::new(
            sql::PURGE_RAW_CONTEXT_BY_TRIPLE,
            triple_params(scope),
        ));
        statements.push(Statement::new(
            sql::PURGE_SEGMENTS_BY_TRIPLE,
            triple_params(scope),
        ));
        statements.push(Statement::new(
            sql::PURGE_INBOX_BY_TRIPLE,
            triple_params(scope),
        ));
        for scrub in &scrubs {
            statements.push(Statement::new(
                sql::SCRUB_MILESTONE_CONTRIBUTORS,
                vec![
                    Value::Text(scrub.contributors.clone()),
                    Value::Text(scrub.id.clone()),
                ],
            ));
        }

        let results = db.batch(statements, BatchMode::Write).await?;
        // Layout: [orphan marks × row_ids, sessions, raw, segments, inbox,
        // milestone scrubs × n].
        let offset = row_ids.len();
        let mut report = EpisodicPurgeReport::default();
        let changes_at = |index: usize| results.get(index).map(|r| r.changes).unwrap_or(0);
        report.session_rows = changes_at(offset);
        report.raw_context = changes_at(offset + 1);
        report.segments = changes_at(offset + 2);
        report.inbox_rows = changes_at(offset + 3);
        for index in 0..scrubs.len() {
            report.milestones_scrubbed += changes_at(offset + 4 + index);
        }
        Ok(report)
    }

    /// Hard-deletes one VISIBLE session row and erases its whole triple.
    ///
    /// The user-facing delete (issue 32): the row (text + embedding), its raw
    /// context, its derived segments, its inbox payloads and the contributor's
    /// own milestone summary text. Children are ORPHANED, never deleted.
    ///
    /// - `Ok(false)` for every invisible row — unknown, wrong project or
    ///   already tombstoned — so REST keeps ONE answer for all three
    ///   (no existence leak, D9). Legacy tombstones are cleared by the CLI
    ///   purge, not from here.
    /// - `Err(EpisodicError::Busy)` while another consolidation or purge holds
    ///   the in-process gate. The guard is held across the visibility read AND
    ///   the purge, so no consolidation can mint a milestone between the
    ///   pre-scan and the scrub (which would let a contributor's summary
    ///   survive the delete).
    ///
    /// This module is the allowlisted home: `purge_session` is defined here,
    /// so the bridge's lexical barrier (`purge` is never reachable from
    /// capture or consolidation) stays green.
    pub async fn hard_delete_session(
        &self,
        project_id: &str,
        id: &str,
    ) -> Result<bool, EpisodicError> {
        check_id("project_id", project_id, MAX_ID_LEN)?;
        check_id("id", id, MAX_ID_LEN)?;
        let Some(_gate) = crate::gate::try_acquire(self.storage_path()) else {
            return Err(EpisodicError::Busy);
        };
        let db = self.ready().await?;
        let Some(row) = query_one(
            db.as_ref(),
            sql::SELECT_SESSION_SCOPE_BY_ID,
            vec![Value::Text(id.to_string())],
        )
        .await?
        else {
            return Ok(false);
        };
        if row.text("project_id").as_deref() != Some(project_id) || !row.is_null("deleted_at") {
            return Ok(false);
        }
        let Some(session_id) = row.text("session_id") else {
            return Ok(false);
        };
        let Some(agent_id) = row.text("agent_id") else {
            return Ok(false);
        };
        let scope = SessionScope::new(project_id, &session_id, &agent_id)?;
        Ok(self.purge_session(&scope).await?.session_rows > 0)
    }
}
