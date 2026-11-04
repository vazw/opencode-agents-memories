# Issue 103 Evidence: Engine Shutdown Exit Bound Calibration & Watchdog

## 1. Background and Attribution

In `tests/engine_shutdown.rs`, `test_capture_stderr_has_no_transcript_canary` (:538) was observed intermittently flaking under system load when guarded by a 6-second exit bound (`wait_bounded(&mut child, Duration::from_secs(6))`).
The test enqueues a capture RPC request requiring asynchronous teardown: background inbox drain, embedding model inference (ONNX Runtime / fastembed), SQLite database flush, web server shutdown, and process termination.

Under idle conditions, teardown takes ~4–5 s. Under CPU contention or concurrent cargo compilation, teardown regularly exceeds 6 seconds (measured at 6.18 s, 6.27 s, and 7.04 s). A 6-second wall-clock bound caused "0 failed" in CI/test runs to be a statement about machine load rather than software correctness.

## 2. Latency Distributions (Measured 2026-11-04)

### A. Idle Baseline

| Test | Workload | Sample Times (s) | Mean (s) |
|---|---|---|---|
| `test_engine_exits_on_stdin_eof` (:187) | `hello` handshake only, empty inbox, no embeddings | 0.33, 0.20, 0.21 (earlier: 0.15–0.22) | ~0.20 s |
| `test_capture_stderr_has_no_transcript_canary` (:538) | Capture RPC queued, embedding inference + DB flush | 5.36, 4.87, 4.33 (earlier: 3.69–5.32) | ~4.58 s |

State difference justification:
`test_engine_exits_on_stdin_eof` exits in ~0.20 s because no capture jobs or database transactions are queued. `test_capture_stderr_has_no_transcript_canary` must drain the queue, run the embedding model, and sync the database, requiring an order of magnitude more CPU work.

### B. Loaded Distribution under Contention

Under concurrent CPU contention:
- Run 1: 7.04 s
- Run 2: 5.80 s
- Run 3: 5.86 s
- Run 4: 6.00 s
- Run 5: 5.59 s
- Historical spikes observed during batch reviews: 6.18 s, 6.27 s.

## 3. Margin Derivation

| Bound | Margin vs Idle Mean (4.58 s) | Margin vs Historical Load (6.27 s) | Margin vs Heavy Contention (7.04 s) | Verdict |
|---|---|---|---|---|
| **6 s** (old) | 1.31x | 0.96x | 0.85x | **Defective** (flaked under load) |
| **30 s** (`SHUTDOWN_WATCHDOG`) | 6.55x | 4.78x | 4.26x | **Calibrated** (hang detector only) |

A 30-second watchdog provides >4.2x headroom over the worst observed execution while remaining well below standard test-runner timeout thresholds. It acts strictly as a deadlock / hang detector.

In addition, an explicit assertion on structured stderr event `"engine.shutdown_complete"` was introduced to verify clean asynchronous pipeline termination without relying solely on process death.

Security invariant check:
Canary assertions (`!stderr.contains(canary)` and `!stderr.contains("transcript")`) at lines 559–565 remain untouched.

## 4. Five Consecutive Loaded Test Executions

Executed under 10 concurrent CPU contention workers:

```text
Starting background CPU contention...
Running 5 consecutive iterations of test_capture_stderr_has_no_transcript_canary under contention...
=== Run 1 ===
running 1 test
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 7.04s
=== Run 2 ===
running 1 test
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 5.80s
=== Run 3 ===
running 1 test
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 5.86s
=== Run 4 ===
running 1 test
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 6.00s
=== Run 5 ===
running 1 test
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 5.59s
Stopping background contention...
Contention stopped.
```

Result: 5 passed, 0 failed.
Notice that Run 1 took 7.04 s, which would have failed under the previous 6 s threshold.
