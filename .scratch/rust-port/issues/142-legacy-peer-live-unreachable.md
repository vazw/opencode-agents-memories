# 142 — `legacyPeerLive` is declared vocabulary but unreachable from the HTTP DTO

Status: done — 2026-11-18 (commit `1fed372`) — retired the value instead of making it reachable: `legacyPeerLive` is gone from `ErasureStateDto`'s doc and from ADR-0019 D-19-3, the ADR records that no answer could ever carry it, and `sweep_state_vocabulary.rs` reds if it is re-added. Filed 2026-11-18 from issue 43 residual 6 (`docs/evidence/issue-43-w6/REPORT.md:146`).

*(Parent: issue 43.)*

## What to build

The sweep-state vocabulary declares five values. One of them can never be observed by a client:

- `crates/mem-web/src/api/dto.rs:355` lists `"swept" | "notYetSwept" | "sweepFailed" | "legacyPeerLive" | …`
  as the declared set.
- `dto.rs:358` already annotates `legacyPeerLive` as **unreachable from HTTP in W6c** — the sweep's block record is
  report-only (`crates/mem-bridge/src/sweep.rs`) and the stamp table stores **successes only**, so no code path can
  ever emit the value.
- `docs/adr/0019-erasure-posture.md:157` publishes the same vocabulary to a reader as if all five were observable.

A declared-but-unreachable value is the same defect class as an untested branch: a client author writes a match arm
for a state the server cannot produce, and a reader of ADR-0019 believes the server reports something it does not.

This was deliberately left open by issue 43, which **refused probing at answer time** — the sweep runs on its own
schedule and the API must not block on it to answer a status read. That refusal stands and is not in question here.

## The decision this issue must make

Pick exactly one, and say which in `## Comments` with the reason:

1. **Make it reachable** — persist a failure/block record (the sweep already produces one, report-only) and have the
   status read surface it. Costs a schema or sidecar write on the failure path; gains a state a client can act on.
2. **Retire the value** — remove `legacyPeerLive` from the declared set in `dto.rs` and from ADR-0019's vocabulary,
   leaving the historical note that it was never reachable. Cheapest, and honest: a vocabulary that matches what the
   server can emit.

A third option is not available: leaving a value declared, unpublished and unreachable is what this issue exists to
end.

## Acceptance criteria

- [x] One of the two directions chosen, with the reason recorded in `## Comments`.
- [x] A test proves the chosen direction: either the value is emitted in a fixture that exercises the chosen path, or
      the value is absent from the DTO's declared set and a test fails if it is re-added.
- [x] `docs/adr/0019-erasure-posture.md:157` and `crates/mem-web/src/api/dto.rs:355-358` agree with each other and with
      the test, in the same commit.
- [x] No answer-time probe of the sweep is introduced — issue 43 refused that, and this issue does not re-open it.

## Blocked by

Nothing. Independent of the rest of issue 43's residuals.

## Comments

Chosen: direction 2 — retire the value. `legacyPeerLive` is removed from the declared sweep-state set in `dto.rs`
and from ADR-0019 D-19-3, and the ADR records the retirement in an `Amended (issue 142, 2026-11-18)` note
saying it was never reachable from HTTP. The reason: the sweep's block record is report-only and the stamp table
stores successes only, so no answer could carry the value. Making it reachable would cost a write on the failure
path for a state no client can act on. The population that might have seen it is a pre-drop old-build state the
owner ruled out on 2026-11-09 (ADR-0025: development phase, one laptop, sole user), and nothing consumes it. The
owner ruled to retire the value and not make it reachable; no replacement field is added and nothing is wired to it.
The emit mapping in `prompts.rs` is unchanged, which is the mechanical proof that no answer-time probe was
introduced (issue 43's refusal stands).

The guard is `crates/mem-web/tests/sweep_state_vocabulary.rs`: it fails if `legacyPeerLive` reappears in `dto.rs`,
if the DTO and ADR declared sets differ from the four emitted labels, or if the ADR amendment note is removed.
The same change folds `swept` into the ADR sentence, which had omitted a label `prompts.rs:114` emits.

Evidence: `docs/evidence/issue-142/RUNS.md`.
