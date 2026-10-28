# 0007 — Bridge strategy: JSON-RPC over stdio between the JS plugin shell and the Rust engine

- **Status:** Accepted
- **Date:** 2026-10-20
- **Wave:** W5.0 (Phase-5 entry decision gate; issue 18 — US 5.2)
- **Amends:** nothing. **Builds on:** `docs/adr/0004-database-encryption-at-rest.md` (the cross-process operation lock),
  `docs/adr/0005-episodic-storage-model.md` (the `episodic.db` storage model, ADR-0005 D-1 … D-16),
  `docs/adr/0006-episodic-consolidation.md` (D-23 the hard delete, D-25 the lock discipline).
- **Grilled:** user grilling session 2026-10-20 — ten decisions ratified (D-30 … D-39 below); the twelve
  decisions of the earlier session stand unchanged (the issue-18 `Status:` line records
  "grilling complete: 12 decisions resolved").
- **User rulings (2026-10-20):** "Accept the bounded guarantee" (the `capture_id` cardinality, USER RULING 1)
  and "Durable capture inbox before acking" (the durable capture inbox, USER RULING 2 → D-48).
- **User ruling (2026-10-21, grilling session):** "Degrade, loudly" (a second OpenCode instance on the same
  storage root, USER RULING 3) — see *Documented limitations* (ii) and D-38's fail-open path; and "Omit
  `darwin/x64`" (the engine target matrix, USER RULING 4) — see *Documented limitations* (iv).
- **Context source:** `.scratch/rust-port/roadmap.md` §W5.0, §0b D1/D2/D3, risks R3/R5; `.scratch/rust-port/issues/18-bridge-strategy-adr.md`;
  upstream `example/opencode-mem/tests/plugin-loader-contract.test.ts`, `example/opencode-mem/src/index.ts:491`,
  `example/opencode-mem/package.json`.
- **Blueprint:** `20261020-1602-w50-bridge-strategy-adr-r3` (sha256 `a37f09a977c6`, `SYS-APPROVED 2026-10-20T16:16Z`;
  `USER-APPROVED 2026-10-20` — "Accept the bounded guarantee; Durable capture inbox before acking").

## Context

### The loader contract makes a JS wrapper mandatory

The OpenCode 1.3.x plugin-loader contract is
`type PluginModule = { id?: string; server: Plugin; tui?: never }`
(`example/opencode-mem/tests/plugin-loader-contract.test.ts:2`), and `server` is a **JS function**. The
plugin is loaded with a dynamic ES import of the built module — `loadDistPlugin` does
`import(modUrl)` against `new URL("../dist/plugin.js", import.meta.url).href`
(`plugin-loader-contract.test.ts:16-19`), and the test asserts that the default export is an object whose
`server` property is a `function` and whose `id` matches the package name (`plugin-loader-contract.test.ts:45-64`).
The file's own header states the contract it guards (`plugin-loader-contract.test.ts:1-7`; the quoted
contract sentence is at line 2).

The consequence is plain: **an N-API addon is a `.node` binary, not an ES module, so it cannot BE the
plugin.** The host imports an ES module and calls a JS `server` function; a `.node` file cannot satisfy either
half. The JS wrapper is therefore mandatory under *both* candidate transports, and the real choice facing this
ADR is only the wrapper↔Rust transport.

### The two candidates

1. **JSON-RPC 2.0 over stdio to a Rust child process.** The wrapper spawns the engine, speaks a line-framed
   protocol over the child's stdio, and owns the replay log.
2. **An N-API native addon loaded by the wrapper.** The wrapper stays the host's plugin module and calls into
   Rust native code in-process.

Option 1 is the roadmap's default stance (`.scratch/rust-port/roadmap.md` §W5.0: "Default stance: JSON-RPC
stdio first (reversible, host-agnostic, matches the V1→V2 adapter's server/client split), N-API only if stdio
latency on per-turn hooks proves unacceptable"), and §W5.0 marks the grilling as mandatory before the decision
closes.

### What this ADR must fix

Four consequences follow from the transport choice, and every later wave depends on them being frozen here:

- **Packaging** — what the npm tarball carries and how the wrapper finds the engine at runtime (D-37).
- **Latency** — which operations sit on the host's per-turn critical path, and the measurable trigger that
  would reopen the rejected option (D-35, "Latency and dependency trade-offs").
- **Shutdown semantics** — how the engine is stopped, bounded in time, with no orphan process or open stdio
  handle (D-45).
- **DB ownership** — which process opens the database files (D-31, D-32).

Roadmap risk **R5** states the stakes: "Determines packaging, latency, and shutdown semantics; hard to reverse
once users depend on it", mitigated by the "W5.0 decision gate; stdio-first keeps it reversible; shutdown tests
(`plugin-shutdown`) ported in W5.1 not deferred"
(`.scratch/rust-port/roadmap.md`, risk table R5).

Two upstream facts that later sections lean on:

- **Upstream starts its web server inside the plugin's own process.** `startWebServer({...})` is called from
  the plugin's `index.ts` (`example/opencode-mem/src/index.ts:491`); the import is at
  `example/opencode-mem/src/index.ts:12` and the factory is
  `example/opencode-mem/src/services/web-server.ts:909`.
- **Upstream already ships native artifacts.** `@tursodatabase/database@^0.8.3` and
  `onnxruntime-node@1.30.0` are runtime dependencies of `example/opencode-mem/package.json` (lines 68 and
  71), and that package already carries a documented platform hole: the `"//"` comment at line 74 states,
  *"Intel Mac (darwin/x64) is unsupported — @tursodatabase/database and fixed onnxruntime releases ship no
  x64 binding"*. `example/opencode-mem/AGENTS.md` repeats it: "Intel Mac (`darwin/x64`) is unsupported (Turso /
  onnxruntime bindings)" (line 90).

### The two 2026-10-20 user rulings (binding on this ADR)

**USER RULING 1 — "Accept the bounded guarantee" (2026-10-20).** The `capture_id` cardinality is settled here
as a bounded guarantee, and **ADR-0005 does not change**: the episodic tier keeps one immutable record per
`(project_id, session_id, agent_id)` and the growth model is unchanged. The guarantee the ADR can honour is
stated, with its boundary, in the `capture_id` cardinality block of "Schema freeze" and restated in
"Documented limitations".

**USER RULING 2 — "Durable capture inbox before acking" (2026-10-20).** The engine persists the capture
payload to a durable inbox before it acks, so an ack means *durably queued*. The inbox's storage layout is
W5.3's; its semantics are pinned by D-48. Both rulings are recorded with their date and their verbatim
one-line title; they are **not** re-argued here.

## Decision

The decision counter is continuous across this repo's ADRs (ADR-0005 used D-1 … D-16, ADR-0006 used
D-18 … D-29), so this ADR continues at D-30.

