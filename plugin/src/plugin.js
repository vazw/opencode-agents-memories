// OpenCode host plugin wrapper (V1 bridge) — wave W5.1.
//
// Normative wire contract: docs/bridge-schema.md (frozen). The default export
// satisfies the OpenCode plugin-loader contract ({ id, server }); the named
// exports hold the canonical serializer that the frozen byte fixtures in
// tests/fixtures/bridge/ pin. Engine lifetime, handshake, fail-open, secrets
// and teardown follow D-38, D-39, D-42, D-43, D-45 in that document:
//   - the engine binary is resolved at engine/<platform>-<arch>/
//     opencode-agents-memory inside the package and spoken to over stdio
//     (one compact, key-sorted JSON object per line);
//   - when the engine is absent, unstartable or protocol-incompatible the
//     wrapper degrades (structured stderr warning + status file, memory
//     disabled) and the plugin still loads and answers hooks — fail-open;
//   - host credentials never reach the child (not in argv, env or stderr);
//   - teardown re-delivers the received signal after cleanup; the wrapper
//     never terminates the host process itself.
//
// Issue 82: the host evaluates this module once per instance/location inside
// one server process, and each evaluation gets its own module graph. The
// mutable bridge state is therefore a process-global registry keyed by the
// resolved storage root (`globalThis[Symbol.for("opencode-mem.bridge.v1")]`):
// one child and one handshake serve every sibling (D-31), while the engine
// lock stays the unchanged cross-PROCESS guard (D-39). The registry holds DATA
// only — no functions — so a later generation runs its own code against the
// shared state; the one process signal handler installed at the first spawn
// walks the registry rather than holding a state.

