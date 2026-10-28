// Issue 67 — the V2 first-class compaction hook (pre-compaction ingest).
//
// The host's V2 surface registers `ctx.session.hook("compaction", cb)` with a
// SINGLE input (`{sessionID, model, system, messages, options, agent, tools}`)
// and no output pair; the port's V1 closure is an `(input, output)` pair with
// `output.context: string[]`. The adapter therefore delegates to the V1
// `experimental.session.compacting` handler and copies every rendered block
// onto `event.system` as a `SystemPart` — never `event.messages` (a user-role
// transcript message) and never `event.result` (it skips the model call).
//
// Two halves are pinned here that the issue demanded as decisions, not prose:
//   - the child gate is NOT applied on V2 (`createLegacyClient` exposes no
//     `session.get`, so `readSession` returns null and ADR-0017 D2's
//     positive-evidence rule says proceed);
//   - there is NO send on V2 (`createLegacyClient` exposes no `session.prompt`),
//     so the restore half stays a read and the adapter announces the skip with
//     one fixed `compaction.restore_skipped` line per forwarded event.
//
// The driver cases run the staged `dist/v2/plugin.js` in a SUBPROCESS
// (`tests/harness/bridge.mjs`, `mode: "v2-compaction"`): `setup` spawns the
// engine, and an in-process import would hold that child on `node:test`'s own
// event loop. The in-process cases use a fake hooks map directly — the seam
// under test is the registered callback, not the engine.

import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import {
  DEFAULT_STUB_CAPABILITIES,
  DRIVER_SOURCE,
  assertReaped,
  runDriver,
  stagePackageTree,
  stageStubEngine,
} from "./harness/bridge.mjs";

const ADAPTER_MODULE = "../src/v2/adapter.js";

// The exact pre-issue-33 engine shape: the capability object is present, the
// `compaction` key is not.
const PRE_68_CAPABILITIES = { ...DEFAULT_STUB_CAPABILITIES };
delete PRE_68_CAPABILITIES.compaction;

const INGEST_HEADING = "## Session Memory Before Compaction";

// The registered-callback seam: `session.hook` records the callbacks and the
// event stream parks until cleanup aborts it (no event is needed here).
function createHarness() {
  const hooks = new Map();
  const ctx = {
    location: {
      directory: "/workspace/project",
      project: { id: "project", directory: "/workspace/project" },
    },
    session: {
      hook: async (name, callback) => {
        hooks.set(name, callback);
      },
    },
    event: {
      async *subscribe({ signal }) {
        if (signal.aborted) return;
        await new Promise((resolve) => signal.addEventListener("abort", resolve, { once: true }));
      },
    },
  };
  return { ctx, hooks };
}

function readWire(requestLogPath) {
  let text = "";
  try {
    text = fs.readFileSync(requestLogPath, "utf8");
  } catch {
    return [];
  }
  return text
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line) => JSON.parse(line).got);
}

// Every structured stderr line, in order. A non-JSON line is kept as an
// `unparsed` entry so a broken channel cannot vanish from the count (the same
// helpers as `tests/compaction-capability.test.mjs`; `logWarn` writes to
// stderr, never `console.error`).
function structuredLines(stderr) {
  const events = [];
  for (const line of stderr.split("\n")) {
    if (line.trim().length === 0) continue;
    try {
      events.push(JSON.parse(line));
    } catch {
      events.push({ unparsed: line });
    }
  }
  return events;
}

function capabilityLines(stderr) {
  return structuredLines(stderr).filter(
    (event) => event !== null && typeof event === "object" && event.event === "engine.capability_missing"
  );
}

function restoreSkippedLines(stderr) {
  return structuredLines(stderr).filter(
    (event) => event !== null && typeof event === "object" && event.event === "compaction.restore_skipped"
  );
}

function injectsOf(injects, mode) {
  return injects.filter((params) => params?.include?.compaction === mode);
}

function bareInjects(injects) {
  return injects.filter((params) => params?.include?.compaction === undefined);
}

