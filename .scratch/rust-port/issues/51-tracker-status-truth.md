# 51 — Tracker · Issue 40's status still says `ready-for-agent` although its code merged

Status: done — 2026-10-25 (commits `ce9e0b5` and `5e743cc` together carry every correction this issue
lists; the 51.3 sweep, its method and its evidence are in the sections below)

## What to build

A tracker correction, not code. Three parts:

1. **Issue 40's `Status:` line is wrong.** `40-dedup-service-memories.md:3` says
   `Status: ready-for-agent — 2026-10-25 (grilled; decisions below are closed. …)`. The endpoint merged as
   **`6bf7b28`** (`feat(mem-web): real POST /api/deduplicate behind MaintenancePort (issue 40)`).
2. **The 501 attribution is corrected in the same file.** The stub that survives is `POST /api/cleanup`
   (`crates/mem-web/src/api/maintenance.rs:234`, D-15) — **issue 39's**, not 40's. `POST /api/deduplicate`
   is a real handler (`maintenance.rs:245`, registered `crates/mem-web/src/api/mod.rs:180-188`) whose 501
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

- [x] **51.1** `40-dedup-service-memories.md`'s `Status:` line names the landing commit `6bf7b28`, states
      what landed (handler + `MaintenancePort` implementation + `production_deduplication_port`), and states
      the residual (factory has no production caller). It does **not** say `done` unless the wiring is
      either in the tree or explicitly moved to another issue, which must be named.
- [x] **51.2** The comments record the two corrections with evidence: the 501 belongs to `POST /api/cleanup`
      (issue 39, D-15), and `ProductionDeduplication::new` is called while the *factory* is uncalled — the
      distinction the audit lost.
- [x] **51.3** A **bounded** sweep of the other issues for the same defect, with a stated method: for each
      issue whose status claims `ready-for-agent`, check whether its named deliverable is already in the
      tree; for each claiming `done`, check that it names a commit or a review outcome. The sweep's output
      is a list in this issue — fixed here, or recorded as "no change needed" **with the reason**. Two
      classes are explicitly **not** findings: issues still open with some waves landed (e.g. **43**, whose
      W0/W1 landed while the issue is correctly open) and issues paused with a pause note (**39**).
- [x] **51.4** One paragraph added to `docs/agents/issue-tracker.md` so the next issue cannot repeat it: a
      status that claims readiness names the symbols or files it will create; a status that claims landing
      names the commit. No other tracker conventions change.

## Out of scope

- Re-designing issue 40 or re-deciding its merged decisions. This issue edits only its own file, the status
  lines and comments it corrects in 40 / 42 / 38 / 37, the `docs/agents/issue-tracker.md` section AC 51.4
  requires, and the two narrow exceptions below — no other file, and it re-opens no decision of issue 40's.
- Whether `production_deduplication_port` *should* be wired, and by whom (issue 37's composition root or a
  follow-up). Naming the owner is in scope; doing the wiring is not.
- **Authorized narrow exception:** the one-line annotation on issue 40's `## What to build` heading,
  marking that section as the state at filing (`## What to build (AS FILED — superseded by …)`), as this
  wave does. A heading is neither status nor a comment; this line authorizes that single heading and
  nothing else inside that section.
- **Authorized narrow exception (2):** the `Why` cell of `docs/web-skeleton-smoke.md`'s D-15 row — the
  **fourth** column, not the third — so that the row stops asserting the port has neither service, names
  `6bf7b28` as where deduplication landed, and attributes each route's 501 to its actual source. A table
  cell is neither a status nor a comment; this line authorizes that single cell and nothing else in that
  file — the transcript comment at `:294` is recorded as out of scope in the reach bullet below, not
  authorized here.
- Any edit to `crates/` or `src/`.

## Blocked by / Blocks

- **Blocked by:** nothing.
- **Blocks:** the credibility of every other status line in the directory, in the weak sense that a reader
  who has been burned once stops trusting the field.

## 51.3 sweep — 2026-10-25

Method, applied at `8a39e01`. **Readiness class:** for every issue whose `Status:` claims
`ready-for-agent`, ask `git log --format='%h %s' -F --grep="(issue NN)" -1` whether a commit already
names it; where one does, read that commit and the issue's own ACs and classify the deliverable as
landed, partial or absent. **Landing class:** for every issue whose `Status:` says `done`, require a
cited commit that resolves in history (`git cat-file -e <sha>^{commit}`) or a recorded review outcome;
the 01–04 backfill that `docs/agents/triage-labels.md` records (2026-10-18) is the sanction for those
four closing without a commit, and the rule added to `issue-tracker.md` is forward-looking, so these
closings are conformant rather than grandfathered exceptions. The two classes 51.3 excludes stay
excluded: an issue open with some waves landed, and a paused issue with a pause note.

