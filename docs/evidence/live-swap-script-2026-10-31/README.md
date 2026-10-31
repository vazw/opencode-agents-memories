# `scripts/live-swap.sh` — the runbook as a program

Evidence for the owner-facing executor of `docs/runbooks/live-swap.md` (landed at `70d9206`, §9b/§10 at
`c37f05c`). Written 2026-10-31. **The live store and the live config were never written**: no
`--swap` and no `--rollback` ran against them, and no `--backup` ran either (OpenCode was running the
whole time, which is exactly the condition every writing mode refuses on).

Two files ship: `scripts/live-swap.sh` (the executor) and `scripts/test-live-swap.sh` (16 behavioural
cases against scratch trees).

## Results

| item | result | artifact |
|---|---|---|
| `bash -n scripts/live-swap.sh` | rc 0 | `help-and-lint.txt` |
| `shellcheck scripts/live-swap.sh` | rc 0, no findings | `help-and-lint.txt` |
| `bash -n` + `shellcheck` on `scripts/test-live-swap.sh` | rc 0 | `test-live-swap.txt` |
| `--help` on the live machine | rc 0, names every mode + the runbook | `help-and-lint.txt` |
| `--dry-run` on the live machine (OpenCode running) | rc 0, `failures=0 warnings=2 pending=2`, nothing written | `dry-run-live.txt` |
| `scripts/test-live-swap.sh` | **PASS 16/16** (13 at `807a739` + 3 guard cases), 7 s | `test-live-swap.txt` |
| rehearsal `--backup` on a scratch copy | 29 DBs checkpointed (`0|0|0`), 85-file manifest verified, integrity 29/29 `ok` | `rehearsal-2-backup.txt` |
| rehearsal `--rollback` on the same scratch copy | restored `data/` byte-identical (60/60 files), port-era store moved aside intact | `rehearsal-4-rollback.txt`, `rehearsal-5-byte-compare.txt` |
| rehearsal `--verify` against a real engine | green: 1 engine, health 200, token minted, search 28 hits (top 0.5619), canary stored + found rank 1, metadata v2 / prompts v2 / 1 shard v3 | `rehearsal-6-verify.txt` |
| rehearsal `--swap` on a fake config | exactly one line changed (byte delta 39 = `len(new) − len(old)`), second run idempotent | `rehearsal-8-swap.txt`, `rehearsal-9-config-diff.txt` |
| stop check | `--swap` refused while an engine held the scratch store root (pid named) | `rehearsal-7-stop-check.txt` |

Live store state at measurement: 151 files under `~/.opencode-mem`, 29 DBs, 654,637,185 B,
`metadata.db` `user_version=1`, 22 project shards, config `webServerPort` 4747 (the engine binds 3742;
`OPENCODE_MEM_WEB_PORT` overrides), `storagePath = ~/.opencode-mem/data`,
`embeddingModel = Xenova/nomic-embed-text-v1`, ONNX cache at `/tmp/.opencode-mem-onnx/Xenova/…` already
canonical (`tokenizer_version=2.0`, `model_onnx_bytes=547552426`).

## The rehearsal, in order

Scratch root `.tmp/scratch/rehearsal-final` (gitignored): `cp -a ~/.opencode-mem` + copies of both config
files. The transcripts are the script's own output, not a re-narration.

