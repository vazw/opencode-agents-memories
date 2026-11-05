# PRD: Harness, Bridge & Gate Hardening (14-Issue Reliability Wave)

## Problem Statement

During continuous test-driven migration of the memory engine from TypeScript to Rust, multiple test harness, process lifecycle, runtime gate, and documentation drift issues accumulated:
- Tests failed non-deterministically due to race conditions in test fixture scratch paths, tight exit timeouts, and target directory leaks.
- Engine shutdown following heavy workloads (e.g. cold model embedding) triggered SIGTERM kills because wrapper response timeouts did not reflect actual teardown latency.
- Daily profile decay sweeps re-armed on every engine restart because sweep stamps were held in memory rather than persisted, risking premature duplicate decay.
- Synthetic host messages and internal prompt markers leaked across the capture boundary, contaminating stored memories with assistant-generated artifacts.
- Chat message injection configuration switches (`chat_message.enabled`) were unread in the injection assembly pipeline, allowing unwanted chat context injection.
- Unverified documentation claims, stale test numbers, and unexamined store modules undermined architectural verification.

## Solution

A consolidated reliability and hardening wave addressed all 14 issues across the Rust engine crates, the Node.js plugin wrapper, test fixtures, and documentation:
- Calibrated engine shutdown exit bounds with hang watchdog protection.
- Formalized live wave ledger timestamps, manifest brackets, and rework state.
- Isolated test scratch directories with PID and atomic sequence counters.
- Reconciled REST endpoint contract numbers and 501/500 status claims.
- Isolated relative cargo target directories in child test processes.
- Aligned documentation with live production session port wiring.
- Captured live OpenCode host wire exchanges for `create` and `generate` routes.
- Audited all 32 `mem-store` modules (16,870 lines) and filed surfaced defects.
- Persisted daily profile decay sweep stamps in a durable JSON sidecar and removed duplicate decay calculations.
- Dropped synthetic text parts before the adapter capture boundary.
- Refused conflicting shard path migration rollbacks by name.
- Calibrated wrapper shutdown response timeouts to 10 seconds to accommodate cold ONNX loads while keeping 2-second SIGTERM escalation.
- Preserved session caches across compaction with privacy-preserving injection telemetry.
- Enforced `chat_message.enabled = false` refusal in context assembly and established an automated, rot-proof census of all 102 configuration fields.

## User Stories

1. As a test runner executing concurrent test suites, I want test scratch directories isolated by process ID and atomic sequence numbers, so that parallel runs never corrupt each other's state.
2. As a CI/CD test runner, I want `cargo test` in integration test sub-crates to inherit workspace target paths, so that child builds do not leak gigabytes of build artifacts into source trees.
3. As an operator running the engine under memory-constrained environments, I want shutdown tests protected by a watchdog rather than an aggressive hardcoded exit bound, so that slow CI nodes do not fail tests spuriously.
4. As an operator restarting the host application frequently, I want the daily decay sweep timestamp persisted across restarts, so that profile memories are not subjected to duplicate decay operations on every restart.
5. As an agent interacting with the memory system, I want my profile confidence scores to decay predictably according to single-application exponential moving average math, so that profile data does not degrade prematurely.
6. As a privacy-conscious user, I want synthetic assistant messages and prompt injections stripped before capture, so that assistant outputs never get remembered as user facts.
7. As an administrator configuring memory context, I want setting `chat_message.enabled = false` to strictly prevent chat memory context injection while leaving user profiles and direct memory tools active, so that chat prompts remain uncluttered when disabled.
8. As a developer auditing configuration options, I want an authoritative, machine-checked census of every configuration field and its live reader line, so that inert or dead configuration fields cannot silently exist in code.
9. As a developer modifying configuration structures, I want an automated test that fails if any field is added, renamed, or removed without updating the configuration census, so that configuration documentation never rots.
10. As a developer performing shard migrations, I want conflicting rollback operations to fail explicitly with typed errors naming the conflicting active shard, so that inconsistent database states are rejected immediately.
11. As a host wrapper managing the engine process, I want graceful shutdown allowed up to 10 seconds when ONNX models are active, so that uncheckpointed SQLite transactions can flush cleanly before SIGTERM escalation.
12. As an agent receiving memory context across multi-turn sessions, I want session memory caches preserved across session compaction events, so that compacted sessions maintain memory continuity without unneeded re-fetches.
13. As a systems auditor, I want live wire contract payloads between the plugin and the host recorded with verbatim bodies and redacted secrets, so that schema drift is caught against actual network traffic.
14. As an architect verifying code coverage, I want every module in storage and database crates audited against acceptance criteria, so that unexamined code paths cannot hide latent bugs.
15. As a wave reviewer checking implementation logs, I want wave ledgers to carry live UTC timestamps, manifest boundary brackets, and rework records, so that historical progress claims are auditable and reproducible.

