# Issue 105 Evidence: Post-Capture Shutdown Timing and SIGTERM Escalation Calibration

## 1. Live Reproduction of Premature SIGTERM Reap (AC1)

Under uncalibrated wrapper constants (`SHUTDOWN_RESPONSE_TIMEOUT_MS = 2000`, `SHUTDOWN_TERM_AFTER_MS = 2000`), when the engine's `shutdown` RPC response is delayed by capture embedding work, the wrapper aborts waiting for the RPC response at 2.0 s and initiates SIGTERM escalation 2.0 s later. Consequently, SIGTERM is delivered to the engine child process at approximately ~4.0 s (~4018 ms) from shutdown dispatch.

### Live Uncalibrated Reproduction Log

Execution of the uncalibrated wrapper against a child delaying shutdown response by 5000 ms:

```text
Driver ready, sending SIGTERM at 2026-11-04T22:15:22.577Z
Driver closed after 4018 ms, code: null signal: SIGTERM
Captured stderr from driver:
{"level":"info","component":"opencode-mem-plugin","event":"inject.requested","session_id":"test","query_chars":2}
{"level":"info","component":"opencode-mem-plugin","event":"inject.received","session_id":"test","block_bytes":0,"handling_ms":0,"status":"error"}
{"level":"warn","component":"opencode-mem-plugin","event":"inject.failed","message":"inject timed out"}
{"level":"info","component":"opencode-mem-plugin","event":"toast.not_delivered","reason":"inject.failed"}
{"level":"info","component":"opencode-mem-plugin","event":"child_reaped","pid":7767,"exit_code":null,"signal":"SIGTERM"}
```

Observation:
The wrapper logs `child_reaped` with `exit_code: null` and `signal: "SIGTERM"` at 4018 ms (~4.0 s).
This confirms directly from live logs that premature SIGTERM escalation fires while the child process is still active.

## 2. Root Cause Analysis: Runtime Block and RPC Timing (AC2)

### A. Measured Latency Intervals
Measured across multiple post-capture shutdown executions:
- `shutdown` RPC write -> ack: `5.05 s`, `5.91 s`, `6.14 s` (always `{ "ok": true }` when allowed to finish; late, never refused).
- Engine shutdown internal execution: `128–814 ms` (measured in `engine.shutdown_complete` logs).
- Disparity: `4.19–5.5 s` of the elapsed wall time is spent *before* the engine's stdio loop reads the `shutdown` command from stdin.

### B. Single-Threaded Runtime Blocking
The engine (`crates/mem-bridge/src/engine.rs:1289`) initializes a single-threaded Tokio runtime:
```rust
let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()
```
When a capture RPC is received, background processing calls `LocalOnnxEmbedder::load()` (`crates/mem-embed/src/local/mod.rs:163-209`):
```rust
let bytes = std::fs::read(&weights_path)...;
let session = LoadedSession::load(model, &bytes, self.options.intra_threads)?;
let tokenizer = load_tokenizer(&model_dir)?;
```
`std::fs::read` reads the ONNX model weights from disk into memory, `LoadedSession::load` initializes ONNX Runtime execution graphs, and `load_tokenizer` parses vocabularies. Because these operations execute synchronously on the current thread, the Tokio event loop is blocked for `4.19–5.5 s`.

During this synchronous block:
- `serve_stdio` cannot poll `tokio::io::AsyncBufReadExt::read_line` on `stdin`.
- The `shutdown` JSON-RPC request sent by the wrapper remains unread in the OS pipe buffer until ONNX loading completes.
- Once model loading yields back to the runtime, `serve_stdio` reads `shutdown`, dispatches it, and returns `{"ok":true}` within 128–814 ms (total write→ack: 5.05–6.14 s).

