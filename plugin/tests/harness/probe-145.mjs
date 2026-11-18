// Issue 145 Step 8 probe: scratch-root block delivery through the V2 entry.
// Run DIRECTLY (not via runDriver): cd plugin && env -i PATH=... HOME=<repo .tmp/home-145>
//   DRIVER_STORAGE_ROOT=<repo .tmp/root-145> OPENCODE_MEM_WEB_PORT=<web-port>
//   PROBE_LEDGER=<ledger> node tests/harness/probe-145.mjs
// Engine argv: copied from engineArguments() (plugin/src/plugin.js:158):
//   --storage-root <root> --web-port <port>; the wrapper spawns the engine via
//   startBridge (plugin/src/plugin.js:898). Source line: plugin/src/plugin.js:158.
// Probe order is fixed: reap guard, root guard, sentinel, engine steering,
// seed COUNT, then the child loop. Never reads OPENCODE_MEM_STORAGE_ROOT from env.
import { appendFileSync, existsSync, readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import path, { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const utc = () => new Date().toISOString().replace(/\.\d+Z$/, "Z");
// Module-anchored: here is plugin/tests/harness; three `..` reach the repo root.
const here = path.dirname(fileURLToPath(import.meta.url));
const REPO_TMP = resolve(here, "..", "..", "..", ".tmp");

// REAP-GUARD-BEGIN
{
  const ledgerPath = process.env.PROBE_LEDGER;
  const text = ledgerPath && existsSync(ledgerPath) ? readFileSync(ledgerPath, "utf8") : "";
  if (!/^exit [0-9]+ /m.test(text)) {
    process.stderr.write("probe refused: no seed exit line in PROBE_LEDGER\n");
    process.exit(1);
  }
}
// REAP-GUARD-END

// ROOT-GUARD-BEGIN
if (!process.env.DRIVER_STORAGE_ROOT) {
  process.stderr.write("probe refused: DRIVER_STORAGE_ROOT unset\n");
  process.exit(2);
}
// ROOT-GUARD-END

if (process.env.PROBE_SENTINEL) writeFileSync(process.env.PROBE_SENTINEL, "guards passed " + utc() + "\n"); // SENTINEL-LINE

// Steer the engine (mirrors bridge.mjs:410).
process.env.OPENCODE_MEM_STORAGE_ROOT = process.env.DRIVER_STORAGE_ROOT;

const LEDGER = process.env.PROBE_LEDGER;
const ledger = (line) => appendFileSync(LEDGER, line + "\n");
const DB = resolve(process.env.DRIVER_STORAGE_ROOT, "episodic.db");
// The nonce lives in the ledger only (Step 0c item 1); the env is exactly the
// Step 8 keys, so the nonce is read from the ledger's seed line.
const nonceMatch = readFileSync(LEDGER, "utf8").match(/SEED_NONCE=([0-9a-f]{16})/);
if (!nonceMatch) {
  process.stderr.write("probe refused: no SEED_NONCE in PROBE_LEDGER\n");
  process.exit(3);
}
const SEED_NONCE = nonceMatch[1];

// Seed row, scratch only (Step 0c item 5 expects COUNT 1).
if (!existsSync(DB)) {
  ledger(`- probe OPEN — seed row not persisted: ${DB} missing ${utc()}`);
  process.stdout.write("OPEN — seed row not persisted\n");
  process.exit(4);
}
const SES_SEED_ID = "ses-seed-" + SEED_NONCE;
let countText = null;
let lastErr = "";
// ponytail: three immediate attempts, no backoff. Ceiling: lock contention longer than three back-to-back reads returns OPEN. Upgrade path: add `-cmd ".timeout 5000"` to the sqlite3 argv.
for (let attempt = 1; attempt <= 3 && countText === null; attempt++) {
  try {
    countText = execFileSync("sqlite3", ["-readonly", DB,
      `SELECT COUNT(*) FROM agent_sessions WHERE session_id='${SES_SEED_ID}' AND project_id='/work/proj' AND deleted_at IS NULL;`],
      { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
  } catch (e) {
    lastErr = String(e.stderr ?? "").trim() || String(e.message).split("\n")[0];
  }
}
if (countText === null) {
  ledger(`- probe OPEN — inbox state unreadable: ${lastErr} ${utc()}`);
  process.stdout.write(`OPEN — inbox state unreadable: ${lastErr}\n`);
  process.exit(8);
}
const count = countText;
ledger(`- probe seed COUNT=${count} rc=0 ${utc()} (Step 8 item 5, expect 1)`);
if (count !== "1") {
  process.stdout.write(`OPEN — seed row not persisted (COUNT ${count})\n`);
  process.exit(5);
}

const { setup } = await import(pathToFileURL(resolve("dist/v2/plugin.js")).href);
const hooks = new Map();
const ctx = {
  location: { directory: "/work/proj", project: null },
  session: { hook: async (name, cb) => hooks.set(name, cb) },
};
await setup(ctx);
const promptHook = hooks.get("prompt");
const contextHook = hooks.get("context");
if (typeof promptHook !== "function" || typeof contextHook !== "function") {
  ledger(`- probe OPEN — v2 hooks not registered (prompt=${typeof promptHook} context=${typeof contextHook})`);
  process.exit(6);
}

let failures = 0;
for (const n of [1, 2, 3]) {
  const sid = "ses_probe_child_" + n;
  await promptHook({ sessionID: sid, messageID: "msg_probe_" + n, prompt: { text: "" } });
  const system = [];
  await contextHook({ sessionID: sid, system });
  const block = system.map((p) => p.text ?? "").join("\n");
  const blockPath = resolve(REPO_TMP, "progress", `probe-145-block-${n}.txt`);
  mkdirSync(resolve(REPO_TMP, "progress"), { recursive: true });
  writeFileSync(blockPath, block);
  const nonceHits = block.split(SEED_NONCE).length - 1;
  const sha = createHash("sha256").update(block).digest("hex");
  const heading = ["## Recent sessions", "## Project facts", "## Captured session summaries", "## Related older sessions"].find((h) => block.includes(h)) ?? "";
  const hasNonce = nonceHits >= 1;
  if (!hasNonce) failures++;
  if (!heading) failures++;
  ledger(`- probe child ${n}: nonce_count=${nonceHits} block_sha256=${sha} heading=${JSON.stringify(heading)} bytes=${Buffer.byteLength(block)} ${utc()}`);
}

const sinkLines = execFileSync("bash", ["-c", `grep -n 'host_contract\\|] inject\\.received:\\|] inject\\.requested:' "$HOME/.opencode-mem/opencode-mem.log" | tail -20 || true`], { encoding: "utf8" });
ledger(`- probe sink lines (verbatim):\n${sinkLines}`);
if (failures) {
  process.stdout.write(`OPEN — block lacks nonce or history heading (${failures} failures)\n`);
  process.exit(7);
}
process.stdout.write("PROBE OK\n");
process.exit(0);
