// Issue 50 (O10) — the wrapper's `include.query`.
//
// The wrapper reads the AUTHORED text of the message being sent and passes it
// as `include.query` so the engine can take the scored read; synthetic
// (injected) and tool parts never contribute. The cap is 2000 CODE POINTS, the
// same unit as the engine's `chars()` cap.
//
// The driver is local to this file because the shared harness driver fires a
// fixed `"hi"` part: each case here needs its own part list, and `bridge.mjs`
// is shared by five suites (no shared file is edited for a fixture need).

import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import {
  assertReaped,
  runDriver,
  stagePackageTree,
  stageStubEngine,
} from "./harness/bridge.mjs";

const DIRECTORY = "/work/proj";

const DRIVER_SOURCE = `import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const pkgDir = process.argv[2];
const fixture = JSON.parse(fs.readFileSync(process.argv[3], "utf8"));
process.env.OPENCODE_MEM_STORAGE_ROOT = process.env.DRIVER_STORAGE_ROOT;
const mod = await import(pathToFileURL(path.join(pkgDir, "dist", "plugin.js")));
const { hooks } = await mod.default.server({ client: {}, directory: fixture.directory });
for (const output of fixture.outputs ?? []) {
  await hooks["chat.message"]({ sessionID: "sess-OTHER" }, output);
}
fs.writeFileSync(process.env.DRIVER_DONE_PATH, "{}");
process.exit(0);
`;

async function runCase(t, fixture) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-inject-query-"));
  const pkgDir = stagePackageTree(tmp);
  const requestLogPath = path.join(tmp, "stub-requests.ndjson");
  const manifestPath = path.join(tmp, "stub-manifest.json");
  stageStubEngine(pkgDir, { requestLogPath, manifestPath, injectMode: "success" });
  t.after(() => assertReaped(manifestPath));

  const donePath = path.join(tmp, "driver-done.json");
  const fixturePath = path.join(tmp, "fixture.json");
  fs.writeFileSync(fixturePath, JSON.stringify(fixture));
  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, DRIVER_SOURCE);

  const result = await runDriver(driverPath, [pkgDir, fixturePath], {
    ...process.env,
    DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
    DRIVER_DONE_PATH: donePath,
    STUB_LOG: requestLogPath,
  });
  assert.equal(result.code, 0, `the inject-query driver failed: ${result.stderr}`);
  const text = fs.readFileSync(requestLogPath, "utf8");
  return text
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line) => JSON.parse(line).got)
    .filter((message) => message && message.method === "inject");
}

test("test_chat_message_sends_the_authored_text_as_the_query", { timeout: 30000 }, async (t) => {
  const wire = await runCase(t, {
    directory: DIRECTORY,
    outputs: [
      {
        message: {
          parts: [
            { type: "text", text: "fix the parser" },
            { type: "tool-invocation", toolName: "bash" },
            { type: "text", text: "IGNORE", synthetic: true },
          ],
        },
      },
    ],
  });

  assert.equal(wire.length, 1, "one inject request");
  assert.deepEqual(
    wire[0].params.include,
    { query: "fix the parser" },
    "the authored text is the query; synthetic and tool parts never contribute"
  );
  assert.equal(wire[0].params.session_id, "sess-OTHER", "session_id is unchanged");
  assert.equal(wire[0].params.project_id, DIRECTORY, "project_id is unchanged");
});

test("test_the_query_is_capped_at_two_thousand_code_points", { timeout: 30000 }, async (t) => {
  const rocket = "\u{1F680}";
  const wire = await runCase(t, {
    directory: DIRECTORY,
    outputs: [
      {
        message: {
          parts: [{ type: "text", text: rocket.repeat(3000) + "TAIL" }],
        },
      },
    ],
  });

  assert.equal(wire.length, 1, "one inject request");
  const query = wire[0].params.include.query;
  assert.equal(
    Array.from(query).length,
    2000,
    "the cap counts CODE POINTS, so 2000 astral characters survive (a UTF-16 slice keeps 1000)"
  );
  assert.equal(query, rocket.repeat(2000));
  assert.equal(query.includes("TAIL"), false, "the tail is cut");
});

test("test_no_authored_text_sends_no_query", { timeout: 30000 }, async (t) => {
  const wire = await runCase(t, {
    directory: DIRECTORY,
    outputs: [
      // (a) only a synthetic (already injected) part.
      { message: { parts: [{ type: "text", text: "IGNORE", synthetic: true }] } },
      // (b) an output with no parts at all.
      {},
    ],
  });

  assert.equal(wire.length, 2, "two inject requests");
  for (const message of wire) {
    assert.equal(
      message.params.include,
      undefined,
      "no authored text means no include, never `{query: \"\"}`"
    );
  }
});