## Implementation Decisions

- **Shutdown Watchdog:** Rather than relying on rigid process wall-clock bounds, engine exit tests employ a 30-second watchdog thread that asserts prompt exit under normal conditions while capturing stderr traces if a hang occurs.
- **Durable Decay Sweep Sidecar:** The 24-hour daily decay sweep stamp is stored in a `decay-sweep.json` sidecar file beside the profile database with restricted file permissions (`0600`), falling back safely to immediate execution if the file is missing or corrupted.
- **Single-Application Decay EMA:** Eliminated duplicate confidence decay in memory models, ensuring decay matches the exact mathematical target (0.55 EMA blend and 0.405 90-day decay threshold).
- **Synthetic Part Filtering:** Host adapter sanitization strips nodes marked with `synthetic: true` from user prompts and assistant responses prior to dispatching auto-capture payloads.
- **Chat Injection Refusal Gate:** `InjectService::assemble` inspects `chat_message.enabled` when handling chat injection requests; if disabled, it returns a null memory block while preserving profile tiers and direct tool injections.
- **Authoritative Configuration Census:** All 102 schema fields (86 root fields and 16 nested struct fields) are indexed in a structured Markdown table with exact source line citations and dead-code dispositions.
- **Rot-Proof Census Guard:** A syntax-directed Markdown table parser test runs against the configuration documentation, asserting 100% field coverage against Rust struct definitions and failing on missing, misspelled, or orphaned rows.
- **Shutdown Response Timeout Calibration:** The Node.js plugin wrapper response budget for engine shutdown requests was calibrated from 2,000 ms to 10,000 ms, while preserving the 2,000 ms post-timeout SIGTERM escalation ladder.
- **Compaction Cache Retention:** The V2 adapter retains cached memory blocks across session compaction notifications unless the session explicitly invalidates or unloads.
- **Target Path Normalization:** Relative `CARGO_TARGET_DIR` paths in CLI test harnesses are normalized against workspace root ancestors before invoking child processes.

## Testing Decisions

- **External Behavioral Verification:** All tests assert public external interfaces (HTTP status codes, stdio RPC responses, database rows, filesystem sidecars) rather than internal private state.
- **RED-to-GREEN Discipline:** Every behavioral change was verified first in a failing state (RED) before applying the minimal fix and verifying passage (GREEN).
- **Negative Testing & Fault Injection:** The configuration census parser test includes 5 adversarial tests verifying detection of missing fields, extra fields, wrong struct headers, empty dispositions, and corrupted tables.
- **Pre-Commit Lint & Dist-Sync Gate:** The pre-commit hook runs rustfmt, clippy, and a byte-identical comparison of Node wrapper `src/` versus `dist/` bundles.

## Out of Scope

- Modifying the underlying SQLite database schemas for `metadata.db` or `user-profiles.db` (zero migration schema changes).
- Re-architecting provider connection logic (deferred to Issue 62).
- Implementing startup warning log emission for inert configuration keys (deferred to Issue 128).
- Resolving the session ID omission in re-embed operations (deferred to Issue 127).

## Further Notes

All 14 issues were reviewed by specialized sub-agents (`code-reviewer`, `syseng`), passed all pre-commit and workspace test gates, and are committed locally on `main` without pushing upstream.