### D-30 — The choice is the transport, not the shape: a stdio child process; N-API is rejected

The loader contract forces a JS wrapper under both options (see "Context"); the decision is therefore the
wrapper↔Rust transport. **The transport is a stdio child process speaking JSON-RPC 2.0.** The N-API native
addon is **rejected** on four grounds:

1. **The host's crash domain.** A Rust panic across the N-API boundary cannot unwind safely and would abort
   the user's OpenCode process together with all of its in-flight host state. A child process dies alone.
2. **Native teardown inside the host's Node runtime.** Upstream itself records this hazard class:
   `example/opencode-mem/tests/plugin-loader-contract.test.ts:71-75` notes that "Bun 1.3.14 can crash during
   process teardown after that native addon was loaded, even though every assertion passed".
3. **Per-platform prebuild burden.** N-API is Node-ABI-coupled, so prebuilds are needed per platform × arch ×
   Node ABI, or an install-time C++ toolchain. The load-bearing counter-fact is that **upstream already pays
   this burden**: `@tursodatabase/database` and `onnxruntime-node` are native runtime dependencies of
   `example/opencode-mem/package.json` (lines 68, 71), and that package already carries a documented platform
   hole (the `"//"` comment, line 74).
4. **Reversibility.** See "Reversibility": the engine's contract is the JSON-RPC schema, not Node's ABI, so
   the move to N-API later is a packaging + transport-module change, not a re-architecture.

Grounds: the loader contract is quoted from `example/opencode-mem/tests/plugin-loader-contract.test.ts:2`
and exercised at lines 16-19 and 45-64; the teardown hazard is upstream's own recorded observation at
`plugin-loader-contract.test.ts:71-75`; the prebuild burden and its counter-fact are the native runtime
dependencies at `example/opencode-mem/package.json:68,71` plus the `darwin/x64` comment at line 74.

### D-31 — One engine process; sessions are logical instances inside it

**One engine process serves every session of a host; a session is a logical instance inside it.** This
**replaces** the earlier phrase "one engine per session": N processes would put N processes on one turso DB
file, and the turso engine explicitly lists **"No multi-process access, No multi-threading"** (quoted in
`.scratch/rust-port/roadmap.md` §0b D3 and risk **R3**), while §0b D3's switch triggers name multi-process DB
access as one of the three triggers that would open the deferred `rusqlite` + `sqlite-vec` escape hatch. R3
names silent corruption as the worst failure class.

Grounds: the single-process limitation is quoted from the turso engine manual via `.scratch/rust-port/roadmap.md`
§0b D3 and restated as risk R3, which also records the application-level serialisation the port keeps and the
silent-corruption failure class that motivates it.

### D-32 — The axum web server runs INSIDE the engine process; the engine is the sole DB owner

**The axum web server runs inside the engine process, and the engine is the sole owner of every DB file.**
`crates/mem-web/Cargo.toml` depends on `mem-store` (it opens the DB), so placing the web server inside the
engine is what makes that ownership single. Upstream parity: upstream's `startWebServer` is called from inside
the plugin's own process (`example/opencode-mem/src/index.ts:491`). Consequence: the web UI talks HTTP
directly to the engine's axum port, so the bridge schema needs **no** web namespace and stays at the minimal
five methods (D-40).

Grounds: `crates/mem-web/Cargo.toml` carries the `mem-store` edge that makes the web server a DB opener, and
`example/opencode-mem/src/index.ts:491` shows upstream calling `startWebServer` from the plugin's own process.

### D-33 — One DB lane, agent-first priority; the lane covers DB work ONLY

**A single serialized lane serves every DB read and write.** The lane covers DB work **only**: the capture
path's summarisation is an LLM call measured in seconds and **must not hold the lane**, or the inject path
inherits seconds of latency. Agent requests preempt queued web-UI requests: a queued agent request always runs
before a queued web request.

Grounds: one engine on one turso file (D-31, §0b D3 / R3) permits no concurrent DB access, so all DB work is
serialised by construction; the LLM summarisation is not DB work and is therefore excluded from the lane.

### D-34 — Every request carries an explicit `session_id`

**`session_id` is a required parameter of every request.** With one process serving many sessions the session
is no longer implicit, so it is carried in the request params of **every** method (see D-46 for the exact
placement and the reserved engine self-check value). It is self-describing, stateless, safe on the single lane
with interleaved sessions, and it makes restart-and-replay correct by construction.

Grounds: D-31 replaces one-process-per-session with one process serving many sessions, so the session can no
longer be inferred from the process; the request carrying it is what keeps interleaved work on the single lane
attributable, and it is what makes a replay after a restart target the same scope.

### D-35 — Only `inject` blocks the host; `capture` is fire-and-forget

**`inject` is the one method on the host's critical path; `capture` is fire-and-forget.** `capture` is
acknowledged immediately, summarised in the background, and its DB work runs outside the lane's blocking path.
**The measurable trigger that re-opens the N-API question is: inject-path bridge overhead > 5 ms at p95 over
100 turns on the reference machine.** Only a pipe round-trip is on the critical path, so the attribution is
clean. The ack's *meaning* is sharpened by D-48: the ack is returned once the capture is **durably queued** in
the inbox, which is a local write of milliseconds — not the LLM call — so "only `inject` blocks the host" and
the capture path's off-critical-path summarisation both stand.

Grounds: the per-turn hook that the host waits on is injection only, so only that path needs a transport
budget; the trigger is stated as a measurement (5 ms at p95 over 100 turns) so re-opening D-30 is an
observation, not an opinion, and `handling_ms` (D-46) makes the transport share of it separable.

### D-36 — `capture_id` is a column on the episodic row the capture created

