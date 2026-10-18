# 19 — W5.1 · V1 plugin adapter — JS wrapper + Rust stdio server (US 5.2)

Status: needs-triage

## What to build

`mem-bridge` — the JS plugin module satisfying the V1 loader contract, launching the Rust engine over the W5.0 bridging strategy (stdio server), forwarding `chat.message`/session events into the engine, with clean shutdown. JS lane (wrapper) and Rust lane (server) against a **frozen message schema** can proceed in parallel once the schema lands. Wave detail: `.scratch/rust-port/roadmap.md` §W5.1.

## Acceptance criteria

- [ ] Behaviors ported: `plugin-loader-contract`, `host-config`, `root-entrypoint`, `bundle-boundary`, `plugin-shutdown`.
- [ ] Message schema frozen (documented) before JS/Rust lanes split.
- [ ] JS wrapper satisfies the V1 host contract (loads under the reference plugin harness behavior from upstream tests).
- [ ] Clean shutdown: no orphaned engine process/stdio handles.

## Blocked by

- 17 · W4.3 (Phases 1–4 complete)
- 18 · W5.0