import pkg from "../package.json" with { type: "json" };
import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { accessSync, appendFile, constants, existsSync, mkdir, mkdirSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { homedir } from "node:os";
import path from "node:path";

import {
  isChildSession,
  readSession,
  resolveSessionAgent,
  resolveSessionModel,
  restorePromptBody,
} from "./compaction.js";
import { registerMemoryTools, createToolRequest } from "./tools.js";

const PROTOCOL = "1.0.0";
const MIN_PROTOCOL = "1.0.0";
const CLIENT_NAME = "opencode-mem-plugin";
const PACKAGE_ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
// Escalation ladder of §5: shutdown reply first; then 2 s → SIGTERM; then
// another 5 s window after that SIGTERM → SIGKILL. All timers live in the
// shutdownTimers array so teardown can clear everything it scheduled.
const SHUTDOWN_TERM_AFTER_MS = 2000;
const SHUTDOWN_KILL_AFTER_MS = 5000;
const SHUTDOWN_RESPONSE_TIMEOUT_MS = 2000;
const CAPTURE_DRAIN_MAX_MS = 2000;
const CAPTURE_DRAIN_POLL_MS = 50;
const REQUEST_TIMEOUT_MS = 15000;
// Issue 82: the handshake bound, and the one-auto-restart budget (never reset,
// so a second unexpected death latches degraded instead of crash-looping).
const HELLO_TIMEOUT_MS = 5000;
const MAX_AUTO_RESTARTS = 1;
// Compaction is on the host's critical path: a wedged engine must not stall it
// for the 15 s default. Both compaction calls pass this explicitly.
const COMPACTION_REQUEST_TIMEOUT_MS = 2000;
// Issue 69: the catalogue read is on the host's awaited `setup` path. With the
// 5 s hello bound, a wedged engine can delay `setup` by up to ~8 s.
const SKILLS_REQUEST_TIMEOUT_MS = 3000;
// Issue 70 wave 2: the agent-facing `memory` tool lane. `add`/`search` pay an
// embedding warm-up on a cold engine, and the hook path's 15 s default was
// chosen for `inject`, so this lane gets its own bound. A timeout still maps to
// `engine_unavailable`; the engine may complete the write afterwards (a
// recorded limitation, not a longer wait to paper over it).
const MEMORY_TOOL_REQUEST_TIMEOUT_MS = 30000;
// Issue 50 (D-26-11): the authored-text cap, in CODE POINTS — the same unit the
// engine's `chars().take(2000)` cap counts (never UTF-16 units or bytes).
const MAX_QUERY_CHARS = 2000;

// ── canonical serialization (§1, D-42) ──────────────────────────────────
// Wire objects serialize compactly with keys in lexicographic order at every
// level; every line ends with exactly one newline.

function canonicalize(value) {
  if (Array.isArray(value)) return value.map(canonicalize);
  if (value === null) return null;
  if (typeof value === "object") {
    const sorted = {};
    for (const key of Object.keys(value).sort()) {
      sorted[key] = canonicalize(value[key]);
    }
    return sorted;
  }
  return value;
}

export function encodeLine(wireObject) {
  return JSON.stringify(canonicalize(wireObject)) + "\n";
}

export function serializeRequest(method, params, session_id, id) {
  const request = { jsonrpc: "2.0", method, id };
  if (session_id === undefined) {
    request.params = params ?? {};
  } else {
    request.params = { ...(params ?? {}), session_id };
  }
  return encodeLine(request);
}

// ── capture ids (§3, D-36) ──────────────────────────────────────────────
// UUIDv7-shaped, ≤ 64 chars, client-minted, no npm dependency: 48-bit unix
// timestamp, version nibble 7, RFC 4122 variant, random restart bits.

export function newCaptureId() {
  const ms = Date.now();
  const bytes = new Uint8Array(16);
  bytes[0] = Math.floor(ms / 2 ** 40) & 0xff;
  bytes[1] = Math.floor(ms / 2 ** 32) & 0xff;
  bytes[2] = Math.floor(ms / 2 ** 24) & 0xff;
  bytes[3] = Math.floor(ms / 2 ** 16) & 0xff;
  bytes[4] = Math.floor(ms / 2 ** 8) & 0xff;
  bytes[5] = ms & 0xff;
  const random = randomBytes(10);
  for (let index = 0; index < 10; index += 1) bytes[6 + index] = random[index];
  bytes[6] = 0x70 | (bytes[6] & 0x0f); // version nibble 7
  bytes[8] = 0x80 | (bytes[8] & 0x3f); // variant nibble [89ab]
  let hex = "";
  for (const byte of bytes) hex += byte.toString(16).padStart(2, "0");
  return (
    hex.slice(0, 8) +
    "-" +
    hex.slice(8, 12) +
    "-" +
    hex.slice(12, 16) +
    "-" +
    hex.slice(16, 20) +
    "-" +
    hex.slice(20)
  );
}

// ── environment and defaults (§9) ───────────────────────────────────────

function rawEnv(name) {
  const value = process.env[name];
  return typeof value === "string" && value.length > 0 ? value : null;
}

function storageRoot() {
  const configured = rawEnv("OPENCODE_MEM_STORAGE_ROOT");
  if (configured !== null) return path.resolve(configured);
  return path.join(homedir(), ".opencode-mem", "data");
}

// The wrapper always knows the storage root (it must write the status file
// there before any engine exists); --mode/--web-host/--web-port/--web-dist are
// forwarded only when explicitly configured. The wrapper adds no default of its
// own: the engine resolves the web bundle staged beside its executable.
function engineArguments() {
  const args = ["--storage-root", storageRoot()];
  const mode = rawEnv("OPENCODE_MEM_MODE");
  if (mode !== null) args.push("--mode", mode);
  const host = rawEnv("OPENCODE_MEM_WEB_HOST");
  if (host !== null) args.push("--web-host", host);
  const port = rawEnv("OPENCODE_MEM_WEB_PORT");
  if (port !== null) args.push("--web-port", port);
  const dist = rawEnv("OPENCODE_MEM_WEB_DIST");
  if (dist !== null) args.push("--web-dist", dist);
  return args;
}

function engineExecutablePath() {
  return path.join(
    PACKAGE_ROOT,
    "engine",
    process.platform + "-" + process.arch,
    "opencode-agents-memory"
  );
}

// ── secrets (§10) ───────────────────────────────────────────────────────
// The child's environment is a fixed minimal whitelist, which also carries the
// wrapper's forwarded --web-dist setting (OPENCODE_MEM_WEB_DIST). The wrapper
// adds no default of its own: the engine resolves the web bundle staged beside
// its executable. Host credentials (an Basic password header, tokens,
// passwords, secret-bearing env vars) never reach the child: not in argv, not
// in this environment, not on stderr.

function childEnvironment() {
  const env = {};
  for (const name of [
    "PATH",
    "HOME",
    // Host transport wave: the engine resolves the live host's service file
    // from `$XDG_STATE_HOME/opencode/service.json`. Dropping this variable makes
    // the child fall back to `$HOME/.local/state`, which is a DIFFERENT answer
    // whenever the host was started with a custom state home.
    "XDG_STATE_HOME",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "OPENCODE_MEM_WEB_DIST",
    // Issue 45 (O5): the mixed-version open guard's override. Without this entry the
    // variable is dropped here and the override would never reach the engine.
    "OPENCODE_MEM_ALLOW_UNGUARDED_OPEN",
  ]) {
    const value = process.env[name];
    if (typeof value === "string" && value.length > 0) env[name] = value;
  }
  return env;
}

// ── structured stderr (§1, D-38) ────────────────────────────────────────

function writeStructured(level, event, fields) {
  try {
    const line = { level, component: "opencode-mem-plugin", event, ...fields };
    process.stderr.write(JSON.stringify(line) + "\n");
  } catch {
    // never throw to the host
  }
}

export function logWarn(event, fields = {}) {
  writeStructured("warn", event, fields);
}

function logInfo(event, fields = {}) {
  writeStructured("info", event, fields);
}

export function shortError(err) {
  const text = err && typeof err.message === "string" ? err.message : String(err);
  return text.slice(0, 120);
}

// ── engine stderr drain (issue 117) ─────────────────────────────────────
// The engine writes machine-readable NDJSON on fd 2 (`structured_stderr`:
// consolidation ticks, capture retry and dead-letter events, degraded notices)
// and everything else — a panic hook, a library's own write — as non-JSON.
// `spawn(..., stdio: ["pipe","pipe","pipe"])` gives fd 2 a pipe, and a pipe
// with no reader is both invisible and a place the child blocks in `write(2)`
// once the socket buffer fills. This drain renders each line into the engine's
// OWN log file in that file's `[ts] message: {json}` shape: no syscall in the
// read handler, a bounded queue, and a counted drop summary, so neither a slow
// nor an unwritable destination can stall the engine or the host's loop.
//
// The engine stays the sole owner of rotation, of the session banner and of
// the 5 MiB/30-day policy (ADR-0015): the wrapper appends whole lines and
// never renames, rotates, truncates or repairs.

// The cap is on the PAYLOAD JSON, not on the rendered line. The line is
// `[` + stamp + `"] "` + message + `": "` + payload + `"\n"`, so a payload
// sitting exactly on the cap still renders a longer line — at most 4259 bytes
// at the charset maximum (1 + 29 + 2 + 128 + 2 + 4096 + 1).
const ENGINE_STDERR_MAX_PAYLOAD_BYTES = 4096;

// The single place truncation happens, so the structured and unstructured
// branches cannot drift apart. The quantity is the PAYLOAD JSON's utf8 length —
// never the raw line's, the truncated text's or the rendered line's. `text` is
// a forensic byte dump of that JSON (so it carries the payload's own leading
// bytes) and `bytes` says how long the whole thing was; re-basing `text` on the
// payload's CONTENT would leave `bytes` measuring an unnamed quantity. The cut
// is by byte, not by codepoint: a cut inside a codepoint renders U+FFFD instead
// of throwing, which is a declared loss and why `truncated` exists — the cost
// is at most one extra U+FFFD, so `text` stays ≤ 4096 + 2 bytes.
function capPayload(payloadJson) {
  const bytes = Buffer.byteLength(payloadJson, "utf8");
  if (bytes <= ENGINE_STDERR_MAX_PAYLOAD_BYTES) return payloadJson;
  const text = Buffer.from(payloadJson, "utf8")
    .subarray(0, ENGINE_STDERR_MAX_PAYLOAD_BYTES)
    .toString("utf8");
  return JSON.stringify(canonicalize({ bytes, text, truncated: true }));
}

// The engine's log sink, resolved per batch: after a rotation the live file is
// this path, and re-resolving is what follows it. Deliberately NOT
// `OPENCODE_MEM_LOG_FILE`: childEnvironment() does not forward that name, so
// the engine cannot see it, and honouring it wrapper-side would split one
// logical log across two files. `home` is an argument so the path is testable
// without mutating process.env.
export function engineLogPath(home = homedir()) {
  return path.join(home, ".opencode-mem", "opencode-mem.log");
}

// The file's stamp grammar, which the frozen reader re-validates: exactly 29
// bytes `YYYY-MM-DDTHH:MM:SS.mmm±HH:MM`, local time with a zero-padded numeric
// offset (chrono's `%Y-%m-%dT%H:%M:%S%.3f%:z`). A stamp that is not that shape
// makes `parse_log_line` return None and the whole line is skipped.
function logTimestamp(date) {
  const pad = (value, width = 2) => String(value).padStart(width, "0");
  const offsetMinutes = -date.getTimezoneOffset();
  const sign = offsetMinutes < 0 ? "-" : "+";
  const magnitude = Math.abs(offsetMinutes);
  return (
    pad(date.getFullYear(), 4) +
    "-" +
    pad(date.getMonth() + 1) +
    "-" +
    pad(date.getDate()) +
    "T" +
    pad(date.getHours()) +
    ":" +
    pad(date.getMinutes()) +
    ":" +
    pad(date.getSeconds()) +
    "." +
    pad(date.getMilliseconds(), 3) +
    sign +
    pad(Math.floor(magnitude / 60)) +
    ":" +
    pad(magnitude % 60)
  );
}

// Renders one engine stderr line into the log's own envelope. The message slot
// holds one of exactly three things: an engine event name, `engine.stderr`, or
// `engine.stderr_unstructured` — never free text, because the frozen reader
// classifies the message and a colon-containing name could forge a stats
// counter.
export function engineStderrLogLine(rawLine, date = new Date()) {
  const line = String(rawLine).replace(/[\r\n]+$/, "");
  let parsed = null;
  let parsedOk = true;
  try {
    parsed = JSON.parse(line);
  } catch {
    parsedOk = false;
  }
  // Not a plain object — unparseable, null, an array, or a scalar. The fold is
  // mandatory: the reader only recognises a payload that begins with `": {"`,
  // so a bare null/array/scalar would be swallowed into the message slot, and
  // reaching for `parsed.event` on null would throw inside the stream handler.
  if (!parsedOk || typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    return (
      "[" + logTimestamp(date) + "] engine.stderr_unstructured: " +
      capPayload(JSON.stringify({ text: line })) +
      "\n"
    );
  }
  // The message slot has a grammar, because the file feeds a frozen classifier:
  // its counter prefixes and error buckets are matched on the message and
  // contain a space, and its generic rule needs `": error"`, so this charset
  // admits neither a space nor a colon (nor `": {"`, the payload delimiter).
  // An engine line therefore cannot invent a `captures` increment or an error
  // bucket out of text the wrapper wrote.
  const eventIsMessage = typeof parsed.event === "string" && /^[A-Za-z0-9_.-]{1,128}$/.test(parsed.event);
  const message = eventIsMessage ? parsed.event : "engine.stderr";
  const rest = { ...parsed };
  // Conditional on purpose: the charset rule protects the MESSAGE slot and says
  // nothing about the payload, so a refused event name stays in the payload
  // verbatim. Reconstruction is then exact per branch — identifier branch
  // `{ event: message, ...payload }` is the input; fallback branch
  // `JSON.parse(payload)` alone is the input.
  if (eventIsMessage) delete rest.event;
  return (
    "[" + logTimestamp(date) + "] " + message + ": " + capPayload(JSON.stringify(canonicalize(rest))) + "\n"
  );
}

// The queue bound, in rendered lines. No rate cap on purpose: a cap needs a
// timer (keeps the host's loop busy) and would HIDE a flood instead of counting
// it. A dropped line is counted and surfaced instead.
const ENGINE_STDERR_MAX_QUEUE = 256;
// The unterminated-chunk bound: a child that writes a binary blob with no
// newline must not grow the wrapper's heap without limit.
const ENGINE_STDERR_MAX_BUFFER = 65536;
// At most one failed write per second while the destination is unwritable. This
// is a timestamp comparison, not a latch: once the destination is writable
// again the next drained line is written.
const ENGINE_STDERR_RETRY_MS = 1000;

// The stderr `data` handler. It splits, bounds, renders (pure) and pushes —
// and nothing else: no filesystem call, sync or async, may appear before the
// enqueue. That rule is what keeps the child's socket empty, because a syscall
// here would starve the read loop, the socket would fill, and the engine would
// block in `write(2)` — this issue's failure mode arriving as a hang with no
// log line.
function enqueueEngineStderr(state, chunk) {
  state.stderrBuffer += typeof chunk === "string" ? chunk : chunk.toString("utf8");
  let index = state.stderrBuffer.indexOf("\n");
  while (index >= 0) {
    const line = state.stderrBuffer.slice(0, index);
    state.stderrBuffer = state.stderrBuffer.slice(index + 1);
    index = state.stderrBuffer.indexOf("\n");
    // The bound is checked BEFORE rendering: a 17 000-line flood must not pay
    // 17 000 JSON round-trips inside the read handler.
    if (state.stderrQueue.length >= ENGINE_STDERR_MAX_QUEUE) {
      state.stderrDropped += 1;
      continue;
    }
    state.stderrQueue.push(engineStderrLogLine(line));
  }
  // No newline in sight and past the heap bound: force the blob out as one
  // (capped, hence truncated) line rather than buffering it forever.
  if (Buffer.byteLength(state.stderrBuffer, "utf8") > ENGINE_STDERR_MAX_BUFFER) {
    if (state.stderrQueue.length >= ENGINE_STDERR_MAX_QUEUE) {
      state.stderrDropped += 1;
    } else {
      state.stderrQueue.push(engineStderrLogLine(state.stderrBuffer));
    }
    state.stderrBuffer = "";
  }
  flushEngineLog(state);
}

// Writes the queue as one newline-terminated batch. The order below is
// load-bearing.
function flushEngineLog(state) {
  // 1. One write at a time: a mid-flood flush must not synthesise a summary
  //    while a batch is in flight.
  if (state.stderrWriting) return;
  // 2. The drop summary — synthesised through the production renderer, never
  //    hand-formatted, and flushed as its own batch because the queue was
  //    empty. The counter is reset here and only here, so the summary appears
  //    at most once per quiet period (intent, not a measured guarantee: a
  //    momentary empty queue between two data events can produce a second,
  //    larger one).
  if (state.stderrQueue.length === 0 && state.stderrDropped > 0) {
    const count = state.stderrDropped;
    state.stderrDropped = 0;
    state.stderrQueue.push(
      engineStderrLogLine(
        JSON.stringify({
          component: "opencode-mem-plugin",
          event: "engine.stderr_dropped",
          level: "warn",
          count,
        })
      )
    );
  }
  // 3.
  if (state.stderrQueue.length === 0) return;
  // 4. The retry window: count the batch and stop. No timer, so a permanently
  //    unwritable destination costs at most one failed write per second rather
  //    than one per line, and a freshly synthesised summary is counted as the
  //    one line it is.
  if (
    state.stderrSuspendedAt !== 0 &&
    Date.now() - state.stderrSuspendedAt < ENGINE_STDERR_RETRY_MS
  ) {
    state.stderrDropped += state.stderrQueue.length;
    state.stderrQueue = [];
    return;
  }
  // 5. Take the WHOLE queue before attempting the write, so a failure path can
  //    never re-enter the flush with a non-empty queue (no spin). A batch is
  //    normally one `write(2)`, so `O_APPEND` appends do not interleave
  //    mid-line with the engine's own write; `appendFile` -> `fs.writeFile`
  //    retries a partial write, so a batch can in principle be split, which is
  //    the same class as the conceded rotation race: ordering only, never a
  //    corrupted line inside one renderer's own output.
  const batch = state.stderrQueue.join("");
  state.stderrQueue = [];
  state.stderrWriting = true;
  const logPath = engineLogPath();
  const done = (err) => {
    if (err) {
      failEngineBatch(state, batch, err);
      return;
    }
    // The flag is cleared on the SUCCESS path only; every error path is
    // failEngineBatch's, which clears it first thing.
    state.stderrWriting = false;
    state.stderrDirReady = true;
    // Re-flush: this is also what emits a pending drop summary, because the
    // queue is empty at this moment.
    flushEngineLog(state);
  };
  if (state.stderrDirReady === true) {
    appendFile(logPath, batch, done);
    return;
  }
  // The engine's `LogSink::init` primes this directory too; a second CREATOR of
  // a directory is harmless where a second WRITER of the file is the conceded
  // risk.
  mkdir(path.dirname(logPath), { recursive: true }, (err) => {
    if (err) {
      failEngineBatch(state, batch, err);
      return;
    }
    state.stderrDirReady = true;
    appendFile(logPath, batch, done);
  });
}

// The ONE accounting path for both failure sites — a `mkdir` error and an
// append error are the same loss, in the same retry window, with the same
// counted lines and the same one-time warning. It is also the flag's single
// clearing site: `stderrWriting = false` is its FIRST statement, because a flag
// cleared only in the success callback latches forever on the branch that never
// gets one (a `mkdir`/EEXIST failure would disable the drain for the life of
// the process: no retry, no recovery, no drop summary — the synthesis sits
// after the flush's first early return).
//
// States audit: the only way `stderrWriting` is left true is a live write, and
// every error path reaches this helper. That invariant is what the recovery
// fixture measures.
function failEngineBatch(state, batch, err) {
  state.stderrWriting = false;
  state.stderrDirReady = false;
  state.stderrSuspendedAt = Date.now();
  // Every batch is whole newline-terminated lines, so this count is exact.
  state.stderrDropped += batch.split("\n").length - 1;
  if (!state.stderrFailureLogged) {
    // Best-effort by nature: on the live host the wrapper's own stderr is a
    // self-peering socketpair with no reader, so the durable signature is the
    // counted drop summary once the file is writable again. One warning per
    // state, not per failure — a per-line warning on a full disk would be a
    // log-line amplifier inside the thing that cannot write. A failure AFTER a
    // successful recovery is silent by that same design; do not relax the gate
    // per failure.
    logWarn("engine.stderr_log_unwritable", { message: shortError(err) });
    state.stderrFailureLogged = true;
  }
  // No re-flush on purpose: the batch is already taken, so there is nothing to
  // spin on.
}

// ── status file (§7, D-38 — wrapper writer n.2) ─────────────────────────
// When there is no engine or an incompatible engine, the wrapper writes
// <storage_root>/status recording its OWN pid. Never deletes a status file it
// did not write (indeed: it never deletes a status file at all).

function writeStatusFile(state, reason) {
  try {
    const root = state.root;
    mkdirSync(root, { recursive: true });
    const ts = new Date().toISOString();
    let line = "degraded: " + reason + " (pid=" + process.pid + ", ts=" + ts + ")";
    if (line.length > 200) line = line.slice(0, 200);
    writeFileSync(path.join(root, "status"), line + "\n");
  } catch (err) {
    logWarn("status_file_write_failed", { message: shortError(err) });
  }
}

// `writeStatus: false` is the lock-loser path (D6): the engine wrote the line
// naming the HOLDER, and the wrapper must not replace it with its own pid.
function degrade(state, reason, { writeStatus = true } = {}) {
  if (state.bridgeState === "degraded") return false;
  state.bridgeState = "degraded";
  state.engineCompatible = false;
  writeStructured("warn", "degraded", { reason });
  if (writeStatus) writeStatusFile(state, reason);
  return true;
}

// ── bridge registry (issue 82) ──────────────────────────────────────────
// Keyed by the resolved storage root: one engine per store per process is
// D-31's "one engine process serves every session of a host". A second root in
// the same process legitimately gets a second child; the key is never the
// package path (two package copies on one root would create a guaranteed
// loser) and never the host directory (one engine per project against one
// store).
//
// The record holds DATA only: a function or closure here would pin the first
// generation's module until the host restarts.

const BRIDGE_REGISTRY_KEY = Symbol.for("opencode-mem.bridge.v1");

// cold: no engine resolved yet (none may exist — fail-open).
function createBridgeState(root) {
  return {
    root,
    bridgeState: "cold", // cold | installed | degraded
    child: null,
    childClosed: null,
    childExit: null, // { pid, code, signal } | null — the CURRENT child's exit
    startingInstance: null,
    pendingResponses: new Map(), // id → { resolve, reject, timer }
    lineBuffer: "",
    // Issue 117: the engine stderr drain. Data only, like every field here.
    // `stderrDropped` is per STATE (per storage root), not per child: it
    // aggregates every child until a summary is emitted. A non-zero counter at
    // process exit is lost — the same class as the queued lines, and the only
    // alternative is synchronous I/O at exit, which the drain forbids.
    stderrBuffer: "",
    stderrQueue: [],
    stderrWriting: false, // a write is in flight for the CURRENT child
    stderrDirReady: false, // the log directory is known to exist
    stderrDropped: 0, // lines lost since the last emitted summary
    stderrFailureLogged: false, // one warning per state, not per failure
    stderrSuspendedAt: 0, // timestamp of the last failed write; 0 = never
    requestCounter: 1, // ids: "hello-1", then "capture-2", ...
    engineCompatible: false,
    engineCapabilities: null, // hello.result.capabilities, or null when absent/unusable
    engineChatCaptureEnabled: null, // hello.result.config.chat_message_enabled, or null when this child advertised nothing
    // ADR-0017 amendment 2026-11-03: hello.result.config.capture_child_sessions,
    // read by the compaction restore's child gate. FAIL-CLOSED: only the exact
    // boolean `true` opens that gate, and this `false` is what an unanswered /
    // pre-handshake read sees (D2). Defence-in-depth only — the total
    // assignment in the handshake runs first on every child, so mutating this
    // initializer is measurably unobservable; do not cite it as coverage.
    engineCaptureChildSessions: false,
    engineProtocolLine: null, // hello.result.protocol, for the mismatch line
    capabilityGapsLogged: new Set(), // one line per capability per process
    pendingCaptures: 0,
    shuttingDown: false,
    shutdownTimers: [],
    autoRestarts: 0,
  };
}

// A foreign or corrupted value at the key is replaced; a same-version record
// is reused. The version lives in the symbol, so a future bridge version can
// never read this one.
function bridgeRegistry() {
  let registry = globalThis[BRIDGE_REGISTRY_KEY];
  if (!registry || !(registry.states instanceof Map)) {
    registry = { states: new Map(), handlersInstalled: false };
    globalThis[BRIDGE_REGISTRY_KEY] = registry;
  }
  return registry;
}

function bridgeStateFor(root) {
  const registry = bridgeRegistry();
  let state = registry.states.get(root);
  if (state === undefined) {
    state = createBridgeState(root);
    registry.states.set(root, state);
  }
  return state;
}

// Resolved at every call: OPENCODE_MEM_STORAGE_ROOT is read then, so a
// generation created before an env switch still targets its own root.
function bridge() {
  return bridgeStateFor(path.resolve(storageRoot()));
}

// Per generation, deliberately NOT shared: one agent's session id, prompt →
// capture ids and toast dedupe must not bleed into another's captures or
// silence another's warnings.
let capturedSessionId = null;
// Issue 98: once per process — the idle path runs per turn, so an unlatched
// "capture skipped" line would be unbounded stderr in exactly the state the
// user asked for silence.
let chatCaptureDisabledLogged = false;
// Issue 86: same one-line-per-process latch — `session.get` is unknown on every
// turn of a host that exposes no `get`, so an unlatched line would be unbounded.
let childStatusUnknownLogged = false;
// Process-unique prompt part ids: `Date.now()` collides on a back-to-back pair.
let compactionPartCounter = 0;

function currentSessionId() {
  return capturedSessionId ?? ("oc-bridge-" + process.pid);
}

function rejectPending(state, reason) {
  for (const [, entry] of state.pendingResponses) {
    clearTimeout(entry.timer);
    entry.reject(new Error(reason));
  }
  state.pendingResponses.clear();
}

function wireRequest(method, params, id) {
  const existingSessionId = params && typeof params === "object" ? params.session_id : undefined;
  return serializeRequest(method, params, existingSessionId, id);
}

function callChild(state, method, params, id, timeoutMs = REQUEST_TIMEOUT_MS) {
  if (state.child === null) {
    return Promise.reject(new Error("bridge not connected"));
  }
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      state.pendingResponses.delete(id);
      reject(new Error(method + " timed out"));
    }, timeoutMs);
    state.pendingResponses.set(id, { resolve, reject, timer });
    try {
      state.child.stdin.write(wireRequest(method, params, id));
    } catch (err) {
      clearTimeout(timer);
      state.pendingResponses.delete(id);
      reject(err);
    }
  });
}

