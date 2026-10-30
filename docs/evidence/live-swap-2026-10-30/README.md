# Live-swap evidence — 2026-10-30

Evidence for `docs/runbooks/live-swap.md` (issue 80). Every decisive number referenced by the runbook is
transcribed here from the dry-run/probe artifacts under `.tmp/` (gitignored), plus the backup, restore and
provisioning rehearsals executed on 2026-10-30 (UTC) against **copies only**.

## What is here

| file | content |
|---|---|
| `dry-run.md` | the in-place migration measured on checkpointed copies: per-DB versions, counts, timings, integrity, search, one-way semantics |
| `export-import.md` | the upstream export → fresh port store probe (the DR / portability route) |
| `backup-restore.md` | the executed backup, manifest verification, integrity check, and the restore rehearsal transcript |
| `backup-manifest-20261030T171653Z.txt` | the sha256 manifest as produced (85 files), sha256 `712d6cee…c4023c92` |
| `backup-verification-20261030T171653Z.txt` | the `sha256sum -c` output of that manifest (85 × `OK`), sha256 `df713a01…c51b972c` |
| `provisioning.md` | ONNX cache A/B/C (absent, tokenizer 1.0, canonical 2.0), the tracked provisioner, the owner-facing `curl` verification |

## Environment (measured)

| item | value |
|---|---|
| host | Void Linux, kernel `7.3.0_1`, x86_64, 12 logical cores |
| tools | `sqlite3` 3.53.4, `node` 24.22.1, `cargo`/`rustc` 1.98.0 |
| repo | `main` = `1bf1e8a` when the rehearsal ran; the tracker and README commits `714e867` / `cb7b29b` landed concurrently, so the evidence sits on top of them |
| engine | `plugin/engine/linux-x64/opencode-agents-memory`, 80,459,320 B, sha256 `72d7f64d5bdcbabd54b5c59b8a35c93712d4a6699c3557f4dbf01f8b24d5e653`, `opencode-agents-memory 0.1.0 (bridge 1.0.0)` — built at `e5154bc`, i.e. **before** W6.6's web routes |
| live store | `~/.opencode-mem/data`, copied read-only (`cp -a`) at 2026-10-30T17:16:53Z |
| live root copy | 654,720,843 B / 148 files; `data/` 595 MB of which `.cache` 546 MB |

## Verdicts

1. **In-place migration is the swap path.** On a checkpointed copy the port migrated `metadata.db` 1→2
   and `user-prompts.db` 1→2, wrote the first project shard 2→3, and preserved every count measured:
   22/22 shards, 3,091/3,091 memories (15:42Z copy), 768-dim vectors read in place, 1,733 prompts, 2/2
   profiles + 10 changelogs, 29/29 DBs `integrity_check = ok`. No `.bak` files.
2. **The restore point works.** A dated `cp -a` backup of the whole root (85 files, 617 MB) plus both
   config files, verified by `sha256sum -c` (85/85) and 29/29 `integrity_check`, restored into a scratch
   directory byte-identically, opened twice by the engine, and matched its pre-state on versions, shard
   count, memories, prompts, profiles and integrity — with the two expected version bumps and nothing
   else.
3. **`search`/`add` need one provisioning step.** With the cache absent they answer `-32603` in 9–11 ms;
   with upstream's `.cache` copied as-is they answer `-32603` after 3.7 s (legacy tokenizer 1.0); after
   `scripts/provision-onnx-cache.sh` (canonical v2.0 + the same weights) `search` works in 2.9 s then
   63 ms, `add` in 73 ms, and the canary is rank 1.
4. **The export is the DR route, not the swap.** 917 memories imported with 0 rejected / 0 skipped and 0
   field mismatches — but no profiles, prompts, milestones, global memories or other 21 projects, and
   ~10–11 min of re-embedding per 917 rows.
5. **One-way is real.** A newer store is refused (`metadata.db` v3 → `list-shards`/`list`/`add` all
   `-32603`; a shard v4 is readable but `add` is refused). The backup is the rollback, not the old plugin.

## Live-store hygiene (verified after all runs)

Compared against the preflight listing of `~/.opencode-mem/data`:

- No write, checkpoint or removal under `~/.opencode-mem/data`. The live `metadata.db` is still
  `user_version = 1` / 22 shards, all 22 project shards are still `v2`, `user-prompts.db` still `v1`.
- No port artifact in the live root: no `data/.auth-token`, `data/.engine.lock` or `data/status`. (The
  `~/.opencode-mem/.auth-token` at the **root** is upstream's own, 64 B, mtime 2026-10-11 — it is inside
  the backup, and the port uses `<storage-root>/.auth-token`, a different file.)
- The live `.cache` tokenizer is untouched: still `"version": "1.0"`, mtime 2026-10-11T00:43:55Z.
- The live plugin kept running and writing during the rehearsal (mtime `user-prompts.db`
  2026-10-30T23:57+07). Row counts taken at different instants are therefore not comparable — do not
  read the 3,104 figure as a leak from 3,091; the store simply grew.

## Deviations from the runbook as executed in this rehearsal

1. **The live store was not stopped.** The rehearsal may not stop OpenCode, so the backup was taken from
   a `cp -a` of the live root and the `PRAGMA wal_checkpoint(TRUNCATE)` step ran on that copy. The runbook
   stops OpenCode first and checkpoints in place; both then produce the same artifact (a checkpointed
   root). The `integrity_check` gate over the copied DBs is what detects a torn copy — 29/29 passed.
2. **`rsync` is not installed on this host.** The PRD's `rsync -a --delete` restore is replaced by
   move-aside (`mv`) + `cp -a` into an absent target, which the rehearsal executed.
3. **The engine is the `e5154bc` build**, not a rebuild from `main` `1bf1e8a`. The web UI served at
   rehearsal is therefore the pre-W6.6 bundle; issue 81 rebuilds with `plugin/scripts/stage-engine.sh`
   and must re-run the web-UI check.
4. **The dry-run report quotes 1,732 prompts for `data-final`; its own artifacts say 1,733.** The 1,732
   belongs to the 15:21Z copy; `pre-final.txt` / `state-after-final.txt` (15:42Z) hold 1,733. This
   evidence records 1,733 for `data-final` and keeps 1,732 for `data`.
5. **`/api/skill` is an OpenCode host endpoint**, not the port's web server. The port's own skills API is
   `/api/skills`. The runbook uses the host endpoint for the skill check because that is where the
   plugin-owned `opencode-mem.memory-usage` appears.
