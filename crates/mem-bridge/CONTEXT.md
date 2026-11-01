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

**Host wiring (ADR-0027, ADR-0029 — transport, wire shapes, schema transport):**

- **host endpoint / service file** — the live OpenCode host records itself in
  `$XDG_STATE_HOME/opencode/service.json` (`0600`: `{id, url, pid, password,
  version}`; `XDG_STATE_HOME` absent ⇒ `$HOME/.local/state`). `host_endpoint`
  resolves `{origin}/api` plus a basic-auth credential, over ONE read+parse
  helper that wraps the password in `Zeroizing` immediately after the
  deserialize. Ten closed reasons (`host_override_invalid`,
  `state_dir_unknown`, `service_file_missing`/`unreadable`/`malformed`,
  `host_pid_dead`, `host_pid_recycled`, `host_url_unsupported`,
  `host_url_not_loopback`, `host_model_unresolved`), each with exactly one
  pinning input in `tests/host_endpoint.rs`.
- **two branches** — an `opencodeBaseUrl` override is branch A (the file is
  then credential-only: same-origin credentials only, no liveness probe); the
  file is branch B (liveness, then start-time recycling, then URL shape, then
  credentials, then the model ref). `"inherit"` is never forwarded.
- **`LazyHostProvider`** — resolve on use, cache on success, clear on ANY inner
  error (a foreign listener on a recycled port answers `401`/`404`/junk, never
  `Network`). `warm()` is the startup probe and never caches a failure. Model
  and provider come from the wiring, and they travel on the GENERATE body as
  `model: Model.Ref{id, providerID, variant?}`; `"inherit"` omits the key
  rather than forwarding the sentinel.
- **the documented wire** — ONE stateless request per attempt:
  `POST {base}/experimental/generate/text`, body `{"prompt": …}` plus `model`
  when one is resolved, no query string (`parameters: []`,
  `additionalProperties:false`). The reply is decoded from
  `{"data":{"text": …}}`; the JSON schema rides in the prompt TEXT
  (`capture_context::SCHEMA_ANSWER_INSTRUCTION`) because the route declares no
  `format` field, and the text is recovered by upstream's three-shape scan
  (direct JSON → fenced block → last balanced object). No session is created,
  named, deleted or tracked. A reply without the envelope is a NAMED
  `host_wire_shape_mismatch` / `errorKind: "wire-shape"`, never `protocol`; a
  text with no structured value is `capture_output_not_json` /
  `errorKind: "model-output"`, retryable, carrying only the reply's byte count.
  Against these documented shapes a capture reaches persistence — but every
  shape here is READ, not measured: no exchange with the live host has ever
  been captured inside the test suite, so live behaviour stays unconfirmed.
- **named failures** — `Auto-capture provider ready` (`baseUrl`,
  `hostVersion?`, `providerId`), `Auto-capture provider unavailable`
  (`reason`, `hostVersion?`) and `Auto-capture provider failed` (`captureId`,
  `reason`, `errorKind`, `status?`, `detail?`, `retryAfterMs?`).
- **retryability is per class, carried by the closed token** — every declared
  host error `_tag` decodes to one of eleven `wire_reason` tokens
  (`invalid-request`, `unauthorized`, `provider-not-found`, `model-not-found`,
  `location-not-found`, `model-unsupported`, `provider-request-retryable`,
  `provider-request-terminal`, `service-unavailable`, `route-moved`,
  `unrecognised`) plus a bounded `detail` built from the declared fields; the
  message stays fixed per status. A permanent class (a `422`, a moved route, a
  terminal `502`) dead-letters on the first drain and consumes no attempt. A
  `retryAfterMs` at or below `HOST_RETRY_AFTER_CEILING_MS` (30 s) is retried;
  ABOVE it the capture is dead-lettered **permanently** — dead rows are
  excluded by the `status = 'queued'` inbox select and nothing requeues them,
  so the retained `payload` is the only recovery path (the missing scheduling
  column is tracked as issue 110). The retry-exhausted dead-letter payload
  carries `reason` plus `errorKind`/`status`/`detail`/`retryAfterMs` when
  present, so no capture dies anonymously.

**Manual capture provider (ADR-0028 — the documented `memoryProvider` fallback):**

- **manual provider** — `manual_provider` builds a real adapter from
  `memoryProvider`/`memoryModel`/`memoryApiUrl`/`memoryApiKey` through
  `ProviderRegistry::create_structured` (no longer a factory with no production
  caller; the STRUCTURED handle is what capture needs — ADR-0029).
  The key is the RESOLVED secret, seated in a `StaticSecretResolver` under the
  synthetic `memoryApiKey` reference and sent per call as
  `Authorization: Bearer` — never stored on the provider, never logged.