function semverParts(version) {
  return String(version)
    .split(".")
    .map((part) => parseInt(part, 10) || 0);
}

function semverCompare(a, b) {
  const left = semverParts(a);
  const right = semverParts(b);
  for (let index = 0; index < 3; index += 1) {
    const delta = (left[index] || 0) - (right[index] || 0);
    if (delta !== 0) return delta;
  }
  return 0;
}

function protocolCompatible(engineProtocol) {
  const left = semverParts(PROTOCOL);
  const right = semverParts(engineProtocol);
  return left[0] === right[0] && semverCompare(engineProtocol, MIN_PROTOCOL) >= 0;
}

// Issue 68: a pre-`include.compaction` engine answers the compaction request
// with the ordinary chat block, which the hooks would append — a silent
// double count. The handshake's flag is the only signal: absent means no-op,
// never "send it and inspect the answer".
function engineCapabilityIn(state, name) {
  return state.engineCapabilities !== null && state.engineCapabilities[name] === true;
}

// Public wrapper: no other module sees the state object.
function engineCapability(name) {
  return engineCapabilityIn(bridge(), name);
}

// The handshake must have happened before the flag can be read (the first hook
// of a fresh process is cold), so this awaits `ensureStarted()` exactly as
// `sendRequest` does. No engine = the hooks' existing fail-open no-op.
async function compactionCapability() {
  if (!(await ensureStarted())) return false;
  if (engineCapability("compaction")) return true;
  logCapabilityGap(bridge(), "compaction");
  return false;
}

