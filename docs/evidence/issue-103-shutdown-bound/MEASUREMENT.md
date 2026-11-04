# Measurement: Issue 103 — Engine Shutdown Exit Bound Calibration

Refer to `docs/evidence/issue-103-shutdown-bound/README.md` for full background and analysis.

## Key Measurements

- Idle baseline `test_engine_exits_on_stdin_eof`: ~0.20 s mean (empty inbox, no embeddings).
- Idle baseline `test_capture_stderr_has_no_transcript_canary`: ~4.58 s mean (capture RPC queued, ONNX embedding + DB flush).
- Loaded baseline under contention: worst-case 7.04 s observed (historical: 6.27 s).
- Old 6 s bound margin under load: 0.85x–0.96x (fragile, caused gate flakes).
- New 30 s watchdog (`SHUTDOWN_WATCHDOG`) margin: 4.26x–4.78x over loaded worst case, 6.55x over idle mean.
- 5 consecutive loaded runs: 7.04 s, 5.80 s, 5.86 s, 6.00 s, 5.59 s — 5/5 passed (0 failed).
- Structured stderr verification: `assert!(stderr.contains("engine.shutdown_complete"))` passes.
- Canary checks: `:559` and `:563` untouched.
