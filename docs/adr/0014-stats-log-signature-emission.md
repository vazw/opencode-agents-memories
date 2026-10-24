# 0014 — Stats Log-Signature Emission

- **Status:** Proposed
- **Date:** 2026-10-24
- **Context source:** blueprint `20261024-1701-log-signature-wiring` (SYS-APPROVED
  2026-10-24T17:25Z), measured on HEAD `367020b`; the upstream `logger.ts` /
  `stats-service.ts` contract via `crates/mem-web/src/api/stats.rs` (frozen reader).
- **Extends:** ADR-0008 (capture drain) and ADR-0013 (panic-free production paths) from
  behaviour to **observability**. **Supersedes:** nothing.
- **Amended (2026-10-25):** the rotation-gap consequence is closed by ADR-0015 (see the CLOSED
  bullet under Consequences).
- **Binding on:** `mem-core::logging`, `mem-bridge::capture`, `mem-web::api::stats` (reader,
  unchanged) and the root composition binary.

## Context

`GET /api/stats` derives its 14-day `daily` series by scraping the structured log: the
classifier in `crates/mem-web/src/api/stats.rs` recognises nine upstream signatures (four
daily counters and five named error buckets) plus the generic `^.*:\s*error$` rule. The
reader was complete and frozen after the previous wave, but the writer did not exist:
`mem_core::logging::log` had **0 production call sites**, `init_logging` was called only from
one test binary, and no process installed the tracing `SinkLayer`. In production the daily
table was therefore all zeros forever, and an operator could not distinguish "nothing
happened" from "the feature is inert".

Measured on HEAD `367020b`: 9 scraped signatures, of which **3 have a production-reachable
event** — `Auto-capture memory persisted`, `Auto-capture skipped`, and `Auto-capture final
error`, all in `crates/mem-bridge/src/capture.rs` (`process_entry`,
`flush_entry_with_supplied_summary`, `retryable_or_dead`, `dead_letter_now`). The other six
are structurally dormant behind subsystems that do not exist yet.

## Decision

1. **One emission facade.** All file logging goes through
   `mem_core::logging::log(message, data)`. It emits a `tracing` event whose target is the
   private `LOG_TARGET = "mem_core::logging"`; `SinkLayer::on_event` accepts events by
   **exact target equality** and returns early for everything else. The 90 pre-existing
   `tracing::*!` call sites stay out of the file.
2. **One signature spelling, two consumers.** The nine constants live in
   `mem_core::logging::signatures` with a doc comment naming either the emit site or the
   blocking subsystem. The reader keeps its own literals (it is frozen);
   `crates/mem-web/tests/stats_signature_contract.rs` is the mandatory drift guard that
   renders each constant through `LogSink::log` and asserts its intended stats cell.
3. **Install once, at the composition root.** `src/main.rs` calls
   `mem_core::logging::init_process_logging(sink)` after the purge dispatch and immediately
   before `engine_run`, so stdio and standalone modes are both covered and the sync modes
   still return first. The level policy (`INFO`) is pinned in `mem-core`; the root crate
   never names `LevelFilter`. An install failure **degrades**: the engine continues and the
   existing machine-readable stderr seam reports `logging.init_failed`.
4. **The reader's directory is derived from the sink.** The composition root computes
   `log_dir = sink.path().parent()` and passes it into `RootWebLauncher`, which puts it in
   `AppState::log_dir`. Writer and reader share one resolution path and cannot disagree.
   `mem_web::api::stats::default_log_dir()` is retained as a reader-side helper for tests and
   spawn helpers only.
5. **Infallible emitters.** `log()` returns `()`; capture call sites never `let _ =` it and
   never give it a `Result`. A logging failure must never fail a capture. Inside
   `SinkLayer::on_event`, append errors are swallowed (`let _ = sink.append(..)`) and there
   is no stderr fallback and no re-entry into `tracing` (no recursion, no NDJSON flood).
6. **Only `Tier1WriteOutcome::Written` counts.** `Replayed` means a row with the same
   deterministic id already existed — the capture was persisted on an earlier pass — so
   emitting there would double-count a retry after a lost `mark_done`. Both production drain
   paths (`process_entry` and `flush_entry_with_supplied_summary`) split the arm and emit
   after `mark_done`.
