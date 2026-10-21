//! W5.3 Step 9 — the injection assembly: the one port-owned
//! `<project_agent_session_history>` block with budget, blocker-first
//! ordering, dedup and the capture-summary/promoted-fact tier distinction.
//!
//! Policy anchors:
//! - **A2 / absent-param policy:** a missing or blank `project_id` returns
//!   `block: None` WITHOUT any shard query and without any scope
//!   derivation (never `-32602`, never an empty tag — the empty tag
//!   selects ALL memories in `list_memories_by_container_tag`).
//! - **D-W5.3-2:** exactly ONE port-owned history block.
//! - **D-W5.3-3:** `token_budget` maps to CHECKED `4 × tokens` bytes; the
//!   `tokens` result figure is a byte-based estimate (`used / 4`).
//! - Tier-1 read leg: `get_all_shards` → `open_shard` →
//!   `list_memories_by_container_tag` — never `get_write_shard`.
//! - A live consolidation (shared `consolidation_active` flag or a live
//!   operation/legacy lock) skips ONLY the tier-1 leg; the episodic block
//!   still renders with the reduced `memory_count` — inject never waits.
//! - Privacy: `<private>` content is stripped before rendering.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::time::Instant;

use mem_core::scope::Scope;
use mem_embed::embedder::Embedder;
use mem_episodic::record::AgentSession;
use mem_episodic::store::EpisodicStore;
use mem_store::db::Row;
use mem_store::operation_lock::{LEGACY_MIGRATION_LOCK_FILE, OPERATION_LOCK_FILE, read_live_lock};
use mem_store::shard_manager::ShardManager;
use mem_store::vector::list_memories_by_container_tag;

use crate::capture::{AUTO_CAPTURE_SOURCE, CAPTURE_SUMMARY_SOURCE_METADATA_KEY};
use crate::project_scope::{ProjectScopeError, ResolvedProjectScope};
use crate::scope_resolver::ProjectScopeResolver;

/// The default token budget (bridge-schema `inject`).
pub const DEFAULT_TOKEN_BUDGET: u64 = 2_000;

/// Recent sessions shown by default.
pub const DEFAULT_RECENT_COUNT: u32 = 5;

/// The tier-1 subsection byte cap: one quarter of the total budget.
pub const TIER1_BUDGET_SHARE: usize = 4;

/// Budget constants for one candidate's required overhead (newline + a
/// conservative 2-byte margin). The block emits `line + "\n"` only (no
/// bullet), so the charge exceeds the emitted bytes and makes the budget
/// invariant provable.
const BULLET_BYTES: usize = 3;

/// The largest tier-1 metadata document the injector will parse. A bigger
/// blob is treated as malformed (skipped with a bounded warning).
const MAX_INJECT_METADATA_BYTES: usize = 8 * 1024;

/// The block headings (frozen order).
pub const HISTORY_BLOCK_OPEN: &str = "<project_agent_session_history>";
pub const HISTORY_BLOCK_CLOSE: &str = "</project_agent_session_history>";
pub const HEADING_FACTS: &str = "## Project facts";
pub const HEADING_SUMMARIES: &str = "## Captured session summaries";
pub const HEADING_RECENT: &str = "## Recent sessions";
pub const HEADING_OLDER: &str = "## Related older sessions";

/// Capped tier-1 read per shard (a generous one-read bound).
const TIER1_READ_LIMIT: usize = 200;

/// Why assembly stopped early (input errors only; busy legs are skips).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InjectError {
    /// The token→byte multiplication or the limit overflowed.
    Overflow,
    /// The project scope derivation failed (ONLY for present non-blank
    /// values; a blank/absent scope never reaches this).
    Scope(ProjectScopeError),
    /// An episodic read failure.
    Store(String),
}

impl std::fmt::Display for InjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Overflow => write!(f, "token budget multiplication overflow"),
            Self::Scope(error) => write!(f, "{error}"),
            Self::Store(message) => write!(f, "inject read failed: {message}"),
        }
    }
}

impl std::error::Error for InjectError {}

