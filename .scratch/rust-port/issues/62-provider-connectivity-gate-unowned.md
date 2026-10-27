# 62 — `mem-ai`'s provider-connectivity gate is dead code, and `provider_ids` has no activation filter

Status: ready-for-agent — 2026-10-27 (re-verified at `8023dd8`: every premise holds, and issue 20's
pointer is already in place. The deliverable is the discriminating measurement, not an implementation;
brief under `## Agent Brief`.)

## PREMISE — measured, not inherited

`crates/mem-ai/src/provider/opencode_host.rs` carries a complete provider-connectivity mechanism:

- `is_provider_connected` (`:467`), `ensure_provider_connected` (`:484`), `refresh_connected_providers`
  (`:498`), a refresher slot (`:479`), and a public mirror of all of it (`:819`, `:830`, `:835`).
- `refresh_connected_providers` fills the snapshot from `GET /config` through `provider_ids` (`:500`).

**Nothing in production calls any of it.** `grep -rn 'ensure_provider_connected\|refresh_connected_providers\|is_provider_connected' --include='*.rs' crates src tests` returns the definitions plus exactly four test
files:

```
crates/mem-ai/tests/provider/ai_provider_config.rs:75,76,77,99,102
crates/mem-ai/tests/provider/opencode_provider.rs:228,229,230
```

The refresher is installed only from those tests (`ai_provider_config.rs:84`, `opencode_provider.rs:215`),
and the snapshot starts empty (`opencode_host.rs:795`). `crates/mem-bridge/src/capture_prompt.rs:309` calls
`self.provider.complete(request)` directly, with **no connectivity check** before it.

The wrapper has **zero** provider surface: `grep -rn provider plugin/src plugin/index.js plugin/dist`
returns nothing. Its only host-client capability is `session.messages` (`plugin/src/plugin.js:668-669`).

**And the derivation is not upstream's even when it runs.** `provider_ids` (`opencode_host.rs:721-733`)
maps `GET /config.providers[].id` with no activation filter and no dedup. Upstream keeps only
`activation ∈ {auto, enabled}` and dedups the result
(`example/opencode-mem/src/v2/legacy-client.ts:246-256`). So this is a **different derivation**, not a
relocated gate.

## WHY IT MATTERS

Issue 20's brief lists `v2-provider-connectivity` and `v2-provider-refresh` among the behaviours W5.2 owns.
The blueprint proposed calling them N-A on the ground that the gate "moved into the engine". The Plan Gate
measured otherwise: the mechanism exists but has no consumer, so those two behaviours are **neither ported
nor deliberately dropped** — they are unowned. Filing them here keeps that honest rather than letting a
blueprint's prose retire them.

## TWO CANDIDATE RESOLUTIONS — pick by measurement, not preference

**A — wire the gate into the one production caller.** `capture_prompt` asks
`ensure_provider_connected(model.provider)` before `complete`, with the refresher installed at composition
time, so a disconnected provider answers a typed skip instead of an attempted call. Cost: a config round
trip on the capture path; benefit: a predictable refusal.

**B — delete the dead gate.** If a call to a disconnected provider already fails promptly and with a typed
error, the gate is machinery guarding a path that fails safely on its own, and the honest resolution is
removal plus a doc line saying why. Cost: none; benefit: less dead code.

**The discriminating measurement**, which both designs must be judged by: bring up the engine with a
configured-but-disconnected provider, drive one capture, and record **what the current `complete` path
does** — how long it takes, whether it retries, and what error the caller sees. If it fails fast and typed,
B wins. If it hangs, retries, or surfaces an opaque error, A wins. If neither, say so and close the item as
measurement.

**Independent of A or B:** `provider_ids` must gain upstream's activation filter and dedup, or the port
must record why it deliberately diverges. That half is not a design question — it is a parity defect in a
function that exists either way.

## ACCEPTANCE — what would close it

- [ ] The discriminating measurement is recorded with its artifact (engine fixture, capture invocation,
      timing, error text).
- [ ] A or B is chosen from that measurement, and the loser is recorded with why it lost.
- [ ] `provider_ids` either matches upstream's activation filter + dedup, or carries a recorded divergence
      with a reason and a test pinning the current behaviour.
