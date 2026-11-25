# 164 — the drop-in rehearsal runbook has four defects that make its own evidence uncollectable

Status: ready-for-agent
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
   cell's recorded output is the thing it claims to measure.
2. `§6a`'s asset assertion fails when the fetched asset is byte-identical to the index.
3. `§6b` exits with a distinct code and message when its evidence directory is absent, and never reports
   `STOP: roster is empty` for a path error.
4. `§8` passes against the port-installed config and fails if the scratch config differs from it.

## Inverted world

Closed as *measurement, not a deliverable* if a clean run shows one of the four is unreachable on any host (for
example, if `§2a`'s live writer can never contend on a supported path) — recorded, not worked around.

## Not in scope

- Issue 28's own verdict (the copy-first phase ran; its two failing cells are separate).
- Issue 165 (the search recall finding).
