// W5.2 shared test harness for the staged-package cases. NOT a test file:
// `node --test` only matches `*.test.mjs`, so this module has no cases of its
// own — its correctness is proved by its eight users: `v2-plugin-adapter.test.mjs`
// and `compaction-v2-inheritance.test.mjs` (`mode: "v2"`),
// `wrapper-toast.test.mjs` (`mode: "v1"`), `compaction-hook.test.mjs`
// (`mode: "v1-compaction"`), `compaction-capability.test.mjs`
// (`mode: "v1-compaction"`, the capability gate), `memory-tools.test.mjs`
// (`mode: "v2-tool"`), `skills-bridge.test.mjs` (`mode: "v2-skills"`) and
// `v2-compaction-hook.test.mjs` (`mode: "v2-compaction"`).
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
 * `./v2/plugin.js` inside the staged package), all of `src/**` and `index.js`.
 */
export function stagePackageTree(tmpRoot) {
  const pkgDir = path.join(tmpRoot, "opencode-mem");
  fs.mkdirSync(pkgDir, { recursive: true });
  fs.copyFileSync(path.join(REAL_PKG_ROOT, "package.json"), path.join(pkgDir, "package.json"));
  fs.cpSync(path.join(REAL_PKG_ROOT, "dist"), path.join(pkgDir, "dist"), { recursive: true });
  fs.cpSync(path.join(REAL_PKG_ROOT, "src"), path.join(pkgDir, "src"), { recursive: true });
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
}) {
  const dir = path.join(pkgDir, "engine", `${process.platform}-${process.arch}`);
  fs.mkdirSync(dir, { recursive: true });
  const binPath = path.join(dir, "opencode-agents-memory");
  fs.writeFileSync(binPath, stubEngineSource({ injectMode, requestLogPath, manifestPath, capabilities, skills, skillsPath }), {
    mode: 0o755,
  });
  fs.chmodSync(binPath, 0o755);
  return binPath;
}

// Static body (no template interpolation inside the script beyond CONFIG):
// answers the canned protocol-1.0.0 results, records its own pid, exits on
// SIGTERM or stdin EOF.
function stubEngineSource({ injectMode, requestLogPath, manifestPath, capabilities, skills, skillsPath }) {
  const config = JSON.stringify({ injectMode, requestLogPath, manifestPath, capabilities, skills, skillsPath });
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
const { injectMode, requestLogPath, manifestPath, capabilities, skills, skillsPath } = CONFIG;
function logLine(object) {
  try { fs.appendFileSync(requestLogPath, JSON.stringify(object) + "\\n"); } catch {}
}
try {
  fs.writeFileSync(manifestPath, JSON.stringify({ pid: process.pid, argv: process.argv, env: process.env }, null, 2) + "\\n");
} catch {}
process.on("SIGTERM", () => process.exit(0));
let buffer = "";
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
    if (reply) process.stdout.write(JSON.stringify(reply) + "\\n");
  }
});
process.stdin.on("end", () => process.exit(0));
function respond(parsed) {
  const id = parsed && parsed.id !== undefined ? parsed.id : null;
  const method = parsed && parsed.method;
  if (method === "inject" && injectMode === "error") {
    return { id, jsonrpc: "2.0", error: { code: -32001, message: "degraded" } };
  }
  // "silent": never answer inject at all (the wrapper's 2 s timeout is the
  // only way out), while hello/capture/shutdown still answer.
  if (method === "inject" && injectMode === "silent") return null;
  let result = null;
  if (method === "hello") {
    result = {
      // \`capabilities === null\` means "no capabilities object at all": the
      // undefined key is dropped by JSON.stringify — the pre-D-43 shape.
      capabilities: capabilities === null ? undefined : capabilities,
      engine_version: "0.1.0",
      // The advertised set moves with the capability: a pre-69 stub shows five.
      methods: capabilities?.skills === true
        ? ["capture", "inject", "recall", "skills", "health", "shutdown"]
        : ["capture", "inject", "recall", "health", "shutdown"],
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
  } else if (method === "inject") {
    if (injectMode === "compaction") {
      const params = (parsed && parsed.params) || {};
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
    process.stdout.write(JSON.stringify(reply) + "\\n", () => process.exit(0));
    return null;
  }
  return reply;
}
`;
}

/**
 * The driver script, written into the case's temp dir by its consumer. One
 * source, six modes (`DRIVER_MODE`), each reading its fixture from the JSON
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
 *          `{ helloCount, captureLine, injectCount, toastEmits }`.
 *   "v2-tool" — imports the staged `dist/plugin.js`, calls `setup(ctx)` against
 *          an async fake tool surface, invokes the registered
 *          `recall_project_sessions` tool and writes
 *          `{ names, payloadText, wire }`.
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
 *          consoleErrors, helloCount }`.
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
    try { messages.push(JSON.parse(line).got); } catch {}
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
  writeRecord({ helloCount, captureLine, injectCount, toastEmits, sessionGetCalls, consoleErrors });
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
