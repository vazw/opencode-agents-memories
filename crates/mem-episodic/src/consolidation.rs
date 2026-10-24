//! Episodic consolidation (W4.3) — the per-agent pipeline and the run-level
//! guarantees.
//!
//! # The per-agent pipeline (normative)
//!
//! For each eligible agent, in ascending `agent_id` order, the consolidator
//! runs EXACTLY these phases:
//!
//! - **(a) claim** — read-only; a store error aborts the run;
//! - **(b) transcript + prefix** — the agent consolidates only the FIRST `k`
//!   claimed rows (`rows[..k]`); `k == 0` is a per-agent skip;
//! - **(c) summarize + validate** — a summarizer error, an empty summary and
//!   an oversize summary are per-agent skips; the sink is NOT called before
//!   this phase succeeds;
//! - **(d) embeddings + dimension checks, then the contributors JSON** — a
//!   dimension mismatch (milestone or promotion embedding) and an over-bound
//!   contributors JSON are per-agent skips; nothing is written for that
//!   agent;
//! - **(e) existence-checked promotions** — a sink error ABORTS the run;
//! - **(f) ONE episodic write batch** — the milestone INSERT, the orphan
//!   marks and the scoped DELETEs ride in ONE atomic batch; a batch error
//!   ABORTS the run.
//!
//! Phases (a)–(d) perform ZERO writes when they fail.
//!
//! # The skip/abort split (D-19)
//!
//! Per-agent skips: summarizer error, empty summary, oversize summary, zero
//! fitting transcript rows, dimension mismatch, metadata decode failure and
//! contributors-JSON-over-bound. Run aborts: store errors, sink errors and
//! batch errors.

use std::sync::Arc;

use mem_embed::embedder::{BoxFuture, EmbedTask, Embedder};
use mem_embed::prefix::{PrefixOptions, apply_task_prefix};
use mem_store::db::{ExecResult, Statement, Value};
use mem_store::episodic_sql as sql;
use mem_store::platform::ProcessProbe;
use mem_store::vector::vector_to_text;

use crate::error::EpisodicError;
use crate::milestone::{encode_contributors, max_contributors_bytes, plan_milestone};
use crate::promotion::{
    PROMOTED_MEMORY_TYPE, PermanentMemorySink, PromoteOutcome, PromotionRecord,
    select_promotable_facts,
};
use crate::store::EpisodicStore;
use crate::summarizer::{
    MAX_TRANSCRIPT_BYTES, MilestoneSummarizer, build_transcript, fit_prefix,
    stripped_source_summary,
};
use crate::validation::MAX_SUMMARY_BYTES;

/// One day in milliseconds.
const DAY_MILLIS: i64 = 86_400_000;

/// The default number of claimed rows per agent per run.
pub const DEFAULT_BATCH_LIMIT: usize = 200;

/// The age policy: how old a session must be before it is consolidated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsolidationPolicy {
    age_days: u32,
}

impl ConsolidationPolicy {
    /// The policy for `age_days` days.
    ///
    /// Zero is rejected: a zero-day window would consolidate everything,
    /// including the session that is being written right now.
    pub fn new(age_days: u32) -> Result<Self, EpisodicError> {
        if age_days == 0 {
            return Err(EpisodicError::validation(
                "consolidation_age_days",
                "must be at least 1",
            ));
        }
        Ok(Self { age_days })
    }

    /// The policy from the episodic config section.
    pub fn from_episodic(episodic: &mem_core::config::Episodic) -> Result<Self, EpisodicError> {
        Self::new(episodic.consolidation_age_days)
    }

    /// The configured age window in days.
    pub fn age_days(&self) -> u32 {
        self.age_days
    }

    /// The cutoff stamp: sessions created strictly before it are eligible.
    ///
    /// Checked arithmetic — an overflow is a validation error, never a
    /// wrapped cutoff.
    pub fn cutoff(&self, now: i64) -> Result<i64, EpisodicError> {
        let window = i64::from(self.age_days)
            .checked_mul(DAY_MILLIS)
            .ok_or_else(|| {
                EpisodicError::validation("consolidation_age_days", "age window overflows")
            })?;
        now.checked_sub(window).ok_or_else(|| {
            EpisodicError::validation("consolidation_age_days", "age window overflows")
        })
    }
}

