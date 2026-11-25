# 28 — W7.2 · Parity matrix & drop-in rehearsal

Status: ready-for-human — 2026-11-25 (copy-first phase executed: the store migrates and serves, with one cell
unobtainable on this machine (`S5 .legacy.bak`, migrated in place elsewhere) and one real finding filed as issue 165;
four runbook defects filed as 164. §11 live swap: ruled superseded, see `## Owner ruling 2026-11-25`. The 23 missing
matrix rows are **issue 71**. §0b D3 hatch: reconfirmed deferred. Owner must rule on the `S5 .legacy.bak` cell.)

## What to build

The final gate: a coverage matrix for **all 110 upstream `*.test.ts` files** with disposition (ported / covered-by / N-A) and zero unexplained red cells; and the end-to-end rehearsal — TS version uninstalled, Rust version installed into the same project, existing memories readable, UI functional: the port's Leptos UI works against the Rust server; REST parity additionally keeps the upstream Svelte dist usable. Closes (or explicitly confirms) the §0b D3 sqlite-vec experimental hatch. Wave detail: `.scratch/rust-port/roadmap.md` §W7.2.

## Acceptance criteria

- [ ] Parity matrix document — **moved to issue 71** (matrix completed by issue 71: 110/110; 15 rows were still missing at r1).
- [ ] Drop-in rehearsal transcript: same project dir, TS removed, Rust installed → memories intact, UI functional, capture/injection working.
- [ ] §0b D3 hatch closed or expressly reconfirmed as deferred.

## Blocked by

- 01–27 (all prior waves)

- **2026-10-27 — triaged `ready-for-human`.** The registry is real but behind: `docs/agents/testing.md`
  holds 87 upstream rows (83 port / 3 covered-by / 1 N-A) against a pinned 110-file suite, and 23 pinned files
  have no row — several with live counterparts (`plugin/tests/*.test.mjs`, the portability and skills crates),
  others genuinely unported (`v2-*`, `tui-companion`, `auto-update`, `i18n`, `runtime-require`). No rehearsal
  transcript exists under `docs/evidence/`; the hatch has a PRD reconfirmation but no closure record. Why human:
  a drop-in rehearsal is a manual install/uninstall property no test can settle, and closing the §0b D3 hatch is
  an owner ruling.

## Owner rulings 2026-10-29

**Target = copy first.** Copy the real storage root to a scratch path and run the migration against the copy
with a scratch config; the live swap is a **separate, owner-executed repeat**, and this issue closes only after
that step is recorded. Reason: the port's `legacy_migrator` renames sources to `.legacy.bak`, so the live root
is the only copy of the user's memory and must not be the test fixture.

**Evidence = a re-derivable text transcript** under `docs/evidence/issue-28-dropin-rehearsal/`, containing:

- the TS store's memory count **before**;
- the swap (`opencode-mem@latest` removed from `opencode.jsonc:171–173`, the local `plugin/` installed);
- the count **after**, plus **one recall by content** (a count alone proves a database opened, not that the data
  is the same);
- the `.legacy.bak` files, proving the migration ran;
- `curl` responses from the UI's own API (index plus a listing endpoint);
- one capture round-trip (a new prompt becomes a memory);
- one injection (the `memory_context` frame appears in a live turn).

No screenshots as primary evidence; no scripted suite — 28 is a manual install property by definition, which is
why the matrix half is issue 71.

**§0b D3 hatch = reconfirmed deferred.** Trigger table:

- **(a) SQLCipher at rest** — not triggered: the vault uses turso's experimental encryption, and issue 53
  measured it across processes.
- **(b) multi-process DB access** — **not suppressed**: the engine documents *"No multi-process access, No
  multi-threading"* and this port depends on concurrent processes (engine, host plugin, CLI). Issue 53 measured
  the unlisted combination working; that is evidence it works, **not** that it is supported.
- **(c) int8/bit vectors, ANN** — not triggered; exact scans at our row counts.

Close condition (roadmap §0b D3): *"If neither switch trigger materializes by Phase 7, close it as
considered-and-deferred."* Trigger (b) is material, so the hatch stays named. **The observation that would open
it:** observed corruption, or a functional need that requires multi-process access on a supported path. The
Phase-7 gate is "closed-or-confirmed"; this is the confirmed branch.

**Order:** after the W3 wave (issue 43) and the ADR-0019 text. The copy-first run may be executed by a subagent;
the live repeat is the owner's.

## Runbook (2026-11-18)