async function runCompactionCase(t, options) {
  const { injectMode = "compaction", capabilities = DEFAULT_STUB_CAPABILITIES, fixture: build } = options;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-v2-compaction-hook-"));
  const pkgDir = stagePackageTree(tmp);
  const requestLogPath = path.join(tmp, "stub-requests.ndjson");
  const manifestPath = path.join(tmp, "stub-manifest.json");
  stageStubEngine(pkgDir, { requestLogPath, manifestPath, injectMode, capabilities });
  t.after(() => assertReaped(manifestPath));

  const fixture = typeof build === "function" ? build(tmp) : build;
  const donePath = path.join(tmp, "driver-done.json");
  const fixturePath = path.join(tmp, "fixture.json");
  fs.writeFileSync(fixturePath, JSON.stringify(fixture));
  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, DRIVER_SOURCE);

  const result = await runDriver(
    driverPath,
    [pkgDir, fixturePath],
    {
      ...process.env,
      DRIVER_MODE: "v2-compaction",
      DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
      DRIVER_DONE_PATH: donePath,
      STUB_LOG: requestLogPath,
    },
    30000
  );
  assert.equal(
    result.code,
    0,
    `the v2-compaction driver failed (signal=${String(result.signal)}): stderr=${result.stderr}`
  );
  const record = JSON.parse(fs.readFileSync(donePath, "utf8"));
  return { record, wire: readWire(requestLogPath), stderr: result.stderr, directory: fixture.location.directory };
}

// The append oracle: BASE plus exactly one injected part, and that part is the
// engine's ingest render.
function assertSystemCarriesTheIngest(system) {
  assert.equal(system.length, 2, "BASE plus exactly one appended ingest part");
  assert.deepEqual(system[0], { type: "text", text: "BASE" }, "the host's own system part is untouched");
  assert.equal(system[1].type, "text", "the append is a SystemPart, never a raw string");
  assert.ok(
    system[1].text.startsWith(INGEST_HEADING),
    `the appended part is the engine's ingest render (saw ${JSON.stringify(system[1].text)})`
  );
}

test(
  "test_v2_compaction_hook_appends_the_v1_render_to_the_request_system",
  { timeout: 5000 },
  async () => {
    const { registerV2Adapter } = await import(ADAPTER_MODULE);
    const harness = createHarness();
    const seen = [];
    const cleanup = await registerV2Adapter(harness.ctx, {
      "experimental.session.compacting": async (input, output) => {
        seen.push(input);
        output.context.push("RENDER");
      },
    });

    const hook = harness.hooks.get("compaction");
    assert.equal(typeof hook, "function", "the adapter registers the host's compaction hook");

    const event = { sessionID: "ses-A", system: [{ type: "text", text: "BASE" }] };
    await hook(event);

    assert.deepEqual(
      event.system,
      [
        { type: "text", text: "BASE" },
        { type: "text", text: "RENDER" },
      ],
      "the V1 render is copied onto event.system as a SystemPart"
    );
    assert.deepEqual(seen, [{ sessionID: "ses-A" }], "the V1 hook receives the host's single input shape");

    await cleanup();
  }
);

test(
  "test_v2_compaction_hook_awaits_the_v1_hook_before_reading_its_output",
  { timeout: 5000 },
  async () => {
    const { registerV2Adapter } = await import(ADAPTER_MODULE);
    const harness = createHarness();
    const cleanup = await registerV2Adapter(harness.ctx, {
      "experimental.session.compacting": async (_input, output) => {
        await new Promise((resolve) => setTimeout(resolve, 5));
        output.context.push("LATE");
      },
    });

    const hook = harness.hooks.get("compaction");
    assert.equal(typeof hook, "function", "the adapter registers the host's compaction hook");

    const event = { sessionID: "ses-A", system: [{ type: "text", text: "BASE" }] };
    // The assertion runs on the callback's own resolution: a fire-and-forget
    // delegation would read `output.context` as [] here.
    await hook(event);

    assert.deepEqual(
      event.system,
      [
        { type: "text", text: "BASE" },
        { type: "text", text: "LATE" },
      ],
      "the callback awaits the V1 hook before reading its output"
    );

    await cleanup();
  }
);

