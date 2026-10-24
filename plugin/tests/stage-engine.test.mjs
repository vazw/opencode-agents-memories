// W6.9 (issue 36) — hermetic contract for plugin/scripts/stage-engine.sh.
//
// The script must stage the built web UI bundle (web-ui/dist) beside the engine
// binary at the packaged candidate (`<out>/web-ui/dist`, the engine's
// exe-relative default), refuse when no built bundle exists (exit 5), and refuse
// to replace an already-staged bundle (exit 7 — bundles are replaced, never
// merged; the operator removes the stale one, the script deletes nothing).
//
// The harness COPIES the repo script into a scratch tree and runs the copy: the
// repo script would mutate plugin/engine/ while another shard integrates. A fake
// cargo/rustc pair on PATH keeps the harness free of a real release build.

import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "..", "..");
const scriptSource = path.join(repoRoot, "plugin", "scripts", "stage-engine.sh");

// Derived exactly as the script derives them (its own uname case arms).
const PLATFORM = process.platform === "darwin" ? "darwin" : "linux";
const ARCH = process.arch === "arm64" ? "arm64" : "x64";
const DEST_REL = path.join("engine", `${PLATFORM}-${ARCH}`);

const INDEX_HTML = "<!doctype html>\n<title>stage-engine fixture</title>\n";
const HASHED_ASSET = "// web-ui-abc.js\n";

/* ---------------------------------------------------------------- helpers */

function makeScratch() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "stage-engine-"));
}

// Fake cargo builds the release binary the script expects; fake rustc reports
// the host triple the script reads on its default (non --target) path.
function writeFakeToolchain(tmp) {
  const bin = path.join(tmp, "bin");
  fs.mkdirSync(bin, { recursive: true });

  const cargo = path.join(bin, "cargo");
  fs.writeFileSync(
    cargo,
    `#!/usr/bin/env bash
set -euo pipefail
build_dir="\${CARGO_TARGET_DIR:-target}"
mkdir -p "$build_dir/release"
printf '#!/usr/bin/env bash\\nexit 0\\n' > "$build_dir/release/opencode-agents-memory"
chmod +x "$build_dir/release/opencode-agents-memory"
`,
    { mode: 0o755 }
  );
  fs.chmodSync(cargo, 0o755);

  const host = `${process.arch === "arm64" ? "aarch64" : "x86_64"}-${
    PLATFORM === "darwin" ? "apple-darwin" : "unknown-linux-gnu"
  }`;
  const rustc = path.join(bin, "rustc");
  fs.writeFileSync(
    rustc,
    `#!/usr/bin/env bash
cat <<'EOF'
rustc 1.90.0 (1159e78c4 2025-09-14)
binary: rustc
commit-hash: 1159e78c4747b9a91e2a001665de898569549f5a
commit-date: 2025-09-14
host: ${host}
release: 1.90.0
LLVM version: 20.1.8
EOF
`,
    { mode: 0o755 }
  );
  fs.chmodSync(rustc, 0o755);
}

// A scratch tree shaped like the package: <tmp>/plugin/scripts/stage-engine.sh
// (a COPY), <tmp>/web-ui/dist (when `webUi`), <tmp>/bin/{cargo,rustc},
// <tmp>/cargo-target.
function stageTree({ webUi = true } = {}) {
  const tmp = makeScratch();
  const scriptDir = path.join(tmp, "plugin", "scripts");
  fs.mkdirSync(scriptDir, { recursive: true });
  const script = path.join(scriptDir, "stage-engine.sh");
  fs.copyFileSync(scriptSource, script);
  writeFakeToolchain(tmp);
  fs.mkdirSync(path.join(tmp, "cargo-target"), { recursive: true });
  if (webUi) {
    const dist = path.join(tmp, "web-ui", "dist");
    fs.mkdirSync(dist, { recursive: true });
    fs.writeFileSync(path.join(dist, "index.html"), INDEX_HTML);
    fs.writeFileSync(path.join(dist, "web-ui-abc.js"), HASHED_ASSET);
  }
  return { tmp, script };
}

