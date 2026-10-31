// Issue 82 — one shared engine bridge per host process.
//
// The live host evaluates the plugin once per instance/location, and today each
// evaluation owns its own module copy (the host cache-busts every local
// specifier per generation). The wrapper's mutable bridge state must therefore
// live in a process-global registry keyed by the resolved storage root: one
// engine child and one handshake serve every sibling, while the engine lock
// keeps its unchanged role as the cross-PROCESS guard.
//
// Every case drives a subprocess (`DRIVER_MODE=shared-bridge` in
// `tests/harness/bridge.mjs`): a `registerHooks` resolve hook rewrites every
// local specifier of a generation, so the two `import()`s below yield two
// whole module graphs in one realm — exactly the host's loader.
//
// Cases 3 and 4 are guards: 3 is green before and after the fix (two roots must
// stay two children — it catches over-sharing), and 4 is the status-file
// regression guard (RED before the registry, green by construction after).

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

/** Asserts `pid` is gone within 2 s (all spawned pids, not just the manifest). */
async function assertPidGone(pid) {
  const deadline = Date.now() + 2000;
  for (;;) {
    try {
      process.kill(pid, 0);
    } catch (error) {
      if (error.code === "ESRCH") return;
      throw error;
    }
    if (Date.now() > deadline) break;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  assert.throws(
    () => process.kill(pid, 0),
    /ESRCH/,
    `stub pid ${pid} must be gone within the 2 s bound`
  );
}

function degradedLines(stderr) {
  return stderr
    .split("\n")
    .filter((line) => line.includes('"event":"degraded"'));
}

/** Extracts the `--storage-root` value from a recorded stub argv. */
function storageRootArgument(argv) {
  const index = argv.indexOf("--storage-root");
  return index >= 0 ? argv[index + 1] : null;
}

/**
 * Stages a temp package copy, optionally stages a stub engine, writes the
 * fixture + driver, runs the driver, and reaps every recorded pid in `t.after`.
 */
async function runCase(t, {
  fixture,
  engine = true,
  engineOptions = () => ({}),
  timeoutMs = 30000,
}) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-shared-bridge-"));
  const pkgDir = stagePackageTree(tmp);
  const root = path.join(tmp, "storage");
  fs.mkdirSync(root, { recursive: true });
  const secondRoot = path.join(tmp, "storage-2");
  fs.mkdirSync(secondRoot, { recursive: true });
  const requestLogPath = path.join(tmp, "stub-requests.ndjson");
  const manifestPath = path.join(tmp, "stub-manifest.json");
  if (engine) {
    const options = engineOptions(root, secondRoot);
    stageStubEngine(pkgDir, {
      ...options,
      requestLogPath,
      manifestPath,
      // The plan's shape: the shared-bridge driver polls the request log for
      // spawned/exited, so this case pins the lifecycle stream to it.
      lifecycleLogPath: requestLogPath,
      // The shared-bridge cases drive the agent-facing `memory` tool, so the
      // stub must advertise its method (every other existing case keeps the
      // harness default and never calls it).
      capabilities: {
        ...DEFAULT_STUB_CAPABILITIES,
        "list-shards": true,
        ...(options.capabilities ?? {}),
      },
    });
  }
  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, DRIVER_SOURCE);
  const donePath = path.join(tmp, "done.json");
  const location = path.join(tmp, "workspace");
  fs.mkdirSync(location, { recursive: true });
  const fixturePath = path.join(tmp, "fixture.json");
  fs.writeFileSync(fixturePath, JSON.stringify({ location, secondRoot, ...fixture }));

  const result = await runDriver(
    driverPath,
    [pkgDir, fixturePath],
    {
      ...process.env,
      DRIVER_MODE: "shared-bridge",
      DRIVER_STORAGE_ROOT: root,
      STUB_LOG: requestLogPath,
      DRIVER_DONE_PATH: donePath,
    },
    timeoutMs
  );

  let record = null;
  if (fs.existsSync(donePath)) {
    record = JSON.parse(fs.readFileSync(donePath, "utf8"));
  }
  const pids = record === null ? [] : (record.spawned ?? []).map((entry) => entry.pid);
  t.after(async () => {
    for (const pid of pids) await assertPidGone(pid);
    if (fs.existsSync(manifestPath)) {
      await assertReaped(manifestPath);
    }
  });

  return { tmp, pkgDir, root, secondRoot, requestLogPath, manifestPath, donePath, result, record };
}

