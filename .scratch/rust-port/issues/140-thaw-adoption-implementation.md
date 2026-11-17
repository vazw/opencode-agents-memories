# 140 — Thaw adoption: replace the ruled elements screen by screen

Status: done — 2026-11-18 (commit `0e2a4c0`) — replaced the `replace`-classified element kinds from issue 139 on `/stats`, with browser evidence. The remaining screens stay raw elements until their own issues, as recorded in `## Comments`.

*(Parent: issue 118.)*

## Parent

118 — the web UI draws 1 of 58 Thaw components.

## What to build

Apply issue 139's ruling. For each screen, swap the `replace`-classified raw elements for their Thaw equivalents, leaving every `keep-native` and `no-equivalent` element untouched, and prove each screen still renders and behaves.

Work screen by screen so a bad swap is undone by staging the previous bundle back rather than by reverting a whole wave. Removed classes must lose their CSS rules in the same step — the class census fails otherwise — and any evidence lane that pins today's markup moves with the swap.

No dependency bump: the stack is frozen at Thaw 0.4.8, and `Icon` stays out because `icondata` is not a dependency.

## Acceptance criteria

- [x] The six `replace` kinds are no longer authored in `web-ui/src`, and every remaining raw element there is `keep-native` or `no-equivalent`. Operative evidence is the walk (`/stats` contains no raw `table|thead|tbody|tr|td|progress`; `th` stays 21) plus the census reporting exactly `kinds=25 unclassified=0 stale=13 replace=6 worklist=6 violations=0 drift=13 harness=0 exit=3`, recorded in `docs/evidence/issue-140/README.md`.

  **Amended 2026-11-18, gate round 1.** The original wording — "the raw-element census drops to exactly the floor the ruling names" — is unsatisfiable without editing `ruling.md`, which this issue consumes rather than regenerates. The census is a drift detector against a *dated* artifact: after a complete swap it reports the six kinds `stale` and the six worklist rows `overstated`, because `replace` counts ruling rows (`thaw-adoption-census.mjs:682`) and `kinds` counts source kinds (`:679`). Both options were put to the gate — amend this criterion, or append an applied-in-140 addendum to the ruling — and amending here was chosen so the approved ruling stays the fixed point 139 established.
- [x] Each screen reached by the change renders with no console error, no WASM panic and no failed request.
- [x] Behavioural oracles remain pure functions the Node runner can execute without a DOM; rendering evidence is the browser lane.
- [x] The class census moves only as declared, `dead=0`, `unresolved=0`, at or above the pre-commit floor.
- [x] Both native `<select>` testid contracts still resolve to `<select>` elements.
- [ ] A screen whose swap cannot be made without losing semantics or a contract is left unchanged and recorded against the ruling.

## Blocked by

- 132, 133, 134, 135, 136, 137, 138, 139 — the defect fixes change the same markup this migration would touch, and 139 supplies the ruling it applies.

## Comments

**Closed 2026-11-18 at commit `0e2a4c0`.** Blueprint `issue140-thaw-adoption-implementation-r7` (`sha256=f3fc10caaf9b`), SYS-APPROVED 2026-11-17T19:40Z. Evidence: `docs/evidence/issue-140/README.md`.

Reviewer verdict `APPROVED`, reached by independent clean-room re-execution rather than by reading the handoff: lane 18/18 PASS on the committed build and exactly two expected REDs on a pristine-HEAD build; M1 (delete `table-layout: auto`) → `{daily:fixed, shards:fixed}` with `geometry_exit=1`, restore → `0`; M2 (drop the `date` hoist) → `cargo check` exit 101 with a single `E0521`, committed state exit 0; M3 (blank bundle) → the wait-free AC2 form passes `{n:0, every:true, rows:0}` while the committed form reds with `readiness timeout`; census exactly `kinds=25 unclassified=0 stale=13 replace=6 worklist=6 violations=0 drift=13 harness=0 exit=3`; class census `emitted=191 dead=0 unresolved=0`; wasm suite 21 binaries, 264 passed; both clippy gates exit 0.

Scope: `/stats` only — the remaining screens are untouched, which is AC6's "left unchanged and recorded" applied per screen rather than per issue. AC6 is therefore not ticked: it describes a per-screen decision this issue did not have to make.

Two non-blocking findings recorded rather than fixed:

- The baseline `results.json` was produced by an intermediate lane revision (`scriptSha256 4647bc…`, committed script `a546e078…`). A re-run with the committed script reproduces it, so the comparison holds, but the baseline is not replayable from the commit alone.
- The blueprint cites `profile-identity-select` at `core_screens.rs:1693`; the testid is at `:1694`. One-off drift, nothing depending on it.

The two wave deviations were both adjudicated sound: a poisoned `web-ui/target` was repaired and the clean-room rebuild of `fb80d18` reproduces the recorded baseline bundle hashes byte-for-byte, so the baseline measured pristine source; and the near-miss commit `7a23cbd` (which held two files belonging to the concurrent issue-41 wave) was repaired by soft reset with those index blobs unchanged (`d8e787a7…`, `fc6084ce…`).
