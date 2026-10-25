# Bridge schema — V1 plugin adapter wire contract (frozen for W5.1)

**Status:** frozen. Normative source for both lanes of W5.1 and for W5.2.
**Owning wave:** W5.1 (V1 plugin adapter).
**Authority:** `docs/adr/0007-bridge-strategy.md` D-30 … D-48. This document is the
normative *copy* of those decisions; where the ADR and this file appear to differ, the ADR
wins and this file is the defect.

Two lanes build against this file and must not change it after wave 0:

- the **JS lane** (`plugin/`, the OpenCode host plugin wrapper), and
- the **Rust lane** (`crates/mem-bridge`, `crates/mem-store`, `src/main.rs`, the engine).

The fixture bytes under `tests/fixtures/bridge/` are part of the contract. A lane that
cannot reproduce them is wrong; the fixtures are not adjusted to a lane.

---

## 1. Transport and framing (D-42)

- One JSON-RPC 2.0 message per line, UTF-8, terminated by exactly one `\n`.
- No length prefix, no framing header, no batching. A JSON **array** on a line is not a
  batch and is rejected with `-32600`.
- Every request carries `id` (string or number); v1 has no notifications, so a request
  without `id` is `-32600`.
- stdout carries the protocol **only**. Logs, warnings and diagnostics go to stderr as
  structured JSON (ADR-0002). A stray `println!` is a defect, not a style issue.
- The bridge is local-only: stdio between the wrapper and its child. No TCP socket, no
  listening port, no env-var endpoint may be introduced. (The engine's axum server is a
  separate surface — D-32 — and keeps the existing web security posture.)

### Canonical serialization (byte-equality fixtures)

Wire objects are serialized **compactly** (no insignificant whitespace) with object keys in
**lexicographic order at every level**. JSON object member order is not semantically
meaningful (RFC 8259 §4), so canonical order changes no schema; it exists so the frozen
fixtures are byte-checkable in both languages. Consequences:

- Rust: `serde_json`'s default map is a `BTreeMap`, so parse→re-serialize is already
  canonical.
- JavaScript: the wrapper's serializer sorts keys recursively before encoding.

Numbers are emitted without a trailing `.0`; no NaN/Infinity; no non-ASCII in fixtures.

## 2. Handshake (D-43)

The wrapper's **first** message after spawn is `hello`. No runtime method may precede it.

```json
{"id":"hello-1","jsonrpc":"2.0","method":"hello","params":{"client":"opencode-mem-plugin","min_protocol":"1.0.0","protocol":"1.0.0"}}
```

```json
{"id":"hello-1","jsonrpc":"2.0","result":{"capabilities":{"fire_and_forget_capture":true,"standalone":true,"web":true},"engine_version":"0.1.0","methods":["capture","inject","recall","health","shutdown"],"protocol":"1.0.0"}}
```

Protocol version is one semver line owned by ADR-0007; its current value is `1.0.0`.
The engine is **compatible** iff

1. `major(engine.protocol) == major(wrapper.protocol)`, **and**
2. `engine.protocol >= wrapper.min_protocol`.

A different major, or a version below the floor, is incompatible: the wrapper MUST NOT call
any runtime method and degrades per D-38 (warning on stderr + status file; memory disabled,
plugin still loads). A newer minor is compatible.

`hello` is **not** a runtime method (D-40): it takes no `session_id` and is not counted in
the five.

## 3. The five runtime methods (D-40, D-46)

`session_id` is a **required top-level parameter of every method**, including `health` and
`shutdown` (D-34). It is 1..=128 characters after the string is taken as-is (no trimming of
the stored value; a whitespace-only value is **blank** and rejected). Missing, blank or
over-long → `-32602`.

`session_id` is **data, not authority** (security posture (f)): it scopes a request and
never grants access to another session's rows.

