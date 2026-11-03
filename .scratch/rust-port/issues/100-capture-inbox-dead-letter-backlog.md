# 100 — `capture_inbox` holds 480 dead-lettered captures from 2026-10-31, and nothing can replay them

Status: wontfix — **owner decision 2026-11-03: drop.** The 480 rows and their 4.8 MB of payloads are
accepted as lost; no replay, no requeue path built for them. The observation below stands as the record
of what was there, and the last acceptance criterion (make a dead set visible on its own) still wants
an owner: this decision resolves the *backlog*, not the silence that hid it.

## Observation (MEASURED 2026-11-03, read-only `file:…?mode=ro` on the live store)

```
SELECT status, count(*), min(created_at), max(created_at) FROM capture_inbox GROUP BY status;
→ dead | 480 | 2026-10-31 09:29:23 | 2026-11-01 19:48:17

SELECT attempts, count(*) FROM capture_inbox GROUP BY attempts;
→ 5 | 480                       -- every row exhausted the ladder

SELECT count(*), sum(length(payload)=0), avg(length(payload)) FROM capture_inbox;
→ 480 | 0 | 10522 bytes          -- 4.8 MB of payload retained, none empty

SELECT count(DISTINCT session_id), count(DISTINCT agent_id), count(*) FROM capture_inbox;
→ 136 | 13 | 480
```

All 480 were **created** on 2026-10-31; the last attempt was 2026-11-01 19:48. Since then the inbox
has produced nothing: no new `dead` rows, and captures are landing normally (`Auto-capture memory
persisted` 93 times, latest 2026-11-03 13:27 +07). So this is a historical backlog from one day of
provider failures, not a live fault.

`status='dead'` is terminal by design: the drain selects only `status = 'queued'`
(`crates/mem-store/src/episodic_sql.rs`, `SELECT_CAPTURE_INBOX_BATCH`), the dead-letter fires after
`MAX_ATTEMPTS` consumed failures (`crates/mem-episodic/src/inbox.rs`), and no CLI mode moves a row
back — the binary offers `purge-session`, `reembed-scope`, `reap-shards`.

## Why it matters

4.8 MB of memory that was captured and never landed: 136 sessions' worth of decisions, reviews and
technical notes. Nothing surfaced it — no metric, no log line, no API route mentions the dead set —
so it took a documentation sweep reading the table to notice.

## Options

1. **Drop.** Accept the loss. Note that no purge path exists either, so this also means writing one
   (or a one-off owner-run `DELETE`, which the fleet's rules do not let an agent do).
2. **Replay.** Flip the rows to `status='queued', attempts=0, not_before=NULL`. Mechanically cheap and
   the drain is idempotent by `capture_id`, but it runs 480 provider calls and may duplicate content
   with the memories that *did* land from the same sessions.

The interesting part is the policy, not the UPDATE: is a dead letter ever worth replaying, and what
stops a replay from double-writing? Issue 110's `not_before` work makes the queue itself durable
across restarts; it says nothing about resurrection.

## Acceptance criteria

- The chosen disposition is recorded here with its measurement, and the *other* option's cost is
  stated rather than implied.
- If replay is chosen: a supported path (mode or migration), idempotence proven against
  `capture_id`, and a count of rows replayed vs. dead-lettered again.
- Either way: `capture_inbox` gains something that makes a dead set visible on its own (a metric, a
  log line, or an API field), so the next 480 do not wait for a docs sweep.

## Decision (2026-11-03) — dropped

The owner chose **drop**: the 480 captures are accepted as lost. Nothing is replayed, and no requeue
mode is built.

What that leaves on the table, stated so a later reader does not mistake silence for absence:

- **The rows stay.** No purge path exists either, so `capture_inbox` keeps 480 `dead` rows until either
  a mode is written or the owner runs a one-off `DELETE` themselves. The fleet's rules keep an agent
  from writing to the live store, which is why this is recorded rather than executed.
- **The evidence is kept.** The observation block above is the only record of what those 136 sessions
  captured; the payloads are still on disk and still readable read-only, even though nothing will
  consume them.
- **The silence is not resolved.** A dead letter still produces no metric, no log line and no API
  field; the next backlog of this size will again wait for someone to read the table. That residual is
  deliberately left open rather than closed by this decision.
