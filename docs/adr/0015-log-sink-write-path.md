# 0015 — Log-Sink Write Path (rotation + ENOENT recovery for the sole writer)

- **Status:** Proposed
- **Date:** 2026-10-25 (UTC 2026-10-24) — the decision is dated by the session's
  local calendar day; the UTC clock still reads 2026-10-24T2x:xxZ with identical local time.
- **Context source:** blueprint `20261024-1939-sink-rotation-recovery` (SYS-APPROVED
  2026-10-24T19:53Z), measured on HEAD `07257dd`; the frozen reader
  `crates/mem-web/src/api/stats.rs`; upstream `example/opencode-mem/src/services/logger.ts`
  (v2.31.3 @ `906038e`).
- **Closes:** ADR-0014's *Rotation gap* consequence (recorded there as CLOSED).
  **Supersedes:** nothing.
- **Binding on:** `mem-core::logging` (writer only). The reader
  (`mem-web::api::stats`), the archive names, the banner text, the line format and the env
  contract are untouched.

## Context

ADR-0014 made `SinkLayer` the sole production writer of the structured log and recorded, as a
gated follow-up, that the layer called `LogSink::append` directly. Measured on HEAD `07257dd`:

- `SinkLayer::on_event` (`crates/mem-core/src/logging.rs:373`) called `sink.append(..)`;
  `append` has neither rotation nor directory recovery.
- The 5 MiB cap was checked only by `init()` and by `log()`'s date-gated `rotate()`, and
  `log()` has **0 production callers** — so the cap was never enforced at runtime and the live
  file grew for the life of the process.
- Deleting the log directory made every subsequent layer write fail `ENOENT` silently until
  restart: the "nothing happened vs. the feature is inert" ambiguity ADR-0014 removed for the
  stats table came back for the file.

The reader is frozen: `list_log_files` recognises exactly `opencode-mem-<YYYY-MM-DD>.log`,
`opencode-mem.log` and `opencode-mem.log.old`, and `collect_daily_stats` counts by the **line**
dates, not by filenames. Any fix must keep those names byte-identical.

## Decision

**D-1 — One private write seam.** `LogSink::write_line(&self, line: &str) -> Result<(), LoggingError>`
owns the whole sequence: prime (directory + banner) → rotate → append → exactly one `ENOENT`
retry. `pub fn log` renders then delegates; `SinkLayer::on_event` renders then delegates and
swallows the `Result`. The method is private on purpose: it has no external consumer (the layer
is in-crate), and a public second write entry point would re-open the bypass class this wave
closes.

**D-2 — Rotation before every write; names unchanged.** `write_line` calls the existing
`rotate()` unconditionally before the append. `rotate()` already owns both triggers —
`meta.len() >= MAX_LOG_SIZE` and the live file's mtime local date ≠ today — so the size cap is
enforced per write while the midnight rollover still works. Archive names stay byte-identical
(`opencode-mem-<mtime-date>.log`; collision → `opencode-mem.log.old`) because the frozen reader
is their contract. No numeric suffixes, no gzip, no directory moves.

**D-3 — The writer lock is a leaf.** The state mutex is acquired once at the top of each public
entry point (`init`, `log`) and held across the whole sequence, making stat→rotate→append
atomic: two writers cannot interleave a rotation between another's size check and append. The
critical section performs only `std::fs` calls, `chrono` date work and pure formatting — it
never calls `tracing`, never touches stderr, never takes another lock and never re-enters
`LogSink`. A same-thread re-entry would self-deadlock the non-reentrant `std::sync::Mutex`,
which is why the rule is structural. The timestamp is rendered **before** the lock: that is one
`Local::now()` fewer; `rotate`'s date work stays inside the section and is leaf-safe (chrono
takes no sink or dispatcher lock). `build_line` keeps the line format unchanged.