| method | blocking | params | result |
|---|---|---|---|
| `capture` | no (fire-and-forget ack) | `session_id` (req), `capture_id` (req), `project_id?`, `agent_id?`, `agent_name?`, `parent_session_id?`, `transcript?`, `summary?`, `metadata?` | `{"accepted":true}` |
| `inject` | **yes** | `session_id` (req), `project_id?`, `agent_id?`, `token_budget?`, `include?` | `{"block":string\|null,"memory_count":int,"tokens":int,"truncated":bool,"handling_ms":int}` |
| `recall` | no | `session_id` (req), `query` (req), `limit?`, `scope?`, `tags?` | `{"items":[…],"count":int}` |
| `health` | no | `session_id` (req) | `{"ok":bool,"mode":"full"\|"degraded","degraded_reason":string\|null,"protocol":semver,"engine_version":semver,"db":{"path":string,"ready":bool},"open_guard":{"override_active":bool},"uptime_ms":int,"sessions":int}` |
| `shutdown` | teardown only | `session_id` (req) | `{"ok":true}` |

W5.1 ships `capture`, `inject` and `recall` as **stubs**:

- `capture` answers `{"accepted":true}`. **The ack carries no durability guarantee in
  W5.1.** D-48's "an ack means durably queued" arrives with W5.3's inbox, which is W5.3's
  entry criterion; until then the stub emits the structured stderr line
  `{"level":"warn","component":"engine","event":"capture.stub_not_durable"}` with **no
  params** (the params include the transcript, and no transcript may reach stderr).
- `inject` answers `{"block":null,"memory_count":0,"tokens":0,"truncated":false,"handling_ms":<n>}`.
- `recall` answers `{"items":[],"count":0}`.

`capture_id` is client-minted, UUIDv7-shaped, ≤ 64 characters, and unique per capture
attempt (D-36, security posture (h)). A collision loses one capture; it never writes a wrong
row. W5.1 does not implement the recapture/upsert policy (W5.3).

`handling_ms` in the `inject` result is the engine's own internal handling time (D-35
defines the bridge-overhead metric against it).

### The wrapper's source of `session_id`

OpenCode's plugin callback receives `{ client, project, directory, worktree, $ }`; there is
no `session_id` on `PluginInput`. Session ids arrive **per hook**: `chat.message` delivers
`input.sessionID`, and `event` delivers session-scoped ids under
`input.event.properties`. The wrapper retains the most recent hook-supplied session id and
uses it for every subsequent request. Before any hook has fired — and for the startup
`health` and teardown `shutdown` — it uses `oc-bridge-<pid>`.

The fixtures `health-request.json` and `shutdown-request.json` carry the fixed literal
`oc-bridge-4242` (never a live pid). A runtime comparison therefore asserts the **shape**
`^oc-bridge-\d+$` (or the recorded hook id), never byte equality.

## 4. Errors (D-44, plus the ADR-0007 amendment item D-48)

| code | name | meaning |
|---|---|---|
| `-32700` | Parse error | the line is not valid JSON |
| `-32600` | Invalid request | not a JSON-RPC 2.0 object, or a batched array, or a missing `id`/`method` |
| `-32601` | Method not found | unknown method |
| `-32602` | Invalid params | including a missing, blank or over-long `session_id` |
| `-32603` | Internal error | an unexpected engine failure |
| `-32001` | `MEM_DEGRADED` | the engine runs but memory is disabled |
| `-32002` | `MEM_INBOX_FULL` | the durable capture inbox is full — never a silent drop |

