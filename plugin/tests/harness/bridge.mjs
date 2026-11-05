// W5.2 shared test harness for the staged-package cases. NOT a test file:
// `node --test` only matches `*.test.mjs`, so this module has no cases of its
// own — its correctness is proved by its nine users: `v2-plugin-adapter.test.mjs`
// and `compaction-v2-inheritance.test.mjs` (`mode: "v2"`),
// `wrapper-toast.test.mjs` (`mode: "v1"`), `compaction-hook.test.mjs`
// (`mode: "v1-compaction"`), `compaction-capability.test.mjs`
// (`mode: "v1-compaction"`, the capability gate), `memory-tools.test.mjs`
// (`mode: "v2-tool"` for the recall case and `mode: "v2-memory-tool"` for the
// issue-70 wave-2 cases), `skills-bridge.test.mjs` (`mode: "v2-skills"`),
// `v2-compaction-hook.test.mjs` (`mode: "v2-compaction"`) and
// `inject-query.test.mjs` (`mode: "v1-compaction"`, issue 50's query params).
// `shared-bridge.test.mjs` (issue 82) adds `mode: "shared-bridge"`.
//
// Why a subprocess and not an import: `plugin/src/plugin.js` spawns the engine
// with piped stdio and no `detached`/`unref` (`:365-368`), and the stub exits
// only on SIGTERM or stdin EOF. A test that imported the staged `dist/plugin.js`
// in-process would hold that child on `node:test`'s own event loop: a
// regression would HANG the file instead of failing it, and a success could
// leave an orphan. Every case therefore runs through `DRIVER_SOURCE` in a child
// process and asserts the stub pid is gone via `assertReaped` in `t.after`.
//
// The stub is staged where `startBridge` looks — `engine/<platform>-<arch>/
// opencode-agents-memory` — and must be executable: an absent file degrades
// with "no engine for …" and a non-`X_OK` one with "engine not executable
// for …" (`plugin/src/plugin.js:353-362`). A degraded bridge writes no wire log
// and no manifest, which would make the three toast failure cases pass for the
// wrong reason and the success pin fail for the wrong reason.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const REAL_PKG_ROOT = path.resolve(here, "..", "..");

/**
 * Complete temp copy of the package: `package.json` (the bundle's JSON import
 * attribute needs it), ALL of `dist/**` (H5: a `setup()` call resolves
 * `./v2/plugin.js` inside the staged package), all of `src/**`, all of
 * `skills/**` (the packaged skill the bridge reads) and `index.js`.
 */
export function stagePackageTree(tmpRoot) {
  const pkgDir = path.join(tmpRoot, "opencode-mem");
  fs.mkdirSync(pkgDir, { recursive: true });
  fs.copyFileSync(path.join(REAL_PKG_ROOT, "package.json"), path.join(pkgDir, "package.json"));
  fs.cpSync(path.join(REAL_PKG_ROOT, "dist"), path.join(pkgDir, "dist"), { recursive: true });
  fs.cpSync(path.join(REAL_PKG_ROOT, "src"), path.join(pkgDir, "src"), { recursive: true });
  fs.cpSync(path.join(REAL_PKG_ROOT, "skills"), path.join(pkgDir, "skills"), { recursive: true });
  fs.copyFileSync(path.join(REAL_PKG_ROOT, "index.js"), path.join(pkgDir, "index.js"));
  return pkgDir;
}

/**
 * Stages the stub engine at the path the wrapper resolves and makes it
 * executable. `injectMode` is the knob the four toast cases need:
 *   "error"   → `inject` is answered with a JSON-RPC error, so `sendRequest`
 *               rejects (`plugin/src/plugin.js:315`), the `chat.message` catch
 *               runs and `notifyHost` is reached;
 *   "success" → `inject` answers the canonical empty block, so no catch runs
 *               and no toast may be emitted.
 */
// Additive (D-43): each key is `true` exactly when the stub serves it. Issue
// 69 adds `skills` here; the wrapper reads known keys and ignores the rest.
export const DEFAULT_STUB_CAPABILITIES = {
  compaction: true,
  fire_and_forget_capture: true,
  skills: true,
  standalone: true,
  web: true,
};

export function stageStubEngine(pkgDir, {
  requestLogPath,
  manifestPath,
  injectMode = "error",
  capabilities = DEFAULT_STUB_CAPABILITIES,
  skills = [],
  skillsPath = null, // when set, re-read per `skills` request (the re-register case)
  // Issue 98 knobs (additive; every existing user leaves them unset):
  // `helloConfig` is emitted verbatim as the hello result's top-level `config`
  // object; `undefined` drops the key (the pre-issue-98 shape).
  helloConfig = undefined,
  // when set, re-read per `hello` handshake (the `skillsPath` idiom), so a
  // replacement child can advertise a different value than its predecessor.
  helloConfigPath = null,
  // method → {code, message, data}: answers that method with a JSON-RPC error
  // whose `data` survives to the wrapper (the issue-70 wave-2 T22 knob).
  wireErrors = {},
  // Issue 82 knobs (additive; every existing user leaves them unset):
  // die after answering `n` non-hello requests (the dead-child restart case);
  diesAfterRequests = null,
  // write the real loser line to `path` at t≈0, answer nothing, exit 75 after
  // `exitDelayMs` (the slow-exit holder-pid case).
  loserStatus = null,
  // Issue 82 lifecycle records (spawned/exited) default to a sidecar so the
  // existing raw-log readers keep seeing wire messages only; the shared-bridge
  // driver points this AT the request log (the plan's literal shape).
  lifecycleLogPath = null,
}) {
  const dir = path.join(pkgDir, "engine", `${process.platform}-${process.arch}`);
  fs.mkdirSync(dir, { recursive: true });
  const binPath = path.join(dir, "opencode-agents-memory");
  fs.writeFileSync(binPath, stubEngineSource({ injectMode, requestLogPath, manifestPath, capabilities, skills, skillsPath, helloConfig, helloConfigPath, wireErrors, diesAfterRequests, loserStatus, lifecycleLogPath: lifecycleLogPath ?? (requestLogPath ? requestLogPath + ".lifecycle" : null) }), {
    mode: 0o755,
  });
  fs.chmodSync(binPath, 0o755);
  return binPath;
}

