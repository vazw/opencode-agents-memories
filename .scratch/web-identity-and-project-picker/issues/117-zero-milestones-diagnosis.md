# 117 — episodic_milestones is empty on the live store: the age window, measured

Status: measured — the age window alone explains the empty table on this host (2026-11-03T06:16:41Z,
read-only); no code change indicated. The run is recorded under **Measurement** below; nothing here
asks a human to do anything.

Filed by `.scratch/web-identity-and-project-picker` (defect B's second half). **Not** fixed by that
wave: D-7 says diagnose and file — and the diagnosis turned out to need no fix.

## Observation (re-attested read-only at 2026-11-03T06:17:22Z)

    GET /api/projects/%2Fhome%2Fvaz%2FCode%2FPersonal%2FRust%2Fopencode-agents-memory/milestones
      → 200 {"success":true,"data":[]}
    GET /api/projects/%2Fhome%2Fvaz%2F.config%2Fopencode/sessions
      → 200 with real rows (newest createdAt 1793684858627)

So the routing and the percent-encoded paste flow work; the milestone table itself is empty while the
session table is not. `/agents` therefore renders the new empty state for every project.

The wave's overview filed the same two GETs first; its instant (`~16:50Z` on 2026-11-02) is
**UNATTESTED** here — no clock was captured with that run. The re-run above is what dates the claim.

## What is NOT the cause

**A production writer exists** — this corrects an earlier claim that none does. `consolidation.rs:514`
writes milestones, driven hourly by `ConsolidationScheduler` (`engine.rs:1638-1646`,
`CONSOLIDATION_INTERVAL_SECS = 3600`, constant at `engine.rs:1796`). A silent absence of a writer is
ruled out. A second writer exists too — the portability **import** path
(`crates/mem-portability/src/service.rs:606`) — which is why an imported root is never in this state.

## Measurement (MEASURED, 2026-11-03T06:16:41Z, read-only)

The clock travels with the query, so the label below cannot drift from the run:

```sh
sqlite3 'file:/home/vaz/.opencode-mem/data/episodic.db?mode=ro' \
  "SELECT MIN(created_at), MAX(created_at), COUNT(*), \
          CAST(strftime('%s','now') AS INTEGER)*1000 AS now_ms \
     FROM agent_sessions WHERE deleted_at IS NULL;"
→ 1793563141278 | 1793684858627 | 130 | 1793686601000      -- now_ms = 2026-11-03T06:16:41Z

cutoff (now_ms − 30 d) = 1791094601000                     → MIN(created_at) > cutoff
sqlite3 'file:…?mode=ro' 'SELECT COUNT(*) FROM episodic_milestones;'   → 0
sqlite3 'file:…?mode=ro' 'SELECT COUNT(*) FROM agent_sessions WHERE deleted_at IS NOT NULL;' → 0
```

The oldest live session is 1.43 days old and the newest is minutes old, so **none of the 130 is
eligible**: the age window alone produces the empty table. Soft-deleted rows are zero, so nothing is
being hidden by the live-session filter either.

**Disposition:** cause **(1)** is confirmed. Causes **(2)** and **(3)** are **not implicated on this
host** — (2) because the promotion pipeline is off, so no gate is installed (below); (3) because
eligibility, not the claim shape, is what fails. The count and the newest stamp move with live traffic
(127 / 1793662774787 in review-4's run, whose own query clock read `1793683644415` =
2026-11-03T05:27:24Z; 130 / 1793684858627 at the 06:16:41Z run above), so the runs are told apart by
their clocks rather than by a shared label; the **oldest** stamp is the number that matters and it is
identical in both.

## Candidate causes, as filed (INFERRED at the time; resolved above)

1. **The age window.** `episodic.consolidationAgeDays` defaults to 30
   (`crates/mem-core/src/config/schema.rs:630`). A session becomes eligible only once it is OLDER than
   that window, so a store whose sessions are recent produces nothing until 30 days have passed.
   **Confirmed by the measurement above.**
2. **The cross-tier promotion gate.** A milestone is written together with its promotions, and
   `promotion_due` gates the phase. The **flag-off** install is `NoopCrossTierTask`
   (`crates/mem-bridge/src/cross_tier.rs:190-193`), whose `promotion_due` returns **`true`**
   (`crates/mem-episodic/src/cross_tier.rs:96-101`) — a flag-off install can never skip, so the earlier
   wording here ("the promotion pipeline flag being off can make the phase skip") was backwards. Only
   the **flag-on** `FactsCrossTierTask` gates, on `updated_at <= now − promotion.ttlFallbackDays`
   (default 7; `crates/mem-bridge/src/cross_tier.rs:106-113`), and a not-due agent writes nothing at all
   — including its milestone — because the pure due check precedes the single write batch
   (`crates/mem-episodic/src/consolidation.rs:427-431`, then the batch at `:452-468`).
   **Not installed on this host:** `~/.config/opencode/opencode-mem.jsonc` carries no
   `promotionPipeline` key, so the default `false` (`crates/mem-core/src/config/schema.rs:632`) applies
   (`/home/vaz/.opencode/` does not exist).
3. **The per-agent claim.** Consolidation claims sessions per `(project_id, agent_id)`; a claim shape
   mismatch (agent ids that never repeat, or a zero claim cap) would yield no work even with eligible
   rows. **Not reachable here** — there is no eligible row to claim.

## If it recurs: the same measurement, and the prohibitions

Run read-only against the live storage root:

```sh
sqlite3 'file:<storageRoot>/episodic.db?mode=ro' \
  'SELECT MIN(created_at), COUNT(*) FROM agent_sessions WHERE deleted_at IS NULL;'
```

- If `MIN(created_at)` is younger than `now - 30 days`, cause **(1)** is confirmed and the remedy is a
  configuration choice (`episodic.consolidationAgeDays`), not a code change.
- If there are eligible (older) rows and still no milestone, causes **(2)/(3)** are live; the next step
  is the engine's consolidation counters/log lines for the last hour, which names the phase that
  aborted.

Never write to the live store, never open it with a schema-upgrading build, and never restart or signal
the live engine to force a consolidation pass (its `SIGTERM` path is the grinder's, not a diagnosis
tool).
