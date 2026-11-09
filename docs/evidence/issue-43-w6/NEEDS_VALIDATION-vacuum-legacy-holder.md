# `vacuum-under-legacy-holder` — NEEDS VALIDATION, not measured

**Cell:** VACUUM running under a live flag-on (pre-drop) holder.
**State:** `needs_validation`. **No measurement exists, and none can be taken from this tree.**

## Why no measurement is possible here

ADR-0025 (W6a) dropped `ExperimentalFeature::MultiprocessWal` from
`ConnectOptions::for_config`: the shipping option set is `[Encryption]` on every
platform, so every open this tree performs is turso's single-process (legacy)
mode. A flag-on opener — the only thing that can hold the `-tshm` byte-0 lock
this cell is about — cannot be produced by any process in this tree.

The W6c sweep takes the engine lock before it opens anything
(`sweep.rs::run`), so the sweep cannot even run beside a live holder: it exits
75 (`sweep.busy`) first. Reaching the cell would need a purpose-built pre-drop
binary (a pre-issue-45 build) launched beside a running sweep, which is not in
this repository and cannot be produced from it.

`MEASURED`: nothing. `READ`: `crates/mem-store/src/db.rs` (`for_config`),
`crates/mem-bridge/src/sweep.rs` (the engine-lock step). `INFERRED`: the cell is
unreachable post-W6a by construction.

## What the audit recorded (carried, not re-derived)

The opsec audit that gated ADR-0019/ADR-0025 (`docs/evidence/opsec-w6-design-2026-10-25/`,
§9 `:385-412`) records the cell as open. That record is carried here verbatim in
spirit: **open, unmeasured, unreachable without a pre-drop binary.**

## What would close it as measurement

Launch a pre-issue-45 build (flag-on) holding the database's `-tshm`, run
`VACUUM` from a build that opens the same file in legacy mode, and observe
whether the two WAL modes lose rows. Until such a binary exists and the run is
recorded, this cell stays `needs_validation` — a fabricated green here would be
the exact defect ADR-0019's honesty rule forbids.