test(
  "test_two_imports_in_one_process_spawn_one_engine_and_both_answer",
  { timeout: 60000 },
  async (t) => {
    const { root, result, record } = await runCase(t, {
      fixture: { variant: "share", concurrent: true },
    });

    assert.ok(record !== null, `driver wrote no record (stderr=${result.stderr})`);
    assert.equal(result.signal, "SIGTERM", `driver must end via its SIGTERM path (code=${result.code})`);
    assert.equal(record.helloCount, 1, `helloCount ${record.helloCount} !== 1`);
    assert.equal(
      record.spawned.length,
      1,
      `spawnCount ${record.spawned.length} !== 1`
    );
    assert.equal(record.answers.length, 2, "both generations must answer the memory tool");
    assert.equal(
      record.answers[0].success,
      true,
      `generation 1 must succeed: ${JSON.stringify(record.answers[0])}`
    );
    assert.equal(
      record.answers[1].success,
      true,
      `generation 2 must succeed: ${JSON.stringify(record.answers[1])}`
    );
    assert.equal(
      storageRootArgument(record.spawned[0].argv),
      path.resolve(root),
      "the one child must carry the shared root"
    );
  }
);

test(
  "test_v2_two_generations_share_the_same_engine_child",
  { timeout: 60000 },
  async (t) => {
    const { root, result, record } = await runCase(t, {
      fixture: { variant: "v2", concurrent: false },
    });

    assert.ok(record !== null, `driver wrote no record (stderr=${result.stderr})`);
    assert.equal(result.signal, "SIGTERM", `driver must end via its SIGTERM path (code=${result.code})`);
    // The per-generation v2 graph is the load-bearing part: if the resolve hook
    // leaked, a phantom third plugin.js would handshake too (helloCount 3).
    assert.equal(record.helloCount, 1, `helloCount ${record.helloCount} !== 1`);
    assert.equal(record.spawned.length, 1, `spawnCount ${record.spawned.length} !== 1`);
    assert.equal(
      record.answers[1].success,
      true,
      `the second generation's v2 tool must answer: ${JSON.stringify(record.answers[1])}`
    );
    assert.equal(
      storageRootArgument(record.spawned[0].argv),
      path.resolve(root),
      "the one child must carry the shared root"
    );
  }
);

test(
  "test_two_storage_roots_in_one_process_get_two_children",
  { timeout: 60000 },
  async (t) => {
    // Sharing guard, green before and after the fix: only over-sharing (one
    // global child regardless of root) can break it.
    const { root, secondRoot, result, record } = await runCase(t, {
      fixture: { variant: "roots", concurrent: false },
    });

    assert.ok(record !== null, `driver wrote no record (stderr=${result.stderr})`);
    assert.equal(result.signal, "SIGTERM", `driver must end via its SIGTERM path (code=${result.code})`);
    assert.equal(record.spawned.length, 2, `spawnCount ${record.spawned.length} !== 2`);
    const roots = record.spawned.map((entry) => storageRootArgument(entry.argv));
    assert.deepEqual(
      [...roots].sort(),
      [path.resolve(root), path.resolve(secondRoot)].sort(),
      "each child must carry its own --storage-root"
    );
    for (const answer of record.answers) {
      assert.equal(
        answer.success,
        true,
        `both roots must answer: ${JSON.stringify(answer)}`
      );
    }
  }
);

