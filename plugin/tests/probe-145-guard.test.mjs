// Probe guard cases (issue 145, Step 8b). The probe under test defaults to
// tests/harness/probe-145.mjs; PROBE_PATH overrides it for the mutant runs.
import { test, after } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { resolve } from "node:path";
import { runDriver } from "./harness/bridge.mjs";

const probePath = resolve(process.env.PROBE_PATH ?? "tests/harness/probe-145.mjs");
// Module-anchored scratch: here is plugin/tests; two `..` reach the repo root.
const here = path.dirname(fileURLToPath(import.meta.url));
const scratchRoot = path.resolve(here, "..", "..", ".tmp");
const made = [];
after(() => { for (const d of made) rmSync(d, { recursive: true, force: true }); });

function fixture() {
  mkdirSync(scratchRoot, { recursive: true });
  const tmp = mkdtempSync(path.join(scratchRoot, "om-probe-guard-"));
  made.push(tmp);
  return tmp;
}

function baseEnv(home) {
  // Explicit env: no spread from process.env, so DRIVER_STORAGE_ROOT is absent
  // unless a case sets it.
  return { PATH: process.env.PATH, HOME: home, OPENCODE_MEM_WEB_PORT: "3750" };
}

test("test_probe_refuses_without_seed_exit_line", async () => {
  const tmp = fixture();
  const home = path.join(tmp, "home");
  mkdirSync(home, { recursive: true });
  const ledger = path.join(tmp, "ledger.md");
  writeFileSync(ledger, "- seed engine spawned pid 1 2026-11-18T00:00:00Z\n");
  const root = path.join(tmp, "root");
  mkdirSync(root);
  const sentinel = path.join(tmp, "sentinel");
  const env = { ...baseEnv(home), PROBE_LEDGER: ledger, DRIVER_STORAGE_ROOT: root, PROBE_SENTINEL: sentinel };
  const r = await runDriver(probePath, [], env);
  assert.equal(r.code, 1, `code=${r.code} stderr=${r.stderr}`);
  assert.match(r.stderr, /no seed exit line/);
  assert.equal(existsSync(sentinel), false);
  assert.equal(readdirSync(root).length, 0);
});

test("test_probe_refuses_without_driver_storage_root", async () => {
  const tmp = fixture();
  const home = path.join(tmp, "home");
  mkdirSync(home, { recursive: true });
  const ledger = path.join(tmp, "ledger.md");
  writeFileSync(ledger, "exit 1 2026-11-18T00:00:00Z\n");
  const sentinel = path.join(tmp, "sentinel");
  const env = { ...baseEnv(home), PROBE_LEDGER: ledger, PROBE_SENTINEL: sentinel };
  const r = await runDriver(probePath, [], env);
  assert.equal(r.code, 2, `code=${r.code} stderr=${r.stderr}`);
  assert.match(r.stderr, /DRIVER_STORAGE_ROOT unset/);
  assert.equal(existsSync(sentinel), false);
});

test("test_probe_reports_unreadable_inbox_as_open", async () => {
  // Precondition: an absent sqlite3 must fail this case, never pass it.
  execFileSync("sqlite3", ["-version"], { stdio: "ignore" });
  const tmp = fixture();
  const home = path.join(tmp, "home");
  mkdirSync(home, { recursive: true });
  const root = path.join(tmp, "root");
  mkdirSync(root);
  writeFileSync(path.join(root, "episodic.db"), "not a sqlite database\n");
  const ledger = path.join(tmp, "ledger.md");
  writeFileSync(ledger, "exit 1 2026-11-18T00:00:00Z\nSEED_NONCE=0123456789abcdef\n");
  const env = { ...baseEnv(home), PROBE_LEDGER: ledger, DRIVER_STORAGE_ROOT: root };
  const r = await runDriver(probePath, [], env);
  assert.equal(r.code, 8, `code=${r.code} stderr=${r.stderr}`);
  assert.match(r.stdout, /^OPEN — inbox state unreadable: .*not a database/m);
  assert.match(readFileSync(ledger, "utf8"), /probe OPEN — inbox state unreadable: .*not a database/);
});
