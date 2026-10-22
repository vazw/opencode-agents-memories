//! mem-episodic — per-project agent session (episodic) memory: session summaries, timelines, consolidation.

/// Stable crate identifier for composition checks.
pub const CRATE_NAME: &str = "mem-episodic";

pub mod attribution;
pub mod consolidation;
pub mod cross_tier;
pub mod embedding;
pub mod error;
pub mod gate;
pub mod ids;
pub mod inbox;
pub mod milestone;
pub mod payload;
pub mod promotion;
pub mod purge;
pub mod raw_context;
pub mod record;
pub mod segments;
pub mod store;
pub mod summarizer;
pub mod timeline;
pub mod tool_schema;
pub mod validation;

pub use attribution::{
    MessageInfo, MessageRole, compaction_injection_agent, resolve_session_agent,
};
pub use consolidation::{
    ConsolidationDeps, ConsolidationPolicy, ConsolidationReport, Consolidator, EpisodicBatchWriter,
    PromotionScope, SkipReason, SkippedAgent, StoreBatchWriter,
};
pub use cross_tier::{
    CrossTierContext, CrossTierReport, CrossTierTask, NoopCrossTierTask, SessionRef,
    SharedCrossTierTask,
};
pub use embedding::{UNKNOWN_BRANCH, compose_embedded_document};
pub use error::EpisodicError;

/// Stream plumbing re-export (upstream `AiProvider::complete` returns a
/// `futures::Stream`; downstream crates drain it without a NEW dependency
/// edge — same house pattern as `mem-store`'s serde/tokio re-exports).
pub use futures;
pub use milestone::{
    Contributor, MAX_CONTRIBUTORS_FRAMING_BYTES_PER_ROW, Milestone, decode_contributors,
    encode_contributors, max_contributors_bytes, plan_milestone,
};
pub use payload::SessionMetadata;
pub use promotion::{
    PermanentMemorySink, PromotableFact, PromoteOutcome, PromotionRecord, ShardPermanentMemorySink,
    VectorCountBump, select_promotable_facts,
};
pub use purge::{EpisodicPurgeReport, SessionScope};
pub use raw_context::{MAX_RAW_CONTEXT_BYTES, RawContext, RawContextWrite, validate_raw_context};
pub use record::{AgentSession, Decision, NewAgentSession, WorkItem, WorkKind};
pub use segments::{MAX_SEGMENT_BYTES, RawSegment, split_segments};
pub use store::{
    Clock, EPISODIC_DB_NAME, EpisodicStore, IdFactory, SessionSearchHit, UpsertOutcome,
};
pub use summarizer::{
    MAX_TRANSCRIPT_BYTES, MILESTONE_SYSTEM_PROMPT, MilestoneSummarizer, ProviderSummarizer,
    build_transcript, fit_prefix, stripped_source_summary,
};
pub use timeline::{DEFAULT_TIMELINE_LIMIT, MAX_TIMELINE_LIMIT, SessionSummary, TimelineFilter};
pub use tool_schema::{
    RECALL_PROJECT_SESSIONS_DESCRIPTION, RECALL_PROJECT_SESSIONS_TOOL_NAME, RecallMode,
    RecallProjectSessionsArgs, RecallProjectSessionsResult,
};
