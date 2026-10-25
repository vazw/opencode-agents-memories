# 51 — Tracker · Issue 40's status still says `ready-for-agent` although its code merged

Status: needs-triage — 2026-10-25. **A merged feature is still advertised as unstarted, and the same audit
that found it also mis-attributed the `/api/cleanup` 501 to it — so the misreading is already spreading.**

## What to build

A tracker correction, not code. Three parts:

1. **Issue 40's `Status:` line is wrong.** `40-dedup-service-memories.md:3` says
   `Status: ready-for-agent — 2026-10-25 (grilled; decisions below are closed. …)`. The endpoint merged as
   **`6bf7b28`** (`feat(mem-web): real POST /api/deduplicate behind MaintenancePort (issue 40)`).
2. **The 501 attribution is corrected in the same file.** The stub that survives is `POST /api/cleanup`
   (`crates/mem-web/src/api/maintenance.rs:229`, D-15) — **issue 39's**, not 40's. `POST /api/deduplicate`
   is a real handler (`maintenance.rs:245`, registered `crates/mem-web/src/api/mod.rs:180-186`) whose 501
   comes only from the *unavailable port* a default/fake wiring injects, not from a leftover stub.
3. **The residual is named rather than implied.** `ProductionDeduplication::new` **is** called — inside
   `production_deduplication_port` (`crates/mem-web/src/api/deduplication/mod.rs:264-271`) — but that
   factory has **no production caller**: its only call site is a test
   (`crates/mem-web/tests/dedup_endpoint.rs:33`), and `crates/mem-web/src/api/dto.rs:493` only mentions it in
   a doc comment. So the honest status is "endpoint and service landed; the port is not wired into the
   composition root" — which is a claim nobody can check from the status line today.

## Why it is worth an issue at all

This is the **fourth** instance of one defect class found on 2026-10-25: *a declared surface disagreeing with
reality*. C4 is 45 config keys that describe behaviour nobody implemented; C2 is a deferral chain pointing at
a lane that never existed; C3 is an erasure promise that misses an artefact; this is a status line that says
"unstarted" about merged code. The other three are getting issues — this one is cheap, and a stale status is
what the next audit will re-derive from scratch.

Note what the stale line has already cost: the design-concerns audit itself read issue 40 as unstarted and
concluded *"the route is still `maintenance::deduplicate` → 501"*, conflating it with `/api/cleanup`. That
error is recorded and corrected in the brief's own correction block
(`.tmp/drafts/20261025-design-concerns-audit-brief.md:42-58`). The status line is the root of it.

## Acceptance criteria

- [ ] **51.1** `40-dedup-service-memories.md`'s `Status:` line names the landing commit `6bf7b28`, states
      what landed (handler + `MaintenancePort` implementation + `production_deduplication_port`), and states
      the residual (factory has no production caller). It does **not** say `done` unless the wiring is
      either in the tree or explicitly moved to another issue, which must be named.
- [ ] **51.2** The comments record the two corrections with evidence: the 501 belongs to `POST /api/cleanup`
      (issue 39, D-15), and `ProductionDeduplication::new` is called while the *factory* is uncalled — the
      distinction the audit lost.
- [ ] **51.3** A **bounded** sweep of the other issues for the same defect, with a stated method: for each
      issue whose status claims `ready-for-agent`, check whether its named deliverable is already in the
      tree; for each claiming `done`, check that it names a commit or a review outcome. The sweep's output
      is a list in this issue — fixed here, or recorded as "no change needed" **with the reason**. Two
      classes are explicitly **not** findings: issues still open with some waves landed (e.g. **43**, whose
      W0/W1 landed while the issue is correctly open) and issues paused with a pause note (**39**).
- [ ] **51.4** One paragraph added to `docs/agents/issue-tracker.md` so the next issue cannot repeat it: a
      status that claims readiness names the symbols or files it will create; a status that claims landing
      names the commit. No other tracker conventions change.

## Out of scope

- Re-designing issue 40 or re-deciding its merged decisions. This issue touches status and comments only.
- Whether `production_deduplication_port` *should* be wired, and by whom (issue 37's composition root or a
  follow-up). Naming the owner is in scope; doing the wiring is not.
- Any edit to `crates/` or `src/`.

## Blocked by / Blocks

- **Blocked by:** nothing.
- **Blocks:** the credibility of every other status line in the directory, in the weak sense that a reader
  who has been burned once stops trusting the field.

## Comments

- Filed 2026-10-25 from the design-concerns brief's **correction block**, not from the concerned list:
  `.tmp/drafts/20261025-design-concerns-audit-brief.md:42-58` records the correction to the audit's own
  issue-40 finding and says a stale status *"is genuinely worth an issue … and it is cheap to fix."* The
  audit proper (`REPORT.md` §3.3, §7) does **not** list it; it is the brief's addition and is filed on the
  brief's authority.
- **Facts asserted here were spot-checked in the tree at `551dca1`:** handler
  `crates/mem-web/src/api/maintenance.rs:245`; route registration `crates/mem-web/src/api/mod.rs:180-186`;
  cleanup 501 stub `maintenance.rs:229`; factory `crates/mem-web/src/api/deduplication/mod.rs:264`; factory
  callers `grep -rn production_deduplication_port` → definition, `dto.rs:493` (doc), `tests/dedup_endpoint.rs:33`.
  The `6bf7b28` commit is quoted from the brief and from issue 40's own text; it is **not** in this
  session's `git log` window, so **51.1 must re-confirm it with `git show --stat 6bf7b28` before writing it
  into the status line**. If the hash is wrong, the status line carries the right one, not this issue's.
- Also corrected here, and not to be re-filed: the audit's *"issue 40 is `ready-for-agent` with a 501 route
  and `ProductionDeduplication::new` uncalled"* is wrong in two of three clauses — see 51.2.
