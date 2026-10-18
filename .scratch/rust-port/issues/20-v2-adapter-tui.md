# 20 — W5.2 · V2 adapter, toast-RPC & TUI companion (US 5.2)

Status: needs-triage

## What to build

The V2 plugin contract support: V2 loader contract, adapter bridging V2→V1 behavior (as upstream does), toast RPC plumbing, and the TUI companion surface. Host-path variants of provider connectivity tests (`v2-provider-connectivity`, `v2-provider-refresh`) live here. Wave detail: `.scratch/rust-port/roadmap.md` §W5.2.

## Acceptance criteria

- [ ] Behaviors ported: `v2-loader-contract`, `v2-plugin-adapter`, `v2-legacy-client`, `tui-companion`, `v2-memory-context`, `v2-provider-connectivity`, `v2-provider-refresh`.
- [ ] V2 host loads the plugin through the adapter with V1 behavior preserved.
- [ ] Toast RPC round-trips without blocking the message path.

## Blocked by

- 19 · W5.1