// One fixed-field line per capability per process: `capability_missing` is
// diagnostic, never a per-call log, and it carries no memory content.
function logCapabilityGap(state, capability) {
  if (state.capabilityGapsLogged.has(capability)) return;
  state.capabilityGapsLogged.add(capability);
  logWarn("engine.capability_missing", {
    capability,
    engine_protocol: state.engineProtocolLine ?? "unknown",
    min_protocol: MIN_PROTOCOL,
  });
}

function handleWireLine(state, rawLine) {
  const line = rawLine.trim();
  if (line.length === 0) return;
  let message;
  try {
    message = JSON.parse(line);
  } catch {
    // stdout of the child is protocol-only; a non-JSON line is an engine
    // defect — report the violation without echoing the content.
    logWarn("engine.stdout_unparseable_line", { bytes: Buffer.byteLength(line) });
    return;
  }
  const entry = message && typeof message === "object" ? state.pendingResponses.get(message.id) : undefined;
  if (entry === undefined) {
    logWarn("engine.unexpected_message", { id: typeof message?.id === "string" ? message.id : null });
    return;
  }
  clearTimeout(entry.timer);
  state.pendingResponses.delete(message.id);
  if (message.error) {
    // The nine tool methods put their refusal in `error.data.reason` (§4); a
    // code-only reject would drop the wrapper's own answer and turn a real
    // refusal into `engine_unavailable`. `data` is advisory and inert for every
    // caller that does not read it.
    entry.reject(
      Object.assign(new Error(message.error.message || "engine error"), {
        code: message.error.code,
        data: message.error.data,
      })
    );
  } else {
    entry.resolve(message.result);
  }
}

// Shutdown path: cached per state so a close that already fired is not waited
// on twice. The handshake uses childClosedFor (bound to the child THIS attempt
// spawned), never state.child, which a stale close may have replaced.
function childClosedPromise(state) {
  if (state.childClosed !== null) return state.childClosed;
  const active = state.child;
  if (active === null) return Promise.resolve(null);
  state.childClosed = new Promise((resolve) => {
    active.once("close", (code, signal) => resolve({ code, signal }));
    active.once("error", () => resolve({ code: active.exitCode, signal: null }));
  });
  return state.childClosed;
}

// Attempt-bound close wait used by the handshake: it never reads state.child,
// so a stale close of a previous child cannot decide this attempt. Resolves
// null when the bound elapses; the timer is always cleared.
function childClosedFor(childProcess, timeoutMs = HELLO_TIMEOUT_MS) {
  return new Promise((resolve) => {
    let timer = null;
    const finish = (value) => {
      if (timer !== null) clearTimeout(timer);
      childProcess.removeListener("close", onClose);
      resolve(value);
    };
    const onClose = (code, signal) => finish({ code, signal });
    if (childProcess.exitCode !== null || childProcess.signalCode !== null) {
      finish({ code: childProcess.exitCode, signal: childProcess.signalCode });
      return;
    }
    timer = setTimeout(() => finish(null), timeoutMs);
    childProcess.once("close", onClose);
  });
}

function teardownChild(state, reason) {
  const active = state.child;
  state.child = null;
  state.engineCapabilities = null;
  // Issue 98 (C6): a dead child's `false` must not leak onto its replacement.
  // D-98-15 keeps an omitting child from overwriting the cache, so this clear
  // is the ONLY thing that retires it.
  state.engineChatCaptureEnabled = null;
  // ADR-0017 amendment 2026-11-03: belt-and-braces. The handshake assigns the
  // restore flag totally, so this reset is redundant today — it is here so the
  // invariant "no child's advertisement outlives it" needs no argument about
  // call order.
  state.engineCaptureChildSessions = false;
  state.engineProtocolLine = null;
  // Issue 117: a partial line from a dead child is not a line. The QUEUE is
  // deliberately kept — those lines were already received and are the dead
  // child's last words; the flush loop drains them. Do not "clean up" by
  // emptying it.
  state.stderrBuffer = "";
  rejectPending(state, reason);
  if (active !== null) {
    try {
      active.stdout.removeAllListeners("data");
      active.stderr.removeAllListeners("data");
      active.removeAllListeners("close");
      active.removeAllListeners("error");
      active.stdin.end();
      active.kill("SIGKILL");
    } catch {
      // harmless — the child is already gone
    }
  }
}