**`capture_id` is a column on the episodic row the capture created.** Idempotency = "a row with this
`capture_id` already exists → skip". The idempotency window is inherited from the row's own lifecycle, and
W4.3's consolidation **hard delete is the purge** (issue 17's decision (c), recorded as ADR-0006 D-23). No new
table, no new growth surface. An in-memory set would break across the very restart that restart-and-replay
exists for. The column's **cardinality** is a normative consequence of this decision, recorded in the schema
freeze as the `capture_id` cardinality block; it is **not** a separate decision and does **not** change
ADR-0005.

Grounds: `crates/mem-episodic/src/store.rs:291-293` is the insert-only create path (a UNIQUE violation maps to
`EpisodicError::Conflict`; there is no upsert), and ADR-0006 D-23 records the hard delete that removes the row
— so the row's lifecycle is the idempotency window by construction rather than by an added store.

### D-37 — The engine binary ships as a per-platform file inside the npm tarball

**The engine ships as ONE binary per (platform, arch) inside the npm tarball**, selected at runtime by
`process.platform` / `process.arch`. Its contract is the JSON-RPC schema, **not** Node's ABI, so a Node upgrade
never forces a rebuild.

Grounds: upstream already ships native artifacts and already has a documented platform hole — the D-30 ground
(iii) evidence, `example/opencode-mem/package.json:68,71` and the `"//"` comment at line 74 — so the packaging
shape is the incumbent one, and a binary whose contract is the protocol rather than the ABI is what removes
the Node-ABI rebuild axis.

### D-38 — Fail-open, but never silently