/// The caller's promotion scope.
///
/// The scope NEVER comes from the episodic `project_id`: the container tag
/// and the scope hash are supplied by the caller (W5.3 owns that mapping).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotionScope {
    /// The container tag the promoted rows carry.
    pub container_tag: String,
    /// The 16-hex scope hash the promoted rows land under.
    pub scope_hash: String,
}

/// Why an agent was skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The summarizer failed.
    SummarizerError,
    /// The summary was empty after stripping.
    EmptySummary,
    /// The summary exceeded `MAX_SUMMARY_BYTES`.
    OversizeSummary,
    /// Not even one session row fits the transcript cap.
    NoFittingTranscriptRows,
    /// An embedding did not match the store's dimensions.
    DimensionMismatch,
    /// A claimed row's metadata could not be decoded.
    MetadataDecodeError,
    /// The assembled contributors JSON exceeded its bound.
    ContributorsTooLarge,
    /// The agent's last write is younger than `promotion.ttlFallbackDays`;
    /// nothing was written or deleted (Q7).
    PromotionNotDue,
}

/// One skipped agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedAgent {
    /// The skipped agent.
    pub agent_id: String,
    /// Why it was skipped.
    pub reason: SkipReason,
}

/// What one consolidation run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsolidationReport {
    /// The run was skipped because another run holds the locks.
    pub busy: bool,
    /// How many agents were considered (the eligible agents).
    pub agents_considered: usize,
    /// The agents that were skipped, with the reason.
    pub agents_skipped: Vec<SkippedAgent>,
    /// How many milestone rows were written.
    pub milestones_written: u64,
    /// How many source rows were removed.
    pub sessions_consolidated: u64,
    /// How many facts were newly promoted.
    pub facts_promoted: u64,
    /// How many facts were already present.
    pub facts_already_present: u64,
    /// The cross-tier phase's report (all three counters).
    pub cross_tier: crate::cross_tier::CrossTierReport,
}

impl ConsolidationReport {
    /// The report of a busy run: nothing was read, written or promoted.
    pub fn busy() -> Self {
        Self {
            busy: true,
            ..Default::default()
        }
    }
}

/// The episodic write-batch port (the seam that makes "zero batch writes"
/// observable).
pub trait EpisodicBatchWriter: Send + Sync {
    /// Executes one atomic write batch.
    fn write_batch<'a>(
        &'a self,
        statements: Vec<Statement>,
    ) -> BoxFuture<'a, Result<Vec<ExecResult>, EpisodicError>>;
}

/// The production batch writer: the store's own atomic batch seam.
pub struct StoreBatchWriter {
    store: Arc<EpisodicStore>,
}

impl StoreBatchWriter {
    /// The writer over `store`.
    pub fn new(store: Arc<EpisodicStore>) -> Self {
        Self { store }
    }
}

impl EpisodicBatchWriter for StoreBatchWriter {
    fn write_batch<'a>(
        &'a self,
        statements: Vec<Statement>,
    ) -> BoxFuture<'a, Result<Vec<ExecResult>, EpisodicError>> {
        Box::pin(async move { self.store.write_batch(statements).await })
    }
}

/// Everything the consolidator needs besides the store.
pub struct ConsolidationDeps {
    /// The milestone summarizer.
    pub summarizer: Arc<dyn MilestoneSummarizer>,
    /// The embedder.
    pub embedder: Arc<dyn Embedder>,
    /// The permanent-memory sink.
    pub sink: Arc<dyn PermanentMemorySink>,
    /// The episodic batch writer.
    pub batch_writer: Arc<dyn EpisodicBatchWriter>,
    /// The embedding prefix options.
    pub prefix_options: PrefixOptions,
    /// How many rows may be claimed per agent per run.
    pub batch_limit: usize,
    /// The process probe for the cross-process operation lock.
    pub probe: Arc<dyn ProcessProbe>,
    /// The cross-tier promotion phase, called AFTER the durable batch.
    ///
    /// `NoopCrossTierTask` when `featureFlags.promotionPipeline` is off; the
    /// production `FactsCrossTierTask` in `mem-bridge` otherwise.
    pub cross_tier: Arc<dyn crate::cross_tier::CrossTierTask>,
}

