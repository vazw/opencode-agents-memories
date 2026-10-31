#!/usr/bin/env node
// Issue 82 ops probe — the REAL engine, one process, two plugin generations.
//
// Mirrors the live host's loader: `registerHooks` rewrites every local
// specifier of a generation (packages/plugin/src/source.node.ts:12-16,47-51),
// so the two cache-busted imports below are two whole module graphs in one
// realm. The probe then drives the agent-facing `memory` tool (`list-shards`)
// from BOTH generations and scans /proc for engines on this root.
//
// Usage:
//   node plugin/scripts/shared-engine-probe.mjs --package plugin --root <dir> [--hold-ms n]
//
// Gate A: one process → `enginePids` length 1, both answers success, storagePath = root.
// Gate B: a second process on the same root while A holds → answers[0].error
//         `engine_unavailable`, enginePids empty, `<root>/status` names A's engine pid.
//
// It never touches a live store: `--root` is a scratch directory, and
// `OPENCODE_MEM_WEB_PORT` (exported by the operator) keeps its engine off 3742.

import fs from "node:fs";
import path from "node:path";
import { registerHooks } from "node:module";
import { pathToFileURL } from "node:url";

function option(name, fallback = null) {
  const index = process.argv.indexOf(name);
  if (index < 0) return fallback;
  const value = process.argv[index + 1];
  return value === undefined ? fallback : value;
}

const packageDir = path.resolve(option("--package", "."));
const root = path.resolve(option("--root", ""));
const holdMs = Number(option("--hold-ms", "0"));

if (option("--root") === null) {
  console.error("usage: shared-engine-probe.mjs --package <dir> --root <dir> [--hold-ms n]");
  process.exit(2);
}

process.env.OPENCODE_MEM_STORAGE_ROOT = root;

const GENERATION_MARKER = /__fixture_generation=(\d+)/;
registerHooks({
  resolve(specifier, context, nextResolve) {
    const resolved = nextResolve(specifier, context);
    const marker = GENERATION_MARKER.exec(context.parentURL ?? "");
    if (marker === null || !resolved.url.startsWith("file:")) return resolved;
    const url = new URL(resolved.url);
    url.searchParams.set("__fixture_generation", marker[1]);
    return { url: url.href, shortCircuit: true };
  },
});

function makeCtx(added) {
  return {
    location: { directory: root, project: { directory: root } },
    session: { hook: async () => {} },
    event: {
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
    rpc: {
      register: async () => ({ events: { emit: async () => {} } }),
    },
  };
}

function enginePids() {
  const found = [];
  let entries = [];
  try {
    entries = fs.readdirSync("/proc");
  } catch {
    return found;
  }
  for (const entry of entries) {
    if (!/^\d+$/.test(entry)) continue;
    let cmdline = "";
    try {
      cmdline = fs.readFileSync(path.join("/proc", entry, "cmdline"), "utf8");
    } catch {
      continue;
    }
    const args = cmdline.split("\0").filter((arg) => arg.length > 0);
    const index = args.indexOf("--storage-root");
    if (index < 0 || args[index + 1] !== root) continue;
    if (!args.some((arg) => path.basename(arg) === "opencode-agents-memory")) continue;
    // Only engines THIS probe spawned: another probe on the same root may
    // legitimately hold one (gate B expects none of its own).
    let stat = "";
    try {
      stat = fs.readFileSync(path.join("/proc", entry, "stat"), "utf8");
    } catch {
      continue;
    }
    const commEnd = stat.lastIndexOf(")");
    const fields = commEnd >= 0 ? stat.slice(commEnd + 2).split(" ") : [];
    if (Number(fields[1]) !== process.pid) continue;
    found.push(Number(entry));
  }
  return found.sort((left, right) => left - right);
}

async function callShards(tool, sessionID) {
  try {
    const result = await tool.execute({ mode: "list-shards" }, { sessionID });
    return JSON.parse(result.content);
  } catch (error) {
    return { success: false, error: "probe_exception: " + String(error && error.message ? error.message : error) };
  }
}

const baseUrl = pathToFileURL(path.join(packageDir, "dist", "plugin.js")).href;
const added1 = [];
const added2 = [];
const gen1 = await import(baseUrl + "?__fixture_generation=1");
const gen2 = await import(baseUrl + "?__fixture_generation=2");
await Promise.all([gen1.setup(makeCtx(added1)), gen2.setup(makeCtx(added2))]);
const tool1 = added1.find((entry) => entry.name === "memory");
const tool2 = added2.find((entry) => entry.name === "memory");
if (tool1 === undefined || tool2 === undefined) {
  console.error("the memory tool was not registered on both generations");
  process.exit(1);
}
const answers = [
  await callShards(tool1, "probe-gen-1"),
  await callShards(tool2, "probe-gen-2"),
];

process.stdout.write(JSON.stringify({ root, enginePids: enginePids(), answers }) + "\n");

// Gate B needs A's engine alive while B runs; the printed line is already out.
if (Number.isFinite(holdMs) && holdMs > 0) {
  await new Promise((resolve) => setTimeout(resolve, holdMs));
}

process.kill(process.pid, "SIGTERM");
// Keep the loop alive across the plugin's async teardown; a re-delivered
// SIGTERM ends the process before this timer fires.
await new Promise((resolve) => setTimeout(resolve, 5000));
