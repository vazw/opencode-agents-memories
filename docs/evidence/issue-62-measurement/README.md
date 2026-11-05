# Evidence: Issue 62 Provider Connectivity Gate Measurement and Ruling

Date: 2026-11-05
Author: Coder Agent (`skill: ponytail`)
Worktree: `.tmp/worktrees/issue-62`
Blueprint: `.tmp/blueprints/blueprint-20261105-1510-issue-62-provider-connectivity-gate-r2.md` (sha256: `ab011ce5efcf`)

## 1. Environment & Measurement Context

- **OS / Kernel**: Linux 7.3.1_1 x86_64
- **CPU**: AMD Ryzen 5 6600U with Radeon Graphics (12 vCPUs)
- **Rust Toolchain**: cargo / rustc 2024 edition target
- **Harness**: `crates/mem-bridge/tests/provider_connectivity_measurement.rs`

## 2. Empirical Measurements

Measurements taken via automated integration test harness executing end-to-end `CaptureService::drain_once()` passes:

| Scenario | Input / Configuration | Measured Latency | Error Class (`reason`) | `error_kind` | `retryable` | Attempts Consumed | Outcome |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **Unreachable Endpoint** | `OpenCodeHostConfig` pointing to closed local port `http://127.0.0.1:40999` | 4.52 ms (< 100 ms) | `summarizer_failed` | `network` | `true` | 1 | Transient retry ladder |
| **Disconnected / Missing Provider** | Mock host responding HTTP 502 with `{"_tag":"ProviderNotFoundError"}` | 2.38 ms (< 10 ms) | `host_provider_not_found` | `config` | `false` | 0 | Immediate dead-letter |

### Command & Output
```bash
cargo test -p mem-bridge --test provider_connectivity_measurement -- --nocapture
```

```text
running 2 tests
{"capture_id":"019a0000-0000-7f00-8000-00000000e002","component":"capture","detail":"providerID=p","error_kind":"config","event":"capture.dead_lettered","level":"warn","project_id":"/srv/measurement","reason":"host_provider_not_found","status":502}
[MEASUREMENT disconnected_provider] latency=2.37592ms, reason=host_provider_not_found, error_kind=config, retryable=false, attempts=0
test test_measurement_disconnected_provider_terminal_deadletter ... ok
[MEASUREMENT unreachable_endpoint] latency=4.523951ms, reason=summarizer_failed, error_kind=network, retryable=true, attempts=1
test test_measurement_unreachable_endpoint_latency_and_typing ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s
```

## 3. Analysis

1. **Failure Is Fast**:
   - Both unreachable host ports and disconnected provider responses fail in single-digit milliseconds (~2.4 ms to ~4.5 ms), far below human-perceptible delays and well within the 100 ms requirement.
2. **Failure Is Typed**:
   - An unreachable port surfaces as `error_kind: "network"`, classified as `summarizer_failed` (`retryable: true`), incrementing attempt count by 1 and adhering to the capture retry ladder.
   - A disconnected/unknown provider returns an explicit `502` with tag `ProviderNotFoundError`, decoded cleanly into `wire_reason::PROVIDER_NOT_FOUND`, which classifies as `host_provider_not_found` (`retryable: false`). It consumes 0 attempts and dead-letters immediately without retries or retry storms.
3. **Pre-call Gate Redundancy & Risks**:
   - In upstream TypeScript (`opencode-mem`), `ensure_provider_connected` was an unowned, uncalled helper that tracked connected providers via `provider_ids` and refreshed them on cache miss.
   - Wiring this pre-call gate into `capture_prompt` (Option A) would:
     - Add extra synchronous or asynchronous roundtrips before every capture call.
     - Introduce the severe "empty snapshot trap": if initialized before provider discovery completes, an in-memory gate would reject 100% of legitimate capture requests.
   - The direct generate call already fails fast, cleanly typed, and safely handles both transient network outages and terminal provider misconfigurations.

## 4. Architectural Ruling

- **Option B (delete dead gate) WINS**:
  Remove `ensure_provider_connected`, `refresh_connected_providers`, `is_provider_connected`, and `ConnectedRefresher` from `crates/mem-ai/src/provider/opencode_host.rs` and `mod.rs`.
- **Option A (wire gate into capture_prompt) is REJECTED**:
  Unnecessary architectural complexity with severe failure modes (empty snapshot trap) and no empirical reliability benefit.