**D-4 — Exactly one `ENOENT` retry.** On `append` → `ErrorKind::NotFound`, `write_line` clears
`initialized`, runs `init_locked` once (create the directory, rotate — a no-op with no file,
append the `\n--- Session started: <ts> ---\n` banner, flag initialized) and appends the
triggering line once. There is no loop, no retry of `init_locked`'s own failure, and no other
place that clears `initialized`. If re-init fails, `initialized` stays false and the next event
retries init; if the retry append fails after a successful init, `initialized` stays true and
the next event appends. Both are accepted loss: no third attempt, no `tracing` call, no stderr,
no diagnostic counter.

**D-5 — No new public API.** The threshold is already `pub const MAX_LOG_SIZE`, so tests drive
the production branch at the production threshold with a sparse `set_len(MAX_LOG_SIZE)` fixture.
No `with_max_bytes` knob, no `pub(crate)` variant, no new `LoggingError` variant, no
dependency, no manifest edit.

## Consequences

- **The cap is enforced when rotation succeeds (the normal case).** The live file is bounded
  below `MAX_LOG_SIZE + one rendered line`; after a successful rotation it holds just the
  triggering line. `rotate()` swallows `rename`/`remove_file` failures (upstream parity), so a
  persistently failing rotation lets the live file grow past the cap — the bound is a property
  of a successful rotation, not of every failure mode. Total disk use is not capped: the live
  file plus up to 30 days of archives accumulate.
- **A deleted directory self-heals on the next write.** One facade or layer write re-creates the
  directory, rewrites the banner and appends its line. After recovery the file contains exactly
  one banner and the triggering line (the pre-deletion file is gone).
- **The ENOENT ambiguity is removed for the ENOENT class only.** "Nothing happened vs. the feature is
  inert" no longer holds when the directory vanishes: the next write recreates it. Persistent
  non-`ENOENT` failures (`EACCES`, `ENOSPC`, `EROFS`) remain indistinguishable from "nothing
  logged" until a health surface exists; this ADR does not claim the general problem is solved.
- **Same-day second rotation and day-rollover collision still collapse into `.old` (upstream
  parity, declared loss).** The first rotation of a day produces `opencode-mem-<date>.log`; a
  later rotation on the same day deletes any previous `opencode-mem.log.old` and renames the
  live file to it. A day-rollover collision has the same effect: the first write of a later day
  rotates a file whose mtime date names an archive that already exists. The reader still counts
  `.old` lines by their line dates **while the file exists**, but the dropped `.old` can hold a
  day's only remaining counted lines, so a day's counters can **decrease between reads** while
  a same-date archive still exists. The loss is accepted and pre-existing; this wave makes
  mid-day second rotations reachable for the first time, which is why it is recorded here.
- **One `metadata()` plus one `Local::now()` per logged line, inside the dispatcher, is fine at
  capture scale.** A slow or networked filesystem costs latency, not deadlock, because the lock
  holder never waits on the dispatcher.
- **The layer is a real writer, not an appender.** `log()` keeps `-> ()` and the layer still
  swallows every error; the reader, the banner text, the line format, the env contract and the
  30-day cleanup are unchanged.

## Rejected

- **Public `LogSink::write_line`.** No external consumer; grows the public contract for a
  test-only convenience and re-opens the bypass class this wave removes.
- **`SinkLayer::on_event` calling `sink.log(message, data)`.** The layer holds an already-rendered
  line while `log` takes `Option<&Value>` and renders its own timestamp: that would re-parse
  JSON and stamp a second time — two renderers for one format.
- **Duplicating rotation/retry inside `on_event`.** Two owners of rotation diverge on the first
  edit; that is exactly the defect this wave removes.
- **A `with_max_bytes` knob (or a `pub(crate)` variant).** No production caller; it would test a
  threshold production never uses, against the recorded upstream contract, and integration tests
  must drive the facade.
- **Numeric suffixes / `.1` / gzip for repeated same-day rotations.** The frozen reader
  recognises only `opencode-mem[-YYYY-MM-DD].log[.old]`; a new suffix would make historical days
  invisible — the exact failure this wave must avoid.