| # | status today | what the tree says | verdict | action |
|---|---|---|---|---|
| **40** | `ready-for-agent` | `6bf7b28` merged the handler, the `MaintenancePort` implementation and the factory; the factory is uncalled by any production path | landed, residual **owned by issue 54** | status rewritten (51.1) |
| **42** | `ready-for-agent` | `e996a63` merged the Maintenance section, confirm flow and wasm tests; its backend `/api/cleanup` is issue 39, PAUSED | frontend half landed | status amended: names the commit and the paused backend |
| **38** | `ready-for-agent` | `4232934` moved `web-ui/Cargo.lock` to `tokio 1.54.1`; the AC evidence needs cargo and is not recorded in that file | deliverable landed, evidence open | status amended: names the commit and the evidence gap |
| 24 | `ready-for-agent` | AC1 ticked; AC2 pending on 26 · 37 · 29, and the line names them | open with waves landed | no change — excluded class, and the line already names what is pending |
| 29 | `ready-for-agent` | the shell slice landed with a reviewer APPROVED; ACs 3–4 named as pending on the same line | open with waves landed | no change — excluded class |
| 37 | `ready-for-agent` | AC4 and AC6 unticked; `src/main.rs` has no `.with_sessions(..)` | not landed | status unchanged; a comment records the AC6 supersession (the 2026-10-25 comment added to that file by this wave) |
| 48 | `ready-for-agent` | rescoped the same day by `8a39e01`; nothing of it is in the tree | not landed | no change |
| 26 `done` files | `done` | 25 close on a recorded review outcome (01–04 via the sanctioned 2026-10-18 backfill; 05, 08–10, 14, 16, 25 and the rest by a recorded reviewer verdict — `APPROVED` for all but 12, which records "reviewer pass approved") and one (`34`, `a96a2bc`) names a commit; `36`'s `9a523b5` is a clean-tree baseline inside a comment, not a status citation | conforms | no change — the landing rule is forward-looking |

Twenty-six `done` lines were checked and none needed a change — one names a commit, twenty-five close on
a recorded review outcome — so the defect is concentrated in the readiness label rather than in the
landing label. Three findings, one defect: a status line asserting a state the tree contradicts.

## Comments

- Filed 2026-10-25 from the design-concerns brief's **correction block**, not from the concerned list:
  `.tmp/drafts/20261025-design-concerns-audit-brief.md:42-58` records the correction to the audit's own
  issue-40 finding and says a stale status *"is genuinely worth an issue … and it is cheap to fix."* The
  audit proper (`REPORT.md` §3.3, §7) does **not** list it; it is the brief's addition and is filed on the
  brief's authority.
- **Facts asserted here were spot-checked in the tree at `551dca1`:** handler
  `crates/mem-web/src/api/maintenance.rs:245`; route registration `crates/mem-web/src/api/mod.rs:180-188`;
  cleanup 501 stub `maintenance.rs:234`; factory `crates/mem-web/src/api/deduplication/mod.rs:264`; factory
  callers `grep -rn production_deduplication_port` → definition, `dto.rs:493` (doc), `tests/dedup_endpoint.rs:33`.
  The `6bf7b28` commit is quoted from the brief and from issue 40's own text; it is **not** in this
  session's `git log` window, so **51.1 must re-confirm it with `git show --stat 6bf7b28` before writing it
  into the status line**. If the hash is wrong, the status line carries the right one, not this issue's.
- Also corrected here, and not to be re-filed: the audit's *"issue 40 is `ready-for-agent` with a 501 route
  and `ProductionDeduplication::new` uncalled"* is wrong in two of three clauses — see 51.2.
- **2026-10-25 — 40's residual, and who owns it.** Three status lines were corrected here (40, 42, 38);
  none became `done`, and none could: in all three the deliverable is in the tree while the line asserted
  otherwise. 40's residual — `production_deduplication_port` is called by nothing in `src/main.rs`, issue
  37's AC6 disclaims maintenance "to issue 39", and issue 39 is PAUSED and scoped to `/api/cleanup` — is
  **owned by issue 54** (`54-dedup-port-wiring-unreachable`), filed 2026-10-25, which scopes the
  `.with_maintenance(..)` wiring in the composition root, a reader test for the two resolved-but-unread
  config keys `deduplication_enabled` / `deduplication_similarity_threshold`, and an anti-regression test
  that the shipped maintenance port is not `Unavailable`. Filing is not scheduling: nothing here says
  when 54 runs. Naming that owner was in this issue's scope; doing the wiring is not.
- **2026-10-25 — the mis-attribution's tracked reach.** The audit's conflation of `/api/deduplicate` with
  the `/api/cleanup` 501 is corrected in three places here: issue 40's `## What to build` body (the
  `unconditional 501 stub` paragraph — marked as filed and superseded at its heading, and corrected in 40's
  two 2026-10-25 correction comments), issue 37's AC6 premise, and `docs/web-skeleton-smoke.md`'s D-15
  `Why` cell — the fourth column, not the third. It also survives in one place this wave's file scope
  does not reach: the D-15 smoke transcript's comment at `docs/web-skeleton-smoke.md:294` still reads
  "no service exists in this build — the stubs answer 501", which `6bf7b28` falsifies for
  `/api/deduplicate`; recorded here, uncorrected, so the next edit of that file fixes it. **This issue's own
  line numbers were a separate drift and are corrected in this pass** — `maintenance.rs:229` → `:234` and
  `api/mod.rs:180-186` → `:180-188`, in the body and in the comment above — because the `551dca1` HEAD that
  comment names already had the stub at `:234` and the route at `:180-188`, so the old pair was already
  stale at the HEAD that block cites; its other numbers (`:245`, `:264`, `dto.rs:493`,
  `tests/dedup_endpoint.rs:33`) were and are exact. The audit brief itself
  (`.tmp/drafts/20261025-design-concerns-audit-brief.md`) is gitignored and absent from this worktree, so
  its correction block cannot be reached by a commit — the durable copies are 40's comments, issue 37's
  comment and the D-15 cell, which is root `AGENTS.md`'s rule: a ruling that lives only in `.tmp/` is one
  `rm -rf` from gone.
