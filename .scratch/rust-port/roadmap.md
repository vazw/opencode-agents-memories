# Delivery Roadmap — opencode-mem → Rust Port

**Revision:** 3 — adds the web-explorer UI stack ruling (§0b D5: Leptos CSR + Thaw), reworked Phase 6 web waves (W6.1 static-hosting + new W6.5), risks R8/R11, ADR candidate (6). Rev-2: verified crate research (ort / turso / sqlite-vec), 2026-10-18

**Status:** DRAFT (pre-blueprint; feeds issue generation)
**Scope:** `OVERVIEW.md` for epics and user stories (US x.y); roadmap.md is authoritative for stack decisions (§0b), sequencing, and shipped-wave addenda. Reference: `example/opencode-mem/` v2.31.3 @ 906038e (READ-ONLY).
**Stack commitments (per OVERVIEW.md, refined by verified research — see §0b):** `tokio`; storage on the **`turso` crate 0.8.x** (NOT the Rust `libsql` crate — no vector support in its engine build); embeddings on **`ort = "=2.0.0-rc.14"` + `tokenizers`**; `axum`; `serde`/`jsonc_parser`.

House structure: **Phase → Wave.** A wave is a batch of independently-grabbable work the fleet runs with parallel agents once its prerequisites are done. Waves are numbered `W<p>.<n>`; each wave ends at a verifiable, demoable state and decomposes into **tracer-bullet slices** (thin vertical cuts, finalized by the issue flow).

---

## 0. Sequencing Rationale (read this before the phases)

1. **Storage engine before everything** (Phase 1 → Epic 2): every other epic writes to or reads from Turso shards + vector search. It has zero dependency on LLMs or the OpenCode host, so it is the safest parallel first build and its upstream tests (`tests/turso-*.test.ts`) are the most precise contract. Research de-risked this phase substantially: the `turso` crate runs the same engine as upstream's JS binding, with the vector SQL surface available verbatim (§0b D1/D2) — Phase 1 is now a transliteration-plus-lifecycle port, not an unknown-terrain build.
2. **AI client + privacy before capture** (Phase 2 → Epic 4.1/4.2): auto-capture and profile learning both call an `AiProvider` and must never persist `<private>`/secrets. Building the trait + filters first means capture waves are thin glue, not new risk surface.
3. **Profile learning (Phase 3 → Epic 3)** before plugin integration because it is a self-contained background subsystem — cold-buffer/locking tests run without any OpenCode host.
4. **Episodic data model (Phase 4 → Epic 1) as a first-class phase, not an afterthought:** the new tier's *storage/schema/query surface* is independent of the host and must exist before (a) the web session endpoints (Epic 5.1), and (b) the capture hooks (US 1.1) which merely *populate* it. Doing the data model here keeps the bridge phase thin and lets the web UI ship the new tab without a second schema migration.
5. **Plugin bridge last among integration phases** (Phase 5 → Epic 5.2): everything testable headless is done; the bridge is the only piece that cannot be verified without the OpenCode host, so it carries the least risk being late. Bridge strategy decision (JSON-RPC stdio vs N-API) is a Phase 5 entry decision — see Risks.
6. **Web explorer backend (Phase 6 → Epic 5.1)** after the bridge because `api-handlers.ts` is the widest surface (1.8k LOC) and benefits from all engines being stable; auth/CORS could be pulled earlier if the user wants to demo the UI sooner (reorderable). **Rev-3 amendment:** the new W6.5 UI shell can trail W6.1 in parallel with W6.2/W6.3 as screens land per endpoint group; the *backend* REST waves are untouched by the UI-stack ruling.
7. **Deferred entirely to Phase 7:** auto-update (`tests/auto-update.test.ts`), i18n translations, request-timeout polish, bundle-boundary/loader-contract parity audit, and the end-to-end drop-in upgrade rehearsal. These are polish, not capability.

**Reorderable knobs:** Phase 6 can precede Phase 5 (pure REST demo earlier). Phase 3 can run interleaved with Phase 4 (disjoint code, both depend only on Phase 1+2).

---

## 0b. Storage-Engine & Embeddings Decisions (verified crate research, 2026-10-18)

These rulings supersede OVERVIEW.md's vaguer "`libsql`/`turso` crate" recommendation. Grounding: digests A/B (crates.io, docs.rs, ort.pyke.io, docs.turso.tech, turso COMPAT.md/manual.md, sqlite-vec repo) — all CONFIRMED unless noted.

**D1 — Storage = `turso` crate 0.8.x.** Upstream's local DB already runs the *new Rust Turso engine*: `turso-db.ts` imports `Database` from `@tursodatabase/database` (npm 0.8.3), the JS binding of the same engine the Rust `turso` crate (0.8.3, 2026-10-12) exposes — tokio-native, `Builder::new_local(...)`, **no C compiler required**. The Rust `libsql` crate (0.10.0) is **NOT viable for this port**: docs.rs shows no `vector` module/feature, and its bundled engine compiles sqlean crypto/fuzzy/math/stats/text/uuid/regexp — no vector sources anywhere in `libsql-sys`. Known wart: docs.rs builds of `turso` fail from 0.8.0-pre.13 (docs availability only, not runtime).

**D2 — Vector SQL is a verbatim transliteration.** Upstream's exact surface — `vector32(?)` (JSON text in), `vector_extract(blob)` (JSON text out), `vector_distance_cos`, exact-scan `ORDER BY vector_distance_cos(...) LIMIT k` (no ANN index; upstream's JS comment confirms "does not ship FTS5") — is documented verbatim in the turso engine manual. Vector functions are built into the engine: no runtime `load_extension` dance. No manual little-endian F32 packing: engine owns the blob encoding; port `vectorToJson`/`parseExtractedVector` as-is.

