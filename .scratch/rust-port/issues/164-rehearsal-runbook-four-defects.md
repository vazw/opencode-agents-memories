# 164 — the drop-in rehearsal runbook has four defects that make its own evidence uncollectable

Status: landed — the four cells corrected and individually tested; AC1 (one uninterrupted four-cell rehearsal) still open
Filed: 2026-11-25, from the copy-first execution of issue 28 (run `20261125T035152Z`)

## The defects

The rehearsal ran, but four of its own steps could not do what the runbook says they do. Each was worked around
during the run and recorded in the transcript; none of the workarounds is the fix.

### 1. `§2a` — `sqlite3 -readonly … .backup` fails with `SQLITE_BUSY` on every database

MEASURED: `SQLITE_BUSY` on all 43 databases. `-readonly` does not make `.backup` read-only from the source's point of
view, so a live writer holds it off. The run substituted a db+`-wal` unit copy plus a scratch WAL replay, validated
three ways (integrity `43 ok/0 bad`, per-shard multiset against the live API, byte-exact by-id read).
`immutable=1` was rejected: it returns the stale pre-`-wal` base, which is the wrong content, not a stale read.

Fix shape: the runbook should copy the db and its `-wal`/`-shm` as one unit and replay the WAL in the scratch
directory, which is what the run actually did. State it as the method, and say why `-readonly .backup` and
`immutable=1` are both wrong.

### 2. `§6a` — the asset grep matches nothing in this build

MEASURED: `assets/[^"]*\.js` returns no match because this build emits root-relative hrefs. The run's
`asset=200` line was the index refetched, not an asset read — a criterion that passed for the wrong reason.

Fix shape: extract the asset path from the index's own markup and assert the fetched bytes differ from the index.

### 3. `§6b` — `scripts/smoke-rest-surface.sh` does not create its `--evidence-dir`

MEASURED: first run `rc=2` with a false `STOP: roster is empty`, while `GET /api/projects` returned 25 projects.
After `mkdir -p` the script ran `rc=0`, 11 GET rows all 200. So the script reports a data failure when it has a
path failure.

Fix shape: the script creates its evidence directory, and a missing directory exits with a distinct message and
code rather than being reported as an empty roster.

### 4. `§8` — the restore expectation is inverted

MEASURED: the runbook expects "TS entry back = 1" after the restore. The restore target *is* the port-installed live
config, so the correct post-restore value is `0` with an empty diff. As written the step fails on success.

Fix shape: invert the expectation and say what it is comparing — the scratch config against the live config, not
against an upstream entry that no longer exists.

## Why these matter

Each one is a criterion that can pass while measuring nothing, or fail while everything works. The rehearsal's
whole value is that its cells are trustworthy; two of these four (1 and 2) produced evidence that had to be
relabelled after the fact, and the transcript records both corrections.

## Acceptance criteria

1. A rehearsal run from a clean checkout executes `§2a`, `§6a`, `§6b` and `§8` with no manual substitution, and each
   cell's recorded output is the thing it claims to measure. **NOT YET MET** — the four cells were each executed in
   isolation against stubs and a scratch engine; no single uninterrupted run has exercised all four in sequence. That
   run is the remaining measurement, and it needs a store copy plus a scratch engine on a spare port.
2. `§6a`'s asset assertion fails when the fetched asset is byte-identical to the index. **MET** — same-bytes stub →
   `ASSET_IS_INDEX STOP` rc 1; a 500-with-body now stops at `ASSET_HTTP` rc 1 before any byte comparison.
3. `§6b` exits with a distinct code and message when its evidence directory is absent, and never reports
   `STOP: roster is empty` for a path error. **MET** — absent / `ENOTDIR` parent / path-is-a-file → rc 3
   `EVIDENCE-DIR:` (the first two share one message by ruling, review-2 §1); present+`0500` → rc 3 `is not writable`;
   empty roster → rc 2 `roster is empty`.
4. `§8` passes against the port-installed config and fails if the scratch config differs from it. **MET** — identical
   pair `diff`/`cmp` rc 0 with `restore_is_identity_with_live_config`; one appended line → `diff` rc 1 (`317a318,319`),
   `cmp` rc 1.

## Inverted world

Closed as *measurement, not a deliverable* if a clean run shows one of the four is unreachable on any host (for
example, if `§2a`'s live writer can never contend on a supported path) — recorded, not worked around.

## Not in scope

- Issue 28's own verdict (the copy-first phase ran; its two failing cells are separate).
- Issue 165 (the search recall finding).

## Wave landed 2026-11-25 — blueprint `073462ef9d95`, review-2 APPROVED

Two files changed against `bf15e14`: `docs/runbooks/parity-drop-in-rehearsal.md` and `scripts/smoke-rest-surface.sh`.
No new files. Gate history: plan rejected twice (round 1 on seven items, round 2 on one numeral), approved round 3;
implementation rejected once (a documented message no execution had ever produced, plus a fail-open security control
and an unbounded HTTP status), approved on re-review.

What each cell now does, and the proof:

- **§2a** — `-readonly … .backup` is gone. The cell copies the db and its `-wal` as one unit and replays the WAL in the
  scratch dir. All three `immutable=1` opens are `mode=ro`; the "why not" paragraph blames the turso engine's exclusive
  hold of the live root, labelled REPORTED, and records both counter-probes (a stock WAL writer leaves `.backup` at
  rc 0; an fcntl holder gives `unable to open database file`). Proof: db-only copy → `no such table: t`; unit copy with
  a held WAL → `1|42`, `unitcopy_failures=0`.
- **§6a** — the asset path is extracted from the index's own markup, the fetched bytes must `cmp`-**differ** from the
  index, extraction-empty is a STOP, and the status is bound **before** any byte comparison. Proof: 500-with-body →
  `ASSET_HTTP STOP` rc 1 (previously rc 0 printing `asset_differs_from_index=yes`); same-bytes → `ASSET_IS_INDEX` rc 1;
  200-and-differing → rc 0.
- **§6b** — refuse-on-absent. Proof: absent → rc 3, creates nothing; `ENOTDIR` parent → rc 3; path-is-a-file → rc 3
  `is not a directory`; present+`0500` → rc 3 `is not writable`; empty roster → rc 2. `full_run`'s own `mkdir -p`
  untouched.
- **§8** — post-restore expectation is `0` entries and an empty diff against the port-installed config, named as the
  comparison target. Proof: identical pair rc 0 both ways; one appended line → `diff` rc 1, `cmp` rc 1.
- **Pids** — an argv-exact `live_pid()` replaces eight literals and a `pgrep` count. Proof: `grep -c '4719'` = 0;
  `live_pid | wc -l` = 1; the naive substring scan returns 2 and is what the argv-exact form defeats.

**Security, carried from this morning's incident.** §6a's temp files now live in `$REH/tmp`, a tree §13 never copies,
and a token gate sits before the evidence tree can be staged. The gate was proven by planting a token: it refuses with
`TOKEN-IN-EVIDENCE STOP` naming the file and stages nothing. Reason: the mem-web index embeds
`window.__OPENCODE_MEM_TOKEN__`, and a capture once committed one (`run-20261125T035152Z/index.html`, redacted in
`bf15e14`). Cleanup-after-use loses on SIGKILL; containment does not.

**Three non-blocking findings remain open**, each with the reviewer's own fix shape, and none affects a criterion:
the handoff's line anchors are stale after the last edit; §13's gate treats a `grep` scan failure (rc 2) as "no hits",
which is fail-open on scan errors; and the `is not a directory` message is the one path message no selftest case pins.
