# PRD: Rust Port — Design Decisions for Phases 2–7

## Problem Statement

The opencode-agents-memory Rust port has a detailed roadmap (`.scratch/rust-port/roadmap.md`) and the issue tracker under `.scratch/rust-port/issues/`, but the design decisions for Phases 2–6 (AI Client Layer, Profile Learning, Episodic Memory, Plugin Bridge, Web Explorer) were underspecified. Without concrete architectural decisions, implementers would make inconsistent choices across waves, leading to rework and contract drift.

**Phase 7 (live swap), added 2026-10-30.** The port is buildable and passes a real-host smoke, but it is not the plugin this machine actually runs: the live OpenCode host still loads upstream `opencode-mem@latest` (2.30.0), which owns the live store (`~/.opencode-mem/data` — 22 shards across projects, 3,091 memories in a checkpointed copy at 15:42Z, 1,732 prompts, 2 profiles). Opening that store with the port migrates it (metadata 1→2, prompts 1→2, shards 2→3 lazily), the tool embedder needs an ONNX cache the upstream store's `.cache` cannot satisfy, and the owner needs a backed-up, reversible swap with a rehearsed restore before dogfooding. Those decisions were previously unrecorded.

## Solution

A grilling session was conducted across all five phases, producing concrete, locked-in design decisions for every wave. This PRD synthesizes those decisions into a single reference document that implementers can consult when writing blueprints and code. Each decision was made with the user's explicit approval and is grounded in the upstream TypeScript reference (`example/opencode-mem/`).

**Phase 7 (live swap).** A read-only dry run on checkpointed copies of the live store measured the migration and its deployment prerequisite (2026-10-30). The decisions below pin the swap path: in-place migration as the primary route, export→import kept as a disaster-recovery artifact, the cache-provisioning prerequisite, the config swap through the config-editor, and the verification/rollback sequence. The runbook and its evidence are issue 80; the owner-gated execution is issue 81.

## User Stories

1. As a Rust port implementer, I want a single document capturing all design decisions for Phases 2–6, so that I don't need to re-derive them during implementation.
2. As a plan-gate reviewer (syseng), I want to verify that blueprints conform to these decisions, so that scope creep is caught early.
3. As a future maintainer, I want to understand why each architectural choice was made, so that I can evaluate whether to revisit it.
4. As the orchestrator, I want to dispatch waves with confidence that the design is settled, so that parallel agents don't make conflicting choices.
5. As a quality reviewer, I want testing decisions documented per phase, so that acceptance criteria are verifiable.
6. As the owner, I want a full-store backup with a checksum manifest before anything opens the live store, so that there is a restore point independent of the port's schema.
7. As the owner, I want the backup's restore command rehearsed into a scratch directory and opened with the engine, so that rollback is proven, not hoped for.
8. As the owner, I want the in-place migration's exact effects measured on copies before it runs on the live store, so that I know every version bump and what survives.
9. As the owner, I want the ONNX cache provisioned with a canonical tokenizer before the swap, so that `search`/`add` do not degrade with `-32603` while the rest of the store works.
10. As the owner, I want the plugin entry swapped to the local path as a single reversible edit with a recorded restore command, so that rollback is one config change plus the backup.
11. As the owner, I want the swap verified in a real host — hello, direct `memory` tool, `memory-usage` skill, skills, web UI — before I trust it, so that a degraded surface is visible immediately rather than during dogfooding.
12. As the owner, I want capture, injection, and compaction exercised under real sessions, because the tool-lane smoke does not cover the hooks.
13. As the owner, I want the 917-memory current-project export kept as a disaster-recovery artifact, so that the current project can be rebuilt into a fresh store if the in-place migration ever goes wrong.
14. As the owner, I want to know that the local plugin does not auto-update, so that a stale plugin is a deliberate state rather than a surprise.
15. As a future maintainer, I want the swap's evidence, versions, and rollback recorded outside `.tmp/`, so that a later upgrade or rollback is reproducible from the repository.

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
- Default port: **3742** (`mem_bridge::engine::DEFAULT_WEB_PORT`; `OPENCODE_MEM_WEB_PORT` overrides at launch). The `webServerPort` config key (default 4747) is read by the standalone `mem-web` server; the plugin's engine launch does not forward it (measured 2026-10-31: the live engine bound 3742 with the config at 4747).

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

### Phase 7 — Live swap (W7.2 rehearsal, pulled forward)

