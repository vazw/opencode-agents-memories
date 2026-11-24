# Issue 162 — the wrapper→adapter `injectStatus` seam has no end-to-end coverage

Status: done — 2026-11-25 (landed; review APPROVED)
Filed: 2026-11-24, out of the issue-152/155 wave (`8a783b4`) review
Severity: low today, high the first time the producer silently dies

## The finding

The 152/155 wave introduced a wrapper→adapter seam: `plugin/src/plugin.js` publishes a non-enumerable
`injectStatus` (`"ok"` / `"empty"` / `"error"`) on the `output` object it was handed, and
`plugin/src/v2/adapter.js` reads it to decide whether an empty engine answer clears the cached block or
the block is kept for a retry.

**MEASURED by the reviewer, two mutants:**

| mutant | what it did | result |
|---|---|---|
| C | deleted `Object.defineProperty(output, "injectStatus", …)` from `dist/plugin.js` | C1, the only wire criterion of issue 155, still passes — `tests 1 / pass 1 / fail 0` |
| B | flipped `enumerable: false` → `true` | the full wire file passes — `tests 15 / pass 15 / fail 0` |

## Why mutant C matters and mutant B does not

Mutant B has no production effect: the object never leaves the adapter, so enumerability only matters to a
test that deep-compares a *real-wrapper* output, and no such test exists. The blueprint's claim that
`plugin/tests/v2-plugin-adapter.test.mjs:129` would red it is **false** — that `output` is adapter-built and
its `chat.message` is a stub that never sets the field. A test that cannot red the thing it is cited for is
worse than no citation, so the claim is recorded here rather than left standing.

Mutant C is the live risk. C1's path is "keep", which is also what an *absent* field yields, so the seam can
die and the suite stays green. What would stop working is the other half of issue 155: an empty SUCCESS
clears the cache, and that is the half that stops a purge serving withdrawn content. `"empty"` and `"ok"` are
only ever set by stubs today (`plugin/tests/v2-plugin-adapter.test.mjs:590/594`,
`plugin/tests/v2-memory-context.test.mjs:139`).

## Why this is a filed issue and not a rejection

The approved plan specified stub-level C2 and C8a. Every criterion the Plan Gate accepted is met, and the
delivered behaviour is correct by READ on both sides of the seam (`src/plugin.js:1439` initialises, `:1454`
assigns, `:1466` defines). This is a coverage gap the plan knowingly carried, not a defect in what shipped.

## The fix shape

ONE wire case, not a suite: an `injectMode: "empty"` prompt through the **real** wrapper with a compaction
between prompts, asserting the cached block is gone afterwards. That single case reds both mutants — C,
because the field is never set and the block is kept; and B, if the case deep-compares the output the wrapper
returned rather than the output the adapter built.

## Acceptance criteria

1. A criterion that fails when `Object.defineProperty(output, "injectStatus", …)` is deleted from
   `plugin/src/plugin.js` (mutant C), RED as an assertion failure rather than a load error.
2. A criterion that fails when `enumerable: false` becomes `true` — i.e. one that deep-compares a real
   wrapper output. If that proves impossible without exporting adapter internals, say so and drop this AC
   with the reason, rather than keeping an enforcement claim with no enforcement.
3. The `injectStatus` doc claim in the 152/155 blueprint's SECURITY section is corrected either way: it
   names a guard that does not exist as written.

## Inverted world

This issue is closed as *measurement* if a wire case at `plugin/tests/v2-inject-delivery.test.mjs` proves
unable to observe the field — for example if the real wrapper's `output` is not the object the test can reach.
That is a real possibility and would make the seam untestable rather than untested, which is a different
finding worth recording.

## Outcome — 2026-11-24

Both acceptance criteria delivered, as two cases in `plugin/tests/v2-inject-delivery.test.mjs`:

- **AC-1** — `test_inject_status_empty_clears_a_compacted_block`. Real wrapper + real adapter through
  `wireRun`: prompt 0 caches `WIRE_BLOCK` in `first` mode, a V2 compaction marks the session, the stub's
  answer switches to an empty success (`{ block: null }`) via the new per-request `injectModePath` harness
  knob, prompt 1 refreshes (`injects` 1) and the adapter clears the cache (context `[]`), prompt 2 stays
  cleared. Deleting the producer from `plugin/dist/plugin.js` reds it as an assertion
  (`actual ["<memory_context>STUB_FIRST_BLOCK</memory_context>"]`, `expected []`).
- **AC-2 delivered as C2**, not dropped — `test_chat_message_output_hides_inject_status_from_enumeration`.
  The wrapper annotates any object handed to the public `hooks["chat.message"]`, so a caller-built output
  the test holds is enough: `Object.keys(output)` is `["message"]`, the descriptor reports `has: true`,
  `enumerable: false`. Flipping `enumerable` to `true` reds it (`true !== false`). `has` is asserted
  because an absent property is also non-enumerable — the key check alone cannot tell a deleted producer
  from a hidden field.

Corrected claim (supersedes the 152/155 blueprint's SECURITY bullet, which names this file's line 129 as
the guard; that `output` is adapter-built against a stub that never sets the field):

> `injectStatus` must be non-enumerable: the hook output carries no third key. The invariant is asserted on
> a caller-built output, not on the adapter's local; mutant B (`enumerable: true`) is the case that pins it.

Fixture note: `wireRun` stages `injectMode: "block"` and the switch is **block → empty success**, not
`error → empty` as the 162 blueprint's prose says. `success` is not a named stub branch; it falls through
to `{ block: null }`, the suite's existing convention. The `injectModePath` read sits before the `error`
branch so every mode is re-read per request, which is what lets a run that *started* on `error` switch too.

## Not in scope

- Issue 152's residuals (a) and (b) — uncleared, and they need a live host.
- The `[exited]`-without-pid nuance the reviewer noted (`shellNote`, the exited branch emits `[exited]` before
  the pid guard): blueprint snippet verbatim, no pid ever reaches `process.kill`, no defect.

## Review, 2026-11-25 — APPROVED

The reviewer re-executed all seven claims rather than inheriting them, including rebuilding the blueprint
body hash against its own recipe. It re-ran the three mutants itself: mutants B, C-on-C1 and C-on-C2 each RED
as an `AssertionError`, each proven landed by pre/post grep, each restored `cmp`-identical, closing on
`DIST-PRISTINE` and `TRAP-RESTORED`.

- **C2** inspects the caller-built literal it handed to the real `hooks["chat.message"]` — `has`,
  `enumerable === false`, `keys === ["message"]` — and mutant C reds it at `has`, which is the half that makes
  deletion loud.
- **C1** seeds `WIRE_BLOCK`, pins `injects === 1`, asserts `[]` after; mutant C reds the clear assertion, so the
  seed and the forced refresh were both live.
- **The knob is inert by default, measured** — the full suite run by the reviewer: 283 pass, 0 fail, `pipefail`.
- `plugin/src`, `plugin/dist` and `crates` untouched, `src`↔`dist` identical, dist-sync green.
- The prose/fixture discrepancy is recorded above and the fixture was left alone, as instructed.

**What this closes and what it does not.** The seam now has end-to-end coverage: the empty-SUCCESS clear can no
longer die silently, and its non-enumerability is asserted on an object a test actually holds. It does not make
issue 155's keep-on-failure path live-observed — that still needs a real refresh failure, and no reading is
claimed for it.
