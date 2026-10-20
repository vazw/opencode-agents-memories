# PRD: Rust Port — Design Decisions for Phases 2–6

## Problem Statement

The opencode-agents-memory Rust port has a detailed roadmap (`.scratch/rust-port/roadmap.md`) and 29 implementation issues, but the design decisions for Phases 2–6 (AI Client Layer, Profile Learning, Episodic Memory, Plugin Bridge, Web Explorer) were underspecified. Without concrete architectural decisions, implementers would make inconsistent choices across waves, leading to rework and contract drift.

## Solution

A grilling session was conducted across all five phases, producing concrete, locked-in design decisions for every wave. This PRD synthesizes those decisions into a single reference document that implementers can consult when writing blueprints and code. Each decision was made with the user's explicit approval and is grounded in the upstream TypeScript reference (`example/opencode-mem/`).

## User Stories

1. As a Rust port implementer, I want a single document capturing all design decisions for Phases 2–6, so that I don't need to re-derive them during implementation.
2. As a plan-gate reviewer (syseng), I want to verify that blueprints conform to these decisions, so that scope creep is caught early.
3. As a future maintainer, I want to understand why each architectural choice was made, so that I can evaluate whether to revisit it.
4. As the orchestrator, I want to dispatch waves with confidence that the design is settled, so that parallel agents don't make conflicting choices.
5. As a quality reviewer, I want testing decisions documented per phase, so that acceptance criteria are verifiable.

## Implementation Decisions

### Phase 2 — AI Client Layer & Privacy (W2.1–W2.3)

**W2.1 — Provider Trait & Host Provider:**
- Trait: `AiProvider` (async, Send+Sync) with `complete` (boxed Stream), `embed(Vec<String>)`, `list_models`, `count_tokens`. Full OpenCode parity.
- Structured output via sealed extension trait `AiProviderStructured::complete_structured` with `serde_json::Value` chunks.
- Error seam: `ProviderError { kind, provider, source }` — no provider types leak above trait.
- Request types carry `timeout: Option<Duration>` + `CancellationToken`.
- Factory: explicit `ProviderRegistry` with `register_builtin()`, typed `ProviderConfig` enum per provider, `SecretResolver` injected from W2.3.
- `HostProvider` is a separate impl (stdio JSON-RPC to OpenCode host); shares only the trait.
- Config-driven default + fallback chain: `default_provider` + `fallback_providers: Vec<String>`.

**W2.2 — Remote Provider Clients (7 adapters):**
- Shared plumbing: `reqwest` + `tower` middleware stack.
- Retry: exponential backoff + jitter, max 3, on 429/5xx/timeout.
- Timeout: per-request via trait → `TimeoutLayer` per call.
- Structured output: incremental JSON assembly across streamed chunks.
- Auth: middleware layer injecting `Authorization: Bearer` from resolved secret.
- Error mapping: unified HTTP status → `ProviderErrorKind` with provider-specific overrides.

**W2.3 — Privacy Filters:**
- Factory middleware `PrivacyFilterLayer` wraps any `AiProvider`.
- `<private>` tag stripping: streaming state machine (handles malformed/nested/partial tags).
- Injected-prompt filter: blocklist + allowlist (internal prompts allowed).
- Secret resolver: strict file-permission checks (`mode & 0o077 == 0`) + shape validation (API key prefixes). Returns `SecretString` (zeroize on drop).
- Config supports `SecretSource::Env(var)` and `SecretSource::File(path)`.

### Phase 3 — User Profile Learning (W3.1–W3.3)

**W3.1 — Prompt Manager & Cold Buffer:** *(PRD hypotheses reconciled 2026-10-19 — user ruled "Ratify immediately" on blueprint 20261019-0929 D-V1..D-V7: the originals were factually wrong against upstream; pending syseng Plan Gate confirmation of the evidence.)*
- Storage: Turso DB `user-prompts.db` (claim state machine `captured ∈ {0,2,1}`, attempts-bounded retry queries) + `cold-buffer.json` single-JSON snapshot with atomic tmp+rename publish. (No JSONL exists upstream.)
- Claim/learning protocol: single-row CAS in coordination DB `.profile-learning-coordination.db` (INSERT ON CONFLICT wins; reclaim only token-conditional AND owner provably dead; DELETE WHERE owner_token release). Cold-buffer file "locking" = the tmp+rename protocol itself. (No fd-lock/fs4.)
- Short-write hardening: fail-CLOSED strict whole-file JSON load (corrupt ⇒ refuse round, never overwrite); save side = create_new 0600 tmp, byte-accurate write-all loop with `write stalled after N bytes` detection, fsync, atomic rename, tmp unlink on every failure.
- Stale-holder recovery: dead PID (kill SIG_0 tri-state ESRCH/EPERM/OK) / PID-reuse starttime mismatch / previous boot-id only — NEVER mtime or TTL steal; live-but-unverifiable owners are never taken.
- Scheduling: host `session.idle` event + pinned 10s per-session debounce (clock-injected scheduler); contention ⇒ skip round, next idle retries. (No 5-min timer; no claim-on-arrival.)
- Privacy (D-V6): `save_prompt` gates persistence through the locked W2.3 public surface only — fully-private or blank-after-strip content is rejected `Private content blocked` before writing.
- Dependencies (D-V7): no new crates except direct-edge `libc = "0.2"` (kill-probe tri-state; already in Cargo.lock); deterministic `{pid}-{millis}-{counter}` owner tokens replace uuid.