// One child's close handler. The identity check is load-bearing: a stale close
// must never tear down — or vote on — the replacement child. `childExit` is
// recorded only after it passes, carrying the pid so a later handshake can
// prove the exit belongs to the child it spawned.
//
// The first unexpected death resets the state to cold (the next call re-spawns
// once, single-flight); a second one latches degraded. `autoRestarts` is never
// reset, so an engine that keeps dying cannot become a crash loop.
function retireChild(state, closing, code, signal) {
  if (state.child !== closing) return;
  state.childExit = { pid: closing.pid, code, signal };
  if (state.shuttingDown) return;
  // A death before install belongs to startBridge's catch; this branch owns
  // only a child that had completed the handshake.
  if (state.bridgeState !== "installed") return;
  teardownChild(state, "engine exited");
  state.autoRestarts += 1;
  if (state.autoRestarts <= MAX_AUTO_RESTARTS) {
    state.bridgeState = "cold";
    logWarn("engine.exited", { code, signal, restarts: state.autoRestarts });
  } else {
    degrade(state, "engine exited unexpectedly (code=" + code + ", signal=" + signal + ")");
  }
}

// Installs the engine child, performs the D-43 handshake and the §2 protocol
// compatibility gate. Any failure degrades (fail-open), never throws.
async function startBridge(state) {
  const execPath = engineExecutablePath();
  if (!existsSync(execPath)) {
    degrade(state, "no engine for " + process.platform + "-" + process.arch);
    return false;
  }
  try {
    accessSync(execPath, constants.X_OK);
  } catch {
    degrade(state, "engine not executable for " + process.platform + "-" + process.arch);
    return false;
  }

  const childProcess = spawn(execPath, engineArguments(), {
    stdio: ["pipe", "pipe", "pipe"],
    env: childEnvironment(),
  });
  state.child = childProcess;
  state.childClosed = null;
  state.childExit = null;
  state.lineBuffer = "";

  childProcess.stdout.setEncoding("utf8");
  childProcess.stdout.on("data", (chunk) => {
    state.lineBuffer += chunk;
    let index = state.lineBuffer.indexOf("\n");
    while (index >= 0) {
      const line = state.lineBuffer.slice(0, index);
      state.lineBuffer = state.lineBuffer.slice(index + 1);
      index = state.lineBuffer.indexOf("\n");
      handleWireLine(state, line);
    }
  });

  // Issue 117: re-establish ALL SEVEN drain fields on the state this call was
  // handed. The registry is deliberately cross-generation, so a state created
  // by the pre-drain module carries none of them and the first `data` event
  // would throw `TypeError: Cannot read properties of undefined (reading
  // 'length')` inside the stream handler — an unhandled throw on the host's
  // loop. Absolute reset for the three that describe a write belonging to the
  // PREVIOUS child (a latched `true` or a stale timestamp must not survive a
  // respawn; `??=` would keep it); `??=` for the four that describe facts
  // about the state (queued lines, the destination, the running loss count,
  // the one-time warning), which must survive it.
  state.stderrBuffer = "";
  state.stderrWriting = false;
  state.stderrSuspendedAt = 0;
  state.stderrQueue ??= [];
  state.stderrDirReady ??= false;
  state.stderrDropped ??= 0;
  state.stderrFailureLogged ??= false;

  // Attached BEFORE the hello call, so a child that writes before it reads is
  // already drained. The `error` listener is not decoration: a dying child
  // yields EPIPE / ERR_STREAM_DESTROYED, and an unhandled `error` event on a
  // stream is a throw on the host's loop.
  childProcess.stderr.setEncoding("utf8");
  childProcess.stderr.on("error", () => {});
  childProcess.stderr.on("data", (chunk) => enqueueEngineStderr(state, chunk));

  childProcess.once("error", (err) => {
    if (state.bridgeState === "cold") {
      degrade(state, "engine spawn failed: " + shortError(err));
    }
  });

  childProcess.once("close", (code, signal) => {
    retireChild(state, childProcess, code, signal);
  });

  installSignalHandlers();

  try {
    const hello = await callChild(
      state,
      "hello",
      { client: CLIENT_NAME, min_protocol: MIN_PROTOCOL, protocol: PROTOCOL },
      "hello-" + state.requestCounter++,
      HELLO_TIMEOUT_MS
    );
    const engineProtocol = hello && typeof hello.protocol === "string" ? hello.protocol : null;
    if (engineProtocol === null || !protocolCompatible(engineProtocol)) {
      degrade(state, "engine protocol " + String(engineProtocol) + " incompatible (need major " + semverParts(PROTOCOL)[0] + ", floor " + MIN_PROTOCOL + ")");
      teardownChild(state, "incompatible engine");
      return false;
    }
    const capabilities = hello !== null && typeof hello === "object" ? hello.capabilities : null;
    state.engineCapabilities =
      capabilities !== null && typeof capabilities === "object" && !Array.isArray(capabilities)
        ? capabilities
        : null;
    state.engineProtocolLine = engineProtocol;
    // Issue 98 (C1/C6): store only what THIS handshake advertised. A child that
    // omits the member leaves the cached value alone — teardownChild is what
    // retires a dead child's `false`, so a replacement that says nothing
    // inherits nothing. Only the exact boolean false refuses (fail-open
    // otherwise).
    const advertisedConfig =
      hello !== null && typeof hello === "object" && hello.config !== null && typeof hello.config === "object"
        ? hello.config
        : null;
    if (advertisedConfig !== null && Object.prototype.hasOwnProperty.call(advertisedConfig, "chat_message_enabled")) {
      state.engineChatCaptureEnabled = advertisedConfig.chat_message_enabled === false ? false : null;
    }
    // ADR-0017 amendment 2026-11-03: deliberately NOT issue 98's guarded
    // lattice. The restore's child gate is fail-closed, so ABSENCE IS A VALUE
    // (D2): a replacement child that advertises nothing must resolve to `false`,
    // never inherit a predecessor's `true`. Assigned totally, with no
    // `hasOwnProperty` guard, and only the exact boolean `true` opens the gate.
    state.engineCaptureChildSessions =
      advertisedConfig !== null && advertisedConfig.capture_child_sessions === true;
    state.engineCompatible = true;
    state.bridgeState = "installed";
    return true;
  } catch (err) {
    // Issue 82 (D6): a genuine lock loser exits 75 after writing the engine's
    // own holder-pid status line, and the wrapper must not replace it with its
    // own pid. Decide from the exit of the child THIS attempt spawned — never
    // from a fixed delay, and never from state.child, which a stale close may
    // have replaced. The pid check is load-bearing: today a `75` exit also
    // latches degraded, so no later child can exist and a stale record is
    // unobservable; the one-restart rule removes that latch, and a previous
    // child's code must never decide a later handshake.
    let recorded = state.childExit;
    if (recorded === null || recorded.pid !== childProcess.pid) {
      recorded = await childClosedFor(childProcess, HELLO_TIMEOUT_MS);
    }
    if (recorded !== null && recorded.code === 75) {
      degrade(state, "engine lock held by another process", { writeStatus: false });
    } else {
      degrade(state, "engine did not answer hello: " + shortError(err));
    }
    teardownChild(state, "handshake failed");
    return false;
  }
}

async function ensureStartedIn(state) {
  if (state.bridgeState === "installed" && state.engineCompatible) return true;
  if (state.bridgeState === "degraded") return false;
  // Node sets exitCode/signalCode before `close` is processed. A call in that
  // window would write into a dead pipe and fail for no reason, so treat the
  // child as gone now and let the retirement budget decide cold vs degraded.
  if (state.child !== null && (state.child.exitCode !== null || state.child.signalCode !== null)) {
    retireChild(state, state.child, state.child.exitCode, state.child.signalCode);
    if (state.bridgeState === "degraded") return false;
  }
  if (state.startingInstance === null) {
    state.startingInstance = startBridge(state).finally(() => {
      state.startingInstance = null;
    });
  }
  return state.startingInstance;
}

async function sendRequestIn(state, method, params, timeoutMs = REQUEST_TIMEOUT_MS) {
  const started = await ensureStartedIn(state);
  if (!started) throw new Error("memory degraded (fail-open): no compatible engine");
  return callChild(state, method, params, method + "-" + state.requestCounter++, timeoutMs);
}