// Static body (no template interpolation inside the script beyond CONFIG):
// answers the canned protocol-1.0.0 results, records its own pid, exits on
// SIGTERM or stdin EOF.
function stubEngineSource({ injectMode, requestLogPath, manifestPath, capabilities, skills, skillsPath, helloConfig, helloConfigPath, wireErrors, diesAfterRequests, loserStatus, lifecycleLogPath }) {
  const config = JSON.stringify({ injectMode, requestLogPath, manifestPath, capabilities, skills, skillsPath, helloConfig, helloConfigPath, wireErrors, diesAfterRequests, loserStatus, lifecycleLogPath });
  const blocks = JSON.stringify({
    restore:
      "## Restored Session Memory\n\n### Memory 1\nRetries are capped at three\n\nTags: project, opencode-mem\n\n### Memory 2\nDeploys go through the staging gate first\n\nTags: project, opencode-mem\n\n",
    ingest:
      "## Session Memory Before Compaction\n\n### Memory 1\nRetries are capped at three\n\nTags: project, opencode-mem\n\n### Memory 2\nDeploys go through the staging gate first\n\nTags: project, opencode-mem\n\n",
    project:
      "<project_agent_session_history>\n## Project facts\n- The deploy key lives in op://infra/deploy\n</project_agent_session_history>",
  });
  return `#!/usr/bin/env node
const CONFIG = ${config};
const BLOCKS = ${blocks};
import fs from "node:fs";
const { injectMode, requestLogPath, manifestPath, capabilities, skills, skillsPath, helloConfig, helloConfigPath, wireErrors, diesAfterRequests, loserStatus, lifecycleLogPath } = CONFIG;
// Issue 70 wave 1's nine wire methods; appended to the hello's advertised set
// only when the capability object advertises the same key.
const TOOL_METHODS = ["add", "search", "forget", "list", "profile", "list-shards", "migrate", "export", "import"];
function logLine(object) {
  try { fs.appendFileSync(requestLogPath, JSON.stringify(object) + "\\n"); } catch {}
}
function lifecycleLine(object) {
  if (lifecycleLogPath) {
    try { fs.appendFileSync(lifecycleLogPath, JSON.stringify(object) + "\\n"); } catch {}
  }
}
// Issue 82: every exit path appends its pid first, so a test can wait for the
// child to be gone before racing the next call.
let exiting = false;
function exitNow(code) {
  if (exiting) return;
  exiting = true;
  lifecycleLine({ exited: process.pid });
  process.exit(code);
}
lifecycleLine({ spawned: process.pid, argv: process.argv });
// The genuine lock loser: write the engine's own line (naming the HOLDER) and
// exit 75 after the configured delay, answering nothing.
if (loserStatus) {
  try {
    fs.writeFileSync(loserStatus.path, "degraded: engine lock held (pid=" + loserStatus.holderPid + ", ts=" + loserStatus.ts + ")\\n");
  } catch {}
  setTimeout(() => exitNow(75), loserStatus.exitDelayMs);
}
try {
  // The capability record is part of the manifest so a case can prove what
  // this stub actually ADVERTISED (the wire log holds requests, not answers).
  fs.writeFileSync(manifestPath, JSON.stringify({ pid: process.pid, argv: process.argv, env: process.env, capabilities }, null, 2) + "\\n");
} catch {}
process.on("SIGTERM", () => exitNow(0));
let buffer = "";
let answered = 0;
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  buffer += chunk;
  let index = buffer.indexOf("\\n");
  while (index >= 0) {
    const line = buffer.slice(0, index);
    buffer = buffer.slice(index + 1);
    index = buffer.indexOf("\\n");
    if (!line.trim()) continue;
    let parsed;
    try { parsed = JSON.parse(line); } catch { continue; }
    logLine({ got: parsed });
    const reply = respond(parsed);
    if (!reply) continue;
    const out = JSON.stringify(reply) + "\\n";
    if (typeof diesAfterRequests === "number" && parsed && parsed.method !== "hello") {
      answered += 1;
      if (answered >= diesAfterRequests) {
        process.stdout.write(out, () => exitNow(1));
        continue;
      }
    }
    process.stdout.write(out);
  }
});
process.stdin.on("end", () => exitNow(0));
function respond(parsed) {
  // The loser answers nothing: the wrapper's hello bound is the only way out.
  if (loserStatus) return null;
  const id = parsed && parsed.id !== undefined ? parsed.id : null;
  const method = parsed && parsed.method;
  // Read inside the generated body (never from the harness's own scope).
  const params = parsed.params ?? {};
  // The T22 knob: an error reply whose data must survive to the wrapper.
  const wireError = wireErrors[method];
  if (wireError) {
    return { id, jsonrpc: "2.0", error: { code: wireError.code, message: wireError.message, data: wireError.data } };
  }
  if (method === "inject" && injectMode === "error") {
    return { id, jsonrpc: "2.0", error: { code: -32001, message: "degraded" } };
  }
  // "silent": never answer inject at all (the wrapper's 2 s timeout is the
  // only way out), while hello/capture/shutdown still answer.
  if (method === "inject" && injectMode === "silent") return null;
  let result = null;
  if (method === "hello") {
    // Issue 98: the resolved chatMessage.enabled, re-read per hello when a path
    // is given (the skillsPath idiom above). \`undefined\` drops the key, which is
    // the pre-issue-98 shape the wrapper must treat as fail-open.
    let helloConfigValue = helloConfig;
    if (helloConfigPath) {
      try { helloConfigValue = JSON.parse(fs.readFileSync(helloConfigPath, "utf8")); } catch { /* keep the startup copy */ }
    }
    result = {
      // \`capabilities === null\` means "no capabilities object at all": the
      // undefined key is dropped by JSON.stringify — the pre-D-43 shape.
      capabilities: capabilities === null ? undefined : capabilities,
      // Issue 98: the sibling object the wrapper reads for the chat-capture
      // gate. \`undefined\` drops the key — a build predating the key.
      config: helloConfigValue === undefined ? undefined : helloConfigValue,
      engine_version: "0.1.0",
      // The base five stay, \`skills\` follows its own key, and the nine issue-70
      // tool methods are APPENDED when their key is true — never rebuild the
      // list from the keys alone, which would drop the base five.
      methods: [
        ...(capabilities?.skills === true
          ? ["capture", "inject", "recall", "skills", "health", "shutdown"]
          : ["capture", "inject", "recall", "health", "shutdown"]),
        ...TOOL_METHODS.filter((name) => capabilities?.[name] === true),
      ],
      protocol: "1.0.0",
    };
  } else if (method === "skills") {
    let answer = skills;
    if (skillsPath) {
      try { answer = JSON.parse(fs.readFileSync(skillsPath, "utf8")); } catch { /* keep the startup copy */ }
    }
    result = { skills: answer };
  } else if (method === "capture") {
    result = { accepted: true };
  } else if (method === "recall") {
    result = { items: [], count: 0 };
  } else if (method === "add") {
    result = { id: "mem-stub-1" };
  } else if (method === "search") {
    result = { count: 1, items: [{ id: "mem-stub-1", content: "stub row", similarity: 0.9, rank: 1 }] };
  } else if (method === "list") {
    result = { count: 0, items: [] };
  } else if (method === "forget") {
    result = { deleted: 1 };
  } else if (method === "profile") {
    // Presence, not truthiness: the wrapper's empty-string write is a write.
    result = Object.prototype.hasOwnProperty.call(params, "content")
      ? { saved: true }
      : { profile: null };
  } else if (method === "list-shards") {
    result = {
      storagePath: "/stub/storage",
      currentProject: {
        tag: "project_stub",
        scopeHash: "0123456789abcdef",
        projectPath: "/workspace/project",
      },
      shards: [],
      summary: {
        total: 0,
        current: 0,
        linked: 0,
        orphaned: 0,
        missingFile: 0,
        empty: 0,
        ambiguous: 0,
      },
    };
  } else if (method === "migrate") {
    result = {
      actions: [
        {
          type: "Rename",
          from: "/old/a.db",
          to: "/workspace/project/a.db",
          detail: "a.db -> a.db",
        },
      ],
      dry_run: params.dry_run === true,
      matched_by: "storedProjectPath",
      migrated_memories: 3,
      migrated_shards: 1,
      new_hash: "0123456789abcdef",
      new_path: "/workspace/project",
      old_hash: "fedcba9876543210",
      old_path: "/old",
    };
  } else if (method === "export") {
    result = {
      container_tag: "project_stub",
      count: 2,
      output_path: params.output_path,
      scope_hash: "0123456789abcdef",
    };
  } else if (method === "import") {
    result = {
      container_tag: "project_stub",
      dry_run: params.dry_run === true,
      imported: 1,
      rejected: [],
      skipped: [{ id: "mem-old", reason: "duplicate" }],
    };
  } else if (method === "inject") {
    if (injectMode === "compaction") {
      const mode = params.include && params.include.compaction;
      if (mode === "restore") {
        result = { block: BLOCKS.restore, memory_count: 7, tokens: 0, truncated: false, handling_ms: 0 };
      } else if (mode === "ingest") {
        result = { block: BLOCKS.ingest, memory_count: 2, tokens: 0, truncated: false, handling_ms: 0 };
      } else {
        // The trap: a bare inject (no include.compaction) gets the chat block,
        // so a no-subtraction implementation appends it to output.context.
        result = { block: BLOCKS.project, memory_count: 1, tokens: 0, truncated: false, handling_ms: 0 };
      }
    } else {
      result = { block: null, memory_count: 0, tokens: 0, truncated: false, handling_ms: 0 };
    }
  } else if (method === "health") {
    result = {
      ok: true,
      mode: "full",
      degraded_reason: null,
      protocol: "1.0.0",
      engine_version: "0.1.0",
      db: { path: "stub.db", ready: true },
      uptime_ms: 1,
      sessions: 0,
    };
  } else if (method === "shutdown") {
    result = { ok: true };
  } else {
    return null;
  }
  const reply = { id, jsonrpc: "2.0", result };
  if (method === "shutdown") {
    process.stdout.write(JSON.stringify(reply) + "\\n", () => exitNow(0));
    return null;
  }
  return reply;
}
`;
}