**D3 — sqlite-vec: considered, DEFERRED behind an experimental feature gate.** Verdict inputs (digest B):
- *Engine gate (decisive):* sqlite-vec **cannot attach to the turso engine** — COMPAT.md: `sqlite3_load_extension`/`sqlite3_enable_load_extension`/`sqlite3_auto_extension` all ❌ No; the `turso` crate exposes no SQLite-extension ABI. "Trying sqlite-vec" therefore means **forking the port onto `rusqlite`(+`bundled`) + statically-registered vec0** — a different storage engine, not an add-on.
- *Switch triggers* (each must outweigh the costs): (a) standard SQLCipher encryption at rest (confirmed working with sqlite-vec + `bundled-sqlcipher`; mind the older embedded SQLite → `AND k = :k` form), (b) **multi-process DB access** (turso engine explicitly lists "No multi-process access, No multi-threading" — sqlite-vec/vanilla SQLite WAL is the one genuine win), (c) int8/bit vectors, Hamming, or released ANN (sqlite-vec 0.1.9 has zero ANN; IVF/DiskANN only on unreleased `main`/alpha line with default-off flags).
- *Costs of switching:* upstream-authored shard files written by the turso engine — vanilla-SQLite readability **unverified**; embedding-blob re-encoding (turso `vector32` blobs ≠ vec0 layout); SQL dialect rewrite; C toolchain becomes a hard build dep (sqlite-vec crate always cc-compiles, zero cargo features); 0.x semver drift (pre-v1, breaking-minor policy, bindings not semver-covered).
- *Capability check:* parity is near-total for this port's needs; sqlite-vec's extras (int8/bit, partition keys, metadata prefilter, ANN-alpha) solve problems opencode-mem doesn't have. Both engines do exact brute-force cosine scans at our row counts — no perf differentiator.
- **Fenced form chosen (i):** an optional, experimental cargo feature — a feature-gated second `Storage` impl behind `rusqlite` + `sqlite-vec` (manual regular-table pattern, `ORDER BY vec_distance_cosine(emb, ?) LIMIT k`, CHECK-constrained blob column), **turso backend stays the default and the only verified path**. No wave carries this feature; it exists as a pre-scoped escape hatch (see ADR candidate 5). If neither switch trigger materializes by Phase 7, close it as considered-and-deferred.

**D4 — Embeddings stack = `ort = "=2.0.0-rc.14"` + `tokenizers`.** ort 2.0.0-rc.14 (2026-10-12) is the only usable line (all 1.x yanked) and docs deem it production-ready; it targets ONNX Runtime 1.31.0 — upstream pins onnxruntime-node 1.30.0, so the same engine one minor apart. 16-bit I/O needs the `half` feature; quantized-*weight* models (nomic `_quantized`/`_int8`) are opaque to the caller (int64 input_ids/attention_mask either way). Default binary pin: `ort = "=2.0.0-rc.14"` exactly (rc-to-rc breaking changes possible). The real parity surface is **post-processing**: pooling/normalization/rescale that transformers.js applies per model config must be hand-rolled in Rust — fastembed-rs ships exactly this recipe for `Xenova/nomic-embed-text-v1` (mean-pooling, L2-norm, `search_document:`/`search_query:` prefixes) and is the proven reference. ort is sync: run inference off tokio workers via `spawn_blocking`; thread pools via `with_intra_threads(n)`. Upstream's macOS `Ort::Env` process-exit crash (fixed in ORT 1.24.1) is moot — ort ships 1.31.

**D5 — Web explorer UI stack = Leptos CSR + Thaw 0.4, built by Trunk; `mem-web` stays a pure axum REST backend.** The Leptos/Thaw UI is a client-side-rendered WASM front-end in a standalone `web-ui/` package **excluded from the workspace member graph** (ADR-0001 amendment note), built with Trunk to `web-ui/dist` and served as static files by `mem-web`'s axum server (ServeDir + `index.html` SPA fallback) in exactly the upstream web-server pattern (`serveStaticFile` + SPA fallback, `dist/web` → `web-ui/dist`). The upstream REST contract (R8) is implemented as hand-written axum handlers — **not** Leptos server functions — and is the compat guarantee that replaces "the existing Svelte UI works" for the Rust port's UI (OVERVIEW US 5.1 amendment). UI↔server coupling is same-origin `fetch` (**credential-free localhost is the primary posture — no login/prompts ship**; Basic forwarding stays available at the single `Authorization` fetch seam for auth-configured servers); CORS exists for the API's external-tooling contract, not for our own UI. Rejected variant (recorded in ADR candidate 6): Leptos full-stack SSR via `leptos_axum` — server functions cannot carry the stable upstream endpoints, the axum handlers are needed regardless, SSR buys nothing for an authenticated local explorer, and it forces the thaw 0.5.0-beta prerelease pairing; ground: searcher digest 2026-10-18 + both local working examples. Version pairing frozen to **leptos `0.7.x` + thaw `^0.4.8`** (the all-stable pairing; official compat table 0.4 ↔ 0.7) until a stable Thaw line exists for a newer leptos (`0.9`-lag risk, R11). Ground: crates.io dep metadata (thaw 0.4.8 → leptos `^0.7.7`), thaw README table, live example `~/Code/Personal/Rust/new-siamstr/siamstr-front`.

---

## Phase 0 — Foundation & Workspace Skeleton

**Goal:** an empty-but-real Rust workspace where every later wave lands in a prepared home: crates, config, errors, logging, test conventions.

### W0.1 — Cargo workspace & crate skeleton
- **Delivers:** multi-crate workspace compiling clean (`cargo fmt`, `clippy -D warnings`, `test`) with per-crate `CONTEXT.md` stubs matching the multi-context domain-doc layout (`CONTEXT-MAP.md` at root). Proposed crates: `mem-core` (scope, tags, errors, utils), `mem-store` (turso engine, migrations, encryption), `mem-embed` (ONNX + remote embeddings), `mem-ai` (provider trait + clients, privacy), `mem-profile` (user learning), `mem-episodic` (new Epic 1 tier), `mem-web` (axum), `mem-bridge` (plugin host integration, thin). Target binary: thin `main` composing crates.
- **Entry prerequisites:** none.
- **Verification:** `cargo build && cargo clippy && cargo test` green on hello-level skeleton; `CONTEXT-MAP.md` lists all crates.
- **Parallelism:** low — one agent; the crate split *is* the deliverable.
- **Tracer-bullet slices:** (a) workspace + `mem-core` skeleton; (b) `CONTEXT.md` stubs + crate dependency rules (no cycles).

### W0.2 — Config loading (jsonc) & error/logging strategy
- **Delivers:** `mem-core::config` — loads `opencode-mem.example.jsonc`-shaped config: jsonc parse (comments tolerated), env overrides, defaults, validation. Plus the repo-wide error strategy (thiserror at boundaries, `anyhow`-free typed errors at seams) and `tracing`-based logging mirroring `src/services/logger.ts`.
- **Entry prerequisites:** W0.1.
- **Verification:** mirrors `tests/config.test.ts`, `tests/config-resolution.test.ts`, `tests/jsonc.test.ts` ported to Rust.
- **Parallelism:** config and error/logging in two concurrent slices once types are agreed.
- **Tracer-bullet slices:** (a) jsonc parser binding + raw config load; (b) resolution/override chain; (c) typed error tree + logger setup.