// Public wrappers: tools.js, skills.js and v2/* keep their existing call
// shapes; every one resolves the state for the current storage root.
function ensureStarted() {
  return ensureStartedIn(bridge());
}

function sendRequest(method, params, timeoutMs = REQUEST_TIMEOUT_MS) {
  return sendRequestIn(bridge(), method, params, timeoutMs);
}

// Issue 69: the engine owns the skill catalogue. A pre-69 engine omits the
// capability, so it is NEVER asked — an absent flag means "no registration",
// not "send it and inspect the answer". The two bounds are the host's awaited
// `setup` worst case: 5 s hello + 3 s catalogue.
export async function readSkillCatalogue(projectId) {
  if (typeof projectId !== "string" || projectId.trim().length === 0) return null;
  if (!(await ensureStarted())) return null; // hello bound: 5000 ms
  if (!engineCapability("skills")) return null;
  try {
    const result = await sendRequest(
      "skills",
      { project_id: projectId },
      SKILLS_REQUEST_TIMEOUT_MS
    );
    if (result === null || typeof result !== "object" || !Array.isArray(result.skills)) {
      logWarn("skills.malformed_response", {});
      return null;
    }
    return result.skills;
  } catch (err) {
    logWarn("skills.read_failed", { message: shortError(err) });
    return null;
  }
}

// Fire-and-forget capture (§3): the ack carries no durability guarantee in
// W5.1; an error is logged once and never blindly retried. pendingCaptures is
// the counter the shutdown drain waits on.
function fireCapture(state, payload) {
  state.pendingCaptures += 1;
  void (async () => {
    try {
      await sendRequestIn(state, "capture", payload, 10000);
    } catch (err) {
      logWarn("capture.failed", { message: shortError(err) });
    } finally {
      state.pendingCaptures = Math.max(0, state.pendingCaptures - 1);
    }
  })();
}

// ── toast intents (W5.2) ────────────────────────────────────────────────
// A V2 host gives the plugin no `ui.toast`; the only user-visible channel is
// the toast RPC the TUI companion renders (`plugin/src/tui.js`).
// The two producers are the inject failure alternate and the post-compaction restore below.
// This function is
// fire-and-forget by contract: it never awaits, so a host that hangs or
// refuses the capability can neither delay nor fail a turn. Deduped per reason
// per process — the wrapper cannot read the engine's config (no JS config
// path, D8), so one toast per failure reason is the functional equivalent of
// the upstream gate. The payload is a fixed string: never the error text,
// never a transcript fragment, never a path.

const NOTIFIED = new Set();

function notifyHost(client, reason) {
  if (NOTIFIED.has(reason)) return;
  NOTIFIED.add(reason);
  let showToast;
  try {
    showToast = client?.tui?.showToast;
  } catch {
    showToast = undefined;
  }
  if (typeof showToast !== "function") {
    logInfo("toast.not_delivered", { reason });
    return;
  }
  try {
    Promise.resolve(
      showToast.call(client.tui, {
        body: {
          title: "opencode-mem",
          message: "Memory injection failed; memory may be disabled.",
          variant: "warning",
        },
      })
    ).catch((err) => logWarn("toast.failed", { reason, message: shortError(err) }));
  } catch (err) {
    logWarn("toast.failed", { reason, message: shortError(err) });
  }
}

// The restore toast (ADR-0017 DECISION 9): one per successful restore, body
// fixed except the engine's `memory_count`. Deliberately NOT routed through
// `NOTIFIED` — upstream toasts every restore, and a per-process dedupe would
// silence all but the first. Fire-and-forget: the call is made synchronously so
// the toast is observable at hook return, the returned promise is not awaited.
function notifyRestore(client, count) {
  let showToast;
  try {
    showToast = client?.tui?.showToast;
  } catch {
    showToast = undefined;
  }
  if (typeof showToast !== "function") {
    logInfo("toast.not_delivered", { reason: "memory.restored" });
    return;
  }
  try {
    Promise.resolve(
      showToast.call(client.tui, {
        body: {
          title: "Memory Restored",
          message: `${count} memories injected after compaction`,
          variant: "success",
          duration: 3000,
        },
      })
    ).catch((err) =>
      logWarn("toast.failed", { reason: "memory.restored", message: shortError(err) })
    );
  } catch (err) {
    logWarn("toast.failed", { reason: "memory.restored", message: shortError(err) });
  }
}

// ── capture/best-effort transcript extraction (never throws) ────────────

function extractText(node) {
  try {
    if (node === null || node === undefined) return null;
    // Issue 66: an injected block is the wrapper's own text, never the
    // human's. `null` (not "") so the array branch's filter drops it instead
    // of leaving a blank line. Only the exact boolean `true` drops a part.
    if (node.synthetic === true) return null;
    if (typeof node === "string") return node;
    if (Array.isArray(node)) {
      const parts = node.map(extractText).filter((part) => part !== null);
      if (parts.length > 0) return parts.join("\n");
      return null;
    }
    if (typeof node.text === "string") return node.text;
    if (typeof node.content === "string") return node.content;
    if (Array.isArray(node.parts ?? node.content)) {
      const parts = (node.parts ?? node.content).map(extractText).filter((part) => part !== null);
      if (parts.length > 0) return parts.join("\n");
    }
    return null;
  } catch {
    return null;
  }
}

// ── shutdown (§5, D-45) ─────────────────────────────────────────────────
// The wrapper never terminates the host process itself: it drops its own
// handlers once, drains pending captures, asks every state's engine to shut
// down, escalates SIGTERM → SIGKILL on a schedule, reaps the children, and
// finally re-delivers the received signal so the host's default disposition
// applies. One handler pair per process walks the registry (issue 82).

function shutdownHandler(sig) {
  process.off("SIGINT", shutdownHandler);
  process.off("SIGTERM", shutdownHandler);
  bridgeRegistry().handlersInstalled = false;
  void shutdownAll(sig);
}

// Closes every state, then re-delivers the signal ONCE. Uses the registry, not
// a captured state, so children added by later generations are closed too.
function shutdownAll(sig) {
  const states = [...bridgeRegistry().states.values()];
  return Promise.all(states.map((state) => runShutdown(state))).then(
    () => process.kill(process.pid, sig),
    () => process.kill(process.pid, sig)
  );
}

function installSignalHandlers() {
  const registry = bridgeRegistry();
  if (registry.handlersInstalled) return;
  registry.handlersInstalled = true;
  process.on("SIGINT", shutdownHandler);
  process.on("SIGTERM", shutdownHandler);
}

async function awaitCaptureDrain(state) {
  const startedAt = Date.now();
  while (state.pendingCaptures > 0 && Date.now() - startedAt < CAPTURE_DRAIN_MAX_MS) {
    await new Promise((resolve) => setTimeout(resolve, CAPTURE_DRAIN_POLL_MS));
  }
}

function scheduleEscalation(state) {
  state.shutdownTimers.push(
    setTimeout(() => {
      if (state.child !== null && state.child.exitCode === null) state.child.kill("SIGTERM");
    }, SHUTDOWN_TERM_AFTER_MS)
  );
  state.shutdownTimers.push(
    setTimeout(() => {
      if (state.child !== null && state.child.exitCode === null) state.child.kill("SIGKILL");
    }, SHUTDOWN_TERM_AFTER_MS + SHUTDOWN_KILL_AFTER_MS)
  );
}

async function runShutdown(state) {
  for (const t of state.shutdownTimers) clearTimeout(t);
  state.shutdownTimers.length = 0;
  state.shuttingDown = true;
  try {
    // Drain in-flight fire-and-forget captures before the engine stops.
    await awaitCaptureDrain(state);
  } catch {
    // drain is best-effort
  }
  if (state.child !== null) {
    if (state.engineCompatible) {
      try {
        // D2: the request carries a PROCESS-scoped id. One child serves every
        // agent, and the engine's shutdown ignores session_id, so a sibling's
        // session id would be the wrong value to send.
        await callChild(
          state,
          "shutdown",
          { session_id: "oc-bridge-" + process.pid },
          "shutdown-" + state.requestCounter++,
          SHUTDOWN_RESPONSE_TIMEOUT_MS
        );
      } catch {
        // engine silent — the escalation timers below still apply
      }
    }
    scheduleEscalation(state);
    const outcome = await Promise.race([
      childClosedPromise(state),
      new Promise((resolve) => setTimeout(() => resolve(null), SHUTDOWN_KILL_AFTER_MS * 2)),
    ]);
    for (const t of state.shutdownTimers) clearTimeout(t);
    state.shutdownTimers.length = 0;
    // Reap: the child that reached this point exited (code !== null) or was
    // killed by signal; after the SIGKILL escalation only a code 0 / null
    // pair / force-kill outcome is possible.
    logInfo("child_reaped", {
      pid: state.child.pid,
      exit_code: outcome ? outcome.code : state.child.exitCode,
      signal: outcome ? outcome.signal : null,
    });
    teardownChild(state, "shutdown complete");
  }
}

