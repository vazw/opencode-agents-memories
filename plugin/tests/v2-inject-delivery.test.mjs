import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { assertReaped, runDriver, stagePackageTree, stageStubEngine } from "./harness/bridge.mjs";

const DRIVER = `import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
process.env.OPENCODE_MEM_STORAGE_ROOT = process.env.DRIVER_STORAGE_ROOT;
const { server } = await import(pathToFileURL(path.join(process.argv[2], "dist/plugin.js")));
const { hooks } = await server({ client: {}, directory: "/work/project" });
const decisions = [];
const turn = () => hooks["chat.message"]({ sessionID: "ses-1" }, { message: { parts: [] } });
await turn(); // starts the child and runs hello
for (const turnIndex of [0, 1]) {
  decisions.push(await hooks.shouldRefresh({ turnIndex, hasCache: true }));
}
// Kill only fixture child. Replacement advertises no policy and must forget first.
fs.writeFileSync(process.env.HELLO_CONFIG, "{}");
const { pid } = JSON.parse(fs.readFileSync(process.env.MANIFEST, "utf8"));
process.kill(pid, "SIGTERM");
await new Promise(resolve => setTimeout(resolve, 100));
await turn(); // respawns: the replacement's hello advertises nothing
decisions.push(await hooks.shouldRefresh({ turnIndex: 2, hasCache: true }));
fs.writeFileSync(process.env.RESULT, JSON.stringify(decisions));
process.exit(0);
`;

test("test_hello_advertises_inject_on_outside_capabilities", { timeout: 30000 }, async (t) => {
  for (const inject_on of ["first", "always", undefined, "invalid"]) {
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-inject-delivery-"));
    const pkgDir = stagePackageTree(tmp);
    const manifestPath = path.join(tmp, "manifest.json");
    const helloConfigPath = path.join(tmp, "hello.json");
    const resultPath = path.join(tmp, "result.json");
    fs.writeFileSync(helloConfigPath, JSON.stringify({ inject_on }));
    stageStubEngine(pkgDir, {
      requestLogPath: path.join(tmp, "requests.ndjson"), manifestPath,
      helloConfigPath, injectMode: "success",
      // Wrong location must never influence the policy.
      capabilities: { inject_on: "first" },
    });
    t.after(() => assertReaped(manifestPath));
    const driverPath = path.join(tmp, "driver.mjs");
    fs.writeFileSync(driverPath, DRIVER);
    const run = await runDriver(driverPath, [pkgDir], {
      ...process.env, DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
      HELLO_CONFIG: helloConfigPath, MANIFEST: manifestPath, RESULT: resultPath,
    });
    assert.equal(run.code, 0, run.stderr);
    assert.deepEqual(JSON.parse(fs.readFileSync(resultPath, "utf8")),
      [true, inject_on !== "first", true], String(inject_on));
  }
});
