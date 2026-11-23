// Issue 154 wrapper wave — contract for the shared temp-root helper.
//
// `tempRoot(prefix)` (exported from `tests/harness/bridge.mjs`) replaces the
// raw `fs.mkdtempSync(path.join(os.tmpdir(), …))` create at every test site:
// everything lands under one per-process run root `om-run-<pid>-XXXXXX`, removed
// by a single `process.on("exit")` listener. These two tests are the helper's
// only tracked guard; the rollout itself is measured by a private-`TMPDIR`
// probe (blueprint Step 6), never by a count in this file.
//
// The harness import is DYNAMIC and module-scope so this file still links and
// runs while `tempRoot` does not exist: RED is a named assertion failure, not an
// ESM "does not provide an export named 'tempRoot'" load error.

import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const harness = await import("./harness/bridge.mjs").catch(() => ({}));
const tempRoot = harness.tempRoot; // undefined until the helper lands
const here = path.dirname(fileURLToPath(import.meta.url));

test("test_temp_root_shares_one_pid_named_run_root", () => {
  assert.equal(typeof tempRoot, "function");
  const a = tempRoot("om-hygiene-a-");
  const b = tempRoot("om-hygiene-b-");
  assert.equal(typeof a, "string");
  assert.equal(typeof b, "string");
  assert.ok(path.isAbsolute(a), `not absolute: ${a}`);
  assert.ok(path.isAbsolute(b), `not absolute: ${b}`);
  assert.equal(fs.existsSync(a), true, `missing: ${a}`);
  assert.equal(fs.existsSync(b), true, `missing: ${b}`);
  const parent = path.dirname(a);
  assert.equal(path.dirname(b), parent, "the two roots must share one parent");
  assert.match(
    path.basename(parent),
    new RegExp(`^om-run-${process.pid}-[A-Za-z0-9]+$`),
    `parent is not the pid-named run root: ${parent}`,
  );
  assert.notEqual(a, b, "two calls must return distinct roots");
});

// Child creates one helper root and one RAW control root, then prints both.
// The exit listener removes the helper; the control must remain, proving the
// child actually ran its creation code (a child that died early would leave
// neither, so the control's survival is the liveness check).
const CHILD = `
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
const { tempRoot } = await import(process.argv[1]);
const helper = tempRoot("om-hygiene-");
const control = fs.mkdtempSync(path.join(os.tmpdir(), "om-hygiene-control-"));
process.stdout.write(JSON.stringify({ helper, control }));
`;

test("test_temp_root_is_removed_when_the_process_exits", async () => {
  assert.equal(typeof tempRoot, "function");
  const bridgeUrl = pathToFileURL(path.resolve(here, "harness", "bridge.mjs")).href;
  const result = await new Promise((resolve, reject) => {
    const child = spawn(
      process.execPath,
      ["--input-type=module", "-e", CHILD, bridgeUrl],
      { stdio: ["ignore", "pipe", "pipe"] },
    );
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.on("error", reject);
    child.on("close", (code) => resolve({ code, stdout, stderr }));
  });
  assert.equal(result.code, 0, `child failed: ${result.stderr}`);
  const { helper, control } = JSON.parse(result.stdout);
  assert.equal(fs.existsSync(helper), false, `exit hook did not remove ${helper}`);
  assert.equal(fs.existsSync(control), true, `control must remain: ${control}`);
  fs.rmSync(control, { recursive: true, force: true });
});