### W0.3 — Test conventions & shared utils
- **Delivers:** documented mapping upstream test → Rust test (integration tests in `crates/*/tests/`, upstream fixtures like `tests/fixtures/profile-cold-buffer-worker-shortwrite.ts` become Rust helper binaries/subprocess fixtures); shared utils: `context-limit` (`src/utils/context-limit.ts`), `math`, `windows-path` handling (`tests/windows-path.test.ts`).
- **Entry prerequisites:** W0.2.
- **Verification:** ported `tests/context-limit.test.ts`, `tests/windows-path.test.ts` green.
- **Parallelism:** utils and the conventions doc concurrently.
- **Tracer-bullet slices:** (a) context-limit port; (b) path utils; (c) `docs/agents/testing.md` conventions page.

---

## Phase 1 — Memory Engine Core (Epic 2)

**Goal:** the full storage engine in Rust: scoped shards, connection lifecycle, vector search with fallback, migrations, encryption, multi-process locking, embedding pipeline. This is the deepest phase and the contract for everything else. Per §0b, all vectors run on the `turso` crate 0.8.x over the same engine upstream's JS binding uses; concurrency is handled at the application level exactly as upstream does (see R3 reconciliation).

### W1.1 — Memory scope & tags
- **Delivers:** `mem-core::scope` + `mem-core::tags` — scope hashing, container-tag parsing/extraction, strict tag validation; **US 2.1** (scoping half).
- **Entry prerequisites:** W0.2.
- **Verification:** `tests/memory-scope.test.ts`, `tests/memory-scope-helper.test.ts`, `tests/client-scope-traversal.test.ts`, `tests/tags.test.ts`, `tests/project-scope.test.ts`, `tests/tool-scope.test.ts`.
- **Parallelism:** scope and tags are two concurrent slices (tags depends on scope hash pattern only).
- **Tracer-bullet slices:** (a) scope-hash validation + traversal rejection; (b) container-tag parse (`extractScopeFromContainerTag`); (c) tag CRUD + migration-skip behavior (`tests/tag-migration-*.test.ts`).

### W1.2 — Turso connection lifecycle & shard management
- **Delivers:** `mem-store` on the `turso` crate 0.8.x (`Builder::new_local`, tokio-native; §0b D1): connection manager, readiness gates, RAII (`Drop`) sqlite-handle release, shard manager/rotation/inventory/path-migration; **US 2.1**.
- **Entry prerequisites:** W1.1 (scope → shard paths).
- **Verification:** `tests/turso-connection-manager.test.ts`, `turso-connection-lifecycle`, `turso-ready-gate`, `sqlite-handle-release`, `turso-shard-rotation`, `turso-shard-recreate`, `shard-inventory`, `shard-path-migrate`, `turso-auxiliary-db-upgrade`.
- **Parallelism:** high — connection/lifecycle, shard rotation/inventory, and path migration are three independent slices against a shared `turso-db.ts` port done first.
- **Tracer-bullet slices:** (a) `turso-db` wrapper + open/close lifecycle; (b) ready gate + handle release RAII; (c) shard rotation + inventory; (d) legacy/path migration.
- **Research note:** no C compile in this crate; known docs.rs wart (`turso` ≥ 0.8.0-pre.13 docs fail — docs only, runtime unaffected). Engine constraint "No multi-process access / multi-threading" shapes R3, not this wave.

### W1.3 — Vector search & migrations
- **Delivers:** `mem-store::vector` — vector search with exact/keyword fallback, vector utils; schema/engine/dimension re-embedding migrations; **US 2.2**.
- **Entry prerequisites:** W1.2.
- **Verification:** `tests/turso-vector-search.test.ts`, `turso-vector-utils`, **`turso-exact-fallback`**, `turso-reembed-migration`, `turso-migrate-dims-preflight`, `turso-engine-migrator`, `schema-migrations`, `turso-legacy-migrator`.
- **Parallelism:** vector search/utils vs. the three migrators are two lanes (migrators can be stubbed against fixed embedding dims first).
- **Tracer-bullet slices:** (a) vector-utils (cosine/dims math); (b) search + exact fallback chain; (c) schema-migrations framework; (d) engine/dims re-embed migrator with preflight.
- **Research note (§0b D2):** vectors bind as JSON text into `vector32(?)` exactly like upstream's `vectorToJson`; read back via `vector_extract()` (JSON), mirroring upstream's `parseExtractedVector` — a straight SQL transliteration, no manual F32 packing, no extension loading. Exact-scan `ORDER BY vector_distance_cos(...) LIMIT k` matches upstream's no-ANN design 1:1. The W1.2→W1.3 risk downgraded from "function may be missing" to "validate the verbatim statements against upstream SQL in a golden test" — low cost, still worth one slice.

### W1.4 — Embedding pipeline (local ONNX + remote)
- **Delivers:** `mem-embed` — `ort = "=2.0.0-rc.14"` (exact pin; all 1.x yanked) + `tokenizers` 1.0.0-rc line: ONNX model resolution/loading, presets, dtypes (fp32/fp16/q8/int8), task prefixes (`search_document:`/`search_query:`), mean-pooling + L2-norm post-processing (the real parity surface — implement the fastembed-rs recipe for the default `Xenova/nomic-embed-text-v1`), remote provider embeddings with exponential backoff; **US 2.3**.
- **Entry prerequisites:** W0.2; can run concurrently with W1.3 (interface: `Embedder` trait producing `Vec<f32>`).
- **Verification:** `tests/embedding-model-presets.test.ts`, `embedding-onnx-dtype`, `embedding-remote-retry`, `embedding-task-prefixes`, `onnxruntime-resolve`, `tests/memory-tool-warmup.test.ts` (warmup behavior); golden-vector tests: recorded TS-side embeddings for a fixed input, compared with a small numeric tolerance (same ONNX engine both sides, ort 1.31 vs upstream 1.30 — one minor apart).
- **Parallelism:** highest in the phase — local `ort` lane and remote-retry lane are fully independent; presets/dtype are testable without a real model using a tiny bundled ONNX fixture.
- **Research note (§0b D4):** fp16/bf16 I/O needs the `half` feature and CPU FP16 Gemm/MatMul is acceleration-gated since ORT 1.30 (no matching kernel → FP32 fallback) — rely on weight-quantized models (`_quantized`/`_int8`, int64 inputs regardless of weight dtype) for the q8 path. ort is sync: `spawn_blocking` for inference; `with_intra_threads(n)` for pool control.

