> Scope: epics & user stories (US x.y). Verified-research amendments, stack decisions, and sequencing: roadmap.md §0b.

# Epic Overview:

- **Goal:** Build a high-performance, memory-safe Rust binary/library for local agent memory, inspired by the TypeScript/Bun `opencode-mem` plugin and service architecture, while extending the memory model from two tiers (Global User Profile + Static Project/Global Facts) to include a third tier: **Per-Project Agent Episodic/Session Memory**.
- **Target Rust Stack Recommendations:**
  - **Async Runtime:** `tokio`
  - **Database & Vector Store:** `turso` crate 0.8.x (matching the existing Turso/SQLite shard architecture in `src/services/turso/`). Verified research: `roadmap.md` §0b (D1, and the deferred escape hatch in D3).
  - **Local Embeddings:** `ort` (Rust bindings for ONNX Runtime, replacing `src/services/onnxruntime-resolve.ts` and `src/services/embedding.ts`)
  - **HTTP / Web API:** `axum` or `actix-web` (replacing `src/services/web-server.ts` and `src/services/api-handlers.ts`)
  - **Serialization & Config:** `serde`, `serde_json`, `jsonc_parser` (replacing `src/services/jsonc.ts` and `src/config.ts`)

---

## Epic 1: Per-Project Agent Session Memory _(New Feature — The Missing Piece)_

_Enable agents working within a specific project to record, summarize, and recall what they (or other agents/sub-agents) did in previous sessions._

> **Scope split:** storage-side scope (schema, queries, consolidation) lands in roadmap Phase 4; wiring-side scope (capture hooks, context injection) lands in Phase 5. US 1.1's capture hooks land in Phase 5.

### User Story 1.1: Automatic Per-Project Agent Session Journal & Capture

- **As a** developer using an AI coding agent in a project,
- **I want** the agent's actions, decisions, modified files, and unresolved tasks to be automatically captured at the end of a turn or session under the current project scope,
- **So that** there is an accurate, persistent record of what the agent accomplished in each session without manual note-taking.
- **Acceptance Criteria:**
  - Extend the existing scoping model (`src/services/memory-scope.ts`) to support an `AgentSession` / `Episodic` memory type bound to a `project_id`, `session_id`, and `agent_id`.
  - Hook into the session lifecycle (`src/services/auto-capture.ts` and `src/services/ai/session/ai-session-manager.ts`) to extract structured session summaries:
    - **Completed work:** Features implemented, bugs fixed, and commands executed.
    - **Files touched:** Key file paths created, modified, or deleted.
    - **Architectural/Codebase decisions:** Why a specific approach was chosen or rejected.
    - **Open threads / Next steps:** Work left incomplete, failing tests, or pending TODOs.
  - Store both a structured metadata payload (timestamp, `session_id`, `agent_name`, git branch/commit if available) and a vector embedding for semantic retrieval.

### User Story 1.2: Previous Session Recall & Context Injection on Session Start

- **As a** developer starting a new coding session in an existing project,
- **I want** the agent to automatically receive a concise summary of recent sessions in this project,
- **So that** the agent immediately knows what was done previously and can pick up right where the last session left off.
- **Acceptance Criteria:**
  - When a new session initializes in a project (`src/services/context.ts`), automatically query the N most recent session memories for that project alongside semantically relevant older session memories based on the user's initial prompt.
  - Inject a dedicated `<project_agent_session_history>` block into the system/context prompt while respecting token budgets (`src/utils/context-limit.ts`).
  - Prioritize unresolved blockers/TODOs and the most recent session's summary at the top of the injected context.

### User Story 1.3: Multi-Agent & Child-Session Attribution

- **As a** developer using specialized agents (e.g., Architect, Coder, Reviewer) or sub-agent workflows,
- **I want** per-project agent memories to preserve which agent performed which task and link child sessions to parent sessions,
- **So that** an agent can recall either its own specific prior work or the broader work done by other agents in the same project.
- **Acceptance Criteria:**
  - Implement and expand child-session tracking and agent preservation (`tests/child-session-capture.test.ts` and `tests/compaction-agent-preservation.test.ts`) so every episodic memory records `agent_role`/`agent_id` and optional `parent_session_id`.
  - During context compaction, ensure agent identity and session continuity summaries are preserved rather than stripped.