**ADR-0007 amendment item (recorded here, not yet in the ADR):** `-32002 MEM_INBOX_FULL` is
the dedicated "capture queue is full" code D-48 requires ("a dedicated JSON-RPC error code,
not a generic internal error"). W5.3 is bound to this number. The ADR amendment itself is
the orchestrator's to land; this document must not diverge from it.

An ordinary failure is never reported as `-32603` when a specific code exists. `-32001` is
an **additional in-protocol** signal; D-38's stderr warning and the status file remain the
ratified degradation path.

The wrapper maps **every** engine error to fail-open: an `inject` error yields an empty
injection (never a host error); a `capture` error is logged and never blindly retried;
`health` reporting `mode: "degraded"` is surfaced through `degraded_reason`.

Error envelopes are canonical wire objects too:

```json
{"error":{"code":-32602,"data":{"session_id":"<129 chars>","violation":"overlong"},"message":"Invalid params"},"id":"capture-2","jsonrpc":"2.0"}
```

`data` is optional and advisory; it must never carry a secret or transcript content. A
parse error answers with `"id":null` (JSON-RPC 2.0: the id is not recoverable).

## 5. Shutdown (D-45)

1. The wrapper sends `shutdown`; the engine replies `{"ok":true}` and stops accepting new
   requests.
2. The engine flushes and closes the DB, stops the axum server, and exits.
3. If the engine has not exited **2 s** after the shutdown response, the wrapper sends
   **SIGTERM**.
4. If it has not exited **5 s** after the SIGTERM, the wrapper sends **SIGKILL**.
5. The wrapper waits for the child's stdio to reach EOF, so no orphan process and no open
   stdio handle remain.

The host is never held indefinitely. Internal budget (D-45): web stop wrapped in ~1 s, DB
flush ≤ 500 ms, total < 2 s.

**Stdio mode installs no SIGTERM/SIGINT handler**: the default disposition terminates the
process. Only `--mode=standalone` installs the grace handlers. An in-flight
`tokio::io::stdin` read otherwise makes runtime teardown wait and the real engine cannot
meet the 2 s bound.

The wrapper **never calls `process.exit`**: it removes its handlers and re-delivers the
signal, and it drains pending captures (`awaitCaptureDrain()` over a `pendingCaptures`
counter) before it does.

## 6. NDJSON examples

`tests/fixtures/bridge/ndjson-framing.txt` holds three consecutive request lines (hello,
capture, health), each terminated by `\n`. Its bytes are the framing oracle for both lanes.

## 7. Engine lifetime: the exclusive lock (D-39, D-47)

- The lock file is `<storage_root>/.engine.lock` — **distinct** from
  `.turso-operation.lock` (operation-scoped) and `.turso-migrate.lock` (W4.3
  consolidation). Reusing either would make `get_write_shard` refuse the engine's own
  writes.
- The lock body is a `LockRecord` (`crates/mem-store/src/operation_lock.rs`): the three
  `LockState` fields plus an **optional, additive** `start_time` —
  `{"pid":<n>,"timestamp":"<UTC ISO-8601>","operation":"engine","start_time":<n>}`. It
  must be `read_live_lock`-parseable: a plain-pid body reads as corrupt and is stolen,
  which would give the DB two owners. `start_time` is the holder's start-time token
  (`/proc/<pid>/stat` field 22), which defeats PID reuse.
  - **Present** on records written by this binary (on a host without procfs it
    serializes as `null`, i.e. unobservable, which reads back as absent).
  - **Absent** on legacy records written before the field existed. That is not an
    error and not a reason to unlink: an absent token cannot prove a reuse, so the
    holder is treated conservatively — an unverifiable holder **reads as alive** and
    keeps its lock.
  - The field is safe to add because no reader parses the body for control flow:
    the only reader is `read_live_lock`, it parses serde-tolerantly (no
    `deny_unknown_fields`, so an old binary ignores the key and a new binary still
    reads an old body), and nothing requires the key. `start_time` only sharpens the
    liveness verdict — present-and-mismatched is the one dead verdict on an existing
    pid — and its absence can only make that verdict more conservative.
- Acquire is `create_new(true)`; a dead-owner or corrupt lock is unlinked and the acquire
  is retried **once**.
- The winner holds the lock for its whole lifetime (`Drop` releases it).
- The loser never opens the DB. It writes the structured stderr warning
  `{"level":"warn","component":"engine","event":"lock_lost","holder_pid":<n>}`, writes the
  status file (below), and exits with code **75** (`ENGINE_LOCK_LOST`).

### Status file (D-38, settled point 2)

- Path: exactly `<storage_root>/status`, where `<storage_root>` defaults to
  `~/.opencode-mem/data` (`mem_core` `DEFAULTS.storage_path`).
- Exactly one line, ≤ 200 characters, grammar:

  ```
  degraded: <short reason> (pid=<n>, ts=<UTC ISO-8601>)
  ```

  This is the **single** grammar; earlier renderings are superseded. No JSON, no secrets,
  no transcript content.
- **Two writers**, and the rule is *not* "only the loser writes it":
  1. **Lock loser (Rust engine):** records the **holder's** pid — the pid read from the
     live `.engine.lock` — so the pid-liveness rule is meaningful.
  2. **JS wrapper:** when there is no engine or an incompatible engine, it writes the status
     file recording **its own** pid.
- The owner never deletes a status it did not write; a reader treats the file as degraded
  only while the named pid is alive.
- The status file is a status **report** only. It is not a second-instance policy: whether a
  second OpenCode instance on the same storage root *should* degrade or be pointed at a
  separate storage root is an **open question for the user** (ADR-0007, documented
  limitation (ii)) and this contract does not answer it.

## 8. Fail-open (D-38)

When the engine is absent, fails to start, speaks an incompatible protocol, or cannot be
executed for the host's platform/arch, the plugin still loads and memory is disabled — with
(a) a structured warning on stderr naming the cause, and (b) the status file above. A
silently memoryless plugin is worse than a loud one.

The wrapper resolves the engine at `engine/<platform>-<arch>/opencode-agents-memory` inside the
package, by `process.platform` / `process.arch`. There is **no** download fallback, no JS
reimplementation and no wrong-arch exec: an exec failure degrades. Whether `darwin/x64` is in
the port's own target matrix is an **open question for the user** (ADR-0007, documented
limitation (iv)); an absent binary degrades — that is all this contract asserts.

