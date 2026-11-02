# 117 — episodic_milestones is empty on the live store: the candidate gates (INFERRED)

Status: ready-for-human — the settling measurement opens the LIVE store, which the coder may not do.
The work this file names is two read-only queries plus the engine's own consolidation counters:
`SELECT MIN(created_at), COUNT(*) FROM agent_sessions WHERE deleted_at IS NULL` against the live
`episodic.db` (read-only, `file:…?mode=ro`), and the engine's `episodic.consolidationAgeDays` value;
run by the owner or a `debugger` session, never by the coder against the live root.

Filed by `.scratch/web-identity-and-project-picker` (defect B's second half). **Not** fixed by that
wave: D-7 says diagnose and file.

## Observation (MEASURED, 2026-11-02 ~16:50Z, read-only GETs against the live engine)

    GET /api/projects/%2Fhome%2Fvaz%2FCode%2FPersonal%2FRust%2Fopencode-agents-memory/milestones
      → 200 {"success":true,"data":[]}
    GET /api/projects/%2Fhome%2Fvaz%2F.config%2Fopencode/sessions
      → 200 with real rows

So the routing and the percent-encoded paste flow work; the milestone table itself is empty while the
session table is not. `/agents` therefore renders the new empty state for every project.

## What is NOT the cause

**A production writer exists** — this corrects an earlier claim that none does. `consolidation.rs:514`
writes milestones, driven hourly by `ConsolidationScheduler` (`engine.rs:1638-1646`,
`CONSOLIDATION_INTERVAL_SECS = 3600`, constant at `engine.rs:1796`). A silent absence of a writer is
ruled out; the question is which gate keeps the job from producing rows.

## Candidate causes (INFERRED — none measured on the live store)

1. **The age window.** `episodic.consolidationAgeDays` defaults to 30
   (`crates/mem-core/src/config/schema.rs:630`). A session becomes eligible only once it is OLDER than
   that window, so a store whose sessions are recent produces nothing until 30 days have passed.
2. **The cross-tier promotion gate.** A milestone is written together with its promotions, and
   `promotion_due` gates the phase; a failing promotion path (e.g. a provider that does not answer, or
   the promotion pipeline flag being off) can make the phase skip rather than write.
3. **The per-agent claim.** Consolidation claims sessions per `(project_id, agent_id)`; a claim shape
   mismatch (agent ids that never repeat, or a zero claim cap) would yield no work even with eligible
   rows.

Both (1) and (2) are consistent with every measurement above. Nothing in this file distinguishes them.

## The measurement that settles it

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