- **precedence (owner ruling, ADR-0028 as amended)** — a NAMED OpenCode host
  (`opencodeProvider` or `opencodeModel` set) wins and is warmed once; only with
  the host unnamed is a COMPLETE manual block the fallback. A CLAIMED but
  unusable block on that unnamed-host branch fails closed with
  `reason: "memory_provider_unusable"` rather than silently switching provider;
  beside a named host it is never evaluated. "Claimed" includes an explicit
  `memoryProvider` key (carried as `Config.memory_provider_claimed`: the
  resolved value always exists, so only the file layer can say it was named).
  `engine_capture_provider` is the single place that decides — and
  `run_in_runtime` is its production caller, passing the selector's result to
  `build_composed_runtime`. `tests/engine_capture_provider.rs` (root package)
  spawns the built engine over a config carrying BOTH blocks to pin that call
  site on the named-host arm. The `Auto-capture provider ready` line names the
  winner with the endpoint's userinfo stripped.
- **cleartext endpoints are named** — `http://` to a NON-loopback host while a
  key is attached emits one `Auto-capture provider insecure endpoint` line
  (`baseUrl`, `scheme`, `hint`), because the key then rides `Authorization:
  Bearer` in the open (ADR-0028 D-28-6). Loopback `http://` and every
  `https://` endpoint stay silent.
- **status report agrees** — `get_auto_capture_provider_status` stays the
  upstream-parity report (opencode first) and is NOT the wiring's rule; since the
  amendment the two agree: both name the host when `opencodeProvider` and
  `opencodeModel` are set.
- **the host arm never touches the manual block** — with the host named, the
  manual block is not built, so `memoryApiUrl` never reaches the sink and the
  cleartext warning cannot fire. An `env://` manual key that cannot resolve
  still fails `Config::load` (exit 70) regardless of which arm would win:
  config resolution runs before selection.
- **`env://` needs the variable in the ENGINE's process** — the wrapper's
  `childEnvironment()` whitelist does not forward arbitrary variables, so
  `env://LITELLM_API_KEY` fails at config load with a message naming it
  (engine exits 70). A `file://` reference works today (create it `0600`: the
  loader checks no permissions); a wrapper allowlist knob is a separate
  security decision.

**Capture provider seam (ADR-0029 — capture asks for structured output):**

- **one capability, no streaming** — `capture_provider::StructuredCapture` exposes
  `complete_structured(request, schema)` and nothing else; `ProviderCaptureSummarizer`
  holds that handle and NO `AiProvider` at all. The streaming arm is unreachable, so a
  schema-free capture cannot be written here. This is the measured defect it closes:
  7 of the 8 concrete `complete` impls drop `request.json_schema` (only
  `opencode_host::run_chunks` honours it), so the prompt's markdown `FORMAT:` block used
  to come back where JSON was expected, and every attempt dead-lettered as
  `summarizer_failed`.
- **three adapters** — `Structured` (a registry handle, delegated verbatim),
  `HostStructured` (the lazy host, resolved on use, cache invalidated on ANY error), and
  `FilteredStructured` (`pub(crate)`, minted only by `build_composed_runtime`: it calls
  `privacy::filter_request_for_delivery` first and returns the reply unfiltered, D8).
- **`CaptureProvider { base, structured }`** — `engine_capture_provider` returns BOTH
  halves of one provider. `host_arm` still warms once (`Auto-capture provider ready` is
  emitted only from `LazyHostProvider::resolve()`, so deleting the warm deletes the
  operator's only startup evidence). The pairing is a composition invariant, not a type
  guarantee.
- **`ComposedRuntime.provider` now means "the filtered capture handle"** — anything that
  expected an `AiProvider` there is a compile error by design. The consolidation
  summarizer deliberately keeps the UNWRAPPED `base` (D-52-7).
- **this does not make capture persist** — the host arm now speaks the DOCUMENTED wire (create decoded
  in both shapes, the prompt on `/generate` carrying exactly `{"prompt"}`, the reply decoded from
  `{"data":{"text": …}}`), with the JSON schema appended to the prompt TEXT because that route has no
  `format` field; a shape mismatch is named `host_wire_shape_mismatch` / `errorKind: "wire-shape"`. What
  is left is runtime confirmation (no exchange with the live host has ever been captured), the
  create-request ruleset rebuild (`permission` vs `permissions: Permission.Ruleset`) and `requeue-captures`.

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
- **profile decay lifecycle** (`spawn_profile_decay_lifecycle`, issue 90) — the
  production caller of `ProfileStore::sweep_decay_daily`: ONE tracked task that
  sweeps once at startup (behind the handshake, never awaited before
  `serve_stdio`) and then wakes every `PROFILE_DECAY_TICK_SECS` (3600). The
  wake-up cadence is only how often the gate is ASKED; the 24 h gate is the
  profile store's own and is in-memory (issue 102). Its seam is the
  `DecaySweeper` trait (the store is the production impl). Two named stderr
  events, counts and closed-set kinds only: `profile.decay_tick` (only when
  something was scanned) and `profile.decay_failed` (`kind`: `lock_poisoned` with
  `lock`/`operation`, or `store`); a failed tick never ends the loop.
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