7. **Immediate dead-letters share the final-error bucket.** `dead_letter_now` (invalid
   `agent_role`, invalid/required raw context) and `retryable_or_dead`'s fifth consumed
   attempt are both permanent capture failures, so both emit `Auto-capture final error`. The
   payload distinguishes them (`reason` vs `attempts`/`outcome`); no second signature is
   invented. `reason` is always one of the existing fixed constants and never the rejected
   value.
8. **Payload binding rule.** Payloads may carry identifiers and filesystem paths
   (`projectId` is a host path; D-9 governs HTTP responses only) but never secrets, key
   material, container tags, scope hashes, raw context, transcript text, or the rejected
   `agent_role` value.

## Consequences

- **The table becomes real but sparse.** `captures`, `captureSkipped` and one error bucket
  are emitted today; six signatures remain structurally zero **by construction** (no profile
  injection path, no compaction hook, no ready-gate caller, no skills host registration, no
  idle loop). Each is a gated follow-up recorded in `signatures.rs`; the zeros are documented,
  not disguised.
- **Raw diagnostics stay out of the file.** Because the layer is target-scoped, no unrelated
  `x: error` diagnostic can ever create a generic error bucket in `/api/stats`.
- **One process-global subscriber.** Tests that install it are once-per-binary and serialize
  sink-touching cases; the production binary installs exactly once at the composition root.
- **Declared undercount.** Any `mark_done` failure or crash after the tier-1 insert loses at
  most one `captures` increment per entry. A `mark_done` error is swallowed by the existing
  `let _ =`; the row stays queued, the retry re-writes and returns `Replayed`, and the
  `Replayed` arm correctly emits nothing. This is an undercount, never a correctness bug.
- **Rotation gap — CLOSED by ADR-0015 (2026-10-25).** *Historical (kept verbatim):*
  `SinkLayer::on_event` calls `sink.append` directly and therefore bypasses `LogSink::log`'s
  5 MiB rotation and its ENOENT re-init. After this wave the layer is the sole production
  writer, so runtime rotation is never enforced and a deleted log directory kills file logging
  silently until restart. *Closed:* the facade and the layer now share one private
  rotation-aware `write_line` path (ADR-0015 D-1/D-2/D-4): `rotate()` runs before every write,
  so the 5 MiB cap is enforced at runtime, and one `ENOENT` retry re-creates the directory and
  rewrites the banner. Stats correctness was unaffected either way — the series is driven by
  the line dates, not by filenames.
- **Dead-letter terminality.** Double-emit is excluded because `dead_letter` sets
  `status = 'dead'` and `dequeue` selects `status = 'queued'`; a persistently failing
  dead-letter leaves the row queued and re-emits on each attempt — identical in shape to the
  existing `structured_stderr` behaviour, and therefore no new defect class.
- **Custom log filenames stay invisible to the reader.** `OPENCODE_MEM_LOG_FILE` with a
  non-canonical filename writes lines the scanner ignores (`list_log_files` recognises only
  `opencode-mem[-YYYY-MM-DD].log[.old]`). Upstream parity; documented, not fixed; the parser
  is frozen.
- **A failed sink write is accepted loss.** Logging is an observation, never a data path; a
  capture completes even when the sink becomes unwritable after install.

## Rejected

- **Direct `LogSink` plumbing into `mem-bridge`.** It would put the sink handle on every
  service, couple domain crates to a file handle, and leave the facade dead.
- **Both mechanisms (facade + direct sink) as a hybrid.** Two writers mean two line formats
  and two rotation owners; the reader would have to tolerate both.
- **An unfiltered `SinkLayer`.** It would persist all 90 diagnostics sites, making the generic
  `x: error` rule unsafe and leaking diagnostic fields into the file.
- **Fallible emitters (`Result`).** A log write failure would become a capture failure —
  inverted priorities; ADR-0013 D-4 is satisfied by the infallible interface, not by
  swallowing a caller's `Result`.
- **Installing the subscriber inside a library crate.** The global subscriber is
  process-wide; a crate that installs it makes embedders (tests, tools) unable to choose.
  The composition root owns it.