/**
 * The driver script, written into the case's temp dir by its consumer. One
 * source, seven modes (`DRIVER_MODE`), each reading its fixture from the JSON
 * file whose path the parent passes as argv[3]:
 *
 *   "v1" — imports the staged `dist/plugin.js`, builds `server()` from the
 *          fixture's client spec, fires its `chat.message` events, races each
 *          hook call against a 2 s guard started immediately before the call,
 *          and writes `{ settled, elapsedMs, toastCalls }`.
 *   "v1-compaction" — drives the staging/compaction hooks in fixture order and
 *          writes the recorded call outcomes.
 *   "v2" — imports the staged `dist/v2/plugin.js`, materializes the fixture's
 *          serializable ctx into real functions, calls `setup(ctx)`, fires the
 *          recorded `prompt` hook when the fixture carries one (the V2→legacy
 *          client→`notifyHost`→toast path), lets the supplied host events flow
 *          through the recorded subscription, polls the stub's wire log for the
 *          `capture` line and writes
 *          `{ helloCount, captureLine, injectCount, toastEmits, sessionGetCalls,
 *          promptCalls, syntheticCalls, consoleErrors }`.
 *   "v2-tool" — imports the staged `dist/plugin.js`, calls `setup(ctx)` against
 *          an async fake tool surface, invokes the registered
 *          `recall_project_sessions` tool and writes
 *          `{ names, payloadText, wire }`.
 *   "v2-memory-tool" — imports the staged `dist/plugin.js`, calls `setup(ctx)`
 *          against an async fake tool surface, invokes the registered `memory`
 *          tool once per fixture call (in order, with the fixture's
 *          `toolContext`) and writes `{ names, payloads, wire }`.
 *   "v2-skills" — imports the staged `dist/v2/plugin.js`, calls `setup(ctx)`
 *          against an incremental fake `skill.transform` (the callback runs at
 *          registration), releases the recorded event stream afterwards (both
 *          the adapter watcher's and the skills subscription's generators
 *          receive the fixture events), polls the wire for the re-register
 *          `skills` request and writes
 *          `{ adds, updates, removes, disposed, catalogueRequests, helloCount,
 *          consoleErrors, elapsedMs }`.
 *   "v2-compaction" — imports the staged `dist/v2/plugin.js`, calls `setup(ctx)`
 *          against a complete fake ctx (session hooks, transcript, session
 *          lookup, send spies and an ALWAYS-registered RPC recorder, so the
 *          no-send row cannot be vacuous), requires the registered `compaction`
 *          hook, fires the fixture prompts then each compaction step, releases
 *          the recorded event stream, awaits cleanup and writes
 *          `{ registeredHooks, systems, compactionErrors, injects, promptCalls,
 *          syntheticCalls, sessionGetCalls, contextCalls, toastEmits,
 *          consoleErrors, helloCount }`; `fixture.session.syntheticRejects`
 *          makes the synthetic write reject (the toast-after-write knob).
 */