/// The consolidator.
pub struct Consolidator {
    store: Arc<EpisodicStore>,
    policy: ConsolidationPolicy,
    deps: ConsolidationDeps,
}

impl Consolidator {
    /// The consolidator over `store`.
    pub fn new(
        store: Arc<EpisodicStore>,
        policy: ConsolidationPolicy,
        deps: ConsolidationDeps,
    ) -> Self {
        Self {
            store,
            policy,
            deps,
        }
    }

    /// Runs one consolidation pass over `project_id`.
    ///
    /// The promoted facts land under the CALLER's `scope`; the consolidator
    /// never derives a scope from the episodic `project_id`.
    pub async fn run(
        &self,
        project_id: &str,
        scope: &PromotionScope,
    ) -> Result<ConsolidationReport, EpisodicError> {
        // A zero claim limit is REJECTED, never coerced (the house rule:
        // reject, never coerce). A silent no-op would count the agent as
        // "considered" while neither consolidating nor skipping it.
        if self.deps.batch_limit == 0 {
            return Err(EpisodicError::validation(
                "batch_limit",
                "must be at least 1",
            ));
        }

        // BOTH locks are acquired BEFORE the first read of the run.
        //
        // The in-process gate (D-24) is process-wide and std-only; the
        // cross-process operation lock is mem-store's job.
        let Some(_gate) = crate::gate::try_acquire(self.store.storage_path()) else {
            return Ok(ConsolidationReport::busy());
        };
        let lock = match mem_store::operation_lock::acquire_turso_operation_lock(
            self.store.storage_path(),
            self.deps.probe.clone(),
            "episodic-consolidation",
        ) {
            Ok(lock) => lock,
            // Busy is NOT an error: a live lock holder means "another run is
            // working on this storage root".
            Err(mem_store::error::StoreError::MigrationLocked { .. }) => {
                return Ok(ConsolidationReport::busy());
            }
            Err(mem_store::error::StoreError::Io {
                kind: Some(std::io::ErrorKind::AlreadyExists),
                ..
            }) => {
                return Ok(ConsolidationReport::busy());
            }
            // Every other lock error aborts the run.
            Err(error) => return Err(EpisodicError::Store(error)),
        };

        let result = self.run_locked(project_id, scope).await;
        // The operation lock is released explicitly (Drop is the fallback).
        lock.release();
        result
    }

    /// The consolidation pass with both locks held.
    async fn run_locked(
        &self,
        project_id: &str,
        scope: &PromotionScope,
    ) -> Result<ConsolidationReport, EpisodicError> {
        let mut report = ConsolidationReport::default();
        let now = self.store.now();
        let cutoff = self.policy.cutoff(now)?;

        // (a) claim — the eligible agents, read-only.
        let agents = self.store.eligible_agents(project_id, cutoff).await?;
        report.agents_considered = agents.len();

        for agent_id in agents {
            self.consolidate_agent(project_id, &agent_id, cutoff, scope, now, &mut report)
                .await?;
        }
        Ok(report)
    }

