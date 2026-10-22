# 19 — W5.1 · V1 plugin adapter — JS wrapper + Rust stdio server (US 5.2)

Status: done — 2026-10-20 (reviewer APPROVED: engine_shutdown 11/11, node 30/30 + e2e 5/5, mem-bridge/mem-store/mem-episodic batteries green, fmt+clippy clean, security gate 10/10 PASS; cargo audit rc=1 is pre-existing dep-tree, out of scope)

## What to build

`mem-bridge` — the JS plugin module satisfying the V1 loader contract, launching the Rust engine over the W5.0 bridging strategy (stdio server), forwarding `chat.message`/session events into the engine, with clean shutdown. JS lane (wrapper) and Rust lane (server) against a **frozen message schema** can proceed in parallel once the schema lands. Wave detail: `.scratch/rust-port/roadmap.md` §W5.1.

## Acceptance criteria

- [x] Behaviors ported: `plugin-loader-contract`, `host-config`, `root-entrypoint`, `bundle-boundary`, `plugin-shutdown`.
- [x] Message schema frozen (documented) before JS/Rust lanes split.
- [x] JS wrapper satisfies the V1 host contract (loads under the reference plugin harness behavior from upstream tests).
- [x] Clean shutdown: no orphaned engine process/stdio handles.

## Blocked by

- 17 · W4.3 (Phases 1–4 complete)
- 18 · W5.0

## Comments

- 2026-10-20T23:38:55Z — W5.1 landed (blueprint 20261020-1839-w51-v1-plugin-adapter-r8; no git writes — the orchestrator commits). What landed: frozen bridge schema (`docs/bridge-schema.md`) + 20 fixtures; JS wrapper (`plugin/`) passing the loader-contract, root-entrypoint, bundle-boundary, host-config (stub-staged credential oracle), shutdown, dist-sync, fixtures and wrapper-glue batteries; Rust engine (`crates/mem-bridge/src/engine.rs` + jsonrpc/error_map/db_lane, composed by `src/main.rs`) with NDJSON framing, five methods + handshake, error-code map, engine lock with fail-open loser (exit 75) and status-file grammar, capture_id additive migration (v4), stdin-EOF exit and graceful/sigterm shutdown bounds.
- 2026-10-20T23:38:55Z — Battery results (all GREEN): `node --test plugin/tests/*.test.mjs` (30 pass across 9 files, measured) + `cargo test -p mem-bridge` and `-p mem-store` / `-p mem-episodic` full units (DONE rows steps 3–11) + integration: `cargo test -p opencode-agents-memory --test engine_shutdown -- --test-threads=1` → 11 passed; 0 failed; `node --test plugin/tests/e2e-shutdown.test.mjs` → 5 pass; 0 fail; `cargo test -p opencode-agents-memory --test workspace_smoke` → 2 passed; `bash -n`/`shellcheck` clean; `cmp plugin/src/plugin.js plugin/dist/plugin.js` byte-identical; `cargo fmt --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean; `npm pack --dry-run --json` tarball oracle green (engine staged, name `opencode-mem`, version `0.1.0`). One integration defect was found and fixed test-side: the harness wrote NDJSON requests without the '\n' frame terminator, two-sided deadlock; diagnosis RED→GREEN recorded in the wave ledger.
- 2026-10-20T23:38:55Z — Known non-gate gap (pre-existing, out of this issue's scope): `cargo audit --deny warnings` rc=1 on three webpki-family advisories (RUSTSEC-2026-0381/0382/0383) reached only via ort-sys's build-dependency chain (ureq → webpki-roots), plus unmaintained fxhash/paste and unsound lru warnings (Cargo.lock frozen this wave).
- 2026-10-20T23:38:55Z — Stays open: V2 adapter (upstream `chat.params` hook, `setup`, real memory tooling — issue 20/W5.2), capture/injection substance beyond the ack-stub (W5.3 durable inbox), W5.3 inbox storage layout, and the deferred docs/agents/testing.md registry rows (W6.2 owns the file). AC checkboxes are evaluated by the orchestrator at commit time.
