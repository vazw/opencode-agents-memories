# Issue 82 — one shared engine bridge per host process: evidence

Run 2026-10-31, worktree `.tmp/worktrees/issue-82-shared-engine-bridge`, branch
`issue-82-shared-engine-bridge` (base `a01cfe7`).

Engine: `plugin/engine/linux-x64/opencode-agents-memory`, **80,459,320 B**, sha256
`72d7f64d5bdcbabd54b5c59b8a35c93712d4a6699c3557f4dbf01f8b24d5e653`, staged 2026-10-30 21:48
(re-derived; the file is gitignored, linked into this worktree for the probe).

## Gate A — one process, two generations, real engine

```sh
mkdir -p .tmp/scratch/issue82 && R=$(mktemp -d .tmp/scratch/issue82/probe-XXXX)
OPENCODE_MEM_WEB_PORT=4759 node plugin/scripts/shared-engine-probe.mjs --package plugin --root "$R"
```

Artifacts: `gate-a.json`, `gate-a.err`, `gate-a.code` (`143` = the probe's own SIGTERM;
the plugin reaped the engine, `child_reaped` in `gate-a.err`), `root.txt`.

Result: `enginePids` length **1**; both `answers[].success === true`; both
`storagePath === $R` (the scratch root).

## Gate B — the same root from a second process while A holds

Same command in a second shell while A runs with `--hold-ms 8000`.

Artifacts: `gate-b.json`, `gate-b.err`, `gate-b.code`, `gate-b-status.txt`.

Result: `answers[0].error === "engine_unavailable"`, `enginePids` empty (the scan is
scoped to the probe's own children), and `<root>/status` reads

```
degraded: engine lock held (pid=1312, ts=2026-10-31T06:56:36.259Z)
```

— the lock loser's own line naming **A's engine pid**, not B's plugin pid. The
wrapper's exit-75 suppression left it intact.

## Gate C — the cross-process lock is unchanged

```sh
sh .tmp/scripts/cargo-serialized.sh cargo test -p mem-store --test engine_lock
```

Artifact: `gate-c.txt` — **7 passed, 0 failed**, including
`test_engine_lock_loser_reports_lock_lost`.

## Suite counts

- `shared-bridge-tests.txt` — `node --test tests/shared-bridge.test.mjs`: **6 tests, 6 pass, 0 fail**.
- `suite.txt` — `node --test tests/*.test.mjs`: **27 files, 166 tests, 166 pass, 0 fail**,
  36,851 ms; `cmp plugin/src/plugin.js plugin/dist/plugin.js` → 0.

## Step 10 — live re-swap (not executed here)

The live gate (owner + syseng, after this branch lands) is the runbook's §9b and §10
battery: one engine, its parent is the host pid, and `memory` `list-shards` answers from
**two different workspaces** (the second workspace is the live discriminator). It requires
stopping OpenCode and touching the real store, so it is deliberately not run by the
implementer; no live-store artifact exists in this directory.