### W1.5 — Encryption at rest & operation locking
- **Delivers:** encryption-key management + encryption migrator; cross-process operation lock (file-lock based); **US 2.4**.
- **Entry prerequisites:** W1.2.
- **Verification:** `tests/encryption-at-rest.test.ts`, `turso-encryption-migrator`, `turso-multiprocess.test.ts`, `turso-dedup-extract.test.ts` (lock interplay).
- **Parallelism:** two slices — encryption lane and lock lane.
- **Research note:** `turso` crate offers `experimental_encryption(true)` + `EncryptionOpts` — ciphers `aegis256`/`aegis128l`/`aes256gcm` family; encrypted files are **not readable as standard SQLite** (proprietary codecs, not SQLCipher). The `libsql` crate's SQLCipher-adjacent path (SQLite3MultipleCiphers, cmake-gated) belongs to the non-viable engine — irrelevant here unless the §0b D3 escape hatch is ever opened (`rusqlite` + `bundled-sqlcipher` + sqlite-vec). Open question flagged in R4: which cipher mode upstream TS shards actually use.
- **Tracer-bullet slices:** (a) key derivation/storage; (b) read-compat spike on a TS-produced shard, then migrator (plaintext→encrypted); (c) `OperationLock` with stale-holder recovery.

**Phase 1 exit demo:** a CLI subcommand (`mem put/search`) stores a fact in a scoped shard, retrieves by vector, falls back to exact match, survives two concurrent processes (serialized by the ported app-level `OperationLock`, never raw concurrent DB access — see R3), on an encrypted shard.

---

## Phase 2 — AI Client Layer & Privacy (Epic 4.1, 4.2)

**Goal:** the `AiProvider` seam, all provider clients, and the privacy surface — everything capture/learning later calls.

### W2.1 — Provider trait, OpenCode host provider, capture-loop isolation
- **Delivers:** `mem-ai::provider` — `#[async_trait] pub trait AiProvider` (deep seam: all providers behind one interface), provider factory/config resolution (`api-key-placeholder`, `provider-config`), OpenCode host provider via host SDK/HTTP, `internal-capture-sessions` loop guard; **US 4.1** (core).
- **Entry prerequisites:** W0.2; W1.4 (remote-embedder retry conventions reusable).
- **Verification:** `tests/ai-provider-config.test.ts`, `internal-capture-sessions.test.ts`, `opencode-provider.test.ts`.
- **Parallelism:** factory/config lane and opencode-provider lane.
- **Tracer-bullet slices:** (a) trait + factory + config resolution; (b) internal-capture-session loop isolation; (c) opencode host transport.

### W2.2 — Remote provider clients (the 6-plex)
- **Delivers:** Anthropic Messages, OpenAI chat-completion, OpenAI responses, Google Gemini, Minimax, Atlas Cloud, OrcaRouter clients; **US 4.1** (clients).
- **Entry prerequisites:** W2.1.
- **Verification:** `tests/anthropic-provider.test.ts`, `openai-chat-completion-provider`, `minimax-provider`, `atlas-cloud-provider`, `orcarouter-provider`, `tests/v2-provider-connectivity.test.ts`, `v2-provider-refresh` (host-path variants ported in Phase 5).
- **Parallelism:** maximal — one agent per provider; all are shape-identical adapter implementations of the trait. Ideal fleet wave.
- **Tracer-bullet slices:** (a) shared request/response plumbing (retries, timeouts, structured-output parsing); (b) one exemplar provider (Anthropic); (c) each remaining provider as an independent slice.

### W2.3 — Privacy, prompt filtering, secret resolution
- **Delivers:** `mem-ai::privacy` — `<private>` tag stripping (incl. malformed tags), injected-prompt filter, secret resolver with strict file-permission checks; **US 4.2**.
- **Entry prerequisites:** W0.2 only (can run concurrently with W2.1/W2.2).
- **Verification:** `tests/privacy.test.ts`, `privacy-malformed-tags`, `injected-prompt-filter`, `secret-resolver-permissions`.
- **Parallelism:** three independent slices.
- **Tracer-bullet slices:** (a) tag stripper; (b) prompt filter; (c) secret resolver + permissions.

---

## Phase 3 — User Profile Learning (Epic 3)

**Goal:** the entire passive-learning subsystem as background tasks, host-independent.

### W3.1 — User-prompt manager & cross-process cold buffer
- **Delivers:** `mem-profile::prompt` — prompt buffer claim/processing, cross-process file/DB locking, short-write hardening; **US 3.1** (locking half).
- **Entry prerequisites:** W2.3 (privacy filter applied to stored prompts); W1.5 (lock primitives).
- **Verification:** `tests/user-prompt-manager-claim.test.ts`, `user-prompt-path-normalization`, `profile-cold-buffer-cross-process`, `user-profile-cold-buffer-isolation`, `profile-learning-lock`, `profile-learning-service-lock`, `profile-learning-storage-error`, `profile-learning-idle`, `profile-write`.
- **Parallelism:** prompt-manager lane vs. lock/cold-buffer lane.
- **Tracer-bullet slices:** (a) prompt buffer store + claim; (b) cross-process claim protocol (worker fixture ported); (c) idle scheduling.

### W3.2 — Learning service, profile manager, language detection
- **Delivers:** extraction of preferences/languages/conventions from prompts via `AiProvider` into the profile store; **US 3.1** (extraction half).
- **Entry prerequisites:** W2.1, W3.1.
- **Verification:** `tests/user-profile-learning-language.test.ts`, `user-profile-learning-prompt-mode`, `user-profile-learning-error`. (profile-tool-runtime is deferred to W5.4 — the tool-registration surface does not exist before W5.4.)
- **Parallelism:** language-detector (pure) runs alongside the learning service.
- **Tracer-bullet slices:** (a) profile storage schema; (b) learning service loop; (c) language detector.