/// The parsed `inject` params.
#[derive(Debug, Clone, Default, PartialEq, mem_store::serde::Deserialize)]
#[serde(crate = "mem_store::serde", default)]
pub struct InjectParams {
    pub project_id: Option<String>,
    pub agent_id: Option<String>,
    pub token_budget: Option<u64>,
    /// Either the frozen `["summary","metadata"]` array form or the
    /// additive options object `{recent_count?, query?}`.
    pub include: Option<mem_store::serde_json::Value>,
}

/// The result body (frozen response schema).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InjectResult {
    pub block: Option<String>,
    pub memory_count: u64,
    pub tokens: u64,
    pub truncated: bool,
    pub handling_ms: u64,
}

/// One candidate row for the greedy budget append.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    /// The included logical row id (the dedup key).
    id: String,
    /// The rendered one-line item.
    line: String,
    /// Whether the row is a blocker (open threads) — blockers outrank.
    blocker: bool,
    /// Which section heading it renders under.
    heading: &'static str,
    /// The producing session, when the row is session-linked (episodic rows
    /// and capture summaries). Drives the accepted-set duplicate suppression.
    session_id: Option<String>,
}

/// The tier-1 read result (a busy/busy-locked read collapses to `Skipped`).
enum Tier1Leg {
    Rows(Vec<Row>),
    Skipped,
}

pub struct InjectService {
    store: Arc<EpisodicStore>,
    shards: Arc<ShardManager>,
    scope_resolver: Arc<dyn ProjectScopeResolver>,
    /// The engine's consolidation gate (RAII set around `Consolidator::run`).
    consolidation_active: Arc<AtomicBool>,
    embedder: Arc<dyn Embedder>,
    probe: Arc<dyn mem_store::platform::ProcessProbe>,
}