test(
  "test_sibling_generations_write_the_status_file_at_most_once",
  { timeout: 60000 },
  async (t) => {
    const { root, result, record } = await runCase(t, {
      fixture: { variant: "degrade", concurrent: false },
      engine: false,
    });

    assert.ok(record !== null, `driver wrote no record (stderr=${result.stderr})`);
    assert.equal(result.signal, "SIGTERM", `driver must end via its SIGTERM path (code=${result.code})`);
    const lines = degradedLines(result.stderr);
    assert.equal(
      lines.length,
      1,
      `degraded lines ${lines.length} !== 1`
    );
    for (const answer of record.answers) {
      assert.equal(
        answer.success,
        false,
        `a memoryless tool must fail open: ${JSON.stringify(answer)}`
      );
      assert.equal(answer.error, "engine_unavailable");
    }
    const statusPath = path.join(root, "status");
    assert.ok(fs.existsSync(statusPath), "the one degrade must write the status file");
    const status = fs.readFileSync(statusPath, "utf8");
    assert.match(status, /^degraded: /, "the status keeps the one-line grammar");
    assert.ok(
      status.includes("pid=" + record.pid),
      `status must name the wrapper process: ${status.trim()}`
    );
    assert.ok(
      status.trimEnd().split("\n").length === 1,
      "the status file holds exactly one line"
    );
  }
);

test(
  "test_a_genuine_engine_lock_loser_keeps_the_holder_pid_in_status",
  { timeout: 60000 },
  async (t) => {
    const holderPid = 424242;
    const sentinelTs = "2026-10-31T00:00:00.000Z";
    const { root, result, record } = await runCase(t, {
      fixture: { variant: "share", concurrent: false },
      engineOptions: (caseRoot) => ({
        loserStatus: {
          path: path.join(caseRoot, "status"),
          holderPid,
          ts: sentinelTs,
          // A suppression fixed at ~250 ms would overwrite the holder pid while
          // the child is still alive; the decision must read the recorded exit.
          exitDelayMs: 900,
        },
      }),
    });

    assert.ok(record !== null, `driver wrote no record (stderr=${result.stderr})`);
    assert.equal(result.signal, "SIGTERM", `driver must end via its SIGTERM path (code=${result.code})`);
    assert.equal(
      record.answers[0].error,
      "engine_unavailable",
      `a lock loser leaves the bridge unavailable: ${JSON.stringify(record.answers[0])}`
    );
    const status = fs.readFileSync(path.join(root, "status"), "utf8");
    assert.ok(
      status.includes("pid=" + holderPid),
      `the holder pid must survive the slow exit: ${status.trim()}`
    );
    assert.ok(
      status.includes("ts=" + sentinelTs),
      `the engine's own sentinel line must survive: ${status.trim()}`
    );
    assert.ok(
      !status.includes("pid=" + record.pid),
      `the wrapper must not overwrite the holder line with its own pid: ${status.trim()}`
    );
  }
);

test(
  "test_a_dead_child_restarts_once_then_degrades_once",
  { timeout: 60000 },
  async (t) => {
    const { result, record } = await runCase(t, {
      fixture: { variant: "dead-child", concurrent: false },
      engineOptions: () => ({
        diesAfterRequests: 1,
        capabilities: {
          ...DEFAULT_STUB_CAPABILITIES,
          // Keep setup from spending the child's one non-hello answer on the
          // skill catalogue: the three list-shards calls must be the ones that
          // kill it.
          skills: false,
          "list-shards": true,
        },
      }),
    });

    assert.ok(record !== null, `driver wrote no record (stderr=${result.stderr})`);
    assert.equal(result.signal, "SIGTERM", `driver must end via its SIGTERM path (code=${result.code})`);
    assert.equal(record.spawned.length, 2, `spawnCount ${record.spawned.length} !== 2`);
    assert.equal(
      record.answers[0].success,
      true,
      `call 1 must answer: ${JSON.stringify(record.answers[0])}`
    );
    assert.equal(
      record.answers[1].success,
      true,
      `call 2 must answer from the replacement child: ${JSON.stringify(record.answers[1])}`
    );
    assert.equal(record.answers[2].success, false);
    assert.equal(
      record.answers[2].error,
      "engine_unavailable",
      `call 3 must degrade: ${JSON.stringify(record.answers[2])}`
    );
    assert.equal(record.exited.length, 2, `exited lines ${record.exited.length} !== 2`);
    const lines = degradedLines(result.stderr);
    assert.equal(lines.length, 1, `degraded lines ${lines.length} !== 1`);
  }
);
