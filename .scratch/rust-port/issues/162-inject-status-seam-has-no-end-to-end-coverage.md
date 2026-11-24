# Issue 162 — the wrapper→adapter `injectStatus` seam has no end-to-end coverage

Status: ready-for-agent
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

## Not in scope

- Issue 152's residuals (a) and (b) — uncleared, and they need a live host.
- The `[exited]`-without-pid nuance the reviewer noted (`shellNote`, the exited branch emits `[exited]` before
  the pid guard): blueprint snippet verbatim, no pid ever reaches `process.kill`, no defect.