- [ ] Issue 20's tracker entry stops calling these two behaviours "N-A — engine-side" and points here
      instead.
- [ ] If B wins: the gate, the refresher slot and their public mirrors are removed, and the tests that
      exercised them go with them — no test left covering deleted code.

## Out of scope

- Re-tuning the provider layer, or changing which providers exist.
- The capture summariser's prompt or schema.
- Any change to `example/` (read-only reference).

## Comments

- **2026-10-27 — filed from the issue-20 Plan Gate.** The gate's finding, verified independently before
  filing: the mechanism is real (`opencode_host.rs:467-502`), its only callers are tests
  (`ai_provider_config.rs:99,102`, `opencode_provider.rs:228-230`), `capture_prompt.rs:309` calls
  `complete` ungated, and `provider_ids` lacks upstream's activation filter
  (`legacy-client.ts:246-256`). Marked `needs-triage` rather than `ready-for-agent` because the resolution
  is a measurement with two plausible winners, and a brief that presupposed "wire it" would beg the
  question.

- **2026-10-27 — re-verified at `8023dd8` (premise holds; option A gains a trap).** The gate is still dead
  code (`opencode_host.rs:467/484/498`, mirrors `:819/824/830/835`; callers are the two test files only),
  `capture_prompt.rs:307` still calls `complete` ungated (moved from `:309` by 52), and `provider_ids`
  (`:721`–`:733`) still maps `providers[].id` with no activation filter and no dedup, against upstream's
  `activation ∈ {auto, enabled}` + `new Set` (`legacy-client.ts:246-256`). The wrapper has no provider
  surface; the only hits are comments in `plugin/src/v2/**` that point here.
  **One AC is already satisfied:** issue 20 landed and records "N-A for this lane and owned by #62"
  (`20-v2-adapter-tui.md:73`).
  **New for option A:** `set_connected_providers_refresher` (`opencode_host.rs:479`, mirror `:824`) is
  never installed in production — only from tests. The snapshot starts empty, and
  `ensure_provider_connected` returns false on an empty snapshot, so naive wiring would silently skip
  **every** capture. Option A costs refresher installation, first-refresh ordering and failure semantics,
  not one call.
  The discriminating measurement is still unrun. Recommendation: `ready-for-agent` with a
  measurement-only brief (artifact + A/B decision), not an implementation brief.
  **State not changed at triage time.**

## Agent Brief

> *This was generated by AI during triage.*

**Category:** bug
**Summary:** decide A or B by measurement, and fix `provider_ids`' missing activation filter either way.

**Current behaviour (verified at `8023dd8`):** `is_provider_connected` / `ensure_provider_connected` /
`refresh_connected_providers` exist (`crates/mem-ai/src/provider/opencode_host.rs:467`, `:484`, `:498`;
public mirrors `:819`, `:830`, `:835`) and are called only from two test files.
`set_connected_providers_refresher` (`:479`, mirror `:824`) is never installed in production, so the
snapshot is always empty and `ensure_provider_connected` would answer false for every provider.
`capture_prompt.rs:307` calls `complete` ungated. `provider_ids` (`:721`–`:733`) maps `providers[].id` with
no activation filter and no dedup, against upstream's `activation ∈ {auto, enabled}` + `new Set`
(`example/opencode-mem/src/v2/legacy-client.ts:246`–`:256`).

**Desired behaviour:** (1) run the discriminating measurement — engine with a configured-but-disconnected
provider, one capture, recording duration, retries and the caller's error; (2) choose A (wire the gate,
including refresher installation and the empty-snapshot policy) or B (delete the gate and its mirrors),
recording the loser; (3) give `provider_ids` upstream's filter + dedup, or a recorded divergence with a
test pinning the current behaviour.

**Key interfaces:** `ConnectedRefresher`, the provider registry, `capture_prompt`'s provider call,
`provider_ids`.

**Acceptance criteria:** the issue's five ACs; the measurement artifact committed under `docs/evidence/`,
not `.tmp/`.

**Out of scope:** retuning the provider layer; the summariser prompt/schema.