export const DRIVER_SOURCE = `import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const pkgDir = process.argv[2];
const fixturePath = process.argv[3];
const mode = process.env.DRIVER_MODE ?? "v1";
const fixture = JSON.parse(fs.readFileSync(fixturePath, "utf8"));
process.env.OPENCODE_MEM_STORAGE_ROOT = process.env.DRIVER_STORAGE_ROOT;

function writeRecord(record) {
  fs.writeFileSync(process.env.DRIVER_DONE_PATH, JSON.stringify(record));
}

function readWire() {
  let text = "";
  try { text = fs.readFileSync(process.env.STUB_LOG, "utf8"); } catch { return []; }
  const messages = [];
  for (const line of text.split("\\n")) {
    if (!line.trim()) continue;
    try {
      const got = JSON.parse(line).got;
      // Spawn/exit records are not wire messages; keep them out of the
      // sequence every existing case counts.
      if (got !== undefined) messages.push(got);
    } catch {}
  }
  return messages;
}

if (mode === "v1") {
  const mod = await import(pathToFileURL(path.join(pkgDir, "dist", "plugin.js")));
  const spec = fixture.client ?? {};
  const client = {};
  if (Array.isArray(spec.sessionMessages)) {
    client.session = { messages: async () => ({ data: spec.sessionMessages }) };
  }
  let toastCalls = 0;
  const toastBodies = [];
  if (spec.showToast === "counting") {
    client.tui = {
      showToast: async (input) => {
        toastCalls += 1;
        toastBodies.push(input?.body ?? input ?? null);
        return { data: true };
      },
    };
  } else if (spec.showToast === "never") {
    client.tui = {
      showToast: (input) => {
        toastCalls += 1;
        toastBodies.push(input?.body ?? input ?? null);
        return new Promise(() => {});
      },
    };
  }
  const { hooks } = await mod.default.server({ client, directory: fixture.directory ?? pkgDir });
  let settled = false;
  let elapsedMs = 0;
  for (const event of fixture.events ?? []) {
    if (event.waitDeadChild) {
      const lifecycleLog = process.env.STUB_LOG + ".lifecycle";
      const deadline = Date.now() + 2000;
      while (Date.now() < deadline) {
        let entries = [];
        try {
          entries = fs
            .readFileSync(lifecycleLog, "utf8")
            .split("\\n")
            .filter(Boolean)
            .map((l) => JSON.parse(l));
        } catch {}
        const spawned = entries.filter((e) => e.spawned !== undefined);
        const lastSpawn = spawned[spawned.length - 1];
        if (lastSpawn !== undefined) {
          const logged = entries.some((e) => e.exited === lastSpawn.spawned);
          let gone = false;
          try {
            process.kill(lastSpawn.spawned, 0);
          } catch (err) {
            gone = err.code === "ESRCH";
          }
          if (logged && gone) break;
        }
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    const startedAt = Date.now();
    const guard = new Promise((resolve) => setTimeout(() => resolve("guard"), 2000));
    const outcome = await Promise.race([
      hooks["chat.message"](event.input ?? {}, event.output ?? {}).then(() => "hook"),
      guard,
    ]);
    settled = outcome === "hook";
    elapsedMs = Date.now() - startedAt;
  }
  writeRecord({ settled, elapsedMs, toastCalls, toastBodies });
  process.exit(0);
}

if (mode === "v1-compaction") {
  const mod = await import(pathToFileURL(path.join(pkgDir, "dist", "plugin.js")));
  // Exactly the four capabilities the compaction handlers may use. The
  // session-keyed maps are the multi-session instrument (fixture C).
  const sessionGetCalls = [];
  const sessionMessagesCalls = [];
  const promptCalls = [];
  const toastBodies = [];
  const calls = [];
  const contextAfter = {};
  const client = {
    session: {
      get: async (input) => {
        const id = input?.path?.id;
        sessionGetCalls.push(id);
        return { data: (fixture.sessions ?? {})[id] ?? {} };
      },
      messages: async (input) => {
        const id = input?.path?.id;
        sessionMessagesCalls.push(id);
        return { data: (fixture.sessionMessages ?? {})[id] ?? [] };
      },
      prompt: async (input) => {
        promptCalls.push({ path: input?.path, body: input?.body });
        return { data: {} };
      },
    },
    tui: {
      showToast: async (input) => {
        toastBodies.push(input?.body ?? input ?? null);
        return { data: true };
      },
    },
  };
  const { hooks } = await mod.default.server({ client, directory: fixture.directory ?? pkgDir });
  let promptPresent = false;
  let promptValue = null;
  async function runCall(label, invoke) {
    const startedAt = Date.now();
    const guard = new Promise((resolve) => setTimeout(() => resolve("guard"), 3000));
    const outcome = await Promise.race([invoke().then(() => "hook"), guard]);
    calls.push({ hook: label, settled: outcome === "hook", elapsedMs: Date.now() - startedAt });
  }
  const steps = fixture.steps ?? ["chat.message", "compacting", "compacted"];
  for (const step of steps) {
    const descriptor = typeof step === "string" ? { hook: step } : step;
    const hookName = descriptor.hook;
    const sessionID = descriptor.sessionID ?? "sess-A";
    if (hookName === "chat.message") {
      if (descriptor.sessionID !== undefined) {
        throw new Error("fixture error: a chat.message step always fires sess-OTHER");
      }
      const output = { message: { parts: [{ type: "text", text: "hi" }] } };
      await runCall("chat.message", () => hooks["chat.message"]({ sessionID: "sess-OTHER" }, output));
    } else if (hookName === "compacting") {
      const output = { context: [] };
      await runCall("compacting", () =>
        hooks["experimental.session.compacting"]({ sessionID }, output)
      );
      contextAfter[sessionID] = output.context;
      promptPresent = Object.prototype.hasOwnProperty.call(output, "prompt");
      promptValue = output.prompt === undefined ? null : output.prompt;
    } else if (hookName === "compacted") {
      const repeat = descriptor.compactedRepeat ?? fixture.compactedRepeat ?? 1;
      for (let index = 0; index < repeat; index += 1) {
        const properties = descriptor.compactedNoId === true ? {} : { sessionID };
        await runCall("compacted", () =>
          hooks.event({ event: { type: "session.compacted", properties } })
        );
      }
    } else {
      throw new Error("unknown fixture step: " + hookName);
    }
  }
  writeRecord({
    contextAfter,
    promptPresent,
    promptValue,
    promptCalls,
    toastBodies,
    sessionGetCalls,
    sessionMessagesCalls,
    calls,
  });
  process.exit(0);
}

if (mode === "v2") {
  const mod = await import(pathToFileURL(path.join(pkgDir, "dist", "v2", "plugin.js")));
  let registeredRpc = null;
  const sessionHooks = new Map();
  const toastEmits = [];
  const sessionGetCalls = [];
  const promptCalls = [];
  const syntheticCalls = [];
  const consoleErrors = [];
  const originalConsoleError = console.error;
  console.error = (...args) => {
    consoleErrors.push(args.map((arg) => String(arg)).join(" "));
  };
  // The handled signal fires only after the consumer asks for the NEXT value,
  // which a for-await loop does after awaiting the handler for the current
  // one. That is what makes the record honest without polling.
  let handledResolve = null;
  const handled = new Promise((resolve) => {
    handledResolve = resolve;
  });
  const ctx = {
    location: fixture.location,
    session: {
      hook: async (name, callback) => {
        sessionHooks.set(name, callback);
      },
      context: async () => fixture.session?.context ?? [],
      get: async ({ sessionID }) => {
        sessionGetCalls.push(sessionID);
        return fixture.session?.get?.[sessionID] ?? null;
      },
      // Issue 112 send spies: the restore's write is a synthetic, and a queued
      // prompt is the mutant. Recording is what makes "zero" an observation.
      prompt: async (input) => {
        promptCalls.push(input);
        return { data: {} };
      },
      synthetic: async (input) => {
        syntheticCalls.push(input);
        return { data: {} };
      },
    },
    event: {
      async *subscribe({ signal }) {
        for (const event of fixture.events ?? []) {
          yield event;
          if (handledResolve !== null) {
            const resolve = handledResolve;
            handledResolve = null;
            resolve();
          }
        }
        await new Promise((resolve) => {
          if (signal.aborted) { resolve(); return; }
          signal.addEventListener("abort", () => resolve(), { once: true });
        });
      },
    },
  };
  if (fixture.rpc?.registered === true) {
    ctx.rpc = {
      register: async (definition) => {
        registeredRpc = definition?.id ?? null;
        return {
          events: {
            emit: async (name, data) => {
              toastEmits.push({ name, data });
            },
          },
        };
      },
    };
  }
  await mod.setup(ctx);
  if (fixture.rpc?.registered === true && registeredRpc !== "opencode-mem") {
    throw new Error("the v2 entry did not register the toast rpc descriptor");
  }
  // The V2 prompt hook is the path setup -> legacy client -> notifyHost -> toast.
  if (fixture.prompt) {
    const promptHook = sessionHooks.get("prompt");
    if (typeof promptHook !== "function") {
      throw new Error("the v2 entry did not register a prompt hook");
    }
    await promptHook(fixture.prompt);
  }
  if ((fixture.events ?? []).length > 0) {
    await Promise.race([handled, new Promise((resolve) => setTimeout(resolve, 5000))]);
  }
  let captureLine = null;
  if (fixture.pollForCapture !== false) {
    const deadline = Date.now() + 10000;
    for (;;) {
      captureLine = readWire().find((message) => message && message.method === "capture") ?? null;
      if (captureLine !== null) break;
      if (Date.now() > deadline) break;
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
  }
  console.error = originalConsoleError;
  const wire = readWire();
  const helloCount = wire.filter((message) => message && message.method === "hello").length;
  const injectCount = wire.filter((message) => message && message.method === "inject").length;
  writeRecord({
    helloCount,
    captureLine,
    injectCount,
    toastEmits,
    sessionGetCalls,
    promptCalls,
    syntheticCalls,
    consoleErrors,
  });
  process.exit(0);
}

if (mode === "v2-compaction") {
  const mod = await import(pathToFileURL(path.join(pkgDir, "dist", "v2", "plugin.js")));
  const sessionHooks = new Map();
  const toastEmits = [];
  const sessionGetCalls = [];
  const contextCalls = [];
  const promptCalls = [];
  const syntheticCalls = [];
  const consoleErrors = [];
  const originalConsoleError = console.error;
  console.error = (...args) => {
    consoleErrors.push(args.map((arg) => String(arg)).join(" "));
  };
  // Released only after the hooks ran: the compaction callback is the subject,
  // and a watcher that consumed an event first would only add scheduling noise.
  let releaseEvents = null;
  const eventsReleased = new Promise((resolve) => {
    releaseEvents = resolve;
  });
  const ctx = {
    location: fixture.location,
    session: {
      hook: async (name, callback) => {
        sessionHooks.set(name, callback);
      },
      context: async (input) => {
        contextCalls.push(input?.sessionID);
        return fixture.session?.context ?? [];
      },
      get: async (input) => {
        const sessionID = input?.sessionID;
        sessionGetCalls.push(sessionID);
        return fixture.session?.get?.[sessionID] ?? null;
      },
      // Recording send spies: the no-send row is vacuous without them.
      prompt: async (input) => {
        promptCalls.push(input);
        return { data: {} };
      },
      synthetic: async (input) => {
        // Issue 112 regression knob: a host that refuses the write must leave
        // no toast and one typed failure line.
        if (fixture.session?.syntheticRejects === true) {
          throw new Error("synthetic rejected");
        }
        syntheticCalls.push(input);
        return { data: {} };
      },
    },
    // Always installed: registerToastRpc registers only when ctx.rpc.register
    // exists, so without it toastEmits stays [] even for a sending mutant.
    rpc: {
      register: async () => ({
        events: {
          emit: async (name, data) => {
            toastEmits.push({ name, data });
          },
        },
      }),
    },
    event: {
      async *subscribe({ signal }) {
        await eventsReleased;
        for (const event of fixture.events ?? []) {
          yield event;
        }
        if (signal.aborted) return;
        await new Promise((resolve) => signal.addEventListener("abort", resolve, { once: true }));
      },
    },
  };

  const cleanup = await mod.setup(ctx);
  const compactionHook = sessionHooks.get("compaction");
  if (typeof compactionHook !== "function") {
    // A missing registration must not surface as "... is not a function" at
    // the call site.
    throw new Error("no compaction hook");
  }

  if (Array.isArray(fixture.prompts) && fixture.prompts.length > 0) {
    const promptHook = sessionHooks.get("prompt");
    if (typeof promptHook !== "function") {
      throw new Error("the v2 entry did not register a prompt hook");
    }
    for (const prompt of fixture.prompts) {
      await promptHook(prompt);
    }
  }

  const systems = [];
  const compactionErrors = [];
  for (const step of fixture.compactions ?? []) {
    const sessionID = step.sessionID;
    const system = step.system ?? [{ type: "text", text: "BASE" }];
    const event = {
      sessionID,
      model: { providerID: "acme", id: "model-a" },
      system,
      messages: [],
      options: {},
      agent: "build",
      tools: {},
    };
    try {
      await compactionHook(event);
    } catch (error) {
      // Recording, not rethrowing: the field must never be vacuous, and the
      // error is serialized so the record stays JSON.
      compactionErrors.push({ sessionID, error: String(error) });
    }
    systems.push({ sessionID, system });
  }

  releaseEvents();
  await cleanup();
  console.error = originalConsoleError;
  const wire = readWire();
  writeRecord({
    registeredHooks: [...sessionHooks.keys()].sort(),
    systems,
    compactionErrors,
    injects: wire
      .filter((message) => message && message.method === "inject")
      .map((message) => ({ ...message.params })),
    promptCalls,
    syntheticCalls,
    sessionGetCalls,
    contextCalls,
    toastEmits,
    consoleErrors,
    helloCount: wire.filter((message) => message && message.method === "hello").length,
  });
  process.exit(0);
}

if (mode === "v2-tool") {
  const mod = await import(pathToFileURL(path.join(pkgDir, "dist", "plugin.js")));
  const added = [];
  const ctx = {
    location: { directory: fixture.directory, project: { directory: fixture.directory } },
    session: { hook: async () => {} },
    event: {
      // No events are fed (that is the "never-yielding" part), but the body
      // MUST resolve on abort: registerV2Adapter's cleanup is
      // controller.abort() then await watcher, so a body that awaits a
      // never-resolving promise hangs await cleanup() and the parent only
      // sees runDriver's 30 s SIGKILL (timedOut: true).
      subscribe: async function* ({ signal }) {
        if (signal.aborted) return;
        await new Promise((resolve) => signal.addEventListener("abort", resolve, { once: true }));
      },
    },
    tool: {
      transform: async (callback) => {
        await callback({ add: (tool) => added.push(tool) });
        return { dispose: async () => {} };
      },
    },
  };
  const cleanup = await mod.setup(ctx);
  const tool = added.find((entry) => entry.name === "recall_project_sessions");
  const result = await tool.execute({ projectId: fixture.directory }, { sessionID: fixture.sessionID });
  const wire = readWire();
  await cleanup();
  writeRecord({ names: added.map((entry) => entry.name), payloadText: result.content, wire });
  process.exit(0);
}

if (mode === "v2-memory-tool") {
  const mod = await import(pathToFileURL(path.join(pkgDir, "dist", "plugin.js")));
  const added = [];
  const ctx = {
    location: { directory: fixture.directory, project: { directory: fixture.directory } },
    session: { hook: async () => {} },
    event: {
      // Same shape as "v2-tool": the body MUST resolve on abort, or the
      // parent only sees runDriver's SIGKILL.
      subscribe: async function* ({ signal }) {
        if (signal.aborted) return;
        await new Promise((resolve) => signal.addEventListener("abort", resolve, { once: true }));
      },
    },
    tool: {
      transform: async (callback) => {
        await callback({ add: (tool) => added.push(tool) });
        return { dispose: async () => {} };
      },
    },
  };
  const cleanup = await mod.setup(ctx);
  const tool = added.find((entry) => entry.name === "memory");
  if (tool === undefined) throw new Error("the memory tool was not registered");
  const payloads = [];
  for (const call of fixture.calls ?? []) {
    // No coercion: an omitted toolContext stays omitted, which is the
    // session-less case the tool must refuse.
    const result = await tool.execute(call.args ?? {}, call.toolContext);
    payloads.push(JSON.parse(result.content));
  }
  const wire = readWire();
  await cleanup();
  writeRecord({ names: added.map((entry) => entry.name), payloads, wire });
  process.exit(0);
}

if (mode === "v2-skills") {
  const mod = await import(pathToFileURL(path.join(pkgDir, "dist", "v2", "plugin.js")));
  const consoleErrors = [];
  const originalConsoleError = console.error;
  console.error = (...args) => {
    consoleErrors.push(args.map((arg) => String(arg)).join(" "));
  };

  const storedSkills = new Map();
  const adds = [];
  const updates = [];
  const removes = [];
  let stored = null;
  let disposed = false;
  let releaseEvents = null;
  const eventsReleased = new Promise((resolve) => {
    releaseEvents = resolve;
  });
  const editor = {
    list: () => [...storedSkills.values()],
    get: (id) => storedSkills.get(id),
    add: (skill) => {
      adds.push(skill);
      storedSkills.set(skill.id, { ...skill });
    },
    update: (id, mutate) => {
      updates.push(id);
      const current = { ...storedSkills.get(id) };
      mutate(current);
      storedSkills.set(id, current);
    },
    remove: (id) => {
      removes.push(id);
      storedSkills.delete(id);
    },
  };
  const ctx = {
    location: fixture.location,
    session: { hook: async () => {} },
    tool: {
      // The tool surface is not this case's subject; a no-op keeps console
      // clean and mirrors the live host's shape.
      transform: async () => ({ dispose: async () => {} }),
    },
    skill: {
      transform: async (callback) => {
        stored = callback;
        callback(editor); // the host runs the callback at registration
        return {
          dispose: async () => {
            disposed = true;
          },
        };
      },
      reload: async () => stored(editor), // the second call the AC pins
    },
    event: {
      // Two subscribers (the adapter watcher and the skills subscription) each
      // get their own generator; both receive the same released events.
      async *subscribe({ signal }) {
        await eventsReleased;
        for (const event of fixture.events ?? []) {
          yield event;
        }
        if (signal.aborted) return;
        await new Promise((resolve) => signal.addEventListener("abort", resolve, { once: true }));
      },
    },
  };

  const startedAt = Date.now();
  const cleanup = await mod.setup(ctx);
  const elapsedMs = Date.now() - startedAt;

  // The re-register case: rewrite the staged catalogue file, release the event
  // stream (the registration's own event predates the subscription) and wait
  // for the second "skills" request.
  if (fixture.skillsAfterPath && Array.isArray(fixture.skillsAfter)) {
    fs.writeFileSync(fixture.skillsAfterPath, JSON.stringify(fixture.skillsAfter));
    releaseEvents();
    const deadline = Date.now() + 3000;
    for (;;) {
      const requests = readWire().filter((message) => message && message.method === "skills");
      if (requests.length >= 2) break;
      if (Date.now() > deadline) break;
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
  } else {
    releaseEvents();
  }

  await cleanup();
  console.error = originalConsoleError;
  const wire = readWire();
  const catalogueRequests = wire
    .filter((message) => message && message.method === "skills")
    .map((message) => message.params);
  const helloCount = wire.filter((message) => message && message.method === "hello").length;
  writeRecord({ adds, updates, removes, disposed, catalogueRequests, helloCount, consoleErrors, elapsedMs });
  process.exit(0);
}

if (mode === "shared-bridge") {
  // Issue 82: reproduce the host's loader exactly. registerHooks mirrors
  // packages/plugin/src/source.node.ts: when the PARENT carries a generation
  // marker, every local specifier it resolves is rewritten with that marker.
  // A cache-busted second import therefore yields a second whole graph — the
  // entry module AND its relative imports — not just a second entry.
  const { registerHooks } = await import("node:module");
  const GENERATION_MARKER = /__fixture_generation=(\\d+)/;
  registerHooks({
    resolve(specifier, context, nextResolve) {
      const resolved = nextResolve(specifier, context);
      const marker = GENERATION_MARKER.exec(context.parentURL ?? "");
      if (marker === null || !resolved.url.startsWith("file:")) return resolved;
      const url = new URL(resolved.url);
      url.searchParams.set("__fixture_generation", marker[1]);
      return { url: url.href, shortCircuit: true };
    },
  });

  function readLog() {
    let text = "";
    try { text = fs.readFileSync(process.env.STUB_LOG, "utf8"); } catch { return []; }
    const entries = [];
    for (const line of text.split("\\n")) {
      if (!line.trim()) continue;
      try { entries.push(JSON.parse(line)); } catch {}
    }
    return entries;
  }

  function makeCtx(added) {
    return {
      location: { directory: fixture.location, project: { directory: fixture.location } },
      session: { hook: async () => {} },
      event: {
        subscribe: async function* ({ signal }) {
          if (signal.aborted) return;
          await new Promise((resolve) => signal.addEventListener("abort", resolve, { once: true }));
        },
      },
      tool: {
        transform: async (callback) => {
          await callback({ add: (tool) => added.push(tool) });
          return { dispose: async () => {} };
        },
      },
      rpc: {
        register: async () => ({ events: { emit: async () => {} } }),
      },
    };
  }

  const baseUrl = pathToFileURL(path.join(pkgDir, "dist", "plugin.js")).href;
  const added1 = [];
  const ctx1 = makeCtx(added1);
  const gen1 = await import(baseUrl + "?__fixture_generation=1");

  async function callShards(tool, sessionID) {
    const result = await tool.execute({ mode: "list-shards" }, { sessionID });
    return JSON.parse(result.content);
  }
  function memoryTool(added) {
    const tool = added.find((entry) => entry.name === "memory");
    if (tool === undefined) throw new Error("the memory tool was not registered");
    return tool;
  }

  const answers = [];
  const roots = fixture.variant === "roots";
  // The roots case needs generation 1's first call BEFORE the env switch, so
  // the first child keeps the first root; generation 2 then lives on the second.
  if (roots) {
    await gen1.setup(ctx1);
    answers.push(await callShards(memoryTool(added1), "ses-1"));
    process.env.OPENCODE_MEM_STORAGE_ROOT = fixture.secondRoot;
  }

  const added2 = [];
  const ctx2 = makeCtx(added2);
  const gen2 = await import(baseUrl + "?__fixture_generation=2");
  if (fixture.concurrent === true) {
    await Promise.all([gen1.setup(ctx1), gen2.setup(ctx2)]);
  } else {
    if (!roots) await gen1.setup(ctx1);
    await gen2.setup(ctx2);
  }

  const tool1 = memoryTool(added1);
  const tool2 = memoryTool(added2);

  if (fixture.variant === "dead-child") {
    for (let index = 0; index < 3; index += 1) {
      answers.push(await callShards(index % 2 === 0 ? tool1 : tool2, "ses-" + (index + 1)));
      const spawned = readLog().filter((entry) => entry.spawned !== undefined);
      const lastSpawn = spawned[spawned.length - 1];
      if (lastSpawn === undefined) continue;
      // The stub logs exited BEFORE process.exit, so the log line alone is
      // not proof the OS has reaped it; wait for the pid to be gone (which is
      // also when the wrapper's close event has run) before the next call.
      const deadline = Date.now() + 2000;
      while (Date.now() < deadline) {
        const logged = readLog().some((entry) => entry.exited === lastSpawn.spawned);
        let gone = false;
        try {
          process.kill(lastSpawn.spawned, 0);
        } catch (error) {
          gone = error.code === "ESRCH";
        }
        if (logged && gone) break;
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
    }
  } else if (!roots) {
    answers.push(await callShards(tool1, "ses-1"));
    answers.push(await callShards(tool2, "ses-2"));
  } else {
    answers.push(await callShards(tool2, "ses-2"));
  }

  const log = readLog();
  const spawned = log
    .filter((entry) => entry.spawned !== undefined)
    .map((entry) => ({ pid: entry.spawned, argv: entry.argv }));
  writeRecord({
    pid: process.pid,
    helloCount: log.filter((entry) => entry.got && entry.got.method === "hello").length,
    spawned,
    exited: log.filter((entry) => entry.exited !== undefined).map((entry) => entry.exited),
    answers,
  });
  // End via the plugin's own SIGTERM path (it reaps the child); with no
  // handler installed the default disposition ends the process directly. The
  // timer keeps the loop alive across the handler's async teardown — without
  // it an empty loop trips Node's unsettled-top-level-await exit (13) before
  // the signal is re-delivered.
  process.kill(process.pid, "SIGTERM");
  await new Promise((resolve) => setTimeout(resolve, 5000));
}

throw new Error("unknown driver mode: " + mode);
`;