// ── plugin surface (D-34: session ids are hook data) ────────────────────

export const server = async function server(input) {
  // No engine is spawned and no host-visible state is touched here: hooks
  // exist even with no engine, so the plugin always loads (fail-open minimum).
  // Issue 34 (D-W5.3-7): the host `input.directory` is captured ONCE as the
  // immutable `project_id` and passed on every capture and inject — never a
  // per-hook override, never from the transcript or event payload.
  const hostProjectId =
    input && typeof input.directory === "string" && input.directory.length > 0
      ? input.directory
      : null;
  const hostInputRef = () => input;
  // Prompt id → capture_id: one UUIDv7 per prompt; a replayed idle delivery
  // of the same prompt reuses the SAME capture_id (idempotent upsert key).
  const captureIdByPrompt = new Map();

  const hooks = {
    "chat.message": async (hookInput, output) => {
      try {
        if (hookInput && typeof hookInput.sessionID === "string" && hookInput.sessionID.length > 0) {
          capturedSessionId = hookInput.sessionID;
        }
        // W5.3: injection happens through the eligible chat.message hook —
        // the per-message TRANSCRIPT capture is gone (the idle capture owns
        // the transcript); an inject error fails open (no block, no error).
        let block = null;
        try {
          const result = await sendRequest("inject", injectParams(queryFromParts(output)));
          if (result && typeof result.block === "string" && result.block.length > 0) {
            block = result.block;
          }
        } catch (err) {
          logWarn("inject.failed", { message: shortError(err) });
          notifyHost(hostInputRef()?.client, "inject.failed");
        }
        if (
          block !== null &&
          output && output.message && Array.isArray(output.message.parts)
        ) {
          output.message.parts.push({ type: "text", text: block, synthetic: true });
        }
      } catch (err) {
        logWarn("chat.message.failed", { message: shortError(err) });
      }
    },
    // Issue 33 / AC 1: the session's own durable memory is appended to the
    // host's compaction prompt as ONE string. Never `output.prompt` — setting
    // it (even to "") replaces the host's prompt and drops `output.context`.
    "experimental.session.compacting": async (hookInput, output) => {
      try {
        const sessionID =
          hookInput && typeof hookInput.sessionID === "string" && hookInput.sessionID.length > 0
            ? hookInput.sessionID
            : null;
        if (sessionID === null) return;
        if (!(await compactionCapability())) return;
        // D-114-2 (owner ruling): the ingest stays UNGATED — no readSession /
        // isChildSession here, so a child keeps ingesting as it does today.
        // The restore below keeps its child gate.
        const result = await sendRequest(
          "inject",
          compactionParams("ingest", sessionID),
          COMPACTION_REQUEST_TIMEOUT_MS
        );
        if (!result || typeof result.block !== "string" || result.block.length === 0) return;
        if (!output || !Array.isArray(output.context)) return;
        output.context.push(result.block);
      } catch (err) {
        logWarn("compaction.ingest_failed", { message: shortError(err) });
      }
    },
    event: async (hookInput) => {
      try {
        // AC 2: the restore branch reads the event's OWN id at the branch's
        // top, NEVER `capturedSessionId`. Dispatch is the envelope's
        // `event.type` — `properties.type` is the V2-merged shape.
        if (hookInput && hookInput.event && hookInput.event.type === "session.compacted") {
          const compactedSessionID =
            typeof hookInput.event.properties?.sessionID === "string"
              ? hookInput.event.properties.sessionID
              : null;
          await handleCompacted(compactedSessionID);
        }
        const properties =
          hookInput && hookInput.event && typeof hookInput.event === "object"
            ? hookInput.event.properties
            : null;
        let sessionId = null;
        if (hookInput && typeof hookInput.sessionID === "string" && hookInput.sessionID.length > 0) {
          sessionId = hookInput.sessionID;
        } else if (properties !== null) {
          if (typeof properties.sessionID === "string" && properties.sessionID.length > 0) {
            sessionId = properties.sessionID;
          } else if (typeof properties.session_id === "string" && properties.session_id.length > 0) {
            sessionId = properties.session_id;
          }
        }
        if (sessionId !== null) capturedSessionId = sessionId;
        // W5.3: the idle event owns the capture (fire-and-forget; never on
        // the message path).
        if (sessionId !== null && properties !== null && properties.type === "session.idle") {
          void fireIdleCapture(sessionId);
        }
      } catch (err) {
        logWarn("event.failed", { message: shortError(err) });
      }
    },
  };

  return { hooks };

  // ── local closures (kept per-server for the frozen input) ──

  // `injectParams()` carries `project_id` and stamps `currentSessionId()`;
  // `session_id` is overridden AFTER the spread with the hook's own id.
  function compactionParams(mode, sessionID) {
    return {
      ...injectParams(),
      session_id: sessionID,
      include: { compaction: mode },
    };
  }

  // AC 2, in upstream's order (`example/opencode-mem/src/index.ts:1311-1361`):
  // read → child gate → inject → no block? → agent → model → no prompt API? →
  // one prompt → toast. The read precedes agent resolution, which is what makes
  // the V2 inheritance pin reachable (no agent there, so the inject is the last
  // observation).
  async function handleCompacted(sessionID) {
    try {
      if (typeof sessionID !== "string" || sessionID.length === 0) return;
      if (!(await compactionCapability())) return;
      // ADR-0017 amendment 2026-11-03: capture the state ONCE, after the
      // capability await (which awaits `ensureStarted`, so this child's
      // handshake has run) — the D-98-7 idiom. `bridge()` re-resolves
      // `path.resolve(storageRoot())`, so re-calling it after the
      // `readSession` await could consult ANOTHER root's child.
      const state = bridge();
      const client = hostInputRef()?.client;
      const session = await readSession(client, sessionID);
      // The restore's child gate is OPT-IN, and FAIL-CLOSED: only the exact
      // boolean `true` from hello.result.config.capture_child_sessions opens it,
      // so an engine that omits the key (or a mixed-version pair) keeps today's
      // behaviour. The pre-compaction ingest is NOT gated here and must stay
      // ungated (D-114-2); the pre-turn `chat.message` inject is ungated on
      // purpose and is not this gate's business (D1).
      if (isChildSession(session) && state.engineCaptureChildSessions !== true) return;
      const result = await sendRequest(
        "inject",
        compactionParams("restore", sessionID),
        COMPACTION_REQUEST_TIMEOUT_MS
      );
      if (!result || typeof result.block !== "string" || result.block.length === 0) return;
      const agent = await resolveSessionAgent(client, sessionID, session);
      if (!agent) {
        // The one surviving skip: the agent gate. Fixed reason string only —
        // never a session id, path or render (the block is user content).
        logWarn("compaction.restore_skipped", { reason: "agent unresolved" });
        return;
      }
      const resolved = resolveSessionModel(client, sessionID, session);
      if (!client || typeof client.session?.prompt !== "function") return;
      await client.session.prompt({
        path: { id: sessionID },
        body: restorePromptBody({
          text: result.block,
          agent,
          model: resolved?.model,
          variant: resolved?.variant,
          now: compactionPartCounter++,
        }),
      });
      notifyRestore(client, result.memory_count);
    } catch (err) {
      logWarn("compaction.restore_failed", { message: shortError(err) });
    }
  }

  // Issue 50 (D-26-11): `include.query` is the authored text of the message
  // being sent — text parts that are NOT synthetic (never our own injected
  // block) and not a tool part — joined with a newline, trimmed, capped at
  // MAX_QUERY_CHARS code points. Absent/blank means the key is omitted, so an
  // ordinary message keeps today's bytes. The extraction never throws: a throw
  // would be swallowed by the hook's catch and the injection would vanish.
  function queryFromParts(output) {
    try {
      const parts =
        output && output.message && Array.isArray(output.message.parts)
          ? output.message.parts
          : output && Array.isArray(output.parts)
            ? output.parts
            : [];
      const text = parts
        .filter(
          (part) =>
            part &&
            part.type === "text" &&
            part.synthetic !== true &&
            typeof part.text === "string"
        )
        .map((part) => part.text)
        .join("\n")
        .trim();
      if (text.length === 0) return undefined;
      return Array.from(text).slice(0, MAX_QUERY_CHARS).join("");
    } catch {
      return undefined;
    }
  }

  function injectParams(query) {
    const params = { session_id: currentSessionId() };
    if (hostProjectId !== null) params.project_id = hostProjectId;
    if (typeof query === "string" && query.length > 0) {
      params.include = { query };
    }
    return params;
  }

  function reuseCaptureId(promptId) {
    if (typeof promptId === "string" && promptId.length > 0) {
      const known = captureIdByPrompt.get(promptId);
      if (typeof known === "string" && known.length > 0) return known;
      const minted = newCaptureId();
      captureIdByPrompt.set(promptId, minted);
      return minted;
    }
    return newCaptureId();
  }

  async function fireIdleCapture(sessionID) {
    try {
      // Issue 98 (D-98-7): the value is read from the state object that owns
      // the child — the same object that answered `hello`, not a second
      // `bridge()` resolution around the await.
      const state = bridge();
      const started = await ensureStartedIn(state);
      if (!started) return;
      // Issue 98 (C9): the engine advertises its resolved chatMessage.enabled
      // in hello.result.config. Return BEFORE any host call — readSession,
      // resolveSessionAgent and session.messages are all skipped, matching
      // upstream's ordering (the transcript is never fetched). Only an exact
      // boolean false refuses: an engine that predates the key advertises
      // nothing and capture continues (fail-open).
      if (state.engineChatCaptureEnabled === false) {
        if (!chatCaptureDisabledLogged) {
          chatCaptureDisabledLogged = true;
          logWarn("capture.chat_message_disabled", { session_id: sessionID });
        }
        return;
      }
      // Project first, then agent: a host that supplies neither gets exactly one
      // reason (not two) and no host round-trips. No directory => no project_id
      // => the engine would dead-letter it, so nothing is sent.
      if (hostProjectId === null) {
        logWarn("capture.project_unresolved", { session_id: sessionID });
        return;
      }
      const client = hostInputRef()?.client;
      const session = await readSession(client, sessionID);
      // Issue 86: `null` is "unknown" (no host `get`, the call threw, or a
      // non-object answer) — named once per process, then read as NOT a child
      // (fail-open, like the compaction legs).
      if (session === null && !childStatusUnknownLogged) {
        childStatusUnknownLogged = true;
        logWarn("capture.child_status_unknown", { session_id: sessionID });
      }
      // Opt-in and fail-closed, exactly like the compaction legs: only the
      // exact boolean `true` from hello.result.config.capture_child_sessions
      // opens the gate, so a child's work is never filed as the human's.
      // Returns BEFORE the transcript fetch, which would be wasted.
      if (isChildSession(session) && state.engineCaptureChildSessions !== true) {
        logWarn("capture.child_session_skipped", { session_id: sessionID });
        return;
      }
      const agentId = await resolveSessionAgent(client, sessionID, session);
      // Fail closed, no sentinel: a capture row must describe the agent that
      // produced it. Checked before the transcript fetch, which would be wasted.
      if (typeof agentId !== "string" || agentId.trim().length === 0) {
        logWarn("capture.agent_unresolved", { session_id: sessionID });
        return;
      }

      let userPrompt = null;
      let promptId = null;
      let textResponses = [];
      let toolCalls = [];
      try {
        if (client && typeof client.session?.messages === "function") {
          const response = await client.session.messages({ path: { id: sessionID } });
          const messages = Array.isArray(response?.data)
            ? response.data
            : Array.isArray(response)
              ? response
              : null;
          if (messages !== null) {
            const parsed = parseIdleTranscript(messages);
            ({ userPrompt, promptId, textResponses, toolCalls } = parsed);
          }
        }
      } catch (err) {
        logWarn("session.idle.messages_failed", { message: shortError(err) });
      }
      if (userPrompt === null) return;
      if (textResponses.length === 0 && toolCalls.length === 0) return;

      // The raw transcript rides INSIDE metadata.autoCaptureContext (the
      // engine removes the envelope before persisting):
      const metadata = { autoCaptureContext: { userPrompt, textResponses, toolCalls } };
      if (promptId !== null) metadata.promptId = promptId;

      const payload = {
        session_id: sessionID,
        agent_id: agentId,
        project_id: hostProjectId,
        // Issue 86: a child's capture is attributed to its parent, and a
        // non-child's payload stays byte-identical (D13).
        ...(isChildSession(session) ? { parent_session_id: session.parentID } : {}),
        capture_id: reuseCaptureId(promptId),
        transcript: userPrompt,
        metadata,
      };
      fireCapture(state, payload);
    } catch (err) {
      logWarn("session.idle.capture_failed", { message: shortError(err) });
    }
  }
};