### W3.3 — Confidence, decay, AI cleanup & validation
- **Delivers:** confidence scoring, temporal decay, AI-assisted dedup/merge/cleanup via validators; **US 3.2**.
- **Entry prerequisites:** W3.2.
- **Verification:** `tests/profile-utils.test.ts`, `user-profile-decay`, `user-profile-learning-confidence`, `ai-cleanup.test.ts`, `ai-cleanup-apply`.
- **Parallelism:** decay/confidence (pure math, `profile-utils`) vs. ai-cleanup (provider-dependent).
- **Tracer-bullet slices:** (a) profile-utils math; (b) validator port; (c) cleanup apply loop.
- **Shipped (rulings R1–R13):** EMA α = 0.3 inside `sync_confidence` (prior = stored confidence, observation = Beta estimator); 30-day base half-life with upstream's frequency scaling routed through the pure `mem_profile::decay` kernel; lazy read (`decay_in_memory`) + persisted daily sweep (`sweep_decay_daily`, injected clock, 24 h gate); incremental cleanup scoped by `mem_core::dedup` (landed trait + `[0.65, 0.9)` conflict-band neighbours) applied through upstream's scoped loop with the no-data-loss-outside-the-pair guarantee; validation bounds 3–500 chars enforced inside `UserProfileValidator::validate`; no timer/thread/queue (D-W3), no new dependencies, no DDL, no `provider_gateway.rs` change. Justified-addition test files: `crates/mem-profile/tests/decay.rs` (8), `validation.rs` (6), `incremental_cleanup.rs` (3), `round_incremental_cleanup.rs` (4) and `crates/mem-core/tests/dedup.rs` (6). **Carry-over (R13):** `ProfileStore::sweep_decay_daily(now)` still needs a daily caller in the host lifecycle (`mem-bridge` W5.x + the composition root), which must also construct `ProfileCleanupHook` before `LearningRound::with_incremental_cleanup(...)`; `mem-web` (W6.1) supplies the serve-loop caller if it hosts the profile store.

---

## Phase 4 — Episodic Session Memory, Core Tier (Epic 1 — NEW FEATURE, storage-side)

**Goal:** the new third tier's **data model, schema, queries, consolidation** — everything that does not require the OpenCode host. Deliberately placed after Phase 1 (needs shards/vector search) and Phase 2 (summaries later need a provider, but only for capture — the schema doesn't). Splitting Epic 1 this way means the novel domain work happens where the codebase is most stable, and Phase 5 only wires lifecycle hooks onto existing APIs.

### W4.1 — Episodic schema & scoping extension
- **Delivers:** `mem-episodic`: `AgentSession`/`Episodic` memory type bound to `project_id` + `session_id` + `agent_id`; structured metadata payload (timestamp, agent name, git branch/commit, files touched, completed work, decisions, open threads); vector embedding per session summary; schema migrations extended; **US 1.1** (storage half).
- **Entry prerequisites:** W1.3 (migrations framework), W1.1 (scoping).
- **Verification:** new Rust tests mirroring the *spirit* of `tests/child-session-capture.test.ts` (record shape) plus a fresh migration test; schema-migrations suite still green.
- **Parallelism:** schema lane vs. metadata-extraction types lane.
- **Tracer-bullet slices:** (a) episodic table + migration; (b) structured summary payload (serde) + validation; (c) embedding hookup for summaries.

### W4.2 — Session timeline queries & recall tool schema
- **Delivers:** chronological + semantic + filtered timeline queries (time range, `last_n_sessions`, `touched_files`, `agent_id`); orphan-free checkpoint linking; the `recall_project_sessions` tool schema as a typed Rust module (registration happens in Phase 5); **US 1.4** (query half), **US 1.3** (linking).
- **Entry prerequisites:** W4.1.
- **Verification:** port of `tests/memory-timeline-orphan.test.ts`; new filter-matrix tests; child/parent linking tests against `child-session-capture` expectations.
- **Parallelism:** timeline queries vs. tool-schema module vs. orphan-linking — three slices.
- **Tracer-bullet slices:** (a) timeline query builder (filters); (b) parent/child session linkage + orphan prevention; (c) tool schema definition.

### W4.3 — Consolidation & compaction
- **Delivers:** merge episodic logs older than N days into milestone summaries; promote enduring architectural facts into permanent Project Memory; agent-identity-preserving compaction; **US 1.5**, **US 1.3** (compaction half).
- **Entry prerequisites:** W4.1, W2.1 (provider for summarization), W2.3 (privacy before summarization).
- **Verification:** port of `tests/compaction-agent-preservation.test.ts`; new consolidation age-threshold tests; interplay with `cleanup-service`/`deduplication-service` ports.
- **Parallelism:** consolidation lane vs. compaction-preservation lane.
- **Tracer-bullet slices:** (a) age-based consolidation job; (b) fact promotion rules; (c) compaction identity preservation.

---

## Phase 5 — Host Plugin Bridge & Agent Integration (Epic 5.2 + Epic 1/3 wiring)

**Goal:** make the Rust engine a drop-in OpenCode plugin: bridge, lifecycle hooks, capture, context injection, tools. The only phase that needs the OpenCode host to verify end-to-end.

