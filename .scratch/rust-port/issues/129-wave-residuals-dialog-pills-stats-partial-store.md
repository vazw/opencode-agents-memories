# 129 — Close out the wave-1/2/3 residuals: ConfirmGate→Dialog, tag-pill retirement, stats latency, partial-store robustness

Status: ready-for-agent — 2026-11-06 (synthesized from the issue 118 wave 1–3 pipeline; all premises measured or statically verified during those waves; brief under `## Agent Brief`)

Triage: ready-for-agent

## Problem Statement

Issue 118 rebuilt the Memory Explorer's design system over three waves (theme + shell, controls + states, the three worst screens). Each wave deliberately parked small items to keep its scope honest. A user of the explorer still feels four of them:

- Destructive actions (Delete memory, Delete session, cleanup confirm) use an inline arm-then-confirm row swap instead of a real dialog. The confirm UI works but is bespoke, cannot be Esc-dismissed consistently, and diverges from the design system the rest of the app now follows.
- Memory tag pills render as raw `<button class="chip">` wrapping a Thaw `Tag`. On any memory that carries tags, the wave-2 acceptance metric "raw `<button>` = 0 on all ten routes" stops holding. The pills are also the only remaining hand-rolled interactive control.
- The `/stats` screen takes ~3.2 s to settle on a cold lane, during which users stare at a spinner. The wave-3 evidence procedure had to build a readiness gate just to avoid mistaking the loading state for the final one. No one has determined whether the latency is inherent (an expensive aggregation) or accidental (a missing index, an N+1, an unbatched scan).
- A storage root left half-initialized (some `.db` files present, others absent) answered HTTP 500 for `/api/stats` and `/api/memories` in one wave-2 lane. The engine never explained why; the wave could only prove that fresh roots answer 200. A user who hits this gets a silent broken explorer with no diagnostic.

## Solution

One close-out wave that retires every parked residual:

1. Confirmations move to Thaw's `Dialog` family (0.4.8): `open: Model<bool>`, `mask_closeable = false`, Esc close, `DialogTitle`/`DialogBody`/`DialogActions`. The existing focus-managed `ConfirmGate` reducer stays as the single state machine — only its view swaps. Focus is trapped to the dialog while open and returns to the invoking button on close.
2. Tag pills become Thaw-native interactive elements with an accessible name: either `InteractionTag`/`InteractionTagPrimary` with an explicit `type="button"`-equivalent guard, or a Thaw `Button` in the Subtle appearance carrying the tag text. The `aria-label="Filter by tag: X"` contract is preserved either way. The `.chip` bespoke rules die with the swap, census paired.
3. The `/api/stats` handler is profiled once, on a populated store: measure the handler's wall time, break it down (shard scan vs aggregation vs serialization), and either (a) fix the dominant cost if it is an obvious defect, or (b) record the measurement and mark the latency inherent, with the Loading state staying as the honest UI. A "measurement that dissolves the work" is a valid outcome.
4. Partial-store startup becomes diagnosable: when the engine opens a storage root missing expected databases, it logs which files it found and which it created (or refused), at a visible level, and the API answers a structured 503-style payload instead of an opaque 500 where the handler can detect the condition. No migration, no auto-repair — detection and honest reporting only.

## User Stories

