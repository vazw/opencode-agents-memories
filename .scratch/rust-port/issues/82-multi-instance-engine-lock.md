# 82 — plugin · The host loads the plugin once per agent; only one engine can win the store lock, so every other agent's memory tool degrades

Status: needs-triage — the fix needs one design decision (process-global bridge vs a shared engine
service) before a blueprint; measured 2026-10-31 on the live swap, tool lane only (the store and the
web surface are healthy).

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
      once the ONNX cache is provisioned; no `engine_unavailable` in the host log.
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