    /// Consolidates one agent (phases (b)–(f)).
    async fn consolidate_agent(
        &self,
        project_id: &str,
        agent_id: &str,
        cutoff: i64,
        scope: &PromotionScope,
        now: i64,
        report: &mut ConsolidationReport,
    ) -> Result<(), EpisodicError> {
        // (a) claim. A metadata decode failure skips the agent ENTIRELY.
        let rows = match self
            .store
            .claim_sessions(project_id, agent_id, cutoff, self.deps.batch_limit)
            .await
        {
            Ok(rows) => rows,
            Err(EpisodicError::Json { .. }) => {
                skip(report, agent_id, SkipReason::MetadataDecodeError);
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if rows.is_empty() {
            return Ok(());
        }

        // (b) transcript + prefix: only the first k rows are consolidated.
        let chunks = build_transcript(&rows);
        let k = fit_prefix(&chunks, MAX_TRANSCRIPT_BYTES);
        if k == 0 {
            skip(report, agent_id, SkipReason::NoFittingTranscriptRows);
            return Ok(());
        }
        let consolidated = &rows[..k];

        // (c) summarize + validate. The sink is NOT called before this
        // phase succeeds.
        let transcript = chunks[..k].join("\n");
        let raw = match self.deps.summarizer.summarize(&transcript).await {
            Ok(raw) => raw,
            Err(_) => {
                skip(report, agent_id, SkipReason::SummarizerError);
                return Ok(());
            }
        };
        let summary = stripped_source_summary(&raw);
        if summary.is_empty() {
            skip(report, agent_id, SkipReason::EmptySummary);
            return Ok(());
        }
        if summary.len() > MAX_SUMMARY_BYTES {
            skip(report, agent_id, SkipReason::OversizeSummary);
            return Ok(());
        }

        // (d) embeddings + dimension checks.
        let milestone_embedding = self.embed(&summary).await?;
        if milestone_embedding.len() != self.store.dimensions() as usize {
            skip(report, agent_id, SkipReason::DimensionMismatch);
            return Ok(());
        }
        // The selection sees the CONSOLIDATED rows only.
        let facts = select_promotable_facts(project_id, consolidated);
        let mut fact_vectors: Vec<Vec<f32>> = Vec::with_capacity(facts.len());
        for fact in &facts {
            let vector = self.embed(&fact.content).await?;
            if vector.len() != self.store.dimensions() as usize {
                skip(report, agent_id, SkipReason::DimensionMismatch);
                return Ok(());
            }
            fact_vectors.push(vector);
        }

        // The milestone id is minted by the STORE's id path (UUIDv7).
        let db = self.store.ready().await?;
        let milestone_id = self.store.mint_id(db.as_ref()).await?;
        let milestone = plan_milestone(
            milestone_id.as_str(),
            consolidated,
            &summary,
            milestone_embedding,
            now,
        );

        // The contributors JSON is assembled and its bound is checked BEFORE
        // the first promotion.
        let contributors_json = encode_contributors(&milestone.contributors)?;
        if contributors_json.len() > max_contributors_bytes(milestone.contributors.len()) {
            skip(report, agent_id, SkipReason::ContributorsTooLarge);
            return Ok(());
        }

        // W7.1/Q7 — the pure due-gate runs BEFORE the promotion phase and
        // BEFORE the write batch: a not-due agent writes nothing and deletes
        // nothing, so its session rows stay claimed for a later run.
        if !self.deps.cross_tier.promotion_due(consolidated, now) {
            skip(report, agent_id, SkipReason::PromotionNotDue);
            return Ok(());
        }

        // (e) existence-checked promotions.
        for (fact, vector) in facts.iter().zip(fact_vectors.iter()) {
            let record = PromotionRecord {
                id: fact.id.clone(),
                content: fact.content.clone(),
                vector: vector.clone(),
                container_tag: scope.container_tag.clone(),
                memory_type: PROMOTED_MEMORY_TYPE.to_string(),
                created_at: now,
                source_row_ids: fact.source_row_ids.clone(),
            };
            match self.deps.sink.promote(&scope.scope_hash, &record).await {
                Ok(PromoteOutcome::Inserted) => report.facts_promoted += 1,
                Ok(PromoteOutcome::AlreadyPresent) => report.facts_already_present += 1,
                // A sink error aborts the whole run.
                Err(error) => return Err(error),
            }
        }

        // (f) ONE episodic write batch.
        // The milestone vector is serialized here, so the batch builder itself stays infallible (ADR-0013 D-3).
        let milestone_vector = vector_to_text(&milestone.embedding)?;
        let statements = self.batch_statements(
            project_id,
            agent_id,
            &milestone,
            &milestone_vector,
            &contributors_json,
            consolidated,
            now,
        );
        let results = self.deps.batch_writer.write_batch(statements).await?;
        report.milestones_written += 1;
        report.sessions_consolidated += deleted_rows(&results, consolidated.len());

        // W7.1 — the cross-tier phase runs AFTER the durable batch, so the
        // milestone it names is already queryable. Its error propagates (the
        // batch stays committed; a retry recollects the same ids).
        let sessions: Vec<crate::cross_tier::SessionRef> = consolidated
            .iter()
            .map(|row| crate::cross_tier::SessionRef {
                row_id: row.id.clone(),
                session_id: row.session_id.clone(),
                agent_id: row.agent_id.clone(),
            })
            .collect();
        let context = crate::cross_tier::CrossTierContext {
            project_id,
            agent_id,
            milestone_id: &milestone.id,
            sessions: &sessions,
            facts: &facts,
            now,
        };
        let cross_tier = self.deps.cross_tier.run(&context).await?;
        report.cross_tier = cross_tier;
        Ok(())
    }

    /// Embeds `text` with the configured document prefix.
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EpisodicError> {
        let prefixed = apply_task_prefix(text, EmbedTask::Document, &self.deps.prefix_options);
        Ok(self.deps.embedder.embed(&prefixed).await?)
    }

    /// The ONE atomic batch: the milestone INSERT, then per consolidated row
    /// the orphan mark and the scoped DELETE.
    // Eight parameters: the milestone vector is threaded as a prepared value
    // (the caller owns serialization, keeping this function infallible) and
    // grouping the rest would only relocate the same argument list.
    #[allow(clippy::too_many_arguments)]
    fn batch_statements(
        &self,
        project_id: &str,
        agent_id: &str,
        milestone: &crate::milestone::Milestone,
        milestone_vector: &str,
        contributors_json: &str,
        consolidated: &[crate::record::AgentSession],
        now: i64,
    ) -> Vec<Statement> {
        let mut statements = vec![Statement::new(
            sql::INSERT_MILESTONE,
            vec![
                Value::Text(milestone.id.clone()),
                Value::Text(milestone.project_id.clone()),
                Value::Text(milestone.agent_id.clone()),
                Value::Text(milestone.summary.clone()),
                Value::Text(contributors_json.to_string()),
                Value::Integer(milestone.source_count),
                Value::Integer(milestone.window_start),
                Value::Integer(milestone.window_end),
                Value::Text(milestone_vector.to_string()),
                Value::Integer(milestone.created_at),
            ],
        )];
        for row in consolidated {
            statements.push(Statement::new(
                sql::MARK_CHILDREN_ORPHANED,
                vec![Value::Integer(now), Value::Text(row.id.clone())],
            ));
            statements.push(Statement::new(
                sql::DELETE_SESSION_SCOPED,
                vec![
                    Value::Text(row.id.clone()),
                    Value::Text(project_id.to_string()),
                    Value::Text(agent_id.to_string()),
                ],
            ));
        }
        statements
    }
}

/// Records a per-agent skip.
fn skip(report: &mut ConsolidationReport, agent_id: &str, reason: SkipReason) {
    report.agents_skipped.push(SkippedAgent {
        agent_id: agent_id.to_string(),
        reason,
    });
}

/// The number of rows the batch actually removed.
///
/// The batch layout is `[INSERT, MARK_0, DELETE_0, MARK_1, DELETE_1, …]`,
/// so the DELETE results are the even indices from 2 onwards. A row that was
/// soft-deleted between the claim and the batch reports `changes = 0` — it
/// is not an error, and it is not counted.
fn deleted_rows(results: &[ExecResult], consolidated_rows: usize) -> u64 {
    let mut removed = 0u64;
    for index in 0..consolidated_rows {
        let result_index = 2 + index * 2;
        if let Some(result) = results.get(result_index) {
            removed += result.changes;
        }
    }
    removed
}