**Path: in-place migration (measured 2026-10-30 on checkpointed copies; the live store was only read).**
- `metadata.db` 1→2 on the first store touch (`list-shards`): repair + partial unique index; 22 rows / 3,089 vectors unchanged; 144–285 ms.
- `user-prompts.db` 1→2 on the first web `?includePrompts=true`: the live file already had all 12 columns, so only the `user_version` stamp changed; 1,732 rows fingerprint-identical; 160–237 ms.
- Project shards 2→3 lazily on first write: `CREATE TABLE shard_stats` only; the other 21 shards stay v2 until written. No migration takes a `.bak`.
- Result on the final 15:42Z copy: 22/22 shards, **3,091/3,091 memories**, 768-dim vectors read in place (**no re-embed**), 1,732/1,732 prompts, 2/2 profiles + 10 changelogs, 29/29 DBs `integrity_check = ok`, and both the stdio and web read surfaces answer.
- Whole first-open read set 5.3 s wall (7.6 s including web); the first search's 3.6–5.9 s is the one-time 547 MB ONNX model load; later searches 63–203 ms. `add` 97 ms.
- Export→import is **not** the primary path: the upstream export carries the current project only (917 memories, schemaVersion 1, no vectors). Measured 2026-10-30 on a fresh scratch store: dry run `917 imported / 0 rejected / 0 skipped` (667.6 s), real import `917 / 0 / 0` (627.8 s), all 917 rows field-for-field identical, store shard v3 / metadata v2 with the same container tag, search rank-1 correct; a second import aborts whole-import in 521 ms (`import_aborted`, 917 duplicate ids) with zero writes. It does **not** carry profiles, prompts, milestones/staging sessions, global memories, or the other 21 projects. The other 21 projects' paths are not recoverable from the store (the registry stores hashes; the session DBs are empty). The export is kept as the disaster-recovery artifact and the cross-machine/per-project route.

**Backup plan (mandatory before the live store is opened).**
- Stop OpenCode so no upstream process writes the store.
- Checkpoint the live DBs (`PRAGMA wal_checkpoint(TRUNCATE)`), then `cp -a` the whole `~/.opencode-mem` root to a dated directory; include `data/`, `.cache/`, and both config files (`~/.config/opencode/opencode.jsonc`, `opencode-mem.jsonc`).
- Write a sha256 manifest for every file and verify the copy; run `PRAGMA integrity_check` on every copied DB.
- Restore: stop OpenCode, `rsync -a --delete <backup>/data/ ~/.opencode-mem/data/`, restore both config files, restart. The restore is rehearsed into a scratch directory and opened with the engine before the swap (issue 80).

**Provisioning prerequisite (measured A/B/C).**
- The tool embedder reads `$TMPDIR/.opencode-mem-onnx` (`crates/mem-bridge/src/composition.rs:424`, `hub: None`), not `{storagePath}/.cache`.
- Absent → `search`/`add` answer `-32603` in 7 ms; upstream's `.cache` copied as-is → `-32603` after 6.8 s (tokenizer v1.0 rejected); a canonical v2.0 `tokenizer.json` → works.
- Provision the model dir (weights + canonical tokenizer) before the swap; because `$TMPDIR` may be volatile, either launch the host with a stable `TMPDIR` or re-provision after reboot. `list`/`list-shards`/`stats`/`skills`/web work without it; `search`/`add` do not.

**Config swap (reversible, config-editor).**
- The port reads the same `~/.config/opencode/opencode-mem.jsonc` (camelCase, unknown keys tolerated) and defaults `storagePath` to the same `~/.opencode-mem/data`.
- The plugin entry in `~/.config/opencode/opencode.jsonc` moves from `opencode-mem@latest` to the local plugin path; the config-editor applies it with a backup and a restore command after the human gate.
- A local plugin does not auto-update; that is a deliberate state, not an oversight.

**One-way semantics (corrected by measurement).**
- The port refuses a store written by a newer build (metadata v3 → every store call `-32603`; shard v4 readable, `add` refused).
- The vendored upstream source does **not** refuse a higher `user_version` (`schema-migrations.ts:37-38` filters versions above its own and returns `current`), so the earlier "the upstream fallback would then refuse" claim is not supported (READ, not executed). Return-trip risks remain unmeasured: upstream never updates `shard_stats`, and metadata v2's partial unique index could reject some upstream registrations. The backup, not the older plugin, is the rollback.

**Verification battery (real host).**
- `hello`/`health`; direct `memory` tool with `options.codemode: false`; `memory-usage` skill listed via `/api/skill`, loaded and used by a model; `skills`; `list`/`search`/`profile`; the web UI; one `add` + search under the provisioned cache. Then capture, injection, and compaction under real sessions — the 2026-10-30 podman run covered the tool lane (nine help commands, four portability modes, direct tool and skill, cost 0), not the hooks.
- The runbook records each check with its expected value, so a failure identifies the degraded surface.
- A **fresh-engine reopen** is an explicit step after the swap (and after any import): never leave a SIGTERM'ed engine as the last opener. Observed once in the import probe (run A): a reopen answered `hello`/`health` but `list-shards`/`list`/`import` with `-32603`; removing stale `*.db-shm`/`*.db-tshm` restored it. The clean control (run B) reopened fine after an ordered `shutdown`. Recovery is: confirm no engine runs, remove the stale shared-memory files (they are rebuilt) — never the DB/WAL.

**Rollback.**
- Restore the two config files and the store backup (stop OpenCode first); the 917-memory export can rebuild the current project into a fresh store. A rollback drill runs on copies before the swap and is recorded in the tracked evidence directory.

**Recorded follow-up (not a swap blocker).**
- `src/main.rs:207` hardcodes the web `user_id` to `"unknown"`, so `GET /api/user-profile` without `?userId=` shows no profile even though the JSON-RPC `profile` path resolves it. It gets its own issue when the swap lands.

## Testing Decisions

- **Phase 2:** Golden-vector tests for embedding parity; provider tests use mock HTTP servers (no real API keys); privacy tests verify malformed tag handling and permission rejection.
- **Phase 3:** Cross-process claim protocol tested under repeat loops (any flake = blocking defect); decay math is pure (unit-tested); AI cleanup tested with mock provider.
- **Phase 4:** Net-new acceptance tests written first (record shape, scope binding, migration); filter-matrix tests for timeline queries; compaction identity preservation test.
- **Phase 5:** Message schema frozen and tested before JS/Rust lanes split; shutdown tests (no orphaned processes).
- **Phase 6:** REST handler tests derive from upstream handler code + tests; `wasm-bindgen-test` for REST client shape; manual smoke per screen.
- **Phase 7 (live swap):** the copy-based dry run is the test seam, not new unit tests — `copy-store.sh` (checkpointed copies), `store-state.sh` (per-DB versions, counts, fingerprints), `probe.mjs` (stdio JSON-RPC + HTTP). The live store is never an input. The backup restore is rehearsed by restoring into a scratch directory, opening it with the engine, and comparing counts, versions, and `integrity_check` against the pre-state. The real-host verification battery is a smoke checklist, not a suite; the port's existing suites remain the behavioral contract. The export→import probe is the fallback-path check, run on a scratch store.

## Out of Scope

- Phase 1 (Memory Engine Core) — already in flight (W1.3–W1.5).
- Phase 7 (Hardening & Parity Audit) — W7.1 (auto-update, i18n, request timeouts) remains deferred; W7.2's live-swap rehearsal is pulled forward and specified above (issues 80–81).
- Auto-update service, i18n translations, request-timeout polish — W7.1.
- Re-embedding or changing the embedding model — the live store and the port both use `Xenova/nomic-embed-text-v1` (768); `--mode=reembed-scope` exists but is not needed.
- The other 21 projects' directory paths — unrecoverable from the store; the full-store backup covers them.
- The web default `userId` gap — recorded as a follow-up, not a swap blocker.
- Executing the upstream plugin against the migrated store — Bun/upstream tooling is off-limits in this repo; the fallback statement is READ, not executed.
- N-API transport implementation — fallback only, not primary.
- sqlite-vec escape hatch — considered and deferred (§0b D3).

## Further Notes

- Phases 2–6 decisions were made via grilling sessions with the user (2026-10-19); the Phase 7 live-swap decisions were measured 2026-10-30 and are recorded above.
- Decisions are grounded in the upstream reference. The vendored clone has moved since the Phase 2–6 session: currently `1ddfdad` (v2.33.0); re-derive the commit and test count before citing either.
- Storage engine: `turso` crate 0.8.x (not `libsql` — no vector support).
- Embeddings: `ort = "=2.0.0-rc.14"` + `tokenizers`.
- Web UI: Leptos CSR + Thaw 0.4 (ADR-0003).
- Issue tracker: `.scratch/rust-port/issues/` (local markdown).
- Roadmap: `.scratch/rust-port/roadmap.md` (Rev 3).
- Model pins: coder → MiMo (fallback Beam Free), all other agents → Beam Free.
- Live-swap evidence (all dated 2026-10-30): dry run `.tmp/progress/live-store-dryrun-20261030T154208Z.md` (gitignored — issue 80 transcribes the decisive measurements into `docs/evidence/`); podman real-host smoke `.tmp/progress/real-host-podman-after-20261030T145706Z.md`; current-project export `~/.opencode-mem/exports/opencode-agents-memory-20261030T151445Z.json` (917 memories, schemaVersion 1).
- The live store's base files read 3,052 rows with `immutable=1`; the 3,091 figure is from the checkpointed copy (WAL applied). The live store was still being written during the dry run — do not compare counts taken at different times.