Written, not executed: `docs/runbooks/parity-drop-in-rehearsal.md`, with the transcript scaffold at
`docs/evidence/issue-28-dropin-rehearsal/TRANSCRIPT.md`. Every transcript cell is `NOT RUN` until the run.
The copy-first phase (§2–§8) is safe with the live engine alive: port **3749** against a copy at
`.tmp/rehearsal-28/<stamp>/store`, the live root read-only, pid 4719 never signalled. The live repeat is
§11 and is what closes AC2.


## Owner ruling 2026-11-25 — the live repeat is superseded, not deferred

Ruled in a grilling session, after both premises were measured.

**§11 (the live swap, which closes AC2) is closed as superseded by the live system.** MEASURED:
`~/.config/opencode/opencode.jsonc:216-220` lists the local `plugin/` and contains no `opencode-mem` reference at
all, and the live root carries `user-profiles.db.schema-v1-*.bak` — the migrator's rename marker. The step §11 asks
for ("remove `opencode-mem@latest`, install the local plugin") describes the state the machine has been in for
weeks. Re-running it would re-do a completed migration on the only copy of the user's memory, which this issue's
own 2026-10-29 ruling forbids.

Recording the trade: this repo will never hold a contemporaneous before/after transcript taken *across* the swap.
What it holds instead is the port running live — engine identity, inject lines, capture rows — plus, once the
copy-first phase runs, evidence that a pristine TS store migrates on a current build.

**§2–§8 (copy-first) stays in scope and is delegated to a subagent.** It is safe with the live engine alive: port
3749 against a copy at `.tmp/rehearsal-28/<stamp>/store`, the live root read-only, the live pid never signalled.
The transcript at `docs/evidence/issue-28-dropin-rehearsal/TRANSCRIPT.md` is still `NOT RUN`.

## Copy-first phase executed, 2026-11-25 (run `20261125T035152Z`)

Delegated per the ruling above, against a 651 MB copy under `.tmp/rehearsal-28/20261125T035152Z/`, engine on port
3749, the live engine (pid 28153, port 3742) never signalled. Transcript:
`docs/evidence/issue-28-dropin-rehearsal/TRANSCRIPT.md`; 54 evidence files landed at
`docs/evidence/issue-28-dropin-rehearsal/run-20261125T035152Z/`.

**What it establishes.** Count before `5602` (copy and live agree) → migration against the copy → count after
`5602`; per-shard multiset against the live API empty-`diff`; integrity `43 ok / 0 bad`; a canary round-trip
(`mem_1795578986897_02e51ee7e` back at `0.6330`, `5602→5603`); REST walk `rc=0` with 11 GET rows all `200`; UI
index/js/wasm/css all `200` with the exact title; a second engine refused (`rc=75`, `lock_lost`); one injection
with the `<memory_context>` frame. Post-run live invariants unchanged: same `lstart`, `.engine.lock` sha256,
root `%Y`, 43 databases, 0 `.legacy.bak`, live config `cmp` rc=0.

**Two cells fail, and only one of them is a defect.**

1. **S5 `.legacy.bak` = 0 — unobtainable on this machine, not a failure of the port.** `.turso-migrated` reads
   `completedAt 2026-10-30T15:14:45.697Z` with shard paths under a *different* home (`/home/vaz/.opencode-mem/data`),
   so `legacy_migrator` returns early and the `.bak` family was reaped by the erase sweep. The cell cannot be
   satisfied on a store migrated in place elsewhere. **Owner question:** accept `completedAt` plus the early-return
   as the "migration path exists" evidence, or require a rehearsal against a store that was never migrated.
2. **S5 recall rank 1 — real, and filed.** The frozen query (`97 sites across 8 crates`) did not return the
   pre-registered id anywhere in the 20 results. Not retuned, per the runbook. Filed as **issue 165** with the
   mechanism it exposed (`/api/search` pools a per-shard cap and then sorts) and the observation that would
   distinguish starvation from an honest low rank.

**Four runbook defects found by the run** — `sqlite3 -readonly … .backup` is `SQLITE_BUSY` on all 43 databases, the
asset grep matches nothing in this build, `scripts/smoke-rest-surface.sh` never creates its `--evidence-dir` and
reports a path failure as an empty roster, and §8's restore expectation is inverted. Filed as **issue 164**.

**Not run:** §9, §10, §11 (the last ruled superseded above). §13's landing was done by the orchestrator rather than
by the script; one artifact (`probe-b/x.db`, 1.3 MB) is deliberately untracked and regenerable.