### Web-launch diagnostics: `web.dist_missing` + `web.launch_failed` (W6.8)

The composition root resolves the web UI directory (§9). When no candidate is READY it does
**not** bind a socket; it fails the auxiliary web LAUNCH:

- `web.dist_missing` — emitted by the **composition root** (`src/main.rs`), level `warn`,
  component `engine`, payload
  `{"dist_candidates":["<path>", …],"reason":"<sentence>"}`. The candidate list is lossy path
  strings (never raw non-UTF-8 paths) and the sentence names the offending or explicitly
  configured path. No token, credential or request data ever appears in it.
- `web.launch_failed` — the engine's existing degradation line, emitted immediately after with
  its `detail_short: true` posture; the engine keeps its stdio purpose with a disabled web
  handle.

Both lines are expected on stderr when no build resolves, and the port stays closed. The extra
`web.dist_missing` line exists because `web.launch_failed` carries no detail and the run path
installs no tracing subscriber, so the detail would otherwise be dropped.

**Landed (issue 36, W6.9):** the packaging script now stages the built UI at
`engine/<platform>-<arch>/web-ui/dist` beside the engine binary, so a plugin-launched engine
resolves the packaged default candidate with no setting and no flag, while an explicit
`OPENCODE_MEM_WEB_DIST` / `--web-dist <dir>` still wins and the wrapper forwards the env name and
appends the flag. Staging refuses with exit 5 (no built bundle) or exit 7 (a bundle is already
staged: replaced, never merged — remove it and re-run), so a refused rework stages nothing. A
bundle-less package keeps the loud `web.dist_missing` + `web.launch_failed` degradation. The W6.7
browser smoke and the W7.2 "web UI functional" rehearsal now pass over the plugin path.

## 9. Environment and defaults

Both lanes read the **same names** and the **same default**, with no
engine-direct exception: `OPENCODE_MEM_WEB_DIST` (and its CLI twin
`--web-dist`) is read by the engine, and the wrapper forwards it — the name via
its child-environment whitelist and the flag as `--web-dist <dir>` appended when
the host sets it (issue 36). When the host sets nothing, the wrapper forwards
nothing and the engine's exe-relative default is the single source of truth.

| variable | default | meaning |
|---|---|---|
| `OPENCODE_MEM_STORAGE_ROOT` | `~/.opencode-mem/data` | storage root (lock, status file, DBs) |
| `OPENCODE_MEM_MODE` | `stdio` | engine launch mode |
| `OPENCODE_MEM_WEB_HOST` | loopback (`127.0.0.1`) | axum bind host (standalone mode) |
| `OPENCODE_MEM_WEB_PORT` | `3742` | axum bind port (standalone mode) |
| `OPENCODE_MEM_WEB_DIST` | first READY exe-relative candidate: `<exe_dir>/web-ui/dist`, then `<exe_dir>/../../web-ui/dist` | directory holding the built web UI that mem-web serves; a plugin-launched engine receives it from the wrapper, which forwards the name and appends --web-dist (issue 36) |

The wrapper needs the storage root to write the status file **before any engine exists**,
which is why the default is shared rather than engine-only.

### The web UI directory (`OPENCODE_MEM_WEB_DIST` / `--web-dist`, W6.8)

- **Precedence:** CLI flag `--web-dist <path>` / `--web-dist=<path>` > env var
  `OPENCODE_MEM_WEB_DIST` > the runtime-derived default. A blank or
  whitespace-only value falls through exactly like an unset one.
- **No config-file surface:** the setting is deliberately **not** an
  `opencode-mem.json` key, so a config file can never redirect the static
  host. It is also not a `mem_core` config field (upstream has no
  `webDistDir`).
- **Default:** derived from the running binary (`--web-dist`'s value is
  absolutized against the process cwd). Candidate 1 is the PACKAGED layout
  (the bundle staged beside the engine binary); candidate 2 is the
  CARGO-CHECKOUT dev layout (`target/<profile>/opencode-agents-memory` →
  `../..` is the checkout root). The first candidate that EXISTS **and** holds
  `index.html` as a regular file wins; a stale partial build is skipped in
  favour of a later candidate. Nothing is hard-coded to an absolute or
  repo-relative path, and a custom `CARGO_TARGET_DIR` simply makes candidate 2
  wrong — the explicit setting covers that.
- **A missing build REFUSES the web launch** at the composition root (never a
  bind-and-404): the launcher emits `web.dist_missing` (see §8) and returns an
  error, so the engine's existing `web.launch_failed` degradation fires and the
  stdio purpose survives.
- **Packaged default (issue 36):** candidate 1 (the bundle staged beside the
  engine binary) is produced by `plugin/scripts/stage-engine.sh`, which stages
  `web-ui/dist` into `plugin/engine/<platform>-<arch>/web-ui/dist`. A staged
  bundle is replaced only by the operator: the script never merges a new bundle
  into an old one and never deletes one, so reworking a stage is the operator's
  act. The wrapper forwards the setting (the env name and `--web-dist`), so a
  plugin-launched engine resolves candidate 1 with no setting or flag of its
  own. A bundle-less package still refuses the web launch loudly — no socket is
  bound, the stdio purpose survives, and it is never a bind-and-404.

## 10. Secrets (security posture (a)–(j), mandates 1–2, 9)

- No method accepts, returns or echoes a token.
- The wrapper never reads `~/.opencode-mem/.auth-token` or any `env://` / `file://` secret.
- The wrapper never forwards host credentials (a `client` object's `Authorization` header,
  tokens, passwords) to the child: not in argv, not in the environment, not in a log line.
- stderr carries no token, no Basic password, no encryption key and no raw transcript; debug
  logging of full request params is forbidden (capture params contain the transcript).
- The status file carries no secret and no transcript.
- `capture_id` collision loses one capture, never writes a wrong row.
