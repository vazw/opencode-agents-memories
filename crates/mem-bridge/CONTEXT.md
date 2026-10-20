# mem-bridge

**Purpose:** OpenCode host integration: V1/V2 plugin bridge, stdio server, TUI companion.

**Ubiquitous language (W5.1, ADR-0007):**

- **engine** — the single Rust process that owns the storage root's databases. It
  serves the five-method JSON-RPC surface over stdio (as the wrapper's child) and,
  in this same process, the axum web server (`WebLauncher` seam, D-32). Lives in
  `engine.rs`; the root composition is a sync arg-parser around it (M1).
- **launcher mode / launch mode** — `stdio` (per-request JSON-RPC over stdin/stdout,
  no signal handler), `standalone` (axum + graceful SIGTERM/SIGINT), and the
  env-gated test-only `hang-on-shutdown` (parks after the shutdown response so the
  wrapper's SIGTERM fallback has a real target).
- **wrapper** — the JS host-side package (`plugin/`), NOT part of this crate; the
  wrapper spawns the engine child and degrades fail-open when absent.
- **hello handshake** — the wrapper's first request; the wrapper applies the
  major-equal + floor compat rule on the reply. `hello` is not a runtime method.
- **session_id** — the required top-level parameter of every runtime method
  (1..=128 chars); DATA, never authority.
- **capture_id** — the client-minted UUIDv7-shaped, ≤64-char identifier added to
  `agent_sessions` by the v4 additive migration (recall: W5.1 has no write path
  for it; W5.3 owns the recapture/upsert policy).
- **engine lock / `.engine.lock`** — the lifetime exclusive lock (mem-store
  `engine_lock`); its body is `LockState` JSON, and the LOSER writes the status
  file naming the HOLDER's pid and exits with code 75 (`ENGINE_LOCK_LOST`).
- **status file** — `<storage_root>/status`; one line
  `degraded: <short reason> (pid=<n>, ts=<UTC ISO-8601>)`; a report, not a policy.
- **DB lane / `SerialDbLane`** — the single ordered drain (agent queue before web
  queue) behind the `DbLane` trait; LLM summarisation is rejected at submit and
  never rides the lane (D-33).
- **canonical serialization** — compact JSON, lexicographic keys at every level
  (`docs/bridge-schema.md` §1); both lanes must reproduce the frozen fixture bytes.
- **volatile fields** — `uptime_ms`, `handling_ms`, `engine_version`, `db.path`;
  masked in response-schema comparisons (contract gate M37).
- **error map** — the specific codes -32700/-32600/-32601/-32602/-32603/-32001
  (MEM_DEGRADED)/-32002 (MEM_INBOX_FULL, W5.3's number); never a substituted -32603.
- **shutdown pipeline** — ordered teardown: web stop wrapped in ~1 s, DB flush
  ≤ 500 ms, total < 2 s (D-45); stdio EOF and standalone signals both converge on it.

**Deferred register rows (orchestrator; W6.2 owns `docs/agents/testing.md`):**
the `-32002` code row, the exit-code-75 row, the status-file-format row.

**Open questions (user's, ADR-0007):** (a) second-instance lock policy; (b) the
`darwin/x64` engine target matrix. Nothing in this crate answers either.