/** Inject params of an idle capture (issue 34: project_id is the host dir). */
/** Port of upstream's tail-start extraction (getAIResponseMessages +
 * extractAIContent, simplified to the LAST user prompt). */
function parseIdleTranscript(messages) {
  const MAX_TOOL_INPUT_LENGTH = 100;
  let lastUserIndex = -1;
  for (let index = 0; index < messages.length; index += 1) {
    const message = messages[index];
    const role = message && message.info && message.info.role ? message.info.role : null;
    if (role === "user") lastUserIndex = index;
  }
  if (lastUserIndex === -1) {
    return { userPrompt: null, promptId: null, textResponses: [], toolCalls: [] };
  }
  const promptMessage = messages[lastUserIndex];
  const userPrompt = extractText(promptMessage?.parts ?? promptMessage) ?? "";
  const promptId =
    promptMessage && promptMessage.info && typeof promptMessage.info.id === "string"
      ? promptMessage.info.id
      : null;

  const textResponses = [];
  const toolCalls = [];
  for (let index = lastUserIndex + 1; index < messages.length; index += 1) {
    const message = messages[index];
    const role = message && message.info && message.info.role ? message.info.role : null;
    if (role === "user") break;
    if (role !== "assistant" || !message.parts || !Array.isArray(message.parts)) continue;
    const textParts = message.parts
      .filter(
        (part) =>
          part &&
          part.type === "text" &&
          typeof part.text === "string" &&
          part.synthetic !== true
      )
      .map((part) => part.text);
    if (textParts.length > 0 && textParts.join("\n").trim().length > 0) {
      textResponses.push(textParts.join("\n").trim());
    }
    for (const part of message.parts) {
      if (!part || part.type !== "tool") continue;
      const name = part.tool || "unknown";
      let input = "";
      try {
        const toolInput = part.state && part.state.input ? part.state.input : null;
        if (typeof toolInput === "string") {
          input = toolInput;
        } else if (toolInput && typeof toolInput === "object") {
          const params = [];
          for (const [key, value] of Object.entries(toolInput)) {
            params.push(String(key) + ": " + JSON.stringify(value));
          }
          input = params.join(", ");
        }
      } catch {
        input = "";
      }
      if (input.length > MAX_TOOL_INPUT_LENGTH) {
        input = input.slice(0, MAX_TOOL_INPUT_LENGTH) + "...";
      }
      toolCalls.push({ name, input });
    }
  }
  return { userPrompt, promptId, textResponses, toolCalls };
}

// ── entry module (loader contract) ─────────────────────────────────────

const id = pkg.name;

// The V2 host contract is a different entry (`{ id, setup }`), but upstream's
// loader-contract test requires the installable entry to expose BOTH. The
// import is deferred to call time: `dist/plugin.js` is the file the host loads
// on every path, so a top-level await would make module evaluation
// asynchronous for V1 users and would read the plugin.js ↔ v2/plugin.js edge
// at evaluation order.
export async function setup(ctx) {
  const mod = await import("./v2/plugin.js");
  const adapterCleanup = await mod.setup(ctx);
  // Issue 22/70: the two agent-facing tools (`recall_project_sessions` and the
  // ten-mode `memory` toolkit) ride the V2 `setup` path's `ctx.tool.transform`
  // door (the only registration surface the live host exposes). The memory
  // tool's whole failure law lives in `createToolRequest`: the handshake, then
  // the advertised capability, then the send. Registration is fail-open: a
  // host without the door keeps capture and injection.
  const toolCleanup = await registerMemoryTools(ctx, {
    sendRecall: (params) => sendRequest("recall", params, REQUEST_TIMEOUT_MS),
    request: createToolRequest({
      ensureStarted,
      capability: (method) => {
        if (engineCapability(method)) return true;
        logCapabilityGap(bridge(), method);
        return false;
      },
      send: (method, params) => sendRequest(method, params, MEMORY_TOOL_REQUEST_TIMEOUT_MS),
    }),
  });
  return async () => {
    await toolCleanup();
    await adapterCleanup();
  };
}

export default { id, server, setup };