### W5.0 (entry decision) — Bridge strategy
- **Decision (ADR required, `docs/adr/`):** JS wrapper plugin that spawns the Rust binary and speaks **JSON-RPC over stdio** vs. **N-API native addon**. Default stance: JSON-RPC stdio first (reversible, host-agnostic, matches the V1→V2 adapter's server/client split), N-API only if stdio latency on per-turn hooks proves unacceptable. This is the single most strategy-laden decision in the plan — grilling it with the user is mandatory. **DECIDED 2026-10-20 — see `docs/adr/0007-bridge-strategy.md` (D-30 … D-39): JSON-RPC over stdio; one engine
  process per host with session-scoped logical instances; the axum web server inside the engine process; one
  agent-prioritised DB lane; an explicit `session_id` on every request; `inject`-only blocking with the
  5 ms / p95 / 100-turn re-open trigger; and the fail-open-but-never-silent degradation path. The N-API addon
  is rejected and is re-opened only by that measured trigger; the message schema is frozen in D-40 … D-47, and
  D-48 adds the durable capture inbox (an ack means *durably queued*). The `capture_id` guarantee is bounded to
  the row that carries it (USER RULING 1, 2026-10-20).**

### W5.1 — V1 plugin adapter (JS wrapper + Rust server process)
- **Delivers:** `mem-bridge`: a JS plugin module satisfying the V1 loader contract that launches/composes the Rust engine, forwards `chat.message`/session events, and exits cleanly; **US 5.2** (V1 half).
- **Entry prerequisites:** Phases 1–4 complete; W5.0 decision.
- **Verification:** `tests/plugin-loader-contract.test.ts`, `plugin-host-config`, `plugin-root-entrypoint`, `plugin-bundle-boundary`, **`plugin-shutdown.test.ts`**.
- **Parallelism:** JS wrapper lane vs. Rust stdio-server lane (against a frozen message schema).
- **Tracer-bullet slices:** (a) stdio JSON-RPC message schema + framing; (b) Rust sidecar lifecycle (spawn, handshake, shutdown); (c) JS wrapper passing loader-contract tests.

### W5.2 — V2 adapter, toast-rpc & TUI companion
- **Delivers:** V2 plugin contract, adapter bridging V2 context → V1 initializer (as upstream does), toast RPC + TUI companion; **US 5.2**.
- **Entry prerequisites:** W5.1.
- **Verification:** `tests/plugin-v2-loader-contract.test.ts`, `v2-plugin-adapter`, `v2-legacy-client`, `tui-companion.test.ts`, `v2-memory-context`.
- **Parallelism:** adapter lane vs. toast/TUI lane.
- **Tracer-bullet slices:** (a) V2 setup/adapter registration; (b) legacy-client bridge; (c) toast-rpc + tui.js companion.

### W5.3 — Session lifecycle capture & context injection (Epic 1 wiring)
- **Delivers:** end-to-end **US 1.1 + US 1.2**: hooks capture structured session summaries on turn/session end into `mem-episodic`; on session start, inject `<project_agent_session_history>` respecting token budgets, blockers first; **US 1.1**, **US 1.2**.
- **Entry prerequisites:** W4.1, W4.2, W5.1, W3.2 (provider path).
- **Verification:** end-to-end host-driven capture → new session → injected block; unit-level ports of `auto-capture.test.ts`, `auto-capture-concurrency`, `auto-capture-context`; summary-extraction tests; `tests/memory-count.test.ts` for injection accounting.
- **Parallelism:** capture-on-end lane vs. inject-on-start lane (both sides of the same contract, agreeable via the message schema first).
- **Tracer-bullet slices:** (a) hook wiring → summary request → episodic store (one happy path); (b) concurrency guards for capture; (c) injection block assembly + budget trimming; (d) child-session/agent attribution at capture time (**US 1.3**).

### W5.4 — Memory tools registration
- **Delivers:** all agent-facing tools live in OpenCode: existing memory tools ported from `src/services/ai/tools/tool-schema.ts` + `recall_project_sessions`; scope rules enforced per tool; **US 1.4** (tool half), plus the `memory-portability-tool` path.
- **Entry prerequisites:** W5.1; W4.2 for the recall tool.
- **Verification:** `tests/tool-scope.test.ts`, `memory-timeline-orphan`, `memory-portability-tool.test.ts`, `profile-tool-runtime` (carries the W3.2-deferred cases: query rejection, blank content, private-content block, missing email, lock acquire/release, fail-closed contention).
- **Parallelism:** tool-by-tool slices.
- **Tracer-bullet slices:** (a) tool registration plumbing; (b) recall_project_sessions; (c) remaining tools port; (d) portability tool.

---

## Phase 6 — Web Explorer Backend & Portability (Epic 5.1, US 4.3)

**Goal:** the axum server serving the exact existing REST contract + the new session endpoints.

### W6.1 — Axum server skeleton: health, CORS, auth, static UI hosting
- **Delivers:** `mem-web`: server bootstrap, `/health`, CORS, web auth + API auth, **static hosting of the `web-ui` SPA bundle (`tower-http` ServeDir over `web-ui/dist`, `index.html` fallback for client-routed paths) mirroring upstream `web-server.ts` static+fallback behavior**; **US 5.1** (skeleton).
- **Verification:** `tests/web-server-health.test.ts`, `web-server-cors`, `web-auth.test.ts`, `web-api-auth`, `web-server-request-lifecycle`; new: static-serving test that `GET /` yields the UI shell and unknown client-routed paths fall back to `index.html`.
- **Entry prerequisites:** W0.2; can start right after Phase 1 (reorderable earlier).
- **Parallelism:** auth lane vs. server/CORS lane vs. static-hosting lane.
- **Tracer-bullet slices:** (a) server + health + request lifecycle; (b) CORS; (c) web-auth + api-auth; (d) ServeDir + SPA fallback.

### W6.2 — API handlers parity
- **Delivers:** the full existing REST surface (`api-handlers.ts`) over `mem-store`/`mem-profile`; container-tag traversal protection; **US 5.1**.
- **Entry prerequisites:** W6.1, Phases 1–3.
- **Verification:** `tests/api-handlers-list-tags.test.ts`, `api-handlers-container-tag-traversal`, `api-handlers-profile-strip`, `bulk-delete`, `turso-api-scope`, `memory-count`, `skills-api-handlers`. With D5, the REST handlers are the *only* web API surface — no Leptos server functions may shim or bypass them.
- **Parallelism:** high — endpoint groups (memories, tags, profile, skills, bulk ops) as separate slices.
- **Tracer-bullet slices:** (a) memories CRUD + search endpoints; (b) tags endpoints; (c) profile endpoints (strip rules); (d) skills endpoints.

### W6.3 — Session history endpoints (new)
- **Delivers:** `GET /api/projects/:project_id/sessions`, `DELETE .../sessions/:session_id` powering the new **Leptos/Thaw** "Session History / Timeline" tab; **US 5.1** (new endpoints), closes **Epic 1** UI-side.
- **Entry prerequisites:** W4.2, W6.2.
- **Verification:** new endpoint tests (contract mirrors upstream handler conventions); manual demo against **`web-ui` (Leptos UI)** with the tab showing timeline data.
- **Parallelism:** two endpoints in parallel once the route module shape exists.
- **Tracer-bullet slices:** (a) list/timeline endpoint; (b) delete endpoint + orphan-safe cascade.

### W6.4 — Portability & skills manager
- **Delivers:** export/import of all tiers (project, global, profiles, **session histories** — the new tier ships in the bundle from day one); skills manager + bridging; **US 4.3**.
- **Entry prerequisites:** W6.2; W4.1 (episodic must be in the export).
- **Verification:** `tests/memory-portability.test.ts`, `shared-api-types`, `v2-skills-bridge.test.ts`, `skills-manager.test.ts`.
- **Parallelism:** portability lane vs. skills lane.
- **Tracer-bullet slices:** (a) portability schemas (serde) + round-trip export/import; (b) all-tier bundling incl. episodic; (c) skills manager + bridge.

### W6.5 — Leptos/Thaw web explorer UI (`web-ui`)
- **Delivers:** the new `web-ui` package (outside the workspace graph per D5): Trunk-based leptos 0.7/thaw 0.4 CSR app — ConfigProvider/Theme shell with light/dark, client routing reproducing the upstream Svelte app's screens (**full screen-set parity**: memories list/search, tags, profile, settings, export/import), the same-origin `fetch` REST client with a single Basic-forwarding `Authorization` seam for auth-configured servers (**credential-free localhost is the primary posture — no login/prompts ship**), and the **session-history/timeline tab** consuming W6.3; **US 5.1**, closes Epic 1 UI shipping.
- **Entry prerequisites:** W6.1 (server skeleton + auth + static hosting). Per-screen slices each gate on their own endpoint being present (W6.2/W6.3); the shell slice needs nothing beyond W6.1.
- **Verification:** `trunk build` produces `web-ui/dist` which W6.1's static hosting serves; `wasm-bindgen-test` for REST-client shape; manual smoke per screen against the Rust server; the session-history tab renders W6.3 data.
- **Parallelism:** shell lane (first), then per-screen lanes (each tied to its endpoint group).
- **Tracer-bullet slices:** (a) crate init + Trunk + ConfigProvider theme + client router; (b) REST fetch client helper (new-siamstr `helper.rs` pattern: one `request<T>` fn + typed wrappers + the single `Authorization` seam); (c) memories browse/search screen; (d) tags/profile screens; (e) session-history tab.
- **Research note (D5):** pins — `leptos = "0.7"` (caret, `^0.7.7` floor via thaw), `thaw = "^0.4.8"` (stable line), `trunk` + `wasm32-unknown-unknown` target, `leptosfmt` for view-macro formatting. No `nightly` leptos features (stable toolchain everywhere; revisit only if wasm perf ever matters).

---

## Phase 7 — Hardening, Parity Audit & Drop-in Rehearsal

**Goal:** nothing new — prove the port is a drop-in replacement and mop up deferred behavior.

### W7.1 — Deferred behavior sweep
- **Delivers:** auto-update service (`tests/auto-update.test.ts`), request-timeouts (`src/services/request-timeouts.ts`), i18n translations (`tests/i18n-translations.test.ts`), runtime-require/diagnostics parity (`tests/runtime-require.test.ts`, `ai-cleanup` diagnostics).
- **Entry prerequisites:** Phases 5–6.
- **Verification:** the named test ports.
- **Parallelism:** one slice per behavior.
- **Tracer-bullet slices:** (a) auto-update; (b) request timeouts wiring; (c) i18n.

### W7.2 — Full parity matrix & upgrade rehearsal
- **Delivers:** a coverage matrix mapping every one of the 110 upstream test files to a Rust test (ported / covered-by-superior-test / N-A); an end-to-end rehearsal: TS plugin uninstalled, Rust plugin installed, same project, memories readable, **web UI functional (the port's Leptos UI; the REST contract additionally keeps the upstream Svelte build usable)**.
- **Entry prerequisites:** all prior waves.
- **Verification:** the matrix document itself + rehearsal transcript; zero unexplained red cells. Doc authority consistency: OVERVIEW.md ↔ roadmap.md §0b (storage crate, UI primacy, Epic 1 split, authority claim) agree; zero unexplained red cells.
- **Parallelism:** matrix audit vs. rehearsal script.
- **Tracer-bullet slices:** (a) parity matrix audit; (b) upgrade rehearsal script; (c) doc authority consistency row.

---

## Risks & Unknowns (with mitigation sketches)

| # | Risk | Why it matters | Mitigation sketch |
|---|------|----------------|-------------------|
| R1 | **ort/ONNX parity** — ort 2.0.0-rc.14 targets ONNX Runtime **1.31.0** vs upstream's pinned `onnxruntime-node` **1.30.0** (same engine, one minor apart); the engine is not the parity risk — **post-processing is**: pooling/normalization/rescale that transformers.js applies per model config must be hand-rolled in Rust. CPU FP16 `Gemm`/`MatMul` is acceleration-gated since ORT 1.30 (nodes without a matching kernel fall back to FP32); fp16-embedding numeric impact unverified (medium confidence). | Wrong pooling/norm math silently degrades retrieval quality for every downstream consumer. | W1.4 implements the fastembed-rs recipe (verified: ort + HF tokenizers + nomic-embed-text-v1/v1.5 incl. quantized variants); golden-vector tests against recorded TS outputs (tolerance-bound); pin `ort = "=2.0.0-rc.14"` exactly (rc-to-rc breaking is possible); prefer weight-quantized models (`_quantized`/`_int8` — int64 inputs identical, caller-opaque) for the q8 path. No blockers found; exhaustive issue sweep not done (medium-high confidence). |
| R2 | **Engine parity** — RESOLVED BY RESEARCH downward: the `turso` crate runs the same engine as upstream's `@tursodatabase/database` JS binding, and `vector32`/`vector_distance_cos`/`vector_extract` are documented verbatim (§0b D1/D2). Residual: unknown-issues sweep for "turso crate + vector + local file DB" was unauthenticated (unverified), and turso engine docs.rs builds fail ≥ 0.8.0-pre.13 (docs only). Wart vs the old "libsql fallback" plan: the Rust `libsql` crate has **no vector support at all** — it is not a viable fallback. | A silent SQL-behavior delta would corrupt search semantics. | Golden-transliteration tests: run the exact statements extracted from upstream `vector-search.ts`/`vector-utils.ts` in W1.2/W1.3; any mismatch is a blocking defect, not an adaptation. The old "compute-in-Rust cosine fallback ADR" is retired — if a vector function ever regresses, the §0b D3 feature-gated rusqlite+sqlite-vec escape hatch is the recorded alternative, not libsql. |
| R3 | **Multi-process access & locking** — RECONCILED: the turso engine explicitly lists **"No multi-process access, No multi-threading"** (manual §Limitations), while upstream's tests (`turso-multiprocess`, `profile-cold-buffer-cross-process`, `profile-cold-buffer-worker-shortwrite`) demand multi-process safety. The upstream answer is application-level serialization — `operation-lock.ts` and `learning-lock.ts` file locks around every DB session — and the port keeps exactly that: raw concurrent DB access never happens; processes coordinate through file locks. Residual risk is file-lock semantics in Rust (`fd-lock`/`fs4`): stale-holder recovery and short-write hardening must match upstream precisely, including on the user's test loopback. | Silent corruption is the worst failure class; a naive `Connection`-per-request design would violate the engine's constraints. | W1.5 lock slice and W3.1 cold-buffer ports first, run cross-process fixture tests in repeat loops; treat any flake as a blocking defect. The §0b D3 escape hatch (rusqlite + sqlite-vec + WAL) is the recorded alternative if true concurrent-read access ever becomes a product requirement. |
| R4 | **Encryption-at-rest compatibility** — SHARPENED: `turso` crate `experimental_encryption(true)` + `EncryptionOpts` gives aegis256/aegis128l/aes256gcm-family ciphers, but encrypted files are **proprietary-format, not readable as standard SQLite** (explicitly not SQLCipher-compatible); it ships behind an "experimental" flag. Which cipher/mode upstream TS shards actually use is an **open question** (did upstream JS encryption use the same engine-level codecs, and which?). | Existing encrypted shards must remain readable, or every upgrade starts with a migration. | W1.5 slice (a) = read-only compatibility spike on a shard produced by the TS version before any migrator work; ADR documents the chosen cipher + key-derivation scheme and whether the experimental flag's risk profile is acceptable. If turso's experimental encryption proves unsuitable and SQLCipher-grade at-rest is required, the §0b D3 switch trigger (a) fires — rusqlite + `bundled-sqlcipher` + sqlite-vec — as a deliberately costed move. |
| R5 | **Plugin bridge strategy (JS/N-API vs JSON-RPC stdio)** | Determines packaging, latency, and shutdown semantics; hard to reverse once users depend on it. | W5.0 decision gate; stdio-first keeps it reversible; shutdown tests (`plugin-shutdown`) ported in W5.1 not deferred. |
| R6 | **V2 adapter complexity** — upstream V2 wraps V1 with a legacy client and injects a fake server URL (`v2/plugin.ts`); reproducing this in a JS wrapper around Rust is nontrivial glue. | Drop-in promise. | Keep the JS wrapper thin and port the V1 plugin surface first (W5.1) so the V2 adapter has the same legacy contract to wrap. |
| R7 | **110-test parity debt** — 110 upstream test files; porting all verbatim is expensive and some (bundle-boundary, package-dependencies) don't translate. | Over-engineering or missed behavior. | W0.3 conventions doc declares three dispositions (port / covered-by / N-A); W7.2 audits the matrix. |
| R8 | **Web UI contract drift** — `api-handlers.ts` is 1.8k LOC and the UI (upstream Svelte app; the port ships its own Leptos replacement per §0b D5) is the only consumer; undocumented response shapes. | Silent UI breakage. | W6.2 slices derive endpoint tests from the *upstream handler code + tests*, and a manual smoke against the Rust server per endpoint group (from the upstream Svelte build until W6.5 screens take over). **The REST contract is the compat guarantee** — keep it hand-written and un-shimmed per D5. |
| R9 | **Context-injection token budgets** — `context-limit.ts` behavior interacts with model context sizes; Rust port must match trimming order. | Prompt degradation. | W0.3 ports context-limit with exact tests before W5.3 consumes it. |
| R10 | **Epic 1 is net-new** — no upstream tests define episodic behavior; risk of designing against ourselves. | The only feature with no TS oracle. | Phase 4 slices each define their own acceptance tests first; consolidation thresholds/config surfaced in config (W0.2 schema) so tuning needs no recompile. |
| R11 | **Thaw maintenance thinness & the leptos-0.9 lag** — thaw's last release is 0.4.8 (2025-08-03), last repo push 2026-05-07 (searcher digest); leptos 0.9.0 stable is imminent (rc0 2026-10-15) but Thaw shows zero sign of a 0.9 migration; the leptos-0.8 line requires the thaw 0.5.0-beta prerelease we deliberately avoid. A candidate issue #431-class panic (reactive_stores `Field` + checkbox under `at_unkeyed`) could hit this UI's forms. | A UI-blocking thaw defect has no fast upstream fix. | Freeze the pairing `leptos 0.7.x + thaw ^0.4.8` (§0b D5) — do **not** chase either line's newest. Localize thaw imports: all Thaw component usage sits behind `web-ui`'s `components/`+`theme` modules (new-siamstr pattern) so a vendor-and-patch fork touches one module. Fallback already proven upstream: leptos is styling-agnostic — plain CSS via Trunk is the escape hatch, and the REST contract keeps the upstream Svelte build usable against the Rust server as a stopgap UI. Upgrade trigger: never on an unstable Thaw pairing; only when a *stable* thaw exists for a newer leptos AND an ADR amendment records it. |

---

## Phase/Wave summary table

| Phase | Theme | Waves | Exit capability |
|-------|-------|-------|-----------------|
| 0 | Foundation & workspace | 3 | Compiling workspace, config/errors/logging, test conventions |
| 1 | Memory engine (Epic 2) | 5 | CLI put/search against encrypted, sharded, fallback-capable store (`turso` crate, app-level locking; escaped-hatch feature stubbed) |
| 2 | AI clients & privacy (Epic 4.1–4.2) | 3 | Trait + 8 providers + privacy filters, host-independent |
| 3 | Profile learning (Epic 3) | 3 | Passive learning w/ cross-process locks, decay, AI cleanup |
| 4 | Episodic core (Epic 1, storage-side) | 3 | Session memories stored, queried, consolidated — headless |
| 5 | Plugin bridge & wiring (Epic 5.2, Epic 1 hooks) | 4 (+decision) | Drop-in plugin: capture, injection, tools, toast/TUI |
| 6 | Web backend, UI & portability (Epic 5.1, US 4.3) | 5 | Full REST parity + Leptos/Thaw explorer UI + session-history endpoints + export/import |
| 7 | Hardening & parity | 2 | Parity matrix green, upgrade rehearsal passed; §0b D3 hatch closed-or-confirmed |

**Proposed ADRs to open during execution:** (1) bridge strategy (W5.0) — landed as `docs/adr/0007-bridge-strategy.md`; (2) ~~vector-function parity fallback~~ retired by research — replaced by **storage-engine ruling: `turso` crate 0.8.x, vector SQL transliteration, engine constraints (single-process) handled at app level** (§0b D1/D2, opens W1.2); (3) encryption cipher/mode + key-derivation ruling incl. the open upstream-shard question (R4 outcome, W1.5); (4) workspace crate decomposition rules (W0.1); (5) **sqlite-vec escape hatch** — considered-and-deferred experimental feature gate (~`rusqlite`+`bundled`+vec0 second `Storage` impl), with three exact switch triggers (std SQLCipher at-rest / true multi-process access / int8-bit-ANN needs) and their costs (file-format readability unverified, blob re-encode, SQL rewrite, C toolchain, 0.x drift); default backend remains `turso`-crate native vectors, unambiguously (§0b D3, opens W1.3 or closes W7.2). (6) **Web explorer UI stack: Leptos CSR + Thaw 0.4 over the axum REST API; rejected full-stack SSR** (§0b D5, W6.1/W6.5) — landed as `docs/adr/0003-web-explorer-ui-stack.md`.