### C. Wrapper Timeout Mismatch
In `plugin/src/plugin.js`:
- `SHUTDOWN_RESPONSE_TIMEOUT_MS = 2000`: wrapper only waits 2000 ms for the RPC response.
- `SHUTDOWN_TERM_AFTER_MS = 2000`: wrapper waits an additional 2000 ms after RPC timeout before delivering SIGTERM.
Total window before SIGTERM: `2000 ms + 2000 ms = 4000 ms` (~4.0 s).
Since write→ack is 5.05–6.14 s, the 4000 ms window expires before the engine can read the RPC, killing the engine with SIGTERM.

## 3. Calibration and Test Verification (AC3)

### Calibration
- `SHUTDOWN_RESPONSE_TIMEOUT_MS` in `plugin/src/plugin.js` is increased from `2000` to `10000` ms.
- `SHUTDOWN_TERM_AFTER_MS = 2000` is kept strictly intact at 2000 ms.
- `plugin/dist/plugin.js` is synced via `cp plugin/src/plugin.js plugin/dist/plugin.js` and verified by `plugin/tests/dist-sync.test.mjs`.

### Headroom Analysis
- Worst measured cold post-capture RPC ack: `6.14 s`.
- Calibrated budget: `10000 ms` (`10.0 s`).
- Headroom margin: `10.0 s / 6.14 s = 1.63x` headroom over the worst observed cold load under contention.
- Idle shutdown completes in `<200 ms` (verified in `tests/engine_shutdown.rs`), so the larger budget does not delay normal exits.

### Test Verification
A new discriminator test `test_post_capture_shutdown_budget_allows_graceful_exit` is added to `plugin/tests/shutdown.test.mjs` with a 5000 ms stub response latency:
1. Under uncalibrated `SHUTDOWN_RESPONSE_TIMEOUT_MS = 2000`:
   - Fails RED: wrapper escalates at 4000 ms, SIGTERM reaps the child (`code: null, signal: "SIGTERM"`), failing `assert.equal(reapedLines[0].exit_code, 0)`.
2. Under calibrated `SHUTDOWN_RESPONSE_TIMEOUT_MS = 10000`:
   - Passes GREEN: wrapper receives response at 5000 ms, clears escalation timers, child exits cleanly (`exit_code: 0, signal: null`).

## 4. Durability and Negative Constraint Verification (AC4)

### A. Persistence Invariant: Zero Lost Work
No work is dropped or lost when SIGTERM is delivered during capture or model loading:
- `CaptureInbox::enqueue` (`crates/mem-episodic/src/inbox.rs:60-96`) commits incoming capture requests directly into SQLite table `capture_inbox` with `status = 'queued'`.
- `CaptureInbox::dequeue` (`crates/mem-episodic/src/inbox.rs:146-173`) is a non-destructive `SELECT` query:
  ```rust
  let mut rows = tx.query(
      "SELECT capture_id, session_id, parent_session_id, project_id, agent_id, agent_name, transcript, summary, metadata_json, created_at, retry_count
       FROM capture_inbox
       WHERE status = 'queued' AND not_before <= ?1
       ORDER BY created_at ASC
       LIMIT ?2",
      params![now, limit],
  ).await?;
  ```
  Dequeuing does NOT alter `status` or delete records.
- Records are deleted ONLY in `CaptureInbox::mark_done` (`crates/mem-episodic/src/inbox.rs:175-189`) via:
  ```rust
  "DELETE FROM capture_inbox WHERE capture_id = ?1"
  ```
  `mark_done` is called only after successful embedding inference and episodic storage commit.
- If a SIGTERM terminates the engine while embedding or loading models, the uncompleted capture items remain in SQLite `capture_inbox` in `'queued'` state.

### B. Startup Replay
On subsequent engine startup, `startup_drain` (`crates/mem-bridge/src/engine.rs`) replays all pending entries from `capture_inbox`.
Durability and recovery behavior verified via:
```text
cargo test -p mem-bridge --test startup_drain
```
Result: 4 passed; 0 failed.
