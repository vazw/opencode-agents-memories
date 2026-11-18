# The scratch `episodic.db` is unreadable while the seed engine lives (issue 145)

Recorded 2026-11-19T00:05Z. This is why Step 0c's live `capture_inbox` read failed twice
in two separate runs, and why no retry or timeout can fix it.

## The observed failure

Two independent runs, same string:

```
Parse error in 3rd command line argument: database is locked (5)
```

- run r24: `sqlite3 -readonly .tmp/root-145/episodic.db "SELECT status, COUNT(*) FROM capture_inbox GROUP BY status;"`
- run r29: the same read through the blueprint's `sq()` helper, `-cmd ".timeout 5000"`, three attempts.

## The mechanism, reproduced in scratch

The engine opens the store with `locking_mode=EXCLUSIVE`, so the file lock is held for the
**connection's lifetime**, not for the duration of a transaction. A controlled reproduction
in `.tmp/scratch/wal-probe/` (`hold.py`, a throwaway WAL database):

| holder | reader | result |
| --- | --- | --- |
| `BEGIN IMMEDIATE`, WAL | `sqlite3 -readonly` | `1`, rc 0 — readers are **not** blocked by an open write transaction |
| `PRAGMA locking_mode=EXCLUSIVE` + `BEGIN IMMEDIATE` | `sqlite3 -readonly` | `Parse error in 3rd command line argument: database is locked (5)`, rc 1 |
| `PRAGMA locking_mode=EXCLUSIVE` + `BEGIN IMMEDIATE` | `sqlite3` (writable) | `Parse error in 2nd command line argument: database is locked (5)`, rc 1 |

The exact error string the wave hit appears only in the exclusive rows. So:

1. The cause is `locking_mode=EXCLUSIVE`, not WAL and not the `-readonly` flag.
2. A **writable** connection fails identically, so switching off `-readonly` does not help either.
3. Raising `busy_timeout` cannot help: the lock is not released between transactions, only when
   the engine process exits.

## Consequence for Step 0c

An "immediate" read while the seed engine lives is **impossible by construction**. Any plan that
gates on one is unrunnable, and any plan that treats its failure as `OPEN — inbox state unreadable`
closes the wave for a reason that has nothing to do with the seed.

## The discriminator is still available, after exit only

The row's own timestamp answers the question the live read was for. Measured on the r29 seed:

- seed row `agent_sessions.created_at` = `1795021010965` = `2026-11-18T16:56:50.965Z`
- engine `exit 30456` at `2026-11-18T16:57:21Z`

The row was persisted **30 s before** the engine exited, so it came from the capture-time
`drain_once` (`crates/mem-bridge/src/engine.rs:672-676`), not from the teardown flush
(`:1773`). `capture_inbox` was empty afterwards, confirming the drain.

So: record the capture-answer wall clock and the engine exit stamp, then after exit compare
`created_at` against the exit time. A gap of tens of seconds is the capture-time drain; a gap
of a second or two is the teardown flush. Both are legitimate; neither is a failure. The
classification is a reading, and only the post-exit row count is a gate.

Note the column is `created_at` (INTEGER, epoch **milliseconds**), not `time_created` —
`agent_sessions` has `created_at` / `updated_at` / `deleted_at`; `time_created` belongs to the
host's `session_message` table in `opencode.db`, a different schema entirely.
