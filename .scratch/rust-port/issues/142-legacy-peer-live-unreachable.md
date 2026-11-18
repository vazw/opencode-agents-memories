# 142 — `legacyPeerLive` is declared vocabulary but unreachable from the HTTP DTO

Status: ready-for-agent — filed 2026-11-18 from issue 43 residual 6 (`docs/evidence/issue-43-w6/REPORT.md:146`).

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

- [ ] One of the two directions chosen, with the reason recorded in `## Comments`.
- [ ] A test proves the chosen direction: either the value is emitted in a fixture that exercises the chosen path, or
      the value is absent from the DTO's declared set and a test fails if it is re-added.
- [ ] `docs/adr/0019-erasure-posture.md:157` and `crates/mem-web/src/api/dto.rs:355-358` agree with each other and with
      the test, in the same commit.
- [ ] No answer-time probe of the sweep is introduced — issue 43 refused that, and this issue does not re-open it.

## Blocked by

Nothing. Independent of the rest of issue 43's residuals.