1. As an explorer user, I want delete confirmations in a proper dialog, so that I can Esc out of a destructive action I started by accident.
2. As an explorer user, I want the dialog to trap focus while open, so that tabbing cannot reach the page behind a decision I have not made.
3. As an explorer user, I want focus returned to the button I clicked after the dialog closes, so that my keyboard position is not lost.
4. As an explorer user, I want delete confirmations to look like the rest of the app, so that the interface feels like one system rather than stitched parts.
5. As an explorer user, I want cleanup's armed state to use the same dialog pattern, so that every destructive flow behaves identically.
6. As an explorer user, I want tag pills to be keyboard-focusable with a spoken name, so that a screen reader announces "Filter by tag: rust" instead of "button".
7. As an explorer user, I want clicking a tag pill to filter memories exactly as before, so that my filtering habit does not change.
8. As an explorer user, I want tag pills to no longer be raw browser buttons, so that the visual weight matches the design system.
9. As an explorer user, I want the stats screen to settle quickly (or to be told honestly that it is loading), so that I am not stuck at an unexplained spinner.
10. As an explorer user, I want a partially-initialized storage root to produce a clear message, so that I know why the explorer is empty or failing instead of seeing silent errors.
11. As an operator, I want the engine to log which storage files it found and created at startup, so that I can diagnose a broken root from the log alone.
12. As an operator, I want API handlers that can detect a missing store to answer a structured unavailable response, so that monitoring can distinguish "empty" from "broken".
13. As a developer, I want `/api/stats` latency measured once with a breakdown, so that future waves know whether it is inherent or a defect.
14. As a developer, I want the wave-1/2/3 residual ledger emptied, so that parked items stop accumulating as undocumented debt.
15. As a developer, I want the ConfirmGate reducer unchanged behind the new dialog view, so that the table-tested state machine and its tests keep passing untouched.
16. As a developer, I want the class census to stay honest through every swap, so that no dead CSS rule survives behind a "live" token.
17. As a developer, I want the evidence scripts to keep working (readiness gate, arms lane), so that the verification investment from waves 2–3 is not lost.
18. As a screen-reader user, I want dialog content announced when it opens, so that I hear the confirmation prompt without hunting for it.
19. As an explorer user, I want Esc and the mask click to behave consistently (Esc closes, mask does not), so that accidental mask clicks cannot dismiss a destructive confirmation.
20. As an explorer user, I want the "+N more" tag overflow to keep working after the pill swap, so that heavily-tagged memories do not grow unbounded rows.

## Implementation Decisions

