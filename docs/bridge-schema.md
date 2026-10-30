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
{"id":"hello-1","jsonrpc":"2.0","result":{"capabilities":{"add":true,"compaction":true,"export":true,"fire_and_forget_capture":true,"forget":true,"import":true,"list":true,"list-shards":true,"migrate":true,"profile":true,"search":true,"skills":true,"standalone":true,"web":true},"engine_version":"0.1.0","methods":["capture","inject","recall","skills","health","shutdown","add","search","forget","list","profile","list-shards","migrate","export","import"],"protocol":"1.0.0"}}
```

Protocol version is one semver line owned by ADR-0007; its current value is `1.0.0`.
The engine is **compatible** iff

1. `major(engine.protocol) == major(wrapper.protocol)`, **and**
2. `engine.protocol >= wrapper.min_protocol`.

A different major, or a version below the floor, is incompatible: the wrapper MUST NOT call
any runtime method and degrades per D-38 (warning on stderr + status file; memory disabled,
plugin still loads). A newer minor is compatible.

The `capabilities` object is additive: each key is `true` exactly when this engine serves that
behaviour, and a wrapper reads only the keys it knows, ignoring unknown ones. `compaction`
means the engine's `inject` honours `include.compaction`; an engine that omits the key answers
the compaction request with the ordinary project chat block, so the wrapper MUST no-op both
compaction hooks. `skills` means the engine answers the `skills` runtime method (issue 69); an
engine that omits the key predates the method, and the wrapper MUST NOT call it.

Issue 70 adds one capability key per `memory` tool method — `add`, `search`, `forget`, `list`,
`profile`, `list-shards`, `migrate`, `export`, `import` — so this build advertises fourteen
keys under fifteen methods. The key means "this build's dispatch answers this method"; it
never means "the store has data". The gating rule wave 2 implements: a wrapper that does not
find a key MUST NOT call that method (the engine predates it); a key that is present but whose
runtime is degraded answers `-32001`, which the tool lane turns into its structured no-op.

`hello` is **not** a runtime method (D-40): it takes no `session_id` and is not counted in
the fifteen.

## 3. The fifteen runtime methods (D-40, D-46)

`session_id` is a **required top-level parameter of every method except `skills`**, which
takes `project_id` and carries no session id. It is required for `health` and `shutdown`
(D-34). It is 1..=128 characters after the string is taken as-is (no trimming of
the stored value; a whitespace-only value is **blank** and rejected). Missing, blank or
over-long → `-32602`.

`session_id` is **data, not authority** (security posture (f)): it scopes a request and
never grants access to another session's rows.

| method | blocking | params | result |
|---|---|---|---|
| `capture` | no (fire-and-forget ack) | `session_id` (req), `capture_id` (req), `project_id?`, `agent_id?`, `agent_name?`, `parent_session_id?`, `transcript?`, `summary?`, `metadata?` | `{"accepted":true}` |
| `inject` | **yes** | `session_id` (req), `project_id?`, `agent_id?`, `token_budget?`, `include?` | `{"block":string\|null,"memory_count":int,"tokens":int,"truncated":bool,"handling_ms":int}` |
| `recall` | no | `session_id` (req); **canonical** when `projectId` is present (camelCase, no unknown keys): `projectId` (req), `mode?` (`"chronological"\|"semantic"\|"filtered"`), `query?` (semantic only), `since?`, `until?`, `lastNSessions?`, `touchedFiles?`, `agentId?`, `sessionId?` (filter key), `includeFullSessions?`; **legacy** otherwise: `query` (req, string), `limit?`, `scope?`, `tags?` (these keys only) | `{"items":[…],"count":int}` |
| `skills` | **yes** | `project_id` (req); no `session_id` | `{"skills":[{"id":string,"name":string,"path":string,"content":string,"autoinvoke":bool,"description"?:string}]}` |
| `health` | no | `session_id` (req) | `{"ok":bool,"mode":"full"\|"degraded","degraded_reason":string\|null,"protocol":semver,"engine_version":semver,"db":{"path":string,"ready":bool},"open_guard":{"override_active":bool},"uptime_ms":int,"sessions":int}` |
| `shutdown` | teardown only | `session_id` (req) | `{"ok":true}` |
| `add` | **yes** | `session_id` (req), `project_id` (req), `content` (req), `type?`, `tags?` (comma string), `metadata?` (object) | `{"id":string}` |
| `search` | **yes** | `session_id` (req), `project_id` (req), `scope` (req: `"project"\|"all-projects"`), `query` (req, non-blank), `limit?` (1..=1000) | `{"count":int,"items":[row + similarity + rank]}` |
| `forget` | **yes** | `session_id` (req), `project_id` (req), `memoryId` (req), `scope?` (default `"project"`) | `{"deleted":int}` |
| `list` | **yes** | `session_id` (req), `project_id` (req), `scope?` (default `"project"`), `limit?` (1..=1000) | `{"count":int,"items":[row]}` |
| `profile` | **yes** | `session_id` (req), `project_id` (req), `content?` (absent = read, present = explicit-preference write) | read `{"profile":object\|null}`; write `{"saved":true}` |
| `list-shards` | **yes** | `session_id` (req), `project_path` (req) | `ListShardsResult` (`storagePath`, `currentProject`, `shards`, `summary`) |
| `migrate` | **yes** | `session_id` (req), `project_path` (req), `from_path?`, `from_hash?` (16 lowercase hex), `dry_run?`, `allow_linked_source?`; at least one of `from_path`/`from_hash` | `{"actions":[…],"dry_run":bool,"matched_by":string,"migrated_memories":int,"migrated_shards":int,"new_hash":string,"new_path":string\|null,"old_hash":string,"old_path":string\|null}` |
| `export` | **yes** | `session_id` (req), `project_id` (req), `output_path` (req, absolute `.json`) | `{"container_tag":string,"count":int,"output_path":string,"scope_hash":string}` |
| `import` | **yes** | `session_id` (req), `project_id` (req), `input_path` (req, absolute `.json`), `dry_run?` | `{"container_tag":string,"dry_run":bool,"imported":int,"rejected":[{"id","reason"}],"skipped":[{"id","reason"}]}` |

`skills` (issue 69) returns the **enabled** skills tagged with the request's project plus the
global ones (`container_tag IS NULL OR container_tag = <project tag>`); an unknown project is
`{"skills":[]}`, never an error. Each entry is exactly the host's `Skill.Info` set — `id`,
`name`, `path` (the absolute `SKILL.md` path), `content`, `autoinvoke`, plus `description` when
non-empty. The order is the manager's `updated_at DESC, id ASC`; consumers MUST NOT depend on
it. `project_id` is required and is never coerced to the global tag: missing, non-string or
whitespace-only → `-32602`. The method is session-free because registration happens at plugin
`setup`, before any session exists.

### The nine tool methods (issue 70)

Seven methods take `project_id` — the immutable host directory — and resolve
their scope through the same resolver as capture and inject: `add`, `search`,
`forget`, `list`, `profile`, `export`, `import`. Two take `project_path` because
that is what the store services address: `list-shards` and `migrate`. Every
parameter is snake_case except `memoryId` on `forget`, which is kept literally as
the tool argument's name (the one recorded exception; `recall`'s canonical shape
already carries camelCase keys). `scope` is `"project"` or `"all-projects"`.
`session_id` is data, never authority — only `add` persists it, and no method
filters by it.

`add` is the only write that is not a profile: content is required and must be
non-blank **after** `strip_private_content`; a fully private message is refused
with `-32602 {"reason":"private"}` **before** any embed or write; the sanitized
content (and, when tags exist, `Topics: <tags joined by ", ">`) is what reaches
the embedder; then one `BatchMode::Write` inserts the row and the
statement-measured count, and the registry adopts it.

The wire row is exactly **16 keys**: `container_tag`, `content`, `created_at`,
`display_name`, `git_repo_url`, `id`, `is_pinned`, `memory_type`, `metadata`,
`project_name`, `project_path`, `session_id`, `tags`, `updated_at`,
`user_email`, `user_name`. A `search` item is those 16 plus `similarity`
and `rank` (18). `created_at`/`updated_at` are epoch milliseconds, `tags` is
always an array, and `metadata` is the parsed object or `null` (a malformed
column is `null`, never an error). `search` hydrates each hit with the frozen
`SELECT *` in the same shard — `SearchResult` alone lacks `memory_type`,
`session_id` and `updated_at` — and a hit whose row vanished is dropped.

Two total orders exist and no others: `list` sorts `created_at DESC, id ASC`;
`search` sorts `similarity DESC, created_at DESC, id ASC` and assigns `rank`
1..=n **after** the truncate. `similarity` is `clamp(1 - cosine_distance, 0, 1)`
and is emitted by `serde_json`, so an exact 1 renders as `1.0` (numerically
equal to JS `1`; consumers must not depend on the textual form). `limit` is
1..=1000 or `-32602 {"reason":"limit"}`; `list` defaults to 20, `search` to the
resolved `max_memories`.

`forget` requires `project_id` (the id carries no tag, so without it the
handler would have to guess a tenant) and deletes every copy of `memoryId`
inside each concrete `(scope, hash)` tuple the registry knows — `all-projects`
expands from the registry only, never to a wildcard and never to `""` (the seam
rejects that as a store fault). `{"deleted":0}` means "not found", not an error.

`profile` with absent `content` reads `get_active_profile(user_email)` with the
read-only fallback identity `"unknown"`; the stored JSON is parsed, the
`centroid`/`anchor` embeddings are stripped, and `{"profile":null}` is the
absent answer. A stored `profile_data` that will not parse is
`-32603 {"reason":"profile_unreadable"}`. A write runs four refusals **before
any store write**, in order: blank (`blank`), fully private after redaction
(`private`), no resolved `user_email` (`no_user_email`), and the fail-closed
learning lock (`-32603 {"reason":"learning_locked","retryable":true}`). The lock
is released on every path, including a parse failure. The write merges one
explicit preference (`category:"explicit"`, `confidence:1.0`, `frequency:1`,
`evidence:["manual-write"]`) into the active profile, or creates it.

`list-shards` returns the ported DTO's own camelCase shape — `storagePath`,
`currentProject`, `shards`, `summary` — with no mapping layer. `migrate` takes
`project_path` plus at least one source selector; an absent **or empty**
`from_path`/`from_hash` is `-32602 {"reason":"from_required"}` at the wire (the
service's own filter would turn `from_path:""` into a resolve error). Service
failures map to `{"reason":"migrate_source","message":…,"candidates":[…]}`,
`{"reason":"migrate_refused","message":…}` or `-32603`. `dry_run` never takes
the operation lock and never touches a file; a real run takes
`.turso-operation.lock` itself, refuses a source path that still exists unless
`allow_linked_source` is set, and rewrites `user_prompts.project_path` through
the prompt-path adapter.

`export`/`import` cross into the portability service through a bridge-owned
factory seam implemented by the composition root (ADR-0001's allowed-edge table
does not move). The wire enforces, before the backend runs: an **absolute**
path ending in the exact lowercase `.json`, else `-32602 {"reason":"path"}`;
`project_id` required; `dry_run` optional (default `false`). `import` is
merge-only (a duplicate id aborts with zero writes; the port's overwrite mode
stays REST-only) and `export` carries no staging tier. There is no sandbox: any
absolute path is attempted, and the writer/reader's own rails (`0600`,
temp+rename, symlink refusal, `create_dir_all`, the 100 MiB read cap) carry the
load.

Wave boundary: these nine are the **wire** methods only. The toolkit that maps
them to upstream's `memory` tool is wave 2, and the portability tool modes are
wave 3; `help` stays local to the wrapper and `recall_project_sessions` stays.
The timeline/pair/orphan assembly is not here — it remains
`mem-web/src/api/memories.rs`.

`inject`'s `include?` accepts either the frozen `["summary","metadata"]` array or the additive options object `{"recent_count"?: int, "compaction"?: "ingest"|"restore", "query"?: string}`: `recent_count` bounds the episodic rows, `compaction` selects the session-scoped render (issue 33), and `query` carries the authored prompt (max 2000 code points; absent or blank keeps the tag-listed read). Unknown keys are ignored; no other field exists.

W5.1 shipped `capture` and `inject` as **stubs**:

- `capture` is no longer a stub: the engine awaits the D-48 durable inbox enqueue
  (`CaptureService::enqueue` → `CaptureInbox::enqueue`, a conditional insert into
  `episodic.db`) before it answers `{"accepted":true}`. An ack therefore means
  durably queued, not processed. A full inbox answers `-32002` (`MEM_INBOX_FULL`),
  never a silent drop; degraded mode refuses the write with `-32001` and writes no
  inbox entry.
- `inject` is no longer a stub: it runs the real assembly (`InjectService::assemble`)
  and returns its `<memory_context>` block when the assembly has content. `block` can
  be `null` when there is nothing to render — a missing or blank `project_id`
  (answered without a store query), a project with no memories and no stored profile,
  or the compaction leg's no-op arms — which is an empty result, not a stub answer.
- `recall` is no longer a stub: the **canonical** shape (above) reads the episodic store and
  returns its matching rows; only the **legacy** shape answers the logged empty page
  `{"items":[],"count":0}` and emits the structured stderr event
  `recall.legacy_shape_unscoped` at `warn` with a constant empty extras object.

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

### The closed `data.reason` vocabulary (issue 70 wave 1)

A `-32602` from the nine tool methods carries `error.data.reason`; a `-32603`
may carry one where noted. The vocabulary is **closed** — every token the build
can emit is listed here, so `<param>` is never a placeholder, and no new numeric
code is introduced (D-44's table does not move). Session-id violations keep
their own `{"session_id", "violation"}` shape instead of a `reason`.

| token | code | meaning |
|---|---|---|
| `blank` | `-32602` | `profile` write content is empty after trim |
| `private` | `-32602` | `add`/`profile` content is private in its entirety |
| `no_user_email` | `-32602` | `profile` write without a resolved user email |
| `learning_locked` | `-32603` | another holder has the profile learning lock (`retryable: true`) |
| `profile_conflict` | `-32603` | the optimistic profile version was lost |
| `profile_unreadable` | `-32603` | stored `profile_data` is not parseable JSON/object |
| `locked` | `-32603` | a foreign live `.turso-operation.lock` blocks the `add` |
| `project_id` | `-32602` | missing, non-string or blank `project_id` |
| `project_path` | `-32602` | missing, non-string or blank `project_path` |
| `scope` | `-32602` | unknown or missing `scope` where it is required |
| `query` | `-32602` | missing or blank `query` |
| `limit` | `-32602` | `limit` not an integer in 1..=1000 |
| `content` | `-32602` | `add` content missing or blank after stripping |
| `type` | `-32602` | `type` is not a string |
| `tags` | `-32602` | `tags` is not a string |
| `metadata` | `-32602` | `metadata` is not an object |
| `memoryId` | `-32602` | missing, non-string or blank `memoryId` |
| `dry_run` | `-32602` | `dry_run` is not a boolean |
| `allow_linked_source` | `-32602` | `allow_linked_source` is not a boolean |
| `path` | `-32602` | `output_path`/`input_path` not absolute or not `.json` |
| `from_required` | `-32602` | neither `from_path` nor `from_hash` supplied (or empty) |
| `migrate_source` | `-32602` | the migration source did not resolve (`message`, `candidates`) |
| `migrate_refused` | `-32602` | the migration refused a linked source (`message`) |
| `file_not_found` | `-32602` | the import file does not exist (`message`) |
| `invalid_json` | `-32602` | the bundle file is not JSON (`message`) |
| `unsupported_version` | `-32602` | the bundle's `schemaVersion` is unsupported (`version`, `message`) |
| `invalid_bundle` | `-32602` | the bundle is structurally invalid (`message`) |
| `too_large` | `-32602` | the bundle exceeds the 100 MiB read cap |
| `unsafe_path` | `-32602` | the destination is a symlink (`message`) |
| `invalid_scope` | `-32602` | the target container tag cannot be resolved (`message`) |
| `import_aborted` | `-32602` | a duplicate id aborted the import; carries `kind` (`"memory"`/`"milestone"`), `count`, `rejected` (memory kind) and `message` |

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

## 11. Local REST auth and failure contract

- **The local token travels only in `x-opencode-mem-token`**
  (`mem_web::AUTH_HEADER`, `crates/mem-web/src/lib.rs:7`). The gate reads that one
  header as the local token (`crates/mem-web/src/gate.rs:89-93`); it never reads
  `Authorization` for it. `Authorization: Bearer <local token>` is not accepted:
  the Bearer comparison runs only against the configured `webServerApiToken`
  (`crates/mem-web/src/gate.rs:94-97`, reading `AppState.api_token`), and
  standalone mode leaves it unset: `src/main.rs:194` sets
  `AppState.api_token: None`, while `src/main.rs:138` passes `api_token: None`
  to the web-server settings, which only the network-auth guard reads
  (`crates/mem-web/src/server.rs:92-96`), so the API-token leg is off and only
  the local-token leg can admit a request.
  `Authorization: Basic <base64("user:<local token>")>` is a different
  credential — the shell/API Basic-auth gate
  (`crates/mem-web/src/gate.rs:69-76`), disabled in standalone mode
  (`src/main.rs:193` builds `WebAuth::new(None, None, "engine")`) — and a
  request carrying it is refused with the same `401`
  (`crates/mem-web/src/gate.rs:108`). Pinned by
  `test_the_local_token_is_accepted_only_on_the_custom_header`
  (`crates/mem-web/tests/api_route_auth.rs`).
- **`501` and `500` carry different meanings and must not be conflated.** `501`
  means the endpoint's service is absent *in this build*: it is emitted by the
  absent-service class (`PortError::Unavailable`, mapped at
  `crates/mem-web/src/api/mod.rs:552`) and its semantics are stable contract.
  `500` means an internal fault: the detail is logged and never returned to the
  client (the fall-through arm at `crates/mem-web/src/api/mod.rs:557-558`).
  Pinned by `crates/mem-web/tests/api_port_failure_class.rs`.
- **Per-endpoint availability is deliberately not enumerated here.** Which
  endpoint answers `501` depends on the composition state of the running build
  (a degraded launch wires the unavailable bundle; a composed launch attaches
  the production ports) and on how the build was packaged, so it belongs in
  release notes — this section is contract, never an endpoint inventory.