1. pre-state — 623 MB, 151 files, 29 DBs, `metadata.db` v1, both config sha256s recorded.
2. `--dry-run --rehearse` — every check `ok`/`info`, one warning (the staged engine is older than
   `plugin/dist/plugin.js` — true of the live machine too, and issue 81's first acceptance line), exit 0.
3. `--backup --rehearse --yes` — 29 checkpoints all `0|0|0`, sidecars removed, 85-file `cp -a`, manifest
   `sha256sum -c` 0 failed, 29/29 `integrity_check = ok`, `LATEST` published **after** verification.
4. mutation (the port era, on the copy only): `metadata.db` 1→2, shard `4182…` 2→3, an extra
   `data/port-era.txt`.
5. `--rollback --rehearse --yes` — both configs restored from the backup, `data/` moved aside to
   `backups/data.port-<TS>`, `cp -a` back, `diff -r` clean.
6. independent verify (not the script's `diff -r`): the backup's manifest still verifies; all 60 files
   under `data/` hash identically in backup and restored store; both configs match byte-for-byte; the
   aside store still carries v2/v3 and `port-era.txt`, the restored store carries v1/v2 and no marker.
7. `--verify --rehearse --web-port 3799` against the staged engine started by hand on the scratch store
   (the script never starts OpenCode or the engine): one engine, `/api/health` = the exact envelope,
   `/api/stats` 401 minting `<store>/data/.auth-token`, `search` 28 hits with top similarity
   `0.561898096957182` (runbook REST figure: 0.5619), one add canary stored and found at rank 1,
   then `metadata.db` v2 / `user-prompts.db` v2 / 1 of 22 shards v3.
8. `--swap --rehearse --yes` — the fake config's one active token replaced by the absolute plugin path;
   byte delta 39 = `len("/home/vaz/…/plugin") − len("opencode-mem@latest")`; the commented line above it
   untouched; every other byte identical; second run reports idempotence and edits nothing.

## Deviations from the runbook, stated as deviations

1. **`LATEST` is written after §5, not after §4.** §4 writes the pointer right after the copy; this
   script writes it only once the manifest verifies and every DB passes `integrity_check`, so `LATEST`
   names a verified restore point. `--rollback`'s fallback (when `LATEST` is absent) also skips a
   directory without `MANIFEST.sha256` + `BACKUP-INFO`. Case 13 of the self-test holds this: a corrupt
   DB must not publish `LATEST`.
2. **`--backup` asks for confirmation too** (the runbook asks only implicitly by saying "stop OpenCode
   first"). The checkpoint deletes the derived `*-shm`/`*-tshm` sidecars, which is a write to the live
   store, so it is confirmed like the config edit. `--yes` skips it; stdin that is not a terminal is
   refused rather than defaulted.
3. **`--rollback` refuses a post-swap backup.** §12 restores `config/opencode.jsonc` from the same backup
   it takes `data/` from. If the owner ever runs `--backup` during the dogfood week, `LATEST` then holds
   an already-swapped config and the rollback would report success while reverting nothing. The script
   detects an active local-plugin entry in the chosen backup's config, refuses, and lists the pre-swap
   stamps for `--from <TS>`. Case 11 holds this.
4. **The swap counts *active* entries, not occurrences.** The live `opencode.jsonc` carries a commented
   `// "/home/vaz/…/plugin"` line directly above the upstream token, so a byte count sees the local path
   "already present". A trimmed whole-line comparison (optionally with the trailing comma) distinguishes
   an entry from a comment. Cases 4–6 hold this; it is the bug the first draft shipped.
5. **`--rehearse` skips `provision-onnx-cache.sh`** — it writes `${TMPDIR}` outside the scratch root.
   Everything else (`--backup`, `--swap`'s config edit, `--rollback`, `--verify`) runs the same code path.
6. **`--verify` writes one canary memory.** It is the runbook's battery row 12 (an `add` + a `search`)
   and the only write in that mode: no config, no checkpoint, no migration. The id is printed so the
   owner can delete it in the web UI. Re-running adds another canary and nothing else.
7. **The rehearsal's parent-pid check reports `info`, not `FAIL`.** Under `--rehearse` the engine's parent
   is the operator's shell, not an OpenCode host; on the live host that same check is the §9b fact-3
   failure and stays a `FAIL`.
8. **`--rollback` verifies the backup before it writes.** Runbook §12 does not, but §5 makes any `FAILED`
   manifest line fatal ("restore point is not valid"). The executor runs `sha256sum -c MANIFEST.sha256`
   before the config copy and the `mv`, and refuses with nothing touched. The `diff -r` after the copy
   only proves the copy was faithful, not that the backup is a valid restore point.

## Rework after review-1 (`807a739` → this tree)

The first review (`.tmp/progress/live-swap-script.review-1.md`) **rejected** `807a739` on five mechanical
points and one security-mandate violation (unvalidated `--from`); the dangerous surface itself was found
sound. This tree is the fix: `scripts/live-swap.sh` sha256 `caa3cf75…1368f1` (was `bfabc551…8343`),
`scripts/test-live-swap.sh` sha256 `2a81b2a2…3effb`. The live store and both live config files were not
written by the rework: config sha256s are still `ef2d9fbe…1427` / `94888c5c…0058`, `metadata.db`
`user_version=1` with its 2026-10-31 01:57:29 mtime, `~/.opencode-mem-backups` still holds only
`20261030T175230Z` with no `LATEST` and no `swap.log`, and no `.engine.lock` / `.auth-token` / `status`
appeared. Every rework path ran under `--rehearse` or on a `--store`/`--config-dir` scratch tree.

| §8 item | change | transcript |
|---|---|---|
| §4.2 | the five `case_09` assertions now `bfail; return 1`; every other `\|\| bfail`/`&& bfail` in the suite made to fail its case too (case 01, 06, 10, 12 included) | `test-live-swap.txt` |
| §4.3 | three new cases: backup root inside the store (case 14), `.engine.lock` naming a live pid (case 15), non-empty `*.db-wal` refused and **kept** (case 16) | `test-live-swap.txt` |
| §4.1 | `engine_is_stale` + `require_plugin` warn; `--swap` prints it **before** the confirmation and the confirmation names it; `--dry-run` reuses the same helper | `rework2-rehearsal-5-swap.txt` |
| §4.4 | `--from` validated as `^[0-9]{8}T[0-9]{6}Z(-[0-9]+)?$` in `parse_args` (exit 2) and in `resolve_restore_ts` (also guards a poisoned `LATEST`); `/`, `..`, absolute paths rejected | `rework2-from-validation.txt` |
| §4.6 | `--backup` closes with "do not start OpenCode: run `--swap` next (or re-run `--backup` …)" | `rework2-rehearsal-1-backup.txt` |
| §4.7 | the no-`LATEST` warning states the real fallback rule (needs `MANIFEST.sha256` **and** `BACKUP-INFO`; the hand-made `20261030T175230Z` needs `--from`) | `dry-run-live.txt` |
| §4.5 | `sha256sum -c MANIFEST.sha256` in the backup, **before** any write and before the `mv`; a rotten backup is refused with nothing touched | `rework2-rehearsal-6-rollback-refuses-a-rotten-backup.txt` |
| §4.8 | `LATEST` names the real `unique_dir` basename (`-2` on a collision); `mkdir`/`swap.log` moved after `require_stopped`; the sidecar `find -delete` dies with a step name; help documents the engine-argv priority, exit 2 and Linux-only; the sha-mismatch is a counted `ck_warn` | `rework2-latest-collision.txt`, `rework2-refused-backup-writes-nothing.txt` |

Mutants, on a scratch tree (`git archive HEAD` + this tree's two scripts, baseline `PASS 16/16`,
script sha identical both sides), each now red:

| # | mutant | suite | transcript |
|---|---|---|---|
| M1 | `BACKUP-INFO` written to `/dev/null` | **FAIL 15/16** — case 09 "no BACKUP-INFO completeness marker" | `rework2-mutant-M1.txt` |
| M2 | backup-root-outside guard deleted | **FAIL 15/16** — case 14 (rc 1 on the process check without the OUTSIDE message; with OpenCode down it creates the in-store root instead) | `rework2-mutant-M2.txt` |
| M3 | `.engine.lock` live-pid refusal deleted | **FAIL 15/16** — case 15 rc 0 where rc 1 is required | `rework2-mutant-M3.txt` |

Rehearsal (fresh copy: `.tmp/scratch/rework2-rehearsal`, `cp -a ~/.opencode-mem` + a fake config, 624 MB,
151 files, 29 DBs, `metadata.db` v1):

1. `--backup --rehearse --yes` — 29 checkpoints, manifest verified, 29/29 integrity, `LATEST` after §5;
   the closing line is the corrected "do not start OpenCode" (`rework2-rehearsal-1-backup.txt`).
2. mutation (port era, copy only): `metadata.db` 1→2, a real shard 2→3, `data/port-era.txt`.
3. `--rollback --rehearse --yes --from <TS>` — the manifest is verified **before** the write, then both
   configs restored, `data/` moved aside, `cp -a` back, `diff -r` clean; 60/60 files hash identically,
   both configs byte-identical, the aside store keeps v2/v3 + the marker, the restored store is v1/v2
   with no marker (`rework2-rehearsal-3-rollback.txt`, `-4-byte-compare.txt`).
4. `--swap --rehearse --yes` twice — the staleness warning fires before the confirmation and the
   confirmation names it; exactly one line changed; the second run reports idempotence
   (`rework2-rehearsal-5-swap.txt`).
5. a 50-byte append to the pre-swap backup's `data/metadata.db` → `--rollback --from <that TS>` exits 1
   with "the backup does not verify (§5) — not restoring" and an unchanged store/config snapshot
   (`rework2-rehearsal-6-rollback-refuses-a-rotten-backup.txt`).

## Not covered by this evidence

- `--swap`, `--backup` and `--rollback` on the **live** paths: the owner runs them (issue 81). The live
  preconditions are in `dry-run-live.txt`. `--swap` now warns **before its confirmation** that the staged
  engine (`72d7f64d…4d5e653`, 2026-10-30 21:48) is older than `plugin/dist/plugin.js` and names the
  rebuild (issue 81's first acceptance line); measured against a scratch tree in
  `rework2-rehearsal-5-swap.txt`.
- The two-workspace `memory list-shards` discriminator (§10 rows 1–2): the script prints the instruction
  because it cannot open two sessions.