**When the engine is absent, fails to start, or speaks an incompatible protocol, the plugin still loads and
memory is disabled — but a structured warning goes to stderr AND a one-line status file is written** that the
web UI and the TUI can read (upstream's toast-rpc lands in W5.2). A silently memoryless plugin is worse than a
loud one. The degradation is signalled on every path; `-32001` (D-44) is an additional in-protocol signal for a
running-but-degraded engine, not a replacement for the warning and the status file.

Grounds: upstream's web server and memory live inside the plugin process
(`example/opencode-mem/src/index.ts:491`), so a failed engine removes a capability the user expects; the
warning plus the one-line status file is the ratified signalling path for every degradation, including
`-32001`.

### D-39 — The engine is standalone-capable as well as a child of the wrapper

**The same executable has two launch modes:** with a stdio peer (plugin mode) or standalone (web-only). **An
exclusive engine lock means exactly one engine owns the DB**; the loser exits with the degradation signal of
D-38. The web UI is therefore available whether or not OpenCode is running, with no transport change and no
daemon lifecycle.

Grounds: D-31's single-process constraint needs an enforcement point, and the engine lock is it; the web UI
speaking HTTP to the engine's axum port (D-32) already has no dependency on the OpenCode host, so standalone is
the same binary minus the stdio peer. The lock must be a distinct file from `.turso-operation.lock` — see D-47.

## Schema freeze (frozen for W5.1)

This section is the normative input to W5.1's blueprint: it is precise enough to blueprint from without a
further decision.

### D-40 — The runtime method set is exactly five: `capture`, `inject`, `recall`, `health`, `shutdown`

**The runtime method set is exactly these five methods and no others.** There is no web namespace (the web UI
speaks HTTP to the engine's axum port — D-32), no `web.*` methods, and no administrative methods beyond
`shutdown`. A sixth method, `hello`, exists **only** for the handshake (D-43) and is not a runtime method.

Grounds: D-32 puts the web server inside the engine, so the bridge carries no web surface; `shutdown` is the
only administrative operation the wrapper needs because the engine's lifetime is the wrapper's to end (D-45).

### D-41 — Only `inject` blocks the host

**`inject` is the only method whose response the host waits on before continuing the turn.** `capture` is
acknowledged immediately (D-35). `recall` and `health` are off the per-turn critical path. `shutdown` is awaited
only during teardown.

Grounds: D-35 makes the per-turn transport budget a property of injection only, so the blocking set is fixed to
the one method that sits on the host's critical path; teardown is not a turn.

### D-42 — NDJSON framing; stdout carries the protocol only, stderr carries structured-JSON logs

**One JSON-RPC 2.0 message per line, UTF-8, terminated by a single `\n`.** Raw newlines never appear inside a
message (JSON escapes them); there is no length prefix, no framing header, and **no batching** — a JSON array on
one line is NOT a batch and MUST be rejected with `-32600`. Every request carries an `id` (string or number) and
every response echoes it; the bridge has no notifications in v1. **stdout is protocol-only**: the engine's logs,
warnings and diagnostics go to stderr as structured JSON per ADR-0002, so a stray `println!` cannot corrupt the
stream.

Grounds: line-delimited JSON needs no length prefix or header to be unambiguous, and JSON's own escaping removes
the only ambiguity a newline could introduce; ADR-0002 fixes the structured-logging strategy, which is what
makes stderr the diagnostics channel and stdout exclusively the protocol channel.

### D-43 — The `hello` handshake and the semver compatibility rule

**The wrapper sends `hello` as the first message after spawn; no runtime method may precede it.**

```
wrapper → engine
{"jsonrpc":"2.0","id":"hello-1","method":"hello","params":{"client":"opencode-mem-plugin","protocol":"1.0.0","min_protocol":"1.0.0"}}

engine → wrapper
{"jsonrpc":"2.0","id":"hello-1","result":{"protocol":"1.0.0","engine_version":"<semver>","methods":["capture","inject","recall","health","shutdown"],"capabilities":{"fire_and_forget_capture":true,"standalone":true,"web":true}}}
```

The protocol version is a single semver line owned by this ADR; its initial value is **`1.0.0`**. The engine is
**compatible** with the wrapper iff `major(engine.protocol) == major(wrapper.protocol)` **and**
`engine.protocol >= wrapper.min_protocol`. A different major, or a version below the wrapper's floor, is
**incompatible**: the wrapper MUST NOT call any runtime method, and it degrades per D-38. A newer MINOR is
compatible — the engine may add capabilities but MUST NOT remove or change an existing method, parameter or
error code within a major line.

The example above lists the capabilities of the initial build and is not exhaustive: the object is additive, and issue 68 added `compaction`.

Grounds: the handshake is what turns a version mismatch into a D-38 degradation instead of a stream of
`-32601`/`-32602` errors from an engine that cannot serve the wrapper; the major-equal + floor rule is the
smallest rule that admits additive engine upgrades while refusing a breaking one.

### D-44 — Errors: the JSON-RPC 2.0 standard codes, plus one reserved implementation-defined code

**The error codes are:** `-32700` parse error (a line that is not valid JSON), `-32600` invalid request (not a
JSON-RPC 2.0 object, or a batched array), `-32601` method not found (an unknown method), `-32602` invalid params
(**including a missing, blank or over-long `session_id`**), `-32603` internal error (an unexpected engine
failure). One implementation-defined server error is reserved: **`-32001` = `MEM_DEGRADED`** — the engine is
running but memory is disabled (for example the DB could not be opened, or the encryption key is missing).
`-32001` is an **additional in-protocol** signal for the case where the engine runs but memory is disabled;
D-38's stderr warning and the one-line status file **remain the ratified signalling path** for every
degradation, including `-32001`. The wrapper maps **every** engine error to fail-open: an `inject` error yields
an empty injection (never a host error); a `capture` error is logged and never blindly retried; `health` reports
`mode: "degraded"` with a `degraded_reason`. D-48 adds one more distinguishable error for "the capture inbox is
full" — a queue failure is **never** reported as a generic internal error and **never** a silent drop.

Grounds: the five standard codes are JSON-RPC 2.0's own vocabulary, so the wrapper's error handling needs no
private registry for the ordinary failures; `-32001` exists because "the engine answered" and "memory is
available" are different facts, and the wrapper must be able to tell them apart without parsing prose.

### D-45 — The shutdown sequence

1. The wrapper sends the `shutdown` request; the engine replies `{"ok":true}` and stops accepting new requests.
2. The engine flushes and closes the DB, stops the axum server, and exits.
3. If the engine has not exited **2 s** after the shutdown response, the wrapper sends **SIGTERM**.
4. If it has not exited **5 s** after the SIGTERM, the wrapper sends **SIGKILL**.
5. The wrapper waits for the child's stdio to reach EOF so no orphan process or open stdio handle remains
   (W5.1's "no orphaned engine process/stdio handles" AC; upstream's `tests/plugin-shutdown.test.ts` is the
   port target).

**The timeouts are bounded — the host is never held indefinitely.** The inbox is drained on graceful shutdown
(D-48), so a graceful stop does not lose a queued capture.

Grounds: an orderly RPC plus two escalating signals is the minimum that both flushes the DB on the normal path
and still ends a wedged child; the 2 s and 5 s deadlines bound the host's exposure, and the EOF wait is what
prevents the unread-pipe orphan that would otherwise outlive the wrapper.

### D-46 — The request/response parameter shapes (normative)

**`session_id` is a REQUIRED top-level parameter of every method**, including `health` and `shutdown` (D-34).
For the two process-scoped methods it identifies the calling session and is recorded but does not scope the
operation; the engine's own self-check uses the reserved value `"__engine__"`.

| method | blocking | params | result |
|---|---|---|---|
| `capture` | no (fire-and-forget ack) | `session_id` (req), `capture_id` (req), `project_id?`, `agent_id?`, `agent_name?`, `parent_session_id?`, `transcript?`, `summary?`, `metadata?` | `{"accepted":true}` |
| `inject` | **yes** | `session_id` (req), `project_id?`, `agent_id?`, `token_budget?`, `include?` | `{"block":string\|null,"memory_count":int,"tokens":int,"truncated":bool,"handling_ms":int}` |
| `recall` | no | `session_id` (req), `query` (req), `limit?`, `scope?`, `tags?` | `{"items":[…],"count":int}` |
| `health` | no | `session_id` (req) | `{"ok":bool,"mode":"full"\|"degraded","degraded_reason":string\|null,"protocol":semver,"engine_version":semver,"db":{"path":string,"ready":bool},"uptime_ms":int,"sessions":int}` |
| `shutdown` | teardown only | `session_id` (req) | `{"ok":true}` |

`capture_id` is a client-minted idempotency key (≤ 64 chars). The `capture` ack is returned **before** the
summarisation and **before** the DB write, so the ack carries no `duplicate` flag: the idempotency check (D-36)
happens later, on the lane, and a skipped duplicate is recorded in the engine's structured stderr log rather
than in the ack. D-48 sharpens *when* the ack is returned: after the payload is **durably queued** in the
inbox, which is a local write of milliseconds — the ack's result shape and the wire protocol are unchanged.
`handling_ms` in the `inject` result is the engine's own internal handling time; it exists so the bridge
overhead of D-35's trigger is attributable (see "Latency and dependency trade-offs").

Grounds: the parameter list is the minimum each method needs to scope work to a project, agent and session
(D-34) without a per-session process (D-31); `handling_ms` is required by D-35's measurement definition, and the
`{"accepted":true}` ack shape is what keeps D-48's change of the ack's *meaning* invisible on the wire.

### D-47 — Restart-and-replay, and the runtime model

**The engine is stateless with respect to the bridge**: the wrapper owns the replay log and re-sends
unacknowledged requests after a restart. The replay set is exactly **the captures that were never durably queued**
(D-48), and the inbox's `capture_id` makes a re-queued capture safe; the episodic row's `capture_id`
(D-36) makes an applied capture idempotent **while the row carrying it exists**.

**The engine runs a `current_thread` tokio runtime.** The exclusive engine lock (D-39) is a **distinct lock
file** in the storage root — it is **not** `.turso-operation.lock`, because that file is **operation-scoped**,
not **lifetime-scoped**: `acquire_turso_operation_lock(storage_path, probe, operation)` records the operation
and releases it when the operation ends (`crates/mem-store/src/operation_lock.rs:25,127`), while the engine's
lock is held for the engine's whole lifetime, so it must be a **distinct file**. The ground is that
`assert_no_turso_migration_in_progress` checks **both** lock files and fails on **any** live holder regardless
of pid (`crates/mem-store/src/operation_lock.rs:83-96`), so `get_write_shard`
(`crates/mem-store/src/shard_manager.rs:687`) refuses to resolve a shard while the engine's own lifetime-long
`.turso-operation.lock` is live — an engine that held the operation lock for its whole lifetime would block its
own writes (the own-lock-tolerant seam is `get_write_shard_allowing_own_lock`, line 703). A second consequence:
an engine holding the operation lock would also **collide with the W4.3 consolidation run's lock** (ADR-0006 D-25),
so a consolidation run would see the engine as busy. The engine lock is a separate marker with its own
stale-holder recovery.

Grounds: `crates/mem-store/src/operation_lock.rs:83-96` fails on any live holder of either lock file and line
127 records an operation-scoped lock, while `crates/mem-store/src/shard_manager.rs:687` calls that assert before
resolving a write shard — so a lifetime-long operation lock would deadlock the engine's own writes; ADR-0006
D-25 records the consolidation run holding the same operation lock, which an engine lock would collide with.

**The `capture_id` cardinality — a normative consequence of D-36 (USER RULING 1, 2026-10-20).**

This block is **not** a decision and **not** an eleventh decision: it is the consequence D-36's own wording
forces once the episodic row's key is read against the schema. **USER RULING 1 — "Accept the bounded
guarantee" — rules that ADR-0005 does NOT change**: the episodic tier keeps one immutable record per
`(project_id, session_id, agent_id)` and the growth model is unchanged. The five statements, normatively:

1. **The row is keyed by the scope triple.** A capture's row is keyed `(project_id, session_id, agent_id)`
   (ADR-0005's UNIQUE, `crates/mem-store/src/episodic_sql.rs:32`), so the row is created by the **first**
   capture and a later capture for the same triple is a **recapture**, not a new row.
2. **`capture_id` is single-valued and holds the latest applied capture's id.** One column, one value: the id
   of the most recent capture applied to that row.
3. **D-36's skip rule protects exactly the most recent capture on that row.** A replay of a **superseded**
   capture is **not** skipped — on today's insert-only path it surfaces as `EpisodicError::Conflict`
   (`crates/mem-episodic/src/store.rs:291-293`).
4. **The guarantee the ADR can honour as such:** restart-and-replay is idempotent for the **latest capture per
   session row** — a `capture_id` is applied at most once **while the row carrying it exists**. D-36's
   guarantee is **not** asserted unconditionally; the restated boundary is recorded in "Documented
   limitations".
5. **The W5.1 consequence:** an **additive migration** adds the `capture_id` column to `agent_sessions` (the
   column exists nowhere in the tree today — `grep -rn capture_id` over `crates/`, `docs/` and `.scratch/`
   returns nothing) plus whatever write-semantics change the above forces.

Additionally, and normatively: **the create path is deduped by BOTH the `capture_id` and the scope triple's
UNIQUE** — a duplicate `capture_id` is skipped (D-36) and a duplicate scope triple is a conflict
(`store.rs:291-293`). A **recapture** (a different `capture_id` for an existing scope triple) is **W5.3's** to
settle, per ADR-0005's own recorded follow-up ("W5.3 recapture/upsert: recapture needs an update path or an
explicit conflict-resolution policy"); this ADR does not settle it and does not soften it.

Grounds: `crates/mem-store/src/episodic_sql.rs:32` carries `UNIQUE (project_id, session_id, agent_id)`, so the
row is keyed by the scope triple and the column can hold only one value per row;
`crates/mem-episodic/src/store.rs:291-293` is insert-only with no upsert, so a superseded replay surfaces as
`EpisodicError::Conflict` rather than being skipped; and `capture_id` appears nowhere in `crates/`, `docs/` or
`.scratch/` today, so W5.1 needs an additive migration.

### D-48 — The durable capture inbox: an ack means *durably queued* (USER RULING 2, 2026-10-20)

**USER RULING 2 — "Durable capture inbox before acking."** The engine persists the capture payload to a
**durable inbox** BEFORE it acks, so "acked" means **durably queued** — not processed, and not durably stored as
an episodic row. A crash between the ack and the DB write therefore loses nothing: the inbox survives the crash
and is drained on restart. The inbox's **storage layout is W5.3's**; its **semantics** are pinned here:

1. **The wire protocol is unchanged.** The `capture` ack's *meaning* changes; the result shape stays
   `{"accepted":true}` and the five-method schema (D-40) is untouched. **The inbox is invisible on the wire.**
2. **The inbox is durable, local to the storage root, bounded, idempotent by `capture_id`, drained on restart
   and on graceful shutdown, and purged once the capture has been applied.**
3. **The ack waits for a durable local write of the inbox entry** — milliseconds — **not** the LLM call. So
   D-35's "only `inject` blocks the host" and the capture path's off-critical-path summarisation are preserved:
   the capture's seconds-long summarisation still happens off the host's critical path and outside the DB lane
   (D-33).
4. **The inbox holds transcripts.** It therefore inherits **ADR-0004's at-rest protection**, lives **inside**
   the same storage root (**never outside it**), and has a **stated retention bound** (the bound's value is
   W5.3's; the bound's existence is pinned here). A capture that cannot be queued — the queue is full —
   **must produce a specific, distinguishable error** (a dedicated JSON-RPC error code, not a generic internal
   error): **never a silent drop**.
5. **The two `capture_id`s are distinct and must not be conflated.** The **inbox's** `capture_id` is the
   queue's dedup key (durable, purged after processing); the **episodic row's** `capture_id` is the row's
   last-applied capture id (D-36; the bounded guarantee of the D-36 consequence block above). A re-queued
   capture is deduplicated by the inbox's key; an applied capture is idempotent by the row's key.
6. **The inbox holds the capture payload as received.** The W5.3 privacy-stripping obligation (ADR-0005's
   recorded follow-up) applies unchanged to the **persisted episodic record**; the inbox is **purged once the
   capture has been applied**. **No credential ever enters the inbox** — no API token, no Basic-auth password,
   no encryption key.
7. **What it does and does not change:** it adds one durable component inside the storage root; it does **not**
   change the episodic schema's growth model, the wire schema, the method set, the DB lane or the degradation
   path.

Grounds: a local durable write before the ack is the only ordering that makes "acked" safe across a crash while
keeping the LLM call off the host's critical path (D-35, D-33); the inbox holds transcripts, so ADR-0004's
at-rest protection and the storage-root containment apply to it exactly as they do to the DB files; the two
`capture_id`s are different keys with different lifetimes, so conflating them would either lose replay
deduplication or break D-36's row-scoped guarantee.

## Security posture

**(a) A degraded engine never writes.** A running-but-degraded engine (mode `degraded`, error `-32001`) never opens the DB in write mode, and a degraded plugin writes nothing: no half-written capture, no partial DB
mutation and no "best effort" write to an unknown schema. The status file is a status report only — one line,
no secrets, no transcript content.

**(b) The lock loser degrades cleanly.** The engine that loses the exclusive lock (D-39) exits with a distinct exit code plus D-38's structured stderr warning and one-line status file; it is never a partially-initialised
writer holding a DB handle.

**(c) The web bind posture is unchanged in standalone mode.** The loopback-only bind, the D-8 Host-header
allowlist (`crates/mem-web/src/gate.rs` — the D-8 Host allowlist is check 0 of the ordered request gate;
`crates/mem-web/src/host_allowlist.rs`) and `assert_network_auth` (`crates/mem-web/src/api_auth.rs`) keep
applying in standalone mode, and a network-exposed bind is still refused without a token or Basic auth.

**(d) No secret crosses the bridge or its logs.** No method accepts, returns or echoes a token; the engine's
stderr carries no token, no Basic password, no encryption key and no raw transcript, and debug-level logging of
the full request params is forbidden (the `capture` params include the transcript).

**(e) The bridge is local-only.** It is stdio between the wrapper and its child: no TCP socket, no listening port and no environment-variable-driven endpoint may be introduced by the bridge.

**(f) `session_id` is data, not authority.** A caller-supplied `session_id` scopes a request and is never an
authorisation token: it never grants access to another session's rows, and the engine scopes by its storage
path and its own configuration (D-34 is not an access-control grant).

**(g) The `darwin/x64` path degrades, never improvises.** Where no engine binary is shipped for the host's
platform and arch, the wrapper takes the D-38 fail-open path: there is no download fallback, no JS reimplementation of the engine and no wrong-architecture exec — an exec failure of a wrong-architecture binary
degrades rather than crashing the host.

**(h) A `capture_id` collision loses one capture, never writes a wrong row.** The id is unique per capture
attempt — a UUIDv7-shaped value of at most 64 characters — and the skip rule is keyed on the stored value, so a
colliding id causes a skipped capture (data loss for that one capture) and never a wrong-row write.

**(i) There is no idempotency across consolidation.** After ADR-0006 D-23's hard delete the row and its
`capture_id` are gone, so a replay after consolidation is not idempotent: the skip rule has nothing to match.

**(j) The durable capture inbox inherits the storage protections.** Because it holds transcripts, the inbox
inherits ADR-0004's at-rest protection, lives inside the storage root and never outside it, has a bounded
retention, and holds no credential (no API token, no Basic-auth password, no encryption key). A full queue
produces a specific, distinguishable error — never a silent drop — and an entry is purged once the capture has
been applied.

These items bound two entries of "Documented limitations": the `capture_id` cardinality boundary (i) there,
and the post-consolidation replay boundary (vi) there. Neither boundary is closed by this ADR; they are stated
so that W5.1 and W5.3 build against the guarantee that actually holds.

## Alternatives rejected

1. **N-API native addon.** Rejected on the four D-30 grounds, restated as the decision record: (i) the host's
   crash domain — a Rust panic across the N-API boundary cannot unwind safely and would abort the user's
   OpenCode process; (ii) native teardown inside the host's Node runtime, the hazard class upstream records at
   `example/opencode-mem/tests/plugin-loader-contract.test.ts:71-75`; (iii) the per-platform prebuild burden
   (platform × arch × Node ABI, or an install-time C++ toolchain); (iv) reversibility — the stdio design keeps
   the move cheap, so paying the N-API costs now buys nothing that cannot be bought later.
2. **"One engine process per OpenCode session".** Replaced by D-31. N processes would put N processes on one
   turso DB file, and the engine lists "No multi-process access, No multi-threading" (`.scratch/rust-port/roadmap.md`
   §0b D3, risk R3); §0b D3 also names multi-process DB access as one of the three triggers that would open the
   deferred `rusqlite` + `sqlite-vec` escape hatch, and R3 names silent corruption as the worst failure class.
3. **"The web server stays in the JS wrapper".** Rejected because `crates/mem-web/Cargo.toml` depends on
   `mem-store` (it opens the DB): keeping the web server in the wrapper would give the DB two owners. D-32 puts
   it inside the engine, which keeps the engine the sole DB owner and happens to match upstream, where
   `startWebServer` is called from the plugin's own process (`example/opencode-mem/src/index.ts:491`).

## Reversibility

**What would it actually cost to move to N-API later?**

1. The wrapper's transport module is replaced (one module) — but the wrapper becomes the engine's host, so the
   host's crash domain now contains the engine's native code.
2. Per-platform prebuilds and a Node-ABI matrix must be shipped and maintained — the burden upstream already
   pays through `@tursodatabase/database` and `onnxruntime-node`
   (`example/opencode-mem/package.json:68,71`).
3. Native teardown inside the host's runtime must be re-solved (upstream's own recorded crash-class evidence:
   `example/opencode-mem/tests/plugin-loader-contract.test.ts:71-75`).
4. The shutdown path must be re-tested in-process.
5. The web-UI availability story changes, because the axum server would then live in the OpenCode host and the
   standalone web-only mode (D-39) would need a separate host.

**Which parts of the design keep that cheap?**

- The framing is transport-neutral: JSON-RPC 2.0 over a byte stream.
- The engine's contract is the JSON-RPC schema, **not** Node's ABI.
- The five methods, the parameter shapes, the error mapping, the handshake and the NDJSON codec all survive
  verbatim as an in-process call surface.
- The DB lane, the `session_id` discipline, the `capture_id` idempotency and the exclusive engine lock are
  transport-independent.
- The engine is already a standalone executable, so an N-API addon could wrap the **same** engine crate.

The bounded cost, stated plainly: the move is a **packaging + transport-module change**, not a re-architecture
of the engine, the DB schema, the web server or the episodic tier.

## Latency and dependency trade-offs

**The budget and the trigger, verbatim.** *"inject-path bridge overhead > 5 ms at p95 over 100 turns on the
reference machine"* re-opens the N-API question (D-35). The metric is defined so that it is measurable: the
**bridge overhead of one `inject` call = the host-observed round-trip** (the wrapper writes the request line,
then reads the response line) **minus the engine-reported `handling_ms`** (D-46), so DB and summarisation work
is excluded and only the transport is measured. The statistic is **p95 over 100 consecutive turns**. The
reference machine's specification is the one recorded in the W5.1 blueprint; this ADR does not invent a machine
spec. The trigger re-opens **D-30's transport choice only** — everything else in this ADR stands.

**The packaging consequences of D-37.** The npm tarball carries the JS wrapper plus one engine binary per
(platform, arch); the wrapper resolves the file at runtime by process.platform / process.arch and spawns it;
the engine's contract is the JSON-RPC schema, so a Node upgrade forces no rebuild. The per-platform prebuild
burden that D-30 charged against N-API is a burden this design still carries — it is simply a burden paid
**once** (one Rust binary per platform) instead of **per Node ABI**.

**The `darwin/x64` hole and what happens there under D-38.** Upstream has no darwin/x64 binding for
`@tursodatabase/database` or the pinned `onnxruntime-node` releases (`example/opencode-mem/package.json:74`
and `example/opencode-mem/AGENTS.md`), so Intel Mac is unsupported upstream. If the port's own target matrix
omits darwin/x64, the wrapper's runtime resolution finds no engine binary: the plugin **still loads**, memory is
**disabled**, a structured stderr warning naming the missing platform is emitted, and the one-line status file
is written — the D-38 fail-open path, exactly. Whether `darwin/x64` is in the port's target matrix is a W5.1
packaging decision; this ADR fixes only the **consequence** (the fail-open path), and records that the port's
Rust engine is not automatically bound by upstream's JS-binding hole.

**The dependency delta.** The bridge itself adds **no** runtime dependency to the npm package beyond the engine
binary; the port **drops upstream**'s JS native dependencies (`@tursodatabase/database` and `onnxruntime-node`
are the upstream package's runtime native deps) and adds only the per-platform engine binary — one Rust binary
per (platform, arch) instead of two JS native dependency trees. The Rust side adds no crate beyond what
ADR-0001 already froze: `mem-bridge` already exists in the workspace graph, and `tokio` and `serde_json` are
already workspace dependencies.

## Consequences

### W5.1 is unblocked

The JS wrapper lane and the Rust stdio-server lane may now split against the frozen schema of D-40 … D-48;
W5.1's blueprint is writable from this ADR without re-litigating anything.

### The crate graph is unchanged (ADR-0001)

No new crate and no new `mem-*` dependency edge. `mem-bridge` remains the bridge crate and `mem-web` remains
the axum backend; the engine binary is the workspace's composition root.

### The web explorer is unchanged (ADR-0003)

The UI still consumes the hand-written REST API over HTTP; there are no Leptos server functions and no web
namespace on the bridge (D-32).

### The episodic tier changes in exactly one additive way (ADR-0005, ADR-0006)

The `agent_sessions` table gains an **additive migration** at W5.1 (a nullable `capture_id` column; the column
exists nowhere in the tree today) plus the write-semantics change the `capture_id` cardinality forces: the
create path is deduped by both the `capture_id` and the scope triple's UNIQUE, and a recapture is W5.3's to
settle. `episodic.db` as the file and ADR-0005's `UNIQUE(project_id, session_id, agent_id)` are otherwise
unchanged; ADR-0005 is **not** amended. `capture_id` is a **column** on the episodic row (D-36), not a new
table, and W4.3's hard delete (ADR-0006 D-23) remains the purge.

### The durable capture inbox adds one durable component (USER RULING 2, D-48)

What it **does**: it makes the `capture` ack mean *durably queued*, so a crash between the ack and the DB write
loses nothing; the inbox is durable, local to the storage root, bounded, idempotent by `capture_id`, drained on
restart and on graceful shutdown, and purged once the capture has been applied. What it **does not** change:
the wire schema (the result shape stays `{"accepted":true}`), the method set, the DB lane, the degradation path,
the episodic schema's growth model, and the fact that only `inject` blocks the host (the ack waits for a durable
local write of milliseconds, not the LLM call). The inbox's **storage layout is W5.3's**; only its semantics are
frozen here.

## Documented limitations

- **(i) The `capture_id` cardinality boundary (restated, not open).** The `capture_id` on an episodic row is
  **single-valued** and holds the **latest applied capture**'s id, so D-36's skip rule protects **exactly the
  most recent capture on that row**; a replay of a **superseded** capture is **not** skipped (on today's
  insert-only path it surfaces as `EpisodicError::Conflict`). The guarantee this ADR can honour is bounded: a
  `capture_id` is applied **at most once** while the row carrying it exists. A **recapture** (a different
  `capture_id` for an existing scope triple) is **W5.3's** to settle, per ADR-0005's own recorded follow-up
  ("W5.3 recapture/upsert: recapture needs an update path or an explicit conflict-resolution policy"); this ADR
  does not settle it. The cardinality question is **not** an open question in this ADR — USER RULING 1 settled
  it as the bounded guarantee above.
- **(ii) A second OpenCode instance on the same storage root degrades — SETTLED (USER RULING 3, 2026-10-21:
  "Degrade, loudly").** With one engine per host (D-31) and an exclusive lock (D-39), a **second** OpenCode
  instance on the same storage root degrades per D-38 while the first owns the DB — that is the accepted
  consequence of D-31 + D-39 under the turso engine's single-process constraint.
  **USER RULING 3 — "Degrade, loudly."** The lock loser does **not** wait for the lock and is **not** pointed at
  a separate storage root: the plugin still loads, the engine does not start, the memory features are off for
  that instance, and D-38's structured stderr warning plus the one-line status file record the loser. The
  rationale is that a second store would mean **silent divergence** — one agent would learn what the other never
  saw, and neither would know — which is a worse failure for a memory product than being unavailable. There is
  exactly one memory store per storage root, and a second window simply has no memory of its own.
  **Consequences:** D-38's fail-open path is the whole of the second-instance behaviour (no new wire error, no
  new method); the `health` response's `mode: "degraded"` + `degraded_reason` is how the loser reports itself
  (D-46); the storage root is **not** re-derived or relocated by the loser (no separate-root escape hatch);
  and the W5.1 packaging work must ship the status-file rule that describes the loser. The separate-storage-root
  alternative is **rejected**, not deferred.
