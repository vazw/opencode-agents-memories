# 80 — ops · Live-swap runbook: backup, restore rehearsal, cache provisioning, verification and rollback

Status: ready-for-agent — creates `docs/runbooks/live-swap.md` and
`docs/evidence/live-swap-2026-10-30/` (the runbook, the provisioning procedure, the executed backup +
restore rehearsal, and the verification/rollback checklist).

## What is missing

The port is buildable and passes a real-host smoke, but the swap to it is neither documented nor
rehearsed. The backup plan, the restore command, the ONNX provisioning requirement, the exact migration
sequence, the verification battery, and the rollback exist only in a dry-run report under `.tmp/`
(gitignored) and in the PRD's Phase 7 section. Without a tracked runbook and a rehearsed restore, the
owner would rediscover the `-32603` cache prerequisite and the migration order at the moment the live
store is on the line.

The measured facts the runbook must carry (2026-10-30 dry run on checkpointed copies):

- `metadata.db` 1→2 on the first store touch: repair + partial unique index, 22 rows unchanged, 144–285 ms.
- `user-prompts.db` 1→2 on the first web `?includePrompts=true`: stamp only (all 12 columns already
  present), 1,732 rows fingerprint-identical, 160–237 ms.
- Project shards 2→3 lazily on first write: `CREATE shard_stats` only; the other 21 shards stay v2 until
  written. No migration takes a `.bak`.
- Final copy: 22/22 shards, 3,091/3,091 memories, 768-dim vectors read in place (no re-embed), 1,732/1,732
  prompts, 2/2 profiles + 10 changelogs, 29/29 DBs `integrity_check = ok`.
- The tool embedder reads `$TMPDIR/.opencode-mem-onnx` (`crates/mem-bridge/src/composition.rs:424`,
  `hub: None`): absent → `-32603` in 7 ms; upstream `.cache` as-is (tokenizer v1.0) → `-32603` after
  6.8 s; a canonical v2.0 tokenizer → works.

## What lands

- `docs/runbooks/live-swap.md` — the owner-facing sequence: stop OpenCode; checkpoint; dated `cp -a`
  backup including both config files; sha256 manifest; per-DB `integrity_check`; the restore command; the
  in-place migration expectations; cache provisioning; the config-editor swap with its restore command;
  the verification battery; the dogfood window; the rollback.
- `docs/evidence/live-swap-2026-10-30/` — the dry run's decisive measurements transcribed out of `.tmp/`
  (per-DB before/after versions and counts, the A/B/C cache probe, the search hits, the timings, the
  29/29 integrity result), the backup manifest, and the restore-rehearsal transcript.
- A provisioning procedure that starts from a clean `$TMPDIR` and ends with a working `search`/`add`:
  copy the model directory from the live `.cache`, canonicalize `tokenizer.json` to v2.0, verify with a
  search and an add. Exact commands in the runbook; a tracked helper under `scripts/` only if the
  procedure needs one.
- An executed backup + restore rehearsal: restore the backup into a scratch directory, open it with the
  engine, and compare versions, shard count, memories, prompts, profiles, and `integrity_check` against
  the pre-state. The live store is never an input.
- The 917-memory export's sha256 and its disaster-recovery role (current project only), plus the
  measured export→import verdict: `917 imported / 0 rejected / 0 skipped` on a fresh store, all 917
  rows field-for-field identical, shard v3 / metadata v2, search rank-1 correct, a second import
  aborts whole-import in 521 ms with zero writes — but it carries no profiles, prompts,
  milestones/staging sessions, global memories, or the other 21 projects, and it re-embeds
  (~10.5 min real / 11.1 min dry-run per 917 on this host). State it as the portability/backup route,
  not the primary swap.
- The fresh-engine reopen rule and its recovery: a reopen after a SIGTERM'ed engine was observed
  answering `-32603` on `list-shards`/`list`/`import` once (run A), fixed by removing stale
  `*.db-shm`/`*.db-tshm` after confirming no engine runs — never the DB/WAL; the clean control
  (ordered `shutdown`, run B) reopened fine. Make the reopen an explicit runbook step.

## Acceptance criteria

- [ ] `docs/runbooks/live-swap.md` exists and covers, in order: stop OpenCode; checkpoint; dated `cp -a`
      backup (including `~/.config/opencode/opencode.jsonc` and `opencode-mem.jsonc`); sha256 manifest;
      per-DB `integrity_check`; the restore command; the in-place migration expectations (metadata 1→2,
      prompts 1→2, shards 2→3 lazily); cache provisioning; the config-editor swap and its restore
      command; the verification battery; the dogfood window; the rollback.
- [ ] The backup procedure is executed once; its directory and manifest are recorded; every copied DB
      passes `integrity_check`; `sha256sum -c` verifies the manifest.
- [ ] The restore rehearsal restores into a scratch directory, opens it with the engine, and matches the
      pre-state (versions, shard count, memories, prompts, profiles); the transcript is in the evidence
      directory.
- [ ] The provisioning procedure runs from a clean `$TMPDIR`; a measured `search` and `add` succeed
      afterwards; the absent-cache and v1.0-tokenizer failures are reproduced and recorded.
- [ ] The evidence directory carries the dry run's decisive numbers transcribed from `.tmp/` — no
      decisive claim is left only in `.tmp/`.
- [ ] The verification battery lists each check with its expected result: `hello`/`health`; direct
      `memory` tool (`options.codemode: false`); `memory-usage` skill listed via `/api/skill`, loaded and
      used by a model; `skills`; `list`/`search`/`profile`; the web UI; one `add` + search under the
      provisioned cache; capture, injection, and compaction under real sessions (naming the 2026-10-30
      podman run as the tool-lane prior).
- [ ] The rollback section gives exact commands: restore config, restore store, restart; and states the
      917-memory export's disaster-recovery role.
- [ ] The runbook records the export→import verdict and the fresh-engine reopen step with its stale
      `*.db-shm`/`*.db-tshm` recovery (never the DB/WAL), transcribed from
      `.tmp/progress/upstream-export-to-port-import-20261030T163933Z.md`.
- [ ] No live-store writes and no live-config edits in this issue; the repo stays clean.

## Reopens when

The engine's embedder cache root becomes configurable (the provisioning procedure changes), the
config-editor's swap target changes, or the store's max-known schema versions change (the runbook's
version expectations move).

## Out of scope

- Executing the swap (issue 81) and any live-store or live-config change.
- A code change making the cache root configurable; this issue records the chosen provisioning procedure
  and names the code change as a follow-up if the owner wants one.
- Re-embedding, or migrating the other 21 projects' directory paths.
