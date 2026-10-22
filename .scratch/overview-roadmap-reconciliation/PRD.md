# PRD: Reconcile OVERVIEW.md with roadmap verified-research amendments

Status: ready-for-agent

## Problem Statement

`OVERVIEW.md` is the **scope** document for the Rust port (5 epics, user stories US x.y), and `roadmap.md` declares `Scope authority: OVERVIEW.md`. In practice the roadmap has absorbed a series of **verified-research amendments** (§0b D1–D5, R4, R11) and **shipped-wave addenda** (W3.3 R1–R13) that OVERVIEW never received. The result is two documents that call each other authoritative yet disagree on concrete points, so a planner, syseng gate, or implementer can cite either source and get contradictory answers.

Two contradictions are load-bearing:

1. **Storage crate.** OVERVIEW's Target Rust Stack recommends "`libsql` / `turso` crate" (L6). Verified research (roadmap §0b D1) rules `libsql` out — its engine build has no vector support — and commits to the `turso` crate 0.8.x only. OVERVIEW still names a non-viable crate.
2. **Web UI primacy.** OVERVIEW US 5.1 frames the upstream Svelte frontend as the primary consumer ("works seamlessly against the Rust server"). Roadmap §0b D5 reframes the port's own **Leptos/Thaw CSR** app as the sole UI deliverable, with the REST contract as its API contract; the Svelte compat line is removed entirely. The roadmap even records "OVERVIEW US 5.1 amendment" — but OVERVIEW was never amended.

Two further gaps: OVERVIEW's Epic 1 conflates storage-side and wiring-side scope that the roadmap splits across Phase 4 (storage) and Phase 5 (capture/context injection); and OVERVIEW omits the roadmap's Phase 7 deferred items and the verified-research amendment set entirely.

## Solution

Amend `OVERVIEW.md` so it reflects the roadmap's verified-research rulings and shipped-wave reality, and drop the "authority" label from OVERVIEW so the two documents no longer contradict each other. No code, manifest, or blueprint changes — documentation reconciliation only. The amendments are mechanical text edits plus one cross-reference line; nothing in the already-approved W3.3/W4.1/W4.2 blueprints is affected (they cite the roadmap and ADRs, not the stale OVERVIEW lines).

## User Stories

1. As a Rust port implementer, I want `OVERVIEW.md` to name the same storage crate the roadmap commits to, so that I do not start a wave on a non-viable dependency.
2. As a plan-gate reviewer (syseng), I want the scope (epics/US) and the decision authority (stack/sequencing/addenda) to be clearly separated and owned by distinct documents, so that a blueprint citing one document cannot be contradicted by the other.
3. As a web-explorer implementer, I want `OVERVIEW.md` US 5.1 to state that the port ships its own Leptos/Thaw UI as the sole UI deliverable, with the REST contract as its API contract, so that no Svelte-compat framing remains to be mistaken for a deliverable.
4. As a Phase 4/5 planner, I want Epic 1 to distinguish storage-side scope (schema, queries, consolidation) from wiring-side scope (capture, context injection), matching the roadmap's Phase 4/5 split.
5. As a future maintainer, I want `OVERVIEW.md` to point at the verified-research amendments (§0b) and shipped-wave addenda, so that the scope stays current without duplicating the roadmap.
6. As the orchestrator, I want a single, internally consistent scope document, so that parallel agents dispatched against `OVERVIEW.md` and `roadmap.md` cannot diverge.
7. As a documentation reviewer, I want the reconciliation to be verifiable by cross-reference, so that drift is detectable in CI or by a quick grep.
8. As a triage maintainer, I want the reconciliation tracked as a `ready-for-agent` issue, so that it can be picked up by an AFK agent alongside other doc waves.

## Implementation Decisions

### D1 — Storage crate (OVERVIEW.md, Target Rust Stack)

- Replace "`libsql` / `turso` crate" with "`turso` crate 0.8.x" and add a one-line pointer: "verified research: `roadmap.md` §0b (D1, and the deferred escape hatch in D3)."
- Do not enumerate §0b's decisions or the escape hatch in OVERVIEW — point only, to avoid duplication drift.
- Do not change the other stack entries (`tokio`, `ort`, `axum`/`actix-web`, `serde`/`jsonc_parser`).

### D2 — Web UI primacy (OVERVIEW.md, US 5.1)