**W3.2 — Learning Service & Language Detection:**
- Task model: bounded mpsc + worker pool (max 2).
- Error recovery: re-queue failed prompts to buffer (no data loss).
- Prompt truncation: 4096 chars before LLM call.
- Language detection: `whatlang` crate (pure Rust, 69 languages).
- Profile store: Turso table in shard (`profile_traits` with confidence, source_prompt_id, timestamps).
- Extraction prompt: config-overridable with default template.

> **§W3.2 reconciliation (as shipped):** `whatlang` is adopted as the single detector mechanism (user-ratified); the earlier "pure, no-deps, franc-min-matching" wording is withdrawn. Truncation is two **char-safe** caps — 4096 chars per prompt plus a 32768-char head-only whole-context cap closed with the upstream `[... context truncated to userProfileMaxContextBytes ...]` marker. The "bounded mpsc + worker pool (max 2)" item is **overruled**: upstream has no queue, and W3.1's shipped single-flight round (in-process flag + cross-process lock) is the scheduler — a second pool would fork the round and contend on that same lock. The shard-level `profile_traits` table is **retired**, including its `source_prompt_id` column: W3.2 reuses W3.1's shipped `user-profiles.db` JSON `ProfileData` store instead of introducing a second profile schema. Failed rounds leave prompts unmarked (`user_learning_captured = 0`) in `user-prompts.db` — that unmarked retry *is* the "re-queue failed prompts" behavior; no separate buffer is added.

**W3.3 — Confidence, Decay & Cleanup:**
- Confidence: exponential moving average (α=0.3).
- Decay: exponential half-life (30 days), lazy on read + daily background sweep.
- AI cleanup: incremental on new trait (not full scan).
- Contradiction resolution: AI-mediated merge via provider.
- Validation: trait type enum + content length bounds (3–500 chars).

> **§W3.3 reconciliation (as shipped):** The PRD's two locks are named constants in `mem_profile::decay` — `CONFIDENCE_EMA_ALPHA = 0.3`, applied inside `sync_confidence` (the single place a stored confidence is derived), and `HALF_LIFE_DAYS = 30.0` as the **base** half-life, with upstream's frequency scaling `halfLife = 30 d · (1 + log2(1 + freq))` retained (a frequency-1 trait therefore has a 60-day effective half-life). Decay is lazy on read (`ProfileStore::decay_in_memory`, injected clock — no `SystemTime::now`, no sleeps) plus a **persisted** daily sweep (`ProfileStore::sweep_decay_daily`, 24 h gate, written through the existing optimistic-version `update_profile`), **with no production scheduler yet — the daily tick and the cleanup-hook construction are carried to W5/W6** (R13). AI cleanup is incremental and scoped by `mem_core::dedup::dedup_groups` (the landed trait plus its `[0.65, 0.9)` conflict-band neighbours) instead of upstream's full-profile automatic call, and the model's mapping is auto-accepted inside that scope; every item outside the scope is carried over verbatim and a rejected merge/removal is restored from the pre-analysis snapshot (no data loss outside the merged pair). Validation adds the PRD's trait-type enum and the 3–500 **character** content bounds, enforced on the provider payload before any merge. `mem_core::dedup` is the shared service W4.3's consolidation reuses. No new dependencies, no DDL, and no `provider_gateway.rs` change.

### Phase 4 — Episodic Session Memory (W4.1–W4.3)

**W4.1 — Episodic Schema:**
- PK: surrogate UUID v7 (time-ordered).
- `session_id: String` required (every record tied to a session).
- Dedup: full unique index `UNIQUE(project_id, session_id, agent_id)`.
- Columns: normalized (timestamp, agent_id, git_branch, git_commit, files_touched, completed_work, decisions, open_threads, summary) + `metadata` JSON blob.
- Embedding: structured template `"Session {session_id} by {agent_id} on {branch}: {summary}. Completed: {completed_work}. Decisions: {decisions}."`
- Storage: per-project shard (same as memories).
- Migration: shared `schema-migrations` framework, ID `episodic-v1`.
- Parent linking: `parent_session_id: Option<String>` FK with `ON DELETE SET NULL`.

**W4.2 — Timeline Queries:**
- Three modes: chronological, semantic (vector search over session embeddings), filtered.
- Semantic: embed query string → cosine search over session embeddings.
- Orphan prevention: soft-delete + orphan flag (`deleted_at`, `orphaned`).
- Tool schema: `recall_project_sessions` with `project_id` required, all filters optional.
- Filter combination: AND-only with optional params.
- Result shape: `SessionSummary` for listings, full `AgentSession` on demand.