/**
 * Spawns the driver under a bounded SIGKILL and resolves with its exit and
 * captured output. `env` is used verbatim — pass `{ ...process.env, ...vars }`.
 */
export function runDriver(driverPath, args, env, timeoutMs = 30000) {
  return new Promise((resolve) => {
    const driver = spawn(process.execPath, [driverPath, ...args], {
      env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    driver.stdout.setEncoding("utf8");
    driver.stdout.on("data", (chunk) => (stdout += chunk));
    driver.stderr.setEncoding("utf8");
    driver.stderr.on("data", (chunk) => (stderr += chunk));
    const killer = setTimeout(() => driver.kill("SIGKILL"), timeoutMs);
    driver.once("close", (code, signal) => {
      clearTimeout(killer);
      resolve({ code, signal, stdout, stderr, timedOut: signal === "SIGKILL" });
    });
  });
}

/**
 * Asserts the stub engine the driver's wrapper spawned is gone (pid `ESRCH`)
 * within 2 s — `plugin/tests/shutdown.test.mjs:234-239` precedent. Called from
 * `t.after(...)` by every case, so a leaked child fails the case instead of
 * outliving the run.
 */
export async function assertReaped(manifestPath, timeoutMs = 2000) {
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  assert.ok(Number.isInteger(manifest.pid), "stub manifest must carry the child pid");
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    try {
      process.kill(manifest.pid, 0);
    } catch (error) {
      if (error.code === "ESRCH") break;
      throw error;
    }
    if (Date.now() > deadline) break;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  assert.throws(
    () => process.kill(manifest.pid, 0),
    /ESRCH/,
    "stub engine must be reaped (pid gone) within the 2 s bound"
  );
}