- Delete the "upstream Svelte frontend works seamlessly against the Rust server" acceptance criterion entirely. The port's own Leptos/Thaw explorer (ADR-0003, `web-ui`) is the sole UI deliverable; the REST contract is its API contract (hand-written, un-shimmed), not a Svelte compat guarantee.
- Keep the two REST endpoints (`GET /api/projects/:project_id/sessions`, `DELETE .../sessions/:session_id`) and the `axum` port list.
- Add a pointer to `roadmap.md` §0b D5 and `docs/adr/0003-web-explorer-ui-stack.md`.

### D3 — Epic 1 scope split (OVERVIEW.md, Epic 1)

- Add a "Scope split" note under Epic 1 pointing to roadmap Phases 4 (storage) and 5 (capture/context-injection wiring). Keep US 1.1 as a single story with its existing acceptance criteria; only clarify that 1.1's capture hooks land in Phase 5.
- Do not split into sub-numbered stories (1.1a/1.1b) and do not renumber US 1.2–1.5.

### D4 — Authority claim (roadmap.md, header; OVERVIEW.md heading)

- Drop the word "authority" from OVERVIEW.md entirely: rename its scope heading to "Scope: epics & user stories (US x.y)" and remove any "scope authority" label from its body.
- Narrow the roadmap header to: `Scope: OVERVIEW.md for epics and user stories (US x.y); roadmap.md is authoritative for stack decisions (§0b), sequencing, and shipped-wave addenda.` The single, narrowed authority claim lives only in the roadmap header.
- Add a one-line pointer at the top of OVERVIEW.md: `> Scope: epics & user stories (US x.y). Verified-research amendments, stack decisions, and sequencing: roadmap.md §0b.` No enumeration (avoids duplication drift).

### D5 — Phase 7 / deferred items (OVERVIEW.md)

- Add a one-liner: `> Deferred to hardening (Phase 7): auto-update, i18n, request-timeout polish, bundle-boundary/loader-contract parity audit, drop-in rehearsal — see roadmap §Sequencing 7, W7.1–W7.2.`
- Do not expand these into user stories and do not add a new section type — they remain roadmap-scoped; the one-liner keeps OVERVIEW's epic/US structure intact.

### Seams and verification

- This is a documentation-only change: the seam is the document boundary (OVERVIEW.md sections + roadmap.md header). No code, crate, or manifest seam is introduced.
- Verification: (a) `git diff --stat` shows only `OVERVIEW.md` and `roadmap.md`; (b) every roadmap §0b decision (D1–D5) has a corresponding OVERVIEW pointer or amendment; (c) no broken relative links introduced; (d) the two previously-contradictory lines (storage crate, Svelte primacy) are gone or explicitly superseded; (e) a W7.2 parity-matrix row records "Doc authority consistency: OVERVIEW.md ↔ roadmap.md §0b (storage crate, UI primacy, Epic 1 split, authority claim) agree; zero unexplained red cells."
- Commit granularity: single atomic commit (all amendments in one diff).
- Prior art: the W3.3/W4.2 blueprints and their `CONTEXT.md`/ADR amendments follow the same "amend the doc, don't rewrite" pattern (e.g. ADR-0003, ADR-0005 addendum).

### Out of scope

- Any code, crate, manifest, or test change.
- Rewriting the roadmap's decisions or re-running any pipeline wave.
- Adding new user stories beyond the Epic 1 split clarification.
- Touching `.scratch/rust-port/issues/*` or the existing `PRD.md` (Phases 2–6 design decisions) — those remain authoritative for their scope.

### Further Notes

- The W3.3 and W4.2 waves are already closed (APPROVED) and their blueprints cite the roadmap and ADRs, not the stale OVERVIEW lines, so this reconciliation cannot invalidate any approved plan.
- After this PRD is actioned, OVERVIEW.md should be re-read by any agent whose spawn prompt cites it as scope authority; the reconciliation does not change epic/user-story numbering, so existing issue references (e.g. `issue #14`, `issue #16`) remain valid.
- Canonical memory references for the underlying decisions: storage crate ruling and UI stack ruling are recorded in `roadmap.md` §0b (D1, D5) and `docs/adr/0003-web-explorer-ui-stack.md`.
- Resolved design decisions (grilling, 2026-10-19): Q1 narrow split (D4); Q2 delete Svelte AC (D2); Q3 note + pointer, no renumber (D3); Q4 drop 'authority' from OVERVIEW (D4); Q5 D1 + general §0b pointer (D1); Q6 manual + W7.2 row (Seams/verification); Q7 single atomic commit (Seams/verification); Q8 no touch to rust-port/PRD.md (Out of scope); Q9 one-liner pointer (D4); Q10 one-liner Phase 7 note (D5).