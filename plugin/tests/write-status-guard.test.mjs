// ADR-0033 / F23 — the wrapper's status writer must NEVER create the storage
// root. During a consolidation window custody owns that path exclusively; an
// unconditional `mkdirSync` here turns a wrapper respawn into an empty root
// that the install rename must then refuse (the L4→L5 wedge).
//
// Why a new file: every existing plugin case pre-creates both roots in
// `runCase` (`shared-bridge.test.mjs:78-81`, F34), so the unconditional-
// `mkdirSync` mutant ships GREEN there. This case drives the degrade path
// against a deliberately ABSENT root, and keeps a positive control proving
// the report is still written into an EXISTING root (the guard must gate
// creation, not writing).

import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { DRIVER_SOURCE, runDriver, stagePackageTree } from "./harness/bridge.mjs";

function uniqueTempDir(prefix) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `write-status-guard-${prefix}-`));
}

function degradedLines(stderr) {
  return stderr
    .split("\n")
    .filter((line) => line.includes('"event":"degraded"'));
}

// The shared-bridge driver needs a fixture root that stays ABSENT: with no
// stub engine staged, every tool call degrades, which is the branch whose
// writer this case fences.
async function runDegradeAgainstRoot(root) {
  const tmp = uniqueTempDir("case");
  const pkgDir = stagePackageTree(tmp);
  const location = path.join(tmp, "workspace");
  fs.mkdirSync(location, { recursive: true });
  const requestLogPath = path.join(tmp, "wire.log");
  const donePath = path.join(tmp, "done.json");
  const fixturePath = path.join(tmp, "fixture.json");
  fs.writeFileSync(
    fixturePath,
    JSON.stringify({
      location,
      secondRoot: path.join(tmp, "storage-2"),
      variant: "degrade",
      concurrent: false,
    })
  );
  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, DRIVER_SOURCE);
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
    30000
  );
  const record = fs.existsSync(donePath)
    ? JSON.parse(fs.readFileSync(donePath, "utf8"))
    : null;
  return { result, record };
}

test("test_write_status_file_never_creates_missing_root", { timeout: 60000 }, async () => {
  // The exact F23(b) L4→L5 wedge input: the storage root does not exist.
  const absentRoot = path.join(uniqueTempDir("absent"), "storage");
  const { result, record } = await runDegradeAgainstRoot(absentRoot);

  assert.ok(record !== null, `driver wrote no record (stderr=${result.stderr})`);
  assert.equal(
    result.signal,
    "SIGTERM",
    `driver must end via its SIGTERM path (code=${result.code}, stderr=${result.stderr})`
  );
  // Non-vacuous oracle: the degrade path must have RUN, or "no file" proves
  // nothing about the writer.
  assert.equal(
    degradedLines(result.stderr).length,
    1,
    `exactly one degrade must fire (stderr=${result.stderr})`
  );
  assert.equal(
    record.answers[0].success,
    false,
    `the degraded bridge must fail open: ${JSON.stringify(record.answers[0])}`
  );
  assert.equal(record.answers[0].error, "engine_unavailable");
  // The mutant (unconditional mkdirSync) creates the root and the status
  // file here; the guard leaves both absent.
  assert.equal(fs.existsSync(absentRoot), false, "the wrapper must never create the storage root");
  assert.equal(
    fs.existsSync(path.join(absentRoot, "status")),
    false,
    "no status file may appear under a root the wrapper must not create"
  );

  // Positive control: an EXISTING root still receives the report — the fix
  // gates creation, not the writer.
  const existingRoot = path.join(uniqueTempDir("existing"), "storage");
  fs.mkdirSync(existingRoot, { recursive: true });
  const control = await runDegradeAgainstRoot(existingRoot);
  assert.ok(control.record !== null, `control driver wrote no record (stderr=${control.result.stderr})`);
  assert.equal(
    degradedLines(control.result.stderr).length,
    1,
    `the control degrade must fire (stderr=${control.result.stderr})`
  );
  const statusPath = path.join(existingRoot, "status");
  assert.ok(fs.existsSync(statusPath), "an existing root must still receive the status report");
  const status = fs.readFileSync(statusPath, "utf8");
  assert.match(status, /^degraded: /, "the status keeps the one-line grammar");
  assert.ok(
    status.trimEnd().split("\n").length === 1,
    "the status file holds exactly one line"
  );
});
