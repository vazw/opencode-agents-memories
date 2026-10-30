# 81 — ops · Execute the live drop-in swap: migrate in place, provision, swap the plugin, verify, dogfood, roll back

Status: paused — behind W6.6 (issue 30) and the owner's stop/go; then owner-executed with the
config-editor after the human gate. Evidence lands in `docs/evidence/live-swap-<date>/`.

## What is missing

The live OpenCode host still loads upstream `opencode-mem@latest` (2.30.0) against the live store
(`~/.opencode-mem/data`). Issue 80 produces the runbook, the executed backup, the rehearsed restore, and
the provisioning procedure; this issue is the owner-gated execution against the live host. It must not
start before W6.6 lands, so the staged engine and the built UI are the ones the swap will run.

## What lands

- The live store migrated in place (metadata 1→2, prompts 1→2, project shards 2→3 lazily on first write),
  preceded by the final checkpointed backup and its manifest.
- The ONNX cache provisioned so `search`/`add` work.
- The plugin entry in `~/.config/opencode/opencode.jsonc` swapped from `opencode-mem@latest` to the local
  plugin path, applied by the config-editor with a backup and a restore command.
- A verification transcript, a dogfood window, and a rollback drill, recorded under
  `docs/evidence/live-swap-<date>/`.

## Acceptance criteria

- [ ] W6.6 (issue 30) landed; no worktree in flight; the staged engine rebuilt from the landed `main`.
- [ ] OpenCode stopped; the live DBs checkpointed; the dated backup and sha256 manifest taken; every
      copied DB passes `integrity_check`.
- [ ] The live store migrated in place; versions verified (metadata 2, prompts 2, shard 3 on first write);
      22/22 shards; memories, vectors (768 dims), prompts, and profiles match the pre-state; no `.bak`
      surprises.
- [ ] The cache is provisioned; a measured `search` and `add` succeed.
- [ ] A fresh-engine reopen after the swap answers `hello`/`health` and `list-shards`/`list`/`search`
      correctly; if `-32603` appears, confirm no engine runs and remove stale `*.db-shm`/`*.db-tshm`
      (never the DB/WAL), then reopen. The last opener must be an ordered `shutdown`, never a
      SIGTERM'ed engine.
- [ ] The plugin swap is applied through the config-editor after the human gate; the restore command is
      recorded.
- [ ] The verification battery is green: `hello`/`health`; direct `memory` tool with
      `options.codemode: false`; `memory-usage` skill listed via `/api/skill`, loaded and used; `skills`;
      `list`/`search`/`profile`; the web UI; one `add` + search.
- [ ] Capture, injection, and compaction are exercised under real sessions, or a recorded, time-boxed
      dogfood window states what was and was not observed.
- [ ] The rollback drill restores the backup and config into a scratch copy and opens it with the engine
      (or the real rollback if a defect forces it); the result is recorded.
- [ ] The evidence directory holds the verification transcript, the backup manifest, the dogfood notes,
      and the 917-memory export's sha256; the repo stays clean.

## Reopens when

The swap fails verification, the owner rolls back, or a later engine/plugin rebuild changes the store's
max-known schema versions or the embedder cache root.

## Out of scope

- Re-embedding, changing the embedding model, or migrating the other 21 projects' directory paths (the
  full-store backup covers them).
- Auto-update service, i18n translations, request-timeout polish (W7.1).
- The web default `userId` gap — recorded as a follow-up, not a swap blocker.