test(
  "test_v2_compaction_hook_forwards_the_host_session_id_and_not_the_last_prompt",
  { timeout: 30000 },
  async (t) => {
    const { record, directory } = await runCompactionCase(t, {
      fixture: (tmp) => {
        const directory = path.join(tmp, "project");
        return {
          location: { directory, project: { id: "project-1", directory } },
          // The prompt runs FIRST and sets the V1 wrapper's captured session id
          // to ses-OTHER. A callback that reuses it (or sends {}) is caught here.
          prompts: [{ sessionID: "ses-OTHER", messageID: "msg-OTHER", prompt: { text: "prior turn" } }],
          compactions: [{ sessionID: "ses-A" }],
          events: [],
          rpc: { registered: true },
        };
      },
    });

    assert.equal(record.helloCount, 1, "one engine client for the whole setup");
    assert.deepEqual(record.registeredHooks, ["compaction", "context", "prompt"]);
    assert.deepEqual(record.compactionErrors, [], "the callback settled");

    const bare = bareInjects(record.injects);
    assert.equal(bare.length, 1, "the prompt hook's own inject reached the wire first");
    assert.equal(bare[0].session_id, "ses-OTHER", "the prompt inject carries the prompt's session id");

    const ingest = injectsOf(record.injects, "ingest");
    assert.equal(ingest.length, 1, "one ingest per compaction request");
    assert.equal(
      ingest[0].session_id,
      "ses-A",
      "the ingest carries the compaction event's session id, never the last prompt's"
    );
    assert.equal(ingest[0].project_id, directory, "project_id is the host directory");

    assert.equal(record.systems.length, 1);
    assert.equal(record.systems[0].sessionID, "ses-A");
    assertSystemCarriesTheIngest(record.systems[0].system);
  }
);

test(
  "test_v2_compaction_hook_no_ops_when_the_engine_lacks_the_capability",
  { timeout: 30000 },
  async (t) => {
    const { record, stderr } = await runCompactionCase(t, {
      capabilities: PRE_68_CAPABILITIES,
      fixture: (tmp) => {
        const directory = path.join(tmp, "project");
        return {
          location: { directory },
          compactions: [{ sessionID: "ses-A" }],
          events: [],
          rpc: { registered: true },
        };
      },
    });

    assert.deepEqual(
      record.systems,
      [{ sessionID: "ses-A", system: [{ type: "text", text: "BASE" }] }],
      "the gated path appends nothing (the stub's bare inject would have carried the project chat block)"
    );
    assert.deepEqual(record.injects, [], "no inject of any kind — the gate runs before the wire");
    assert.deepEqual(record.compactionErrors, [], "a gated no-op is not a rejection");
    assert.equal(record.sessionGetCalls.length, 0, "the gate precedes the host read too");

    const gaps = capabilityLines(stderr);
    assert.equal(gaps.length, 1, `exactly one engine.capability_missing line (stderr=${stderr})`);
    assert.equal(gaps[0].capability, "compaction");
    assert.equal(gaps[0].event, "engine.capability_missing");
  }
);

test(
  "test_v2_compaction_hook_runs_once_per_compaction_request",
  { timeout: 30000 },
  async (t) => {
    const { record } = await runCompactionCase(t, {
      fixture: (tmp) => {
        const directory = path.join(tmp, "project");
        return {
          location: { directory },
          // The host compacts repeatedly in one session: a process-level
          // "already ingested" Set would starve the second request.
          compactions: [{ sessionID: "ses-A" }, { sessionID: "ses-A" }],
          events: [],
          rpc: { registered: true },
        };
      },
    });

    assert.deepEqual(record.compactionErrors, []);
    assert.equal(record.systems.length, 2, "both compaction requests were driven");
    for (const entry of record.systems) {
      assert.equal(entry.sessionID, "ses-A");
      assertSystemCarriesTheIngest(entry.system);
    }
    assert.equal(injectsOf(record.injects, "ingest").length, 2, "two requests, two ingest injects");
  }
);