**W4.3 — Consolidation & Compaction:**
- Trigger: age-based (default 30 days) OR count-based (default 50 sessions), config-driven.
- Output: milestone record (new `AgentSession` with `agent_id = "consolidated"`, LLM summary, `consolidated_from` IDs). Originals soft-deleted.
- Fact promotion: high-confidence traits (>0.8 after decay) promote to permanent memory via W3.3 profile store.
- Compaction: preserve `agent_id` + set `metadata.compacted = true`.
- Dedup: shared `mem-core::dedup` service used by both W3.3 and W4.3.

### Phase 5 — Plugin Bridge (W5.0)

**W5.0 — Bridge Strategy ADR:**
- Primary: JSON-RPC 2.0 over stdio (Rust binary spawned by JS wrapper).
- Fallback: in-process (N-API or library call) for environments forbidding child processes.
- Protocol is transport-agnostic — same message schema, versioned method names (`mem/v1/capture`, etc.).
- Wire format: JSON (human-debuggable, matches upstream).
- Reversibility: transport swap only; protocol + JS wrapper unchanged.
- Message schema frozen in `docs/adr/0004-bridge-protocol.md` before W5.1.

### Phase 6 — Web Explorer (W6.1–W6.5)

**W6.1 — Axum Skeleton:**
- Auth: Basic auth, optional (config-driven, localhost default).
- CORS: config-driven allowlist, localhost default.
- Health: upstream-parity shape (`{ status, version }`).
- Static hosting: `tower-http::ServeDir` + SPA fallback to `index.html`.
- Entry: dual (CLI `mem serve` + library `mem-web::serve`).
- Default port: **7474** (config-overridable).

**W6.2 — API Handlers Parity:**
- Structure: router modules per group (`memories_router`, `tags_router`, etc.).
- State: `State<Arc<AppState>>` (store + profile + config + auth).
- Traversal: middleware + per-handler defense in depth.
- Response: exact upstream JSON shapes (camelCase via serde rename).
- Errors: upstream shape `{ error, code }`.
- Pagination: hybrid (cursor primary + offset fallback for compat).

**W6.3 — Session History Endpoints:**
- List: `GET /api/projects/:project_id/sessions` with query params.
- Delete: soft-delete (hard via `?hard=true`).
- Response: summary list + next_cursor + total.
- Semantic: `?query=` triggers vector search over session embeddings.

**W6.4 — Portability & Skills:**
- Format: JSON bundle, nested by project.
- Import: merge (upsert) default, replace via `?mode=replace`.
- Episodic: included from day one (all records).
- Skills: config-file based registry.
- Versioning: bundle version field + migration on import.

**W6.5 — Leptos/Thaw UI:**
- Routing: `leptos_router` 0.7 (client-side).
- State: signals + context (auth token, theme, project).
- REST client: `gloo-net` with single `request<T>` + typed wrappers.
- Auth seam: single function, adds `Authorization: Basic` when token present.
- Theme: Thaw `ConfigProvider`, light/dark toggle persisted to localStorage.
- Sessions: tab within project view (Memories, Tags, Profile, Sessions).
- Pins: leptos 0.7, thaw 0.4.8, trunk, wasm32-unknown-unknown, no nightly.

## Testing Decisions

- **Phase 2:** Golden-vector tests for embedding parity; provider tests use mock HTTP servers (no real API keys); privacy tests verify malformed tag handling and permission rejection.
- **Phase 3:** Cross-process claim protocol tested under repeat loops (any flake = blocking defect); decay math is pure (unit-tested); AI cleanup tested with mock provider.
- **Phase 4:** Net-new acceptance tests written first (record shape, scope binding, migration); filter-matrix tests for timeline queries; compaction identity preservation test.
- **Phase 5:** Message schema frozen and tested before JS/Rust lanes split; shutdown tests (no orphaned processes).
- **Phase 6:** REST handler tests derive from upstream handler code + tests; `wasm-bindgen-test` for REST client shape; manual smoke per screen.

## Out of Scope

- Phase 1 (Memory Engine Core) — already in flight (W1.3–W1.5).
- Phase 7 (Hardening & Parity Audit) — deferred to after all phases complete.
- Auto-update service, i18n translations, request-timeout polish — Phase 7.
- N-API transport implementation — fallback only, not primary.
- sqlite-vec escape hatch — considered and deferred (§0b D3).

## Further Notes

- All decisions were made via grilling sessions with the user (2026-10-19).
- Decisions are grounded in upstream reference: `example/opencode-mem/` v2.31.3 @ 906038e.
- Storage engine: `turso` crate 0.8.x (not `libsql` — no vector support).
- Embeddings: `ort = "=2.0.0-rc.14"` + `tokenizers`.
- Web UI: Leptos CSR + Thaw 0.4 (ADR-0003).
- Issue tracker: `.scratch/rust-port/issues/` (local markdown, 29 issues).
- Roadmap: `.scratch/rust-port/roadmap.md` (Rev 3).
- Model pins: coder → MiMo (fallback Beam Free), all other agents → Beam Free.