- **(iii) The reference-machine spec is not fixed here.** The reference machine for D-35's trigger is recorded
  in W5.1, not here.
- **(iv) The `darwin/x64` target-matrix entry is W5.1's — SETTLED (USER RULING 4, 2026-10-21: "Omit
  `darwin/x64`").** The port's own engine target matrix is **linux-x64 / linux-arm64 / darwin-arm64 /
  windows-x64**; **`darwin/x64` (Intel Mac) is not in it.** An Intel-Mac host therefore takes D-38's
  fail-open path (no engine binary for the platform → the plugin loads, the engine does not start, memory
  features degrade; no download fallback, no wrong-architecture exec — see D-38(g)).
  **Why:** upstream declares the same platform unsupported ("Intel Mac (darwin/x64) is unsupported" —
  `example/opencode-mem/package.json:74` and `AGENTS.md:90`), because `@tursodatabase/database` and the
  fixed `onnxruntime-node` releases ship no x64 binding. The port inherits the hole through a **different
  door**: `turso` 0.8.3 is pure Rust and would build for `x86_64-apple-darwin` without trouble, but
  `ort` 2.0.0-rc.14 ships **no prebuilt ONNX Runtime for that target at all** — its distribution table
  (`ort-sys-2.0.0-rc.14/build/download/dist.tsv`) contains `aarch64-apple-darwin` and **zero**
  `x86_64-apple-darwin` rows — so supporting Intel Mac would mean compiling ONNX Runtime from source as a
  release-pipeline commitment. That is a packaging cost the port does not need to pay for parity.
  **Consequences:** D-38(g) is the whole of the `darwin/x64` behaviour (fail-open, never improvise); W5.1's
  packaging ships no `darwin/x64` engine artifact; the wrapper's runtime resolution finds no engine binary
  and takes the degraded path. Revisiting this would mean a per-target `ort` feature gate (e.g. remote
  embeddings only on Intel Mac) and is **not** part of this ADR.