test(
  "test_v2_compaction_seam_ingests_then_reads_after_without_a_send",
  { timeout: 30000 },
  async (t) => {
    const { record, stderr, directory } = await runCompactionCase(t, {
      fixture: (tmp) => {
        const directory = path.join(tmp, "project");
        return {
          location: { directory },
          session: {
            // A non-summary assistant message with an agent: the restore read
            // resolves an agent and then stops at the missing prompt API.
            context: [{ id: "msg-1", type: "assistant", agent: "builder", content: [] }],
            get: { "ses-A": { location: { directory } } },
          },
          compactions: [{ sessionID: "ses-A" }],
          // The envelope carries the directory, so the locator decides without
          // asking the host; `data.sessionID` becomes `properties.sessionID`.
          events: [
            { type: "session.compaction.ended", location: { directory }, data: { sessionID: "ses-A" } },
          ],
          rpc: { registered: true },
        };
      },
    });

    assert.deepEqual(record.compactionErrors, []);
    assertSystemCarriesTheIngest(record.systems[0].system);
    assert.equal(injectsOf(record.injects, "ingest").length, 1, "the ingest half ran");

    const restore = injectsOf(record.injects, "restore");
    assert.equal(restore.length, 1, "the inherited restore read still reaches the engine");
    assert.equal(restore[0].session_id, "ses-A");
    assert.deepEqual(record.contextCalls, ["ses-A"], "agent resolution read the session transcript once");

    // The no-send pin: the spies are installed, so zero is an observation.
    assert.equal(record.promptCalls.length, 0, "no session.prompt under V2");
    assert.equal(record.syntheticCalls.length, 0, "no session.synthetic under V2");
    assert.deepEqual(record.toastEmits, [], "no send path, therefore no restore toast");

    const skipped = restoreSkippedLines(stderr);
    assert.equal(skipped.length, 1, `exactly one compaction.restore_skipped line (stderr=${stderr})`);
    assert.deepEqual(
      Object.keys(skipped[0]).sort(),
      ["component", "event", "level", "reason"],
      "the skip line carries fixed fields only (no session id, path, render or error text)"
    );
    assert.equal(skipped[0].reason, "session.prompt unavailable");
  }
);

test(
  "test_v2_compaction_hook_does_not_apply_the_child_gate",
  { timeout: 30000 },
  async (t) => {
    const { record } = await runCompactionCase(t, {
      fixture: (tmp) => {
        const directory = path.join(tmp, "project");
        return {
          location: { directory },
          session: {
            context: [],
            // Child-shaped: a `session.get` the host could answer, which the
            // legacy client deliberately cannot reach.
            get: { "ses-A": { parentID: "ses-parent", location: { directory } } },
          },
          compactions: [{ sessionID: "ses-A" }],
          events: [],
          rpc: { registered: true },
        };
      },
    });

    assert.deepEqual(
      record.sessionGetCalls,
      [],
      "the legacy client exposes no session.get, so the child gate cannot fire"
    );
    assert.deepEqual(record.compactionErrors, []);
    assertSystemCarriesTheIngest(record.systems[0].system);
    assert.equal(injectsOf(record.injects, "ingest").length, 1, "the ingest proceeds on a child-shaped session");
  }
);

test(
  "test_v2_compaction_hook_settles_without_the_v1_hook_or_a_system_array",
  { timeout: 5000 },
  async () => {
    const { registerV2Adapter } = await import(ADAPTER_MODULE);

    async function invoke(name, legacyHooks, event) {
      const harness = createHarness();
      const cleanup = await registerV2Adapter(harness.ctx, legacyHooks);
      const hook = harness.hooks.get("compaction");
      assert.equal(typeof hook, "function", `${name}: the adapter registers the compaction hook`);
      const outcome = { name, rejected: false, error: null, system: null };
      try {
        await hook(event);
      } catch (error) {
        outcome.rejected = true;
        outcome.error = String(error);
      }
      outcome.system = event.system ?? [];
      await cleanup();
      return outcome;
    }

    const results = [];
    // 1. No V1 handler at all: the copy loop must not call `undefined`.
    results.push(await invoke("no-v1-hook", {}, { sessionID: "ses-A", system: [] }));
    // 2. A handler that throws: the adapter must contain it, not surface it.
    results.push(
      await invoke(
        "throwing-handler",
        {
          "experimental.session.compacting": async () => {
            throw new Error("engine round-trip failed");
          },
        },
        { sessionID: "ses-A", system: [] }
      )
    );
    // 3. No `system` array on the input: the copy loop must not dereference it.
    results.push(
      await invoke(
        "no-system",
        {
          "experimental.session.compacting": async (_input, output) => {
            output.context.push("RENDER");
          },
        },
        { sessionID: "ses-A" }
      )
    );

    // Recording, not rethrowing: one run shows all three reasons instead of
    // aborting on the first. The host bridges the callback with
    // `Effect.promise`, so a rejection is a defect that aborts its compaction.
    assert.deepEqual(
      results.map(({ name, rejected, error }) => ({ name, rejected, error })),
      [
        { name: "no-v1-hook", rejected: false, error: null },
        { name: "throwing-handler", rejected: false, error: null },
        { name: "no-system", rejected: false, error: null },
      ],
      "the compaction callback settles on all three shapes"
    );
    assert.deepEqual(
      results.map(({ system }) => system),
      [[], [], []],
      "no invocation appends to a system array it cannot reach"
    );
  }
);