- **ConfirmGate state machine is frozen.** The arm-then-confirm reducer (single armed id per panel, refresh invalidates, focus-managed today) is table-tested and survives untouched. Only the view layer swaps: the inline confirmation row is replaced by a `Dialog` whose `open` model derives from the same armed state. `mask_closeable = false` (0.4.8's default is true — it must be set explicitly); Esc close is native to the 0.4.8 Dialog.
- **All three ConfirmGate call sites** (memory delete, session delete, maintenance confirm) swap together so the pattern stays uniform; the maintenance screen's armed notice MessageBar is *not* replaced by a dialog — it stays as the armed-state indicator, with the dialog only wrapping the final confirm.
- **Focus management** moves from bespoke to native-Dialog behavior; the spec's earlier parking note (`mask_closeable=false` + a focus study) is resolved by this decision: the study is this wave's dialog work itself.
- **Tag pills**: Thaw 0.4.8 has no accessible-name-carrying clickable Tag (`InteractionTagPrimary` renders a bare button with no type prop — a submit hazard the wave-2 grilling already rejected; `dismissible Tag` renders an unnamed dismiss button). The decided shape is a Thaw `Button` (Subtle appearance, compact) carrying the tag text with the preserved `aria-label`, replacing `button.chip > Tag`. The `+N more` overflow span stays. If the Button shape proves visually wrong in the live capture, the fallback is `InteractionTag` with a type-guard — decided by the rendered evidence, not preference.
- **`.chip` rules retire in the same step as the pill swap**; census pairing per the standing discipline. The pre-commit `--min-emitted` floor is re-derived in that step (it drops by the retired rule count).
- **Stats latency**: measure before touching anything. Breakdown at the handler seam: wall time, shard-scan time, aggregation time, serialization time, on a populated store (thousands of memories) and on an empty store. If the dominant cost is an obvious defect (e.g., a full scan where an index exists, per-shard sequential awaits that could be joined), fix it; otherwise record the measurement in `docs/evidence/` as an inherent-cost note and keep the Loading state. Either outcome closes the item as *measurement*.
- **Partial-store detection**: the engine's storage-root open path gains a startup audit — enumerate the expected database files, log found/created/refused at a visible level (structured `tracing` + the existing `structured_stderr` seam from issue 128). Handlers that already distinguish Unavailable (`PortError::Unavailable` → the 501 degrade) are the model; where a handler can detect the partial-root condition without new state, it answers the structured unavailable payload instead of a bare 500. No DDL, no auto-repair, no new file formats.
- **No new dependencies.** Dialog, Button, Tag, InteractionTag are all thaw 0.4.8. The stats breakdown uses existing `tracing` spans.
- **The native-select contracts are untouched**: `profile-identity-select`, `sessions-project-select`, and the `agents-project-select` Ready-arm region stay exactly as they are.
- **ADR-0003 stack frozen**: leptos 0.7.x + thaw 0.4.8, CSR wasm-only, `web-ui/` excluded from the workspace.

## Testing Decisions

- **Good tests assert external behavior at existing seams only** — the three seams this repo already pays for, no new ones:
  1. **Pure-function oracles** (wasm Node runner, no DOM): the ConfirmGate reducer's existing table tests keep passing unchanged (the state machine is frozen); the tag-pill decision, if it introduces any pure helper (e.g., overflow-count logic), follows the `memories_empty_copy`/`page_after_shrink` pattern — table-tested, mutant-killed.
  2. **CDP lanes** (`web-ui/scripts/capture-evidence.cjs` + the dedicated lane scripts): the readiness-gated route census re-derives all rows after the dialog and pill swaps; the Sessions arms lane (wave 3's permanent guard) proves the timeline still transitions; a new assertion set *inside the existing lane script* covers dialog behavior (open→focus trapped→Esc closes→mask click does not→confirm executes→focus returns) and the pill contract (`aria-label` present, no `.chip` in the DOM, raw-button count still 0 on populated fixtures). The lanes fabricate distinguishing inputs via fetch interception with `new Response` (plain JS objects fail the wasm `dyn_into`).
  3. **REST contract seam**: the partial-store audit gets one integration test at the storage-open seam (partial root → startup log names the missing files; handlers that detect the condition answer the structured payload), following `tests/production_api_wiring.rs` and the existing degrade-path tests as prior art. The stats measurement records its breakdown as evidence, not as a gate.
- **Prior art**: wave 2's `test-skill-form-cdp.cjs` (RED→GREEN browser regression), wave 3's Sessions arms lane (fabricated-input lane with non-vacuity proof), the census self-test (`check-class-coverage.mjs --self-test`), and the production-api-wiring degrade tests.
- **Non-vacuity rule**: every fix must first RED on the pre-fix tree at its asserting seam (the wave-3 precedent: the lane caught the frozen timeline only because it ran against the buggy bundle first). Evidence artifacts declare the bundle id they were captured against.
- **Leptos reactivity discipline** (wave-3 bug-pattern, binding on this wave): view closures read signals tracked and are passed as children, never invoked; `get_untracked` appears only in callback/reducer contexts; the post-fix grep check is recorded in the ledger.

## Out of Scope

- Any rethème or new component visual design beyond the dialog and pill swaps.
- `DataTable`/`Dropdown`/`Icon` — they do not exist in thaw 0.4.8 or would need new deps.
- Any storage migration, auto-repair, or new on-disk format for the partial-store work — detection and honest reporting only.
- Fixing the stats latency by changing the aggregation's semantics or the API contract — the client and server pagination/filter contract is frozen.
- The Sessions screen's "Load timeline" button removal (kept by the wave-3 ruling).
- Issue 39 (cleanup retention redesign) — it stays paused on its own blocker; only its armed-notice *view* interacts with this wave's dialog work, not its storage design.
- Any change to `profile-identity-select`, `sessions-project-select`, or the `agents-project-select` region.

## Further Notes

- Synthesized 2026-11-06 from the issue 118 wave 1–3 close-outs: the Dialog parking note (wave 3 §What-NOT-to-do item 4), the tag-pill retention note (wave 2 §2.3 with the a11y rationale), the stats latency observation (wave 3 debug-1: ~3.2 s on a cold lane, readiness gate built because of it), and the partial-store 500 (wave 2 handoff-5/review-2: positive half reproduced — fresh roots answer 200; negative half unreproducible because the failing root was deleted; the "partial store" mechanism was left INFERRED, contradicting evidence noted).
- The stats measurement may close as *measurement, not a deliverable* (workflow §A/B rule): if the breakdown shows no defect, the honest outcome is the recorded number and the existing Loading state — not a fix.
- The partial-store item's first deliverable is the *diagnosis protocol*: the wave-2 wave could not settle cause because the failing root was gone. The startup audit is what makes the next occurrence self-describing.
- Verification posture carried from issue 118: every behavioural claim in the wave's evidence is labelled MEASURED/READ/INFERRED with artifacts; the census and floor move in the same commits as the class-site changes.