### User Story 1.4: On-Demand Session Timeline & Recall Tool

- **As an** AI agent working on a complex task,
- **I want** a dedicated tool call to search and browse chronological session history for the current project,
- **So that** I can answer questions like _"What did we change in the auth module yesterday?"_ or _"Why did we revert that migration two sessions ago?"_
- **Acceptance Criteria:**
  - Expose a new tool schema (`src/services/ai/tools/tool-schema.ts`) (e.g., `recall_project_sessions`) supporting filters for:
    - Time range / relative session count (e.g., `last_n_sessions: 5`, `since: timestamp`)
    - Semantic search query (`query: "database migration"`)
    - File path filter (`touched_files: ["src/config.rs"]`)
    - Agent filter (`agent_id: "planner"`)
  - Prevent orphaned timeline entries (`tests/memory-timeline-orphan.test.ts`) by linking episodic checkpoints cleanly to their parent session record.

### User Story 1.5: Episodic Memory Consolidation & Compaction

- **As a** long-term project maintainer,
- **I want** older granular session logs to be periodically consolidated into high-level milestone summaries,
- **So that** the project database does not bloat with stale turn-by-turn logs and context windows remain focused.
- **Acceptance Criteria:**
  - Extend the cleanup and deduplication services (`src/services/cleanup-service.ts`, `src/services/deduplication-service.ts`) to merge old episodic session logs (e.g., > 30 days old) into condensed milestone memories while promoting enduring architectural facts into permanent Project Memory.

---

## Epic 2: Core Persistent Project & Global Memory Engine (Rust)

_Implement the persistent memory storage, sharding, encryption, and vector search engine in idiomatic Rust._

### User Story 2.1: Scoped Project & Global Memory Storage with Turso/libSQL

- **As a** user working across multiple repositories,
- **I want** my memories strictly separated into **Project Scope** (repo-specific knowledge) and **Global Scope** (cross-project facts),
- **So that** project-specific details do not pollute unrelated projects while global knowledge remains accessible everywhere.
- **Acceptance Criteria:**
  - Implement `src/services/memory-scope.ts` and `src/services/tags.ts` in Rust with strict path/tag validation to prevent container tag or scope traversal (`tests/client-scope-traversal.test.ts`, `tests/api-handlers-container-tag-traversal.test.ts`).
  - Implement database connection management, readiness gates, and deterministic handle release (`src/services/turso/connection-manager.ts`, `src/services/turso/ready.ts`, `src/services/turso/sqlite-handle-release.ts`) using Rust's RAII (`Drop` trait) and async mutexes.
  - Implement shard management, rotation, and path migration (`src/services/turso/shard-manager.ts`, `src/services/shard-inventory-service.ts`, `src/services/shard-path-migration-service.ts`).

### User Story 2.2: Vector Search & Hybrid Exact Fallback

- **As an** agent querying persistent memory,
- **I want** fast vector similarity search with automatic fallback to exact/keyword matching,
- **So that** relevant project and global memories are reliably retrieved even if embeddings are migrating or unavailable.
- **Acceptance Criteria:**
  - Implement `src/services/turso/vector-search.ts` and `src/services/turso/vector-utils.ts` in Rust.
  - Support exact-match fallback when vector search fails or returns insufficient hits (`tests/turso-exact-fallback.test.ts`).
  - Implement schema, engine, and dimension re-embedding migrations (`src/services/turso/schema-migrations.ts`, `src/services/turso/engine-migrator.ts`, `tests/turso-reembed-migration.test.ts`, `tests/turso-migrate-dims-preflight.test.ts`).

### User Story 2.3: Local ONNX & Remote Embedding Pipeline

- **As a** user,
- **I want** to generate embeddings either locally via ONNX Runtime or via remote embedding APIs with presets and retries,
- **So that** memory indexing works offline with zero API cost or scales with cloud providers.
- **Acceptance Criteria:**
  - Replace `src/services/embedding.ts` and `src/services/onnxruntime-resolve.ts` with the Rust `ort` crate.
  - Support model presets, ONNX data types (`f32`, `fp16`, `int8`/`q8`), task prefixes, and remote exponential-backoff retries (`tests/embedding-model-presets.test.ts`, `tests/embedding-onnx-dtype.test.ts`, `tests/embedding-remote-retry.test.ts`, `tests/embedding-task-prefixes.test.ts`).

### User Story 2.4: Encryption at Rest & Cross-Process Locking

- **As a** security-conscious developer,
- **I want** my local SQLite/Turso memory shards encrypted at rest and safe under concurrent multi-process access,
- **So that** sensitive project memories are protected and multiple IDE/CLI instances do not corrupt the database.
- **Acceptance Criteria:**
  - Implement `src/services/turso/encryption-key.ts`, `src/services/turso/encryption-migrator.ts`, and `src/services/turso/operation-lock.ts` in Rust.
  - Pass multi-process concurrency and encryption migration checks (`tests/encryption-at-rest.test.ts`, `tests/turso-multiprocess.test.ts`).

---

## Epic 3: Automatic User Profile Learning (Rust)

_Implement the automatic user preference, coding style, and workflow learning system in Rust._

### User Story 3.1: Asynchronous User Profile Extraction & Cold-Buffer Isolation

- **As a** developer,
- **I want** the system to passively learn my preferences, preferred languages, and coding conventions from my prompts in the background,
- **So that** the agent adapts to my working style across all projects without slowing down interactive responses.
- **Acceptance Criteria:**
  - Implement `src/services/user-memory-learning.ts`, `src/services/user-profile/user-profile-manager.ts`, and `src/services/user-prompt/user-prompt-manager.ts` in Rust background tasks.
  - Implement cross-process file/DB locking (`src/services/user-profile/learning-lock.ts`, `tests/profile-cold-buffer-cross-process.test.ts`, `tests/user-profile-cold-buffer-isolation.test.ts`) so concurrent agent processes safely claim and process user prompts without race conditions or short-write corruption (`tests/fixtures/profile-cold-buffer-worker-shortwrite.ts`).
  - Detect user language preferences automatically (`src/services/language-detector.ts`, `tests/user-profile-learning-language.test.ts`).

### User Story 3.2: Confidence Scoring, Temporal Decay & AI Cleanup

- **As a** developer whose habits evolve over time,
- **I want** learned profile traits to have confidence scores, decay when unreinforced, and support AI-assisted deduplication/cleanup,
- **So that** one-off requests do not become permanent rules and contradictory traits are resolved cleanly.
- **Acceptance Criteria:**
  - Implement profile confidence calculation and temporal decay (`src/services/user-profile/profile-utils.ts`, `tests/user-profile-decay.test.ts`, `tests/user-profile-learning-confidence.test.ts`).
  - Implement `src/services/user-profile/ai-cleanup.ts` and `src/services/ai/validators/user-profile-validator.ts` to validate and merge profile items via configured LLM providers.

---

## Epic 4: Multi-Provider AI Client, Privacy & Portability (Rust)

_Implement the LLM provider abstractions, privacy filters, skills manager, and import/export tools in Rust._

### User Story 4.1: Unified Rust AI Provider Trait & Session Manager

- **As a** user configuring `opencode-mem`,
- **I want** to connect any supported LLM provider (OpenAI, Anthropic, Google Gemini, Minimax, Atlas Cloud, OrcaRouter, or the OpenCode host SDK) for memory extraction and cleanup,
- **So that** background memory and session summarization works with my existing API keys.
- **Acceptance Criteria:**
  - Replace `src/services/ai/providers/base-provider.ts` and `src/services/ai/ai-provider-factory.ts` with a unified Rust `#[async_trait] pub trait AiProvider`.
  - Implement provider clients for Anthropic Messages (`anthropic-messages.ts`), OpenAI Chat/Responses (`openai-chat-completion.ts`, `openai-responses.ts`), Google Gemini (`google-gemini.ts`), Minimax (`minimax.ts`), Atlas Cloud (`atlas-cloud.ts`), OrcaRouter (`orcarouter.ts`), and OpenCode Host (`opencode-provider.ts`).
  - Prevent infinite capture loops by isolating internal capture sessions (`src/services/ai/internal-capture-sessions.ts`).

