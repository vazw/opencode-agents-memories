//! The PRODUCTION cross-tier task (W7.1, D1c–e): it turns the consolidator's
//! selected facts into `facts.db` rows whose `source_ref` is the milestone id,
//! with one provenance row per contributing session.
//!
//! # Wiring
//!
//! [`cross_tier_task`] chooses between the production task and
//! [`NoopCrossTierTask`](mem_episodic::cross_tier::NoopCrossTierTask) from
//! `featureFlags.promotionPipeline` (flag off, or no store) — so a flags-off
//! composition provably writes nothing. `FactsStore::new` touches no disk
//! until `ready`, so constructing it is not a write either.
//!
//! # Idempotency
//!
//! The task follows a COMMITTED consolidation batch. A retry over the same
//! milestone finds the facts present with the same content and the same
//! `source_ref`, reports them as `already_present`, and writes nothing (the
//! version is unchanged). A fact whose content changed is re-written through
//! the store's upsert.
//!
//! # The classifier
//!
//! Each candidate's tier comes from `mem_core::tier::classify_fact_tier` —
//! pure and deterministic — never from the milestone or a default.

use std::sync::Arc;

use mem_core::tier::classify_fact_tier;
use mem_embed::embedder::BoxFuture;
use mem_episodic::cross_tier::{
    CrossTierContext, CrossTierReport, CrossTierTask, NoopCrossTierTask,
};
use mem_episodic::error::EpisodicError;
use mem_episodic::promotion::PromotableFact;
use mem_episodic::record::AgentSession;
use mem_facts::FactsError;
use mem_facts::store::{FactInput, FactSource, FactWrite, FactsStore};

/// The content bound above which a candidate is skipped rather than promoted.
///
/// A fact is an atomic statement; a document-sized candidate is not one. The
/// skip is counted in [`CrossTierReport::skipped`], never silently dropped.
pub const FACTS_MAX_CONTENT_BYTES: usize = 8_192;

/// One day in milliseconds.
const DAY_MILLIS: i64 = 86_400_000;

/// The production cross-tier task over one `facts.db`.
pub struct FactsCrossTierTask {
    facts: Arc<FactsStore>,
    ttl_days: u32,
}

impl FactsCrossTierTask {
    /// The task with a TTL of `ttl_days` (rejected when `0`: a zero-day TTL
    /// would promote every agent without waiting).
    pub fn new(facts: Arc<FactsStore>, ttl_days: u32) -> Result<Self, EpisodicError> {
        if ttl_days == 0 {
            return Err(EpisodicError::validation(
                "promotion.ttlFallbackDays",
                "must be at least 1",
            ));
        }
        Ok(Self { facts, ttl_days })
    }

    /// The configured TTL in days.
    pub fn ttl_days(&self) -> u32 {
        self.ttl_days
    }

    /// The facts store this task writes to.
    pub fn facts(&self) -> &Arc<FactsStore> {
        &self.facts
    }

    /// The provenance sources of one candidate: the sessions whose `row_id`
    /// appears in its `source_row_ids`, each naming the milestone.
    fn sources_for(
        &self,
        context: &CrossTierContext<'_>,
        candidate: &PromotableFact,
    ) -> Vec<FactSource> {
        candidate
            .source_row_ids
            .iter()
            .filter_map(|row_id| {
                context
                    .sessions
                    .iter()
                    .find(|session| &session.row_id == row_id)
            })
            .map(|session| FactSource {
                project_id: context.project_id.to_string(),
                session_id: session.session_id.clone(),
                agent_id: session.agent_id.clone(),
                milestone_id: context.milestone_id.to_string(),
            })
            .collect()
    }
}

impl CrossTierTask for FactsCrossTierTask {
    /// Q7: the trigger keys on the LAST WRITE (`updated_at`), while the
    /// consolidator's eligibility keys on `created_at`. All rows must be due.
    fn promotion_due(&self, rows: &[AgentSession], now: i64) -> bool {
        let ttl_millis = i64::from(self.ttl_days).saturating_mul(DAY_MILLIS);
        let Some(cutoff) = now.checked_sub(ttl_millis) else {
            return false;
        };
        rows.iter().all(|session| session.updated_at <= cutoff)
    }

    fn run<'a>(
        &'a self,
        context: &'a CrossTierContext<'a>,
    ) -> BoxFuture<'a, Result<CrossTierReport, EpisodicError>> {
        Box::pin(async move {
            let mut report = CrossTierReport::default();
            for candidate in context.facts {
                if candidate.content.len() > FACTS_MAX_CONTENT_BYTES {
                    report.skipped += 1;
                    continue;
                }
                let tier = classify_fact_tier(&candidate.content).as_tier();
                let sources = self.sources_for(context, candidate);
                let existing = self
                    .facts
                    .get_by_key(context.project_id, tier, &candidate.key)
                    .await
                    .map_err(cross_tier_error)?;
                let unchanged = existing.as_ref().is_some_and(|fact| {
                    fact.content == candidate.content
                        && fact.source_ref.as_deref() == Some(context.milestone_id)
                });
                if unchanged {
                    report.already_present += 1;
                    continue;
                }
                let input = FactInput {
                    project_id: context.project_id.to_string(),
                    tier,
                    content: candidate.content.clone(),
                    key: Some(candidate.key.clone()),
                    source_ref: Some(context.milestone_id.to_string()),
                    confidence: None,
                    metadata: None,
                    embedding: None,
                    model_id: None,
                    sources,
                };
                match self.facts.insert(input).await.map_err(cross_tier_error)? {
                    FactWrite::Created => report.promoted += 1,
                    FactWrite::Updated => {
                        if existing.is_some() {
                            report.already_present += 1;
                        } else {
                            report.promoted += 1;
                        }
                    }
                }
            }
            Ok(report)
        })
    }
}

/// Maps a store error into the cross-tier error, never echoing content.
fn cross_tier_error(error: FactsError) -> EpisodicError {
    EpisodicError::CrossTier {
        message: error.to_string(),
    }
}

/// The flag-gated task choice: the production task only when
/// `featureFlags.promotionPipeline` is on AND a store was composed.
pub fn cross_tier_task(
    enabled: bool,
    facts: Option<Arc<FactsStore>>,
    ttl_days: u32,
) -> Result<Arc<dyn CrossTierTask>, EpisodicError> {
    // The TTL is validated regardless of the flag, so a broken config is
    // rejected at composition time instead of surfacing later.
    if ttl_days == 0 {
        return Err(EpisodicError::validation(
            "promotion.ttlFallbackDays",
            "must be at least 1",
        ));
    }
    match (enabled, facts) {
        (true, Some(facts)) => Ok(Arc::new(FactsCrossTierTask::new(facts, ttl_days)?)),
        _ => Ok(Arc::new(NoopCrossTierTask)),
    }
}
