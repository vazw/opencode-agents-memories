# 82 — plugin · The host loads the plugin once per agent; only one engine can win the store lock, so every other agent's memory tool degrades

Status: landed — 2026-10-31 — fix at `c37f05c` (ff-only from `issue-82-shared-engine-bridge`; base
`a01cfe7`; 20 files, +1634/−276). Residual: the Step 10 live re-swap, owned by issue 81.

## What is happening

The live host loads the local plugin **four times** at startup (one per agent), and each plugin
instance spawns its own engine. The engine's single-engine lock (`.engine.lock`, issue 45) lets exactly
one win; the losers write the degradation status and their plugin instances fail closed. The agent
whose instance lost gets `engine_unavailable` for every engine-backed `memory` mode (`help` still works
— it is local), while the winning engine keeps serving its own agent and the web UI.

Measured on the live swap (2026-10-31, 01:50 local):

- host log, `~/.local/share/opencode/log/opencode.log` at `18:50:14`, same run, 10 ms apart:
  `loading plugin id=/home/vaz/Code/Personal/Rust/opencode-agents-memory/plugin` × **4**, all entry
  `file:///home/vaz/Code/Personal/Rust/opencode-agents-memory/plugin/dist/plugin.js`;
- one engine survives: `opencode-agents-memory --storage-root ~/.opencode-mem/data`, pid **27943**;
  it holds `.engine.lock` (`pid=27943`, `start_time=118719`, matching `/proc/27943/stat`);
- `~/.opencode-mem/data/status`:
  `degraded: engine lock held (pid=27943, ts=2026-10-30T18:50:59.619Z)` — the loser's grammar
  (`crates/mem-store/tests/engine_lock.rs:183-194` names the holder pid);
- `memory` modes `list-shards` / `list` / `profile` / `search` → `{"success":false,"error":"engine_unavailable"}`;
  `help` → the port's local shape (`unavailable: []`), so the plugin, not the store, is the failure;
- the winner's web surface is healthy: `127.0.0.1:3742/api/health` = 200, `/api/stats` = 401 then 200
  with the token minted at `~/.opencode-mem/data/.auth-token`;
- the store migrated correctly: `metadata.db` v2, 22 shards, 3,116 memories;
- the backup is pre-migration: `~/.opencode-mem-backups/20261030T175230Z` (617 MB).

**Why the smoke missed it:** the 2026-10-30 podman run drove a single session/agent, so one plugin
instance spawned one engine and the lock never contended. The live host loads the plugin per agent.

## What the fix must do

One engine per **host process** (or a shared engine reachable by every plugin instance), so all
agents see the same memory lane:

- the smallest shape: move the bridge state (`bridgeState`, `child`, `pendingResponses`,
  `startingInstance`, the line buffer, the shutdown timers) into a process-global registry keyed by the
  resolved storage root (e.g. `globalThis[Symbol.for("opencode-mem.bridge")]`), so four module
  instances in one host process share one child and one handshake;
- confirm the four loads are in **one** process (the log run is one; the engine's parent is the host),
  which the global repair requires; if the host ever loads the plugin in separate worker processes, a
  shared engine service (a socket) is the alternative, not a per-process child;
- cover the V2 adapter and the TUI companion (they must not bypass the registry);
- keep the engine lock as the cross-**process** guard; it must not be defeated by the shared bridge.

## Acceptance criteria

- [ ] Two module instances of the plugin in one process (a cache-busted second `import`, the live-host
      shape) spawn **one** child and both answer `list-shards` with the same 22-shard view.
- [ ] The live host after a full quit/start: all agents' `memory` modes answer; `search`/`add` answer
      once the ONNX cache is provisioned; the two workspaces' `memory list-shards` both answer and the
      wrapper emits no structured `"event":"degraded"` stderr (the host log records no tool results —
      a whole-log grep is not a valid signal).
- [ ] The single-engine lock still refuses a **second process** (unchanged issue-45 behaviour).
- [ ] The `status` file is written only by a genuine loser (another process), never by a sibling
      instance of the same process.
- [ ] A regression fixture runs two imports against one scratch store and asserts one engine process.
- [ ] No store-format change; the migrated store is reused as-is.

## Out of scope

- The ONNX cache provisioning (`scripts/provision-onnx-cache.sh`, issue 80) — a separate prerequisite;
  the tool lane above failed before any embedding ran.
- The web port (`3742` vs the config's `webServerPort` 4747) — a separate finding already recorded in
  the runbook/README.
- Store migration, rollback, or the backup (issue 81).

## Comments

- **2026-10-31 — the acceptance criterion is corrected (Plan Gate r1).** "No `engine_unavailable` in the
  host log" is unmeasurable: the host log records no tool results (whole-log `grep -c '"success":false'`
  = 0; run `d7efb345` has 0 `list-shards` lines; the 20 `engine_unavailable` hits are agent-authored
  bash text). The measurable signal is the two workspaces' `memory list-shards` results plus the
  wrapper's structured `"event":"degraded"` stderr. Full gate report:
  `.tmp/progress/blueprint-20261030-1905-issue82-shared-engine-bridge.gate-1.md`.

## Landed

2026-10-31 — the shared-engine bridge landed at **`c37f05c`** (`fix(plugin): share one engine bridge per
host process (#82)`; ff-only from `issue-82-shared-engine-bridge`; base `a01cfe7`; 20 files,
+1634/−276). Reviewer APPROVED on independent re-execution
(`.tmp/progress/blueprint-20261030-1905-issue82-shared-engine-bridge-r3.review-1.md`):

| gate | result |
|---|---|
| plugin suite | **27 files / 166 tests / 166 pass / 0 fail** (re-derived; 36.9 s) |
| `cmp src dist` | 0 (byte-identical, 43,125 B) |
| Gate A (real engine, one process, two imports) | `enginePids` length 1, both generations `success:true`, both `storagePath === $R` |
| Gate B (second process while A holds) | both answers `engine_unavailable`, `enginePids []`, `$R/status` names A's engine pid (ppid = A's probe) |
| `engine_lock` (cross-process) | **7/7 passed**, unchanged |
| mutants | **3/3 killed** — registry key, exit-75 suppression, restart cap |
| pre-fix RED | re-derived independently: `helloCount 2 !== 1` (cases 1/2), `degraded 2 !== 1` (case 4), case 3 green (2 spawns, distinct argv) |

The per-root state is data-only (no closures); the per-agent split (`capturedSessionId`,
`captureIdByPrompt`, `NOTIFIED`, `compactionPartCounter`) is intact; the only cross-generation function
is the documented first-spawn `shutdownHandler` → `shutdownAll`. `docs/bridge-schema.md` §7/§8 and the
runbook §9b/§10 rows were updated. Step 10 (the live re-swap: one engine for the 26 loads, the
parent-pid check, two workspaces' `list-shards`) is the owner gate and remains with issue 81.
