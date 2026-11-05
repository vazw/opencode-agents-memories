# 20 — W5.2 · V2 adapter, toast-RPC & TUI companion (US 5.2)

Status: done — 2026-11-04 (landed 2026-10-27 as merge `56df86a` (branch `issue-20/v2-adapter-tui`: landing `1a71abc`, records `6640fec`, oracle rework `846355e`, location-filter fix `1d9ad6f`) — only the V1 wrapper was present at filing; the V2 entry, the adapter, the toast RPC and the TUI companion landed under `issue-20/v2-adapter-tui`, with the pre-V2 pin inverted in `plugin/tests/bundle-boundary.test.mjs`. **Three reviews, none finding a code defect.** 77 tests (40 pre-wave + 37), zero assertions removed, production byte-identical to the reviewed landing, and AC15 re-run green on the merged tree. Residual: provider connectivity, owned by issue 62; the location filter's decision is pinned but five fail-closed host-shape alternatives are unexercised (recorded below). Brief under `## Agent Brief`.

## What to build

The V2 plugin contract support: V2 loader contract, adapter bridging V2→V1 behavior (as upstream does), toast RPC
plumbing, and the TUI companion surface. The two provider-connectivity host-path variants are **not** this lane's:
they are unported and owned by #62. Wave detail: `.scratch/rust-port/roadmap.md` §W5.2.

## Acceptance criteria

- [ ] Behaviors: **4 ported** (`v2-loader-contract`, `v2-plugin-adapter`, `v2-legacy-client`, `tui-companion`) / **3 N-A with owners** (`v2-memory-context` — case-partitioned, cache half ported here, store-backed half owned by #63 and #52; `v2-provider-connectivity`, `v2-provider-refresh` — owned by #62). Registry rows: `docs/agents/testing.md` (W5.2). ACs 2–3 unchanged.
- [ ] V2 host loads the plugin through the adapter with V1 behavior preserved.
- [ ] Toast RPC round-trips without blocking the message path.

## Blocked by

- 19 · W5.1

## Agent Brief

**Category:** enhancement
**Summary:** add the V2 host contract — loader, adapter, toast RPC and the TUI companion.

**Current behavior (verified at HEAD `206cf9f`):**
Only the V1 wrapper exists. Its `server()` returns `{ hooks }` with `chat.message` and `event`, the package
declares no V2 or TUI entry, and the executable pre-V2 pin is `plugin/tests/bundle-boundary.test.mjs:17-24`
(`readdirSync(dist)` is exactly `["plugin.js"]`) — not a test on the `setup` export, which no file asserts.
There is no TUI crate in the workspace. The toast-related config keys parse and have zero readers.

**Desired behavior:**
The V2 host loads the plugin through an adapter that preserves V1 behaviour, exactly as upstream bridges the
two contracts. The loader contract is honoured (the entry the host imports, plus the companion entry), toast
RPC round-trips without blocking the message path, and the TUI companion surface exists. Four of the seven upstream
behaviours this lane owns are ported here: `v2-loader-contract`, `v2-plugin-adapter`, `v2-legacy-client`,
`tui-companion`. `v2-memory-context` is case-partitioned — its cache half is ported here, its store-backed half is
owned by #63 and #52. `v2-provider-connectivity` and `v2-provider-refresh` are unported and owned by #62.

**Key interfaces:**
- The wrapper's exported entry shape — the V2 contract is a different entry, not a variant of the V1 one.
- The adapter boundary: V2 requests are translated into the V1 behaviour, so the engine surface does not
  fork.
- The toast RPC channel — it must not block the message path.
- The executable pre-V2 pin — `plugin/tests/bundle-boundary.test.mjs:17-24` (the shipped `dist/` is exactly
  `["plugin.js"]`) — must be inverted in place, or the suite asserts the old world.

**Acceptance criteria:**
- [ ] The issue's three ACs, each pinned by a test that would fail on today's tree.
- [ ] The executable pre-V2 pin (`plugin/tests/bundle-boundary.test.mjs:17-24`) is inverted in place — recursive
      file walk, six shipped files — not deleted.
- [ ] V1 behaviour is provably unchanged: the existing V1 wrapper tests still pass untouched.
- [ ] Toast RPC round-trips, and a test proves the message path does not block on it.

**Out of scope:**
- Tool registration and `recall` (issue 22).
- The portability/skills round-trip (issue 26) — this lane only unblocks its bridge half.
- The compaction hooks (issue 33).

## Residual — provider connectivity: owner #62

No port-side provider-connectivity gate exists. The wrapper has no provider surface, and the only client
capability the ported V1 behaviour consumes is `session.messages` (`plugin/src/plugin.js:668-669`; this wave adds
`tui.showToast` as a second). The Rust mechanism is dead code: `ensure_provider_connected` /
`refresh_connected_providers` (`crates/mem-ai/src/provider/opencode_host.rs:484-502`) have no production caller —
the calls and setters are tests only (`ai_provider_config.rs:74,75,76,77,90,99,102` with the refresher installed at `:84`;
`opencode_provider.rs:214,222,228-230` with the refresher installed at `:215`) — the snapshot is born empty
(`opencode_host.rs:795`), and `crates/mem-bridge/src/capture_prompt.rs:307` calls `complete` ungated. The
derivation is not upstream's either: `provider_ids` (`:721-733`) keeps every `GET /config` id with no activation
filter and no dedup, where upstream keeps `activation ∈ {auto, enabled}` deduped
(`example/opencode-mem/src/v2/legacy-client.ts:246-256`).

`v2-provider-connectivity` and `v2-provider-refresh` are therefore N-A **for this lane** and owned by **#62**
(`.scratch/rust-port/issues/62-provider-connectivity-gate-unowned.md`), whose discriminating measurement, A-or-B
ruling and `provider_ids` parity fix are the resolution — including this issue's obligation to stop calling these
two behaviours "N-A — engine-side". This section adds no second owner and no second measurement.

## Landing — 2026-10-27

Merged as `56df86a`, last of the trio, so its eight registry rows land at the end of
`docs/agents/testing.md`'s table. AC15 re-run **on the merged tree**: `8 / 0 / 1 / 5 / 0 / 3 / 0 / 0 / 0
/ 0 / 1`, the rows sit at `:153`–`:160` between the last pre-existing row and `Maintenance rule:` at
`:162`, and issue 61's paragraph survived the rebase. Whole wrapper suite on merged `main`: `tests 77
/ pass 77 / fail 0 / cancelled 0`, exit 0.

**Three reviews, no code defect in any of them.** What they found was tests that could not fail:

**Wave 2 — eleven unpinned rules.** The first review's two findings were that the upstream `:58`
oracle never created the stale block it claimed to remove (deleting `adapter.js:38` left 77/77
green) and that eleven in-process tests had no `{ timeout }`, so removing `controller.abort()` hung
the file (exit 124) instead of failing it. A debugger then audited **all 55 rules** in the plugin
diff and found **20 unpinned, 7 correctness- or privacy-relevant** — including two provably
invisible to a grep: the counter's placement and a second `profile_tier()` call, because a grep
counts a literal line and cannot see control flow.

**Wave 3 — the tenth branch, and it failed open.** After the eleven were pinned, the review found
`legacy-client.js:182-185`, the session comparison. The harness returned one directory for every
session id and the only unresolvable one *threw*, so `sessionDirectory !== ctx.location.directory`
was unreachable: replacing the equality with a `typeof` check left 7/7 and 66/66 green, and a
foreign project's idle event would have been captured under **this** project's id. The nine
directory branches were pinned only because every foreign-directory fixture supplies a top-level or
payload directory and returns before reaching the session branch.

> **The transferable rule:** enumerate branches by *reachability*, not by reading the code. For
> each `return`, ask which fixture makes the opposite value reachable — if the answer is none, the
> branch is decoration however carefully it is written.

**Recorded residual, not a defect.** The filter's *decision* is pinned in both directions (fixture
present ⇒ the named wrong implementations redden at `actual: 6, expected: 5`; fixture absent ⇒ they
stay 7/7 and 77/77 green). Five fail-**closed** host-shape alternatives remain unexercised —
`raw.payload.data`, `raw.properties`, `data.info.id`, `session.directory`, `session.data.directory`.
Every one drops an event rather than forwarding one, so none is a hole; the blueprint requires the
chain be implemented verbatim, not that every `??` alternative be pinned. Recorded so the next audit
does not re-derive it.

**Also recorded:** the wave-2 ledger carried five DONE timestamps that could not be observations —
`R1` after the commit it preceded, `R5` 56 minutes after it and 29 minutes in the future. They are
marked inadmissible in the ledger with the measurement, and nothing was rewritten. A timestamp is a
measurement like any other.