impl InjectService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Arc<EpisodicStore>,
        shards: Arc<ShardManager>,
        scope_resolver: Arc<dyn ProjectScopeResolver>,
        consolidation_active: Arc<AtomicBool>,
        embedder: Arc<dyn Embedder>,
        probe: Arc<dyn mem_store::platform::ProcessProbe>,
    ) -> Self {
        Self {
            store,
            shards,
            scope_resolver,
            consolidation_active,
            embedder,
            probe,
        }
    }

    /// Assembles the injection block.
    pub async fn assemble(&self, params: &InjectParams) -> Result<InjectResult, InjectError> {
        let started = Instant::now();

        // 0. A2: absent/blank scope → empty result. No derivation and no
        // shard query (never an empty tag).
        let Some(scope_input) = params
            .project_id
            .as_deref()
            .filter(|p| !p.trim().is_empty())
        else {
            return Ok(empty(started));
        };

        let budget_bytes = byte_budget(params.token_budget)?;
        let (recent_count, query_text) = include_options(&params.include);
        let project_id: &str = scope_input;

        // 1. The ONE scope resolution (capture used the same triple).
        let scope = self
            .scope_resolver
            .resolve(scope_input)
            .map_err(InjectError::Scope)?;

        // 2. Recent sessions: 2 × N rows, blockers first with stable
        // recency inside the groups.
        let recent_raw = self.read_recent(project_id, recent_count).await?;
        let mut seen: HashSet<String> = HashSet::new();
        let mut recent: Vec<&AgentSession> = recent_raw.iter().collect();
        recent.sort_by(|a, b| {
            let ab = !a.metadata.open_threads.is_empty();
            let bb = !b.metadata.open_threads.is_empty();
            bb.cmp(&ab).then(b.created_at.cmp(&a.created_at))
        });

        let mut candidates: Vec<Candidate> = Vec::new();
        for row in &recent {
            seen.insert(row.id.clone());
            candidates.push(Candidate {
                id: row.id.clone(),
                line: recent_line(row),
                blocker: !row.metadata.open_threads.is_empty(),
                heading: HEADING_RECENT,
                session_id: Some(row.session_id.clone()),
            });
        }

        // 3. Related older sessions (semantic when a query exists),
        // blockers before similarity, recent ids excluded.
        if let Some(query_text) = &query_text
            && let Ok(query_vector) = self.embedder.embed(query_text).await
        {
            let hits = self
                .store
                .search_sessions(
                    project_id,
                    &query_vector,
                    usize::try_from(recent_count.saturating_mul(2)).unwrap_or(usize::MAX),
                )
                .await
                .map_err(store_error)?;
            for hit in &hits {
                if seen.insert(hit.session.id.clone()) {
                    candidates.push(Candidate {
                        id: hit.session.id.clone(),
                        line: recent_line(&hit.session),
                        blocker: !hit.session.metadata.open_threads.is_empty(),
                        heading: HEADING_OLDER,
                        session_id: Some(hit.session.session_id.clone()),
                    });
                }
            }
        }

        // 4. The tier-1 leg — skipped entirely on a live consolidation.
        let tier1 = if self.tier1_leg_available() {
            match self.read_tier1(&scope).await {
                Ok(leg) => leg,
                // Any busy/locked/store error skips ONLY this leg.
                Err(_) => Tier1Leg::Skipped,
            }
        } else {
            Tier1Leg::Skipped
        };
        if let Tier1Leg::Rows(rows) = tier1 {
            let mut facts_section: Vec<Candidate> = Vec::new();
            let mut summaries_section: Vec<Candidate> = Vec::new();
            let mut skipped_metadata = 0u64;
            for row in rows {
                let Some(id) = row.text("id") else {
                    continue;
                };
                if !seen.insert(id.clone()) {
                    continue;
                }
                let metadata_text = row.text("metadata");
                let Some(class) = classify_row(metadata_text.as_deref()) else {
                    // Malformed/oversized metadata: skip with a bounded
                    // warning (never the metadata text, never a fake fact).
                    skipped_metadata += 1;
                    continue;
                };
                match class {
                    FactOrSummary::Fact => facts_section.push(Candidate {
                        id,
                        line: san(&row.text("content").unwrap_or_default()),
                        blocker: false,
                        heading: HEADING_FACTS,
                        session_id: None,
                    }),
                    FactOrSummary::Summary => {
                        // The duplicate suppression is applied AFTER the
                        // budget pass, against the ACTUALLY INCLUDED episodic
                        // rows: a budget-dropped episode row must not suppress
                        // its capture summary (neither would render).
                        let session =
                            mem_store::vector::resolve_session_id(None, metadata_text.as_deref());
                        summaries_section.push(Candidate {
                            id,
                            line: san(&row.text("content").unwrap_or_default()),
                            blocker: false,
                            heading: HEADING_SUMMARIES,
                            session_id: session,
                        });
                    }
                }
            }
            if skipped_metadata > 0 {
                tracing::warn!(
                    skipped_metadata,
                    "skipped tier-1 rows with unparseable metadata"
                );
            }
            candidates.extend(facts_section);
            candidates.extend(summaries_section);
        }

        // 5. Greedy budget append; tier-1 rows capped at 25% of the total.
        // The heading bytes are charged INSIDE the greedy pass (see `greedy`),
        // so the emitted block can never exceed `4 × token_budget`.
        let tier1_cap = budget_bytes / TIER1_BUDGET_SHARE;
        let (mut included, truncated) = greedy(candidates, budget_bytes, tier1_cap);

        // 5b. Duplicate suppression against the ACCEPTED episodic rows only.
        let accepted_sessions: HashSet<String> = included
            .iter()
            .filter(|candidate| {
                candidate.heading == HEADING_RECENT || candidate.heading == HEADING_OLDER
            })
            .filter_map(|candidate| candidate.session_id.clone())
            .collect();
        if !accepted_sessions.is_empty() {
            included.retain(|candidate| {
                !(candidate.heading == HEADING_SUMMARIES
                    && candidate
                        .session_id
                        .as_ref()
                        .is_some_and(|session| accepted_sessions.contains(session)))
            });
        }

        if included.is_empty() {
            return Ok(InjectResult {
                block: None,
                memory_count: 0,
                tokens: 0,
                truncated,
                handling_ms: ms(started),
            });
        }

        // 6. Render grouped by heading.
        let mut body = String::from(HISTORY_BLOCK_OPEN);
        let mut last_heading = "";
        for candidate in &included {
            if last_heading != candidate.heading {
                body.push_str(candidate.heading);
                body.push('\n');
                last_heading = candidate.heading;
            }
            body.push_str(&candidate.line);
            body.push('\n');
        }
        body.push_str(HISTORY_BLOCK_CLOSE);
        let tokens = (body.len() / 4) as u64;

        Ok(InjectResult {
            block: Some(body),
            memory_count: included.len() as u64,
            // The byte-based estimate of the EMITTED block (D-W5.3-3).
            tokens,
            truncated,
            handling_ms: ms(started),
        })
    }

    async fn read_recent(
        &self,
        project_id: &str,
        recent_count: u32,
    ) -> Result<Vec<AgentSession>, InjectError> {
        self.store
            .list_sessions(
                project_id,
                usize::try_from(recent_count.saturating_mul(2)).unwrap_or(usize::MAX),
            )
            .await
            .map_err(store_error)
    }

    /// The tier-1 leg's availability: the shared flag OR any live
    /// operation/legacy lock → skip.
    fn tier1_leg_available(&self) -> bool {
        if self.consolidation_active.load(AtomicOrdering::SeqCst) {
            return false;
        }
        let storage_path = self.store.storage_path();
        read_live_lock(storage_path, OPERATION_LOCK_FILE, self.probe.as_ref()).is_none()
            && read_live_lock(
                storage_path,
                LEGACY_MIGRATION_LOCK_FILE,
                self.probe.as_ref(),
            )
            .is_none()
    }

    /// The tier-1 read: get_all_shards → open_shard → list by tag.
    async fn read_tier1(
        &self,
        scope: &ResolvedProjectScope,
    ) -> Result<Tier1Leg, mem_store::error::StoreError> {
        let infos = self
            .shards
            .get_all_shards(Scope::Project, Some(&scope.scope_hash))
            .await?;
        let mut rows: Vec<Row> = Vec::new();
        for info in infos {
            let db = self.shards.open_shard(&info.db_path).await?;
            rows.extend(
                list_memories_by_container_tag(db.as_ref(), &scope.container_tag, TIER1_READ_LIMIT)
                    .await?,
            );
        }
        Ok(Tier1Leg::Rows(rows))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn empty(started: Instant) -> InjectResult {
    InjectResult {
        block: None,
        memory_count: 0,
        tokens: 0,
        truncated: false,
        handling_ms: ms(started),
    }
}

fn ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

/// The CHECKED token→byte mapping (D-W5.3-3): reject overflow, never wrap.
fn byte_budget(token_budget: Option<u64>) -> Result<usize, InjectError> {
    let tokens = token_budget.unwrap_or(DEFAULT_TOKEN_BUDGET);
    let bytes = tokens.checked_mul(4).ok_or(InjectError::Overflow)?;
    usize::try_from(bytes).map_err(|_| InjectError::Overflow)
}

fn include_options(include: &Option<mem_store::serde_json::Value>) -> (u32, Option<String>) {
    match include {
        Some(value) if value.is_object() => {
            let recent_count = value
                .get("recent_count")
                .and_then(|v| v.as_u64())
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(DEFAULT_RECENT_COUNT);
            // Bounded by the timeline maximum.
            let recent_count = recent_count.min(u32::try_from(MAX).unwrap_or(recent_count));
            let query = value
                .get("query")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            (recent_count, query)
        }
        _ => (DEFAULT_RECENT_COUNT, None),
    }
}

/// `MAX_TIMELINE_LIMIT` (mem-episodic) is the bound on `recent_count`; a
/// small const alias keeps the bounds math in u32 space.
const MAX: usize = mem_episodic::timeline::MAX_TIMELINE_LIMIT;

fn store_error(error: mem_episodic::error::EpisodicError) -> InjectError {
    InjectError::Store(error.to_string())
}

/// Renders one episodic row as a one-line item (blocker first).
fn recent_line(row: &AgentSession) -> String {
    let summary = san(&row.summary);
    let threads = row.metadata.open_threads.join("; ");
    if threads.is_empty() {
        format!("{}: {}", row.agent_id, summary.replace('\n', " "))
    } else {
        format!(
            "{}: {} [blocker: {}]",
            row.agent_id,
            summary.replace('\n', " "),
            san(&threads)
        )
    }
}

/// One-line stripping + rendering hygiene.
fn san(content: &str) -> String {
    mem_ai::privacy::strip_private_content(content)
        .replace('\n', " ")
        .replace('\r', "")
        .trim()
        .to_string()
}

/// The row classification: `metadata.source == "auto-capture"` is a
/// captured session summary; anything else with a well-formed metadata
/// document is a fact-tier row. A row with NO metadata keeps the historical
/// fact-tier treatment; a MALFORMED or OVERSIZED document is `None` (skipped
/// with one bounded warning) — never rendered as a promoted fact.
enum FactOrSummary {
    Fact,
    Summary,
}

fn classify_row(metadata_text: Option<&str>) -> Option<FactOrSummary> {
    let Some(metadata_text) = metadata_text else {
        return Some(FactOrSummary::Fact);
    };
    if metadata_text.len() > MAX_INJECT_METADATA_BYTES {
        return None;
    }
    let value: mem_store::serde_json::Value =
        mem_store::serde_json::from_str(metadata_text).ok()?;
    if !value.is_object() {
        return None;
    }
    let source = value
        .get(CAPTURE_SUMMARY_SOURCE_METADATA_KEY)
        .and_then(|source| source.as_str())
        .unwrap_or_default();
    if source == AUTO_CAPTURE_SOURCE {
        return Some(FactOrSummary::Summary);
    }
    Some(FactOrSummary::Fact)
}

/// The greedy budget append.
///
/// **Loop invariant:** at the loop top, `used` is exactly the budgeted byte
/// cost of every accepted candidate — the block overhead plus, per section,
/// `heading.len() + 1`, plus `line.len() + BULLET_BYTES` per item — and
/// `used <= budget_bytes`. The accepted set is prefix-stable. The index
/// advances once per finite item, so it terminates.
///
/// **Heading charge:** a candidate that OPENS a section pays its heading.
/// "Already open" is derived from the last ACCEPTED candidate, so a skipped
/// candidate can never mark a heading as paid for. This is what makes the
/// emitted block provably bounded:
/// `block.len() = 63 + Σ(h+1) + Σ(line+1) <= 65 + Σ(h+1) + Σ(line+3) = used
/// <= budget_bytes`, with no slop.
///
/// Tier-1 candidates (`## Project facts` / `## Captured session summaries`)
/// additionally respect the 25%-byte sub-budget, charged on the same basis.
fn greedy(
    candidates: Vec<Candidate>,
    budget_bytes: usize,
    tier1_cap: usize,
) -> (Vec<Candidate>, bool) {
    let mut accepted: Vec<Candidate> = Vec::new();
    let mut used = HISTORY_BLOCK_OPEN.len() + HISTORY_BLOCK_CLOSE.len() + 2;
    let mut tier1_used = 0usize;
    let mut truncated = false;
    for candidate in candidates {
        let heading_cost = match accepted.last() {
            Some(last) if last.heading == candidate.heading => 0,
            _ => candidate.heading.len() + 1,
        };
        let cost = candidate.line.len() + BULLET_BYTES + heading_cost;
        if used + cost > budget_bytes {
            truncated = true;
            continue;
        }
        let tier1 = candidate.heading == HEADING_FACTS || candidate.heading == HEADING_SUMMARIES;
        if tier1 {
            if tier1_used + cost > tier1_cap {
                truncated = true;
                continue;
            }
            tier1_used += cost;
        }
        used += cost;
        accepted.push(candidate);
    }
    (accepted, truncated)
}