- **(v) The wire schema, not every field type's encoding.** This ADR fixes the wire schema; the JSON codec is
  W5.1's blueprint's.
- **(vi) The post-consolidation replay boundary.** After ADR-0006 D-23's hard delete the row **and its
  `capture_id`** are gone, so **a replay after consolidation is not idempotent**: the row no longer exists to
  carry the id, and the skip rule has nothing to match. The purge window is therefore the boundary of D-36's
  guarantee, and it is not closed by this ADR.
- **(vii) The durable capture inbox's own boundaries.** The inbox is a new durable surface inside the storage
  root: it holds transcripts (so it inherits ADR-0004's at-rest protection), it is **bounded** (the bound's
  value is W5.3's), a full queue produces a specific, distinguishable error rather than a silent drop, and an
  entry is purged once the capture has been applied. It does **not** make the episodic row's write durable — the
  row's write still happens on the lane — and it does not survive a storage-root loss.

## Evidence index

| Artifact | What it proves |
|---|---|
| `example/opencode-mem/tests/plugin-loader-contract.test.ts` (lines 1-7, 16-19, 45-64, 71-75) | The OpenCode 1.3.x loader contract: an ES module default export whose `server` is a JS function, loaded by dynamic import; and upstream's own recorded native-teardown crash class. |
| `example/opencode-mem/src/index.ts:491` (import at line 12) | Upstream starts its web server inside the plugin's own process. |
| `example/opencode-mem/src/services/web-server.ts:909` | The `startWebServer` factory called at `index.ts:491`. |
| `example/opencode-mem/package.json` lines 68, 71, 74 | Upstream already ships native runtime deps (`@tursodatabase/database`, `onnxruntime-node`) and already has the `darwin/x64` hole (the `"//"` comment). |
| `example/opencode-mem/AGENTS.md:90` | Repeats the upstream `darwin/x64` unsupported statement. |
| Turso engine manual §Limitations, quoted in `.scratch/rust-port/roadmap.md` §0b D3 | "No multi-process access, No multi-threading" — the single-process constraint behind D-31. |
| `.scratch/rust-port/roadmap.md` §0b D1/D2/D3 | The storage-engine ruling (`turso` crate 0.8.x), the vector SQL transliteration, and the deferred escape hatch's three switch triggers. |
| `.scratch/rust-port/roadmap.md` risks R3, R5 | R3: multi-process access and locking, with silent corruption as the worst failure class. R5: the bridge strategy determines packaging, latency and shutdown semantics and is hard to reverse. |
| `docs/adr/0004-database-encryption-at-rest.md` | The cross-process operation lock and the at-rest protection the durable capture inbox inherits. |
| `docs/adr/0005-episodic-storage-model.md` (D-1 … D-16) | The `episodic.db` storage model and the `UNIQUE(project_id, session_id, agent_id)` scope-triple key this ADR reads the `capture_id` cardinality against. |
| `docs/adr/0006-episodic-consolidation.md` (D-23, D-25) | D-23: the hard delete that is the `capture_id` purge and the post-consolidation replay boundary. D-25: the in-process gate plus the cross-process operation lock the engine lock must not collide with. |
| Issue 17, decision (c) | The user-approved "hard delete + keep source summaries in the milestone" that ADR-0006 D-23 records. |
| `crates/mem-store/src/operation_lock.rs` (lines 25, 83-96, 127) | `OPERATION_LOCK_FILE = ".turso-operation.lock"`; `assert_no_turso_migration_in_progress` checks both lock files and fails on any live holder regardless of pid; `acquire_turso_operation_lock` is operation-scoped. |
| `crates/mem-store/src/shard_manager.rs` (lines 682, 687, 703) | `get_write_shard` calls the lock assert before resolving a shard; `get_write_shard_allowing_own_lock` is the own-lock-tolerant seam. |
| `crates/mem-store/src/episodic_sql.rs:32` | `UNIQUE (project_id, session_id, agent_id)` on `agent_sessions` — the scope triple is the row key. |
| `crates/mem-episodic/src/store.rs:291-293` | The insert-only create path: a UNIQUE violation maps to `EpisodicError::Conflict`, and there is no upsert. |
| `crates/mem-web/src/gate.rs` | The ordered request gate; the D-8 Host allowlist is check 0. |
| `crates/mem-web/src/host_allowlist.rs`, `crates/mem-web/src/api_auth.rs` | The D-8 Host allowlist and `assert_network_auth` / `local_token_matches`, which keep applying in standalone mode. |
| `crates/mem-web/Cargo.toml` | `mem-web` depends on `mem-store` — the web server is a DB opener, which is why D-32 puts it inside the engine. |





