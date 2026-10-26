# 20 — W5.2 · V2 adapter, toast-RPC & TUI companion (US 5.2)

Status: ready-for-agent — 2026-10-27 (verified absent at HEAD: only the V1 wrapper exists and a test pins the V2 `setup` export as forbidden; brief under `## Agent Brief`)

## What to build

The V2 plugin contract support: V2 loader contract, adapter bridging V2→V1 behavior (as upstream does), toast RPC plumbing, and the TUI companion surface. Host-path variants of provider connectivity tests (`v2-provider-connectivity`, `v2-provider-refresh`) live here. Wave detail: `.scratch/rust-port/roadmap.md` §W5.2.

## Acceptance criteria

- [ ] Behaviors ported: `v2-loader-contract`, `v2-plugin-adapter`, `v2-legacy-client`, `tui-companion`, `v2-memory-context`, `v2-provider-connectivity`, `v2-provider-refresh`.
- [ ] V2 host loads the plugin through the adapter with V1 behavior preserved.
- [ ] Toast RPC round-trips without blocking the message path.

## Blocked by

- 19 · W5.1

## Agent Brief

**Category:** enhancement
**Summary:** add the V2 host contract — loader, adapter, toast RPC and the TUI companion.

**Current behavior (verified at HEAD `206cf9f`):**
Only the V1 wrapper exists. Its `server()` returns `{ hooks }` with `chat.message` and `event`, the package
declares no V2 or TUI entry, and the port's own test asserts that the V2 `setup` export **must not exist yet** —
so the current state is pinned negatively. There is no TUI crate in the workspace. The toast-related config
keys parse and have zero readers.

**Desired behavior:**
The V2 host loads the plugin through an adapter that preserves V1 behaviour, exactly as upstream bridges the
two contracts. The loader contract is honoured (the entry the host imports, plus the companion entry), toast
RPC round-trips without blocking the message path, and the TUI companion surface exists. The seven upstream
behaviours this lane owns are ported: `v2-loader-contract`, `v2-plugin-adapter`, `v2-legacy-client`,
`tui-companion`, `v2-memory-context`, `v2-provider-connectivity`, `v2-provider-refresh`.

**Key interfaces:**
- The wrapper's exported entry shape — the V2 contract is a different entry, not a variant of the V1 one.
- The adapter boundary: V2 requests are translated into the V1 behaviour, so the engine surface does not
  fork.
- The toast RPC channel — it must not block the message path.
- The port's negative test on the `setup` export — it must be inverted when this lands, or the suite asserts
  the old world.

**Acceptance criteria:**
- [ ] The issue's three ACs, each pinned by a test that would fail on today's tree.
- [ ] The negative assertion forbidding the V2 `setup` export is inverted, not deleted.
- [ ] V1 behaviour is provably unchanged: the existing V1 wrapper tests still pass untouched.
- [ ] Toast RPC round-trips, and a test proves the message path does not block on it.

**Out of scope:**
- Tool registration and `recall` (issue 22).
- The portability/skills round-trip (issue 26) — this lane only unblocks its bridge half.
- The compaction hooks (issue 33).