function runStage(tmp, script) {
  return spawnSync("bash", [script], {
    cwd: tmp,
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${path.join(tmp, "bin")}:${process.env.PATH}`,
      CARGO_TARGET_DIR: path.join(tmp, "cargo-target"),
      TMPDIR: tmp,
    },
  });
}

/* ------------------------------------------------------------ tests */

test("test_stage_engine_copies_web_ui_bundle_beside_binary", () => {
  const { tmp, script } = stageTree();
  const result = runStage(tmp, script);
  assert.equal(result.status, 0, `stage script failed (exit ${result.status}): ${result.stderr}`);

  const dest = path.join(tmp, "plugin", DEST_REL);
  const binary = path.join(dest, "opencode-agents-memory");
  assert.ok(fs.existsSync(binary), `staged binary missing at ${binary}`);
  fs.accessSync(binary, fs.constants.X_OK);

  const stagedIndex = path.join(dest, "web-ui", "dist", "index.html");
  assert.ok(fs.existsSync(stagedIndex), `staged bundle missing index.html at ${stagedIndex}`);
  assert.equal(
    fs.readFileSync(stagedIndex, "utf8"),
    INDEX_HTML,
    "staged index.html bytes must equal the source bundle"
  );

  assert.deepEqual(
    fs.readdirSync(dest, { recursive: true }).sort(),
    [
      "opencode-agents-memory",
      "web-ui",
      "web-ui/dist",
      "web-ui/dist/index.html",
      "web-ui/dist/web-ui-abc.js",
    ],
    "the staged tree must be exactly the binary plus web-ui/dist re-rooted (no dist/dist nesting, no extra files)"
  );
});

test("test_stage_engine_refuses_when_web_ui_build_is_absent", () => {
  const { tmp, script } = stageTree({ webUi: false });
  const result = runStage(tmp, script);
  assert.equal(
    result.status,
    5,
    `expected exit 5 when ../web-ui/dist has no index.html, got ${result.status}: ${result.stderr}`
  );
  assert.match(result.stderr, /web-ui\/dist/, "stderr must name the missing bundle directory");
  assert.match(result.stderr, /index\.html/, "stderr must name the missing index.html");

  const binary = path.join(tmp, "plugin", DEST_REL, "opencode-agents-memory");
  assert.ok(
    !fs.existsSync(binary),
    `a refused stage must not leave a staged binary at ${binary}`
  );
});

test("test_stage_engine_refuses_to_replace_an_existing_bundle", () => {
  const { tmp, script } = stageTree();
  const dest = path.join(tmp, "plugin", DEST_REL);
  const stagedDist = path.join(dest, "web-ui", "dist");
  fs.mkdirSync(stagedDist, { recursive: true });
  const staleIndex = "<!doctype html>\n<title>stale bundle</title>\n";
  const staleAssetBytes = "// stale hashed asset — must survive byte-for-byte\n";
  fs.writeFileSync(path.join(stagedDist, "index.html"), staleIndex);
  fs.writeFileSync(path.join(stagedDist, "web-ui-DEADBEEF.js"), staleAssetBytes);

  const result = runStage(tmp, script);
  assert.equal(
    result.status,
    7,
    `expected exit 7 for an already-staged bundle, got ${result.status}: ${result.stderr}`
  );
  assert.ok(
    result.stderr.includes(`engine/${PLATFORM}-${ARCH}/web-ui/dist`),
    `stderr must name the dest directory, got: ${result.stderr}`
  );

  // Replace, never merge: the staged bundle is byte-unchanged and no new hashed
  // asset (trunk rehashes names) appeared anywhere under the staged web-ui/dist.
  assert.equal(
    fs.readFileSync(path.join(stagedDist, "index.html"), "utf8"),
    staleIndex,
    "the stale index.html must be byte-unchanged (never replaced)"
  );
  assert.equal(
    fs.readFileSync(path.join(stagedDist, "web-ui-DEADBEEF.js"), "utf8"),
    staleAssetBytes,
    "the stale hashed asset must be byte-unchanged (never merged)"
  );
  assert.deepEqual(
    fs.readdirSync(stagedDist, { recursive: true }).sort(),
    ["index.html", "web-ui-DEADBEEF.js"],
    "no new hashed asset may appear under the staged web-ui/dist (replace, never merge)"
  );
});