### User Story 4.2: Privacy Tag Stripping, Prompt Filtering & Secret Resolution

- **As a** developer working with proprietary code and credentials,
- **I want** `<private>` tags, injected system prompts, and secrets to be stripped before memories or session logs are persisted,
- **So that** sensitive data is never written to disk or sent to embedding/LLM APIs.
- **Acceptance Criteria:**
  - Implement `src/services/privacy.ts` (including malformed tag handling from `tests/privacy-malformed-tags.test.ts`), `src/services/injected-prompt-filter.ts`, and `src/services/secret-resolver.ts` (enforcing strict file permission checks as in `tests/secret-resolver-permissions.test.ts`).

### User Story 4.3: Memory Portability & Skills Management

- **As a** user,
- **I want** to export/import my memories (including project memories, global memories, user profiles, and project session histories) and manage reusable agent skills,
- **So that** I can back up or migrate my knowledge base across machines.
- **Acceptance Criteria:**
  - Implement `src/services/memory-portability-service.ts` and `src/shared/api/portability-schemas.ts` in Rust using `serde` validation.
  - Implement `src/services/skills/skills-manager.ts` for managing and bridging skills (`tests/v2-skills-bridge.test.ts`).

---

## Epic 5: Host Plugin Bridge, TUI & Web Explorer Backend (Rust)

_Maintain backward compatibility with the OpenCode host environment. This project ships its own Leptos/Thaw explorer UI (ADR-0003, `web-ui`) as the sole UI deliverable; the REST contract is its API contract (verified research: `roadmap.md` §0b D5)._

### User Story 5.1: Web Server API & Explorer Compatibility

- **As a** user of the web explorer,
- **I want** the REST contract as the API contract for the Leptos/Thaw UI (ADR-0003, `web-ui`), including the Session History/Timeline tab,
- **So that** this project's own Leptos/Thaw explorer UI (ADR-0003, `web-ui`) can display project session history and the REST contract stays the sole API contract.
- **Acceptance Criteria:**
  - Implement `src/services/web-server.ts`, `src/services/api-handlers.ts`, `src/services/web-auth.ts`, `src/services/web-api-auth.ts`, and `src/services/cors.ts` to `axum`.
  - Add REST endpoints `GET /api/projects/:project_id/sessions` and `DELETE /api/projects/:project_id/sessions/:session_id` to inspect and manage per-project agent session memories.
- Verified research: `roadmap.md` §0b D5 and `docs/adr/0003-web-explorer-ui-stack.md`.

### User Story 5.2: OpenCode V1/V2 Plugin Adapter & TUI Companion

- **As an** OpenCode CLI/TUI user,
- **I want** the Rust binary/FFI plugin to integrate with the OpenCode plugin loader contract and toast RPC notifications,
- **So that** upgrading from the TypeScript version to the Rust version is a drop-in replacement.
- **Acceptance Criteria:**
  - Support the V1/V2 plugin contracts (`src/plugin.ts`, `src/v2/plugin.ts`, `src/v2/adapter.ts`, `src/v2/toast-rpc.ts`, `src/tui.ts`) via a lightweight JS/N-API wrapper or JSON-RPC stdio/HTTP bridge to the compiled Rust binary.
  - Ensure graceful shutdown and resource cleanup on host exit (`tests/plugin-shutdown.test.ts`).


> Deferred to hardening (Phase 7): auto-update, i18n, request-timeout polish, bundle-boundary/loader-contract parity audit, drop-in rehearsal — see roadmap §Sequencing 7, W7.1–W7.2.
