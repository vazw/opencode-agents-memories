# mem-bridge

**Purpose:** OpenCode host integration: V1/V2 plugin bridge, stdio server, TUI companion.

**Ubiquitous language (W5.1, ADR-0007):**

- **engine** — the single Rust process that owns the storage root's databases. It
  serves the fifteen-method JSON-RPC surface over stdio (as the wrapper's child) and,
  in this same process, the axum web server (`WebLauncher` seam, D-32). Lives in
  `engine.rs`; the root composition is a sync arg-parser around it (M1).
- **launcher mode / launch mode** — `stdio` (per-request JSON-RPC over stdin/stdout,
  no signal handler), `standalone` (axum + graceful SIGTERM/SIGINT), and the
  env-gated test-only `hang-on-shutdown` (parks after the shutdown response so the
  wrapper's SIGTERM fallback has a real target).
- **wrapper** — the JS host-side package (`plugin/`), NOT part of this crate; the
  wrapper spawns the engine child and degrades fail-open when absent.
- **hello handshake** — the wrapper's first request; the wrapper applies the
  major-equal + floor compat rule on the reply. `hello` is not a runtime method.
  The reply's additive `capabilities` object is how the wrapper learns which
  behaviours this build serves — e.g. `compaction` (issue 68) and `skills`
  (issue 69: this build's dispatch answers the `skills` method).
- **session_id** — the required top-level parameter of every runtime method
  except `skills`, which takes `project_id` instead and carries no session id
  (1..=128 chars); DATA, never authority.
- **the nine tool methods** (issue 70 wave 1) — `add`, `search`, `forget`,
  `list`, `profile`, `list-shards`, `migrate`, `export`, `import`. Each carries
  `session_id` (`forget` adds `project_id` and `memoryId`; the two inventory
  methods take `project_path`); only `add` persists the session id, no read
  filters by it. `dispatch` answers them from `EngineRuntime.tools`; a runtime
  without it degrades (-32001). The toolkit that maps them to upstream's
  `memory` tool is wave 2, the portability tool modes wave 3.
- **capture_id** — the client-minted UUIDv7-shaped, ≤64-char identifier added to
  `agent_sessions` by the v4 additive migration (recall: W5.1 has no write path
  for it; W5.3 owns the recapture/upsert policy).
- **engine lock / `.engine.lock`** — the lifetime exclusive lock (mem-store
  `engine_lock`); its body is `LockState` JSON, and the LOSER writes the status
  file naming the HOLDER's pid and exits with code 75 (`ENGINE_LOCK_LOST`).
- **status file** — `<storage_root>/status`; one line
  `degraded: <short reason> (pid=<n>, ts=<UTC ISO-8601>)`; a report, not a policy.
- **DB lane / `SerialDbLane`** — the single ordered drain (agent queue before web
  queue) behind the `DbLane` trait; LLM summarisation is rejected at submit and
  never rides the lane (D-33).
- **canonical serialization** — compact JSON, lexicographic keys at every level
  (`docs/bridge-schema.md` §1); both lanes must reproduce the frozen fixture bytes.
- **volatile fields** — `uptime_ms`, `handling_ms`, `engine_version`, `db.path`;
  masked in response-schema comparisons (contract gate M37).
- **error map** — the specific codes -32700/-32600/-32601/-32602/-32603/-32001
  (MEM_DEGRADED)/-32002 (MEM_INBOX_FULL, W5.3's number); never a substituted -32603.
- **shutdown pipeline** — ordered teardown: web stop wrapped in ~1 s, DB flush
  ≤ 500 ms, total < 2 s (D-45); stdio EOF and standalone signals both converge on it.

**Deferred register rows (orchestrator; W6.2 owns `docs/agents/testing.md`):**
the `-32002` code row, the exit-code-75 row, the status-file-format row.

**Open questions (user's, ADR-0007):** (a) second-instance lock policy; (b) the
`darwin/x64` engine target matrix. Nothing in this crate answers either.

**W7.1 additions (sound-memory capture, cross-tier promotion, purge CLI):**

- **SoundMemorySettings** — `featureFlags.soundMemory` +
  `soundMemory.rawContextRequired`, set with the builder setter
  `CaptureService::with_sound_memory` (default OFF). With it on, the capture
  drain persists the privacy-stripped raw context and its derived segments
  after the durable upsert, falling back to the transcript when no
  `raw_context` was supplied; `rawContextRequired` dead-letters a payload
  with no raw text before the summarizer. An invalid raw context or an
  unknown `agent_role` is dead-lettered with a fixed message that never
  echoes the value. The supplied `agent_role` is persisted with one targeted
  `UPDATE` (no session constructor changes).
- **FactsCrossTierTask** — the production `CrossTierTask`: classifies each
  promotable fact (`mem_core::tier::classify_fact_tier`), writes `facts.db`
  rows with `source_ref` = the milestone id, records one provenance row per
  contributing `SessionRef`, skips candidates over 8192 bytes, and is
  idempotent for the same milestone. `cross_tier_task(enabled, facts,
  ttl_days)` wires `NoopCrossTierTask` when `featureFlags.promotionPipeline`
  is off or no store is composed.
- **`--mode=purge-session`** — the one sanctioned erasure path. Flags:
  `--project`, `--session`, `--agent` (all three: the triple is the key) and
  `--confirm="{project}:{session}:{agent}"` (verbatim). Root:
  `--storage-root`, else `OPENCODE_MEM_STORAGE_ROOT`, else
  `$HOME/.opencode-mem/data` (the JSONC `storagePath` is NOT the engine
  root). It takes the same in-process gate and cross-process operation lock
  as consolidation, BEFORE the milestone pre-scan; the facts leg runs first
  and the episodic leg second. It skips the facts leg entirely when
  `{root}/facts.db` is absent (`facts_leg = "skipped_no_database"`, never
  creates the file). Exit codes: **0** erased, **64** usage/confirmation/
  scope, **74** store-or-IO, **75** busy. Success prints exactly one JSON
  line with counts and a mandatory, non-empty `not_erased` array naming the
  surviving legacy `memories` shard.

**Issue 43 W7 additions (retired-shard reaping, ADR-0023):**

- **`--mode=reap-shards`** — removes the files of shards a scope has already
  retired. Flags: `--apply` alone is the confirmation (the default is a dry
  run) and `--storage-root` (the same root resolution as the other sync
  modes). `--apply=<value>`, `--apply <token>` (non-flag), `--scope`,
  `--project`, `--session`, `--agent` and `--confirm` (any occurrence, bare
  included) are usage errors (exit **64**), validated in `src/main.rs` before
  `Config::load_default` so a bad invocation never creates the root. Exit codes: **0** success including
  nothing to do, **1** a completed report with at least one file-operation
  failure, **64** usage, **74** store/operation failure including a lost
  engine lock (`reap.refused`), **75** busy (`reap.busy`, the operation lock
  or the ready gate's legacy step is held); any other store error is
  `reap.failed` + 74. A completed run prints exactly one JSON line
  (`reap.completed`: `dry_run`, `candidates`, `reaped`, `skipped`,
  `bytes_freed`) and one `reap.file_delete_failed` stderr line per failed
  file.
- **The CLI dry run is not side-effect-free.** The ready gate's legacy step
  reconciles the registry before the report, so a dry run may adopt an orphan
  shard file as a row (it never removes files). The store-level dry run
  mutates nothing. The reaper closes the ready gate's cached read-write
  connections before its predicate: a cached writable handle answers the
  `-tshm` liveness query `Held` for this same process.

## W8a additions

- `vault_source_ref` (single construction site for vault `source_ref` strings).
- `VaultFeeder` (session-end feeder: user-marked verbatim content only).
- `CaptureParams.vault` flag and `CaptureService.with_vault_feeder` builder.
- `mem-bridge → mem-vault` dependency edge (ADR-0001 W8a amendment).

- Provenance is mandatory: `VaultEntry.source_ref` / `VaultRecord.source_ref`
  are `String`, never `Option`. The feeder always supplies a ref via
  `vault_source_ref(Session, …)`; content with no external referent uses
  `VaultSourceKind::Manual` → `manual:<opaque>`. The API enforces presence,
  not truthfulness.

**Issue 44 additions (the profile tier):**

- **`ProfileInjection`** — `inject`'s typed profile-tier outcome carried INSIDE
  `Ok`: `Disabled`, `NotWired`, `NoProjectScope`, `NoIdentity`, `NoProfile`,
  `Unusable`, `Empty`, `Rendered`. Never an error, never a silent skip, and
  `NotWired` (no `.with_profile`) is distinct from `Disabled` (the knob is off)
  so a forgotten composition line is visible. The tier is fail-soft: no arm
  turns an `Ok` into an `Err`.
- **`profile inject` counter + the profile block's accounting** — emitted once
  per `Rendered` block with `data: None` (ADR-0014 D-2; no profile text, no
  identity). The block is PREPENDED to `<project_agent_session_history>`; the
  history bytes are untouched, `memory_count` counts history rows only, and
  `tokens` is the byte estimate of the emitted block INCLUDING the profile —
  `0` iff there is no block, on the normal path and on the `included.is_empty()`
  early return alike.
