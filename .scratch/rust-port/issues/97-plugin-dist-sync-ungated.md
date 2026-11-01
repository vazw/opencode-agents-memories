# 97 — `plugin/dist/` is what the host loads, and no commit gate keeps it byte-identical to `plugin/src/`

Status: ready-for-agent — 2026-11-01. Work will add a `plugin/**` branch to `.githooks/pre-commit`
(after the P4 fast path, before the Rust-only exit) and its behavioural cases to
`scripts/test-pre-commit.sh`; the check itself is the existing
`cd plugin && node --test tests/dist-sync.test.mjs`. No production file changes.

## What to build

Every wrapper-touching change must regenerate `plugin/dist/plugin.js`. That obligation is written down
(`.githooks/pre-commit`'s header says it runs no tests; the blueprint convention records "copy src over
dist" as a step) and asserted by a test
(`plugin/tests/dist-sync.test.mjs`: `test_dist_plugin_byte_identical_to_src`, a raw `Buffer` compare
equivalent to `cmp`) — but nothing in a commit forces either, because the gate exits before looking.

MEASURED, against the tree:

- `plugin/package.json` resolves the host's import to `dist`: `"main": "dist/plugin.js"`, and every
  `exports` entry (`"."`, `"./server"`, `"./v2"`, `"./tui"`) points at a `dist/` file. A stale `dist` is
  a stale plugin.
- `.githooks/pre-commit` P4 exits 0 when no staged path matches `*.rs | Cargo.toml | Cargo.lock`
  (`grep -n plugin .githooks/pre-commit` returns nothing). A commit staging only `plugin/src/plugin.js`
  therefore runs **no** check at all — cargo is skipped by design, and the node suite is never invoked.
- The pre-commit gate explicitly "runs no tests", so wiring `dist-sync.test.mjs` into it is a deliberate
  widening of a lint-and-format gate and needs a sentence in `docs/adr/0016-pre-commit-gate.md`
  (cost: one `node` process, no cargo).

So the defect is not the missing test — it is that the only enforcement runs in a suite no gate calls.

## Acceptance criteria

- [ ] A commit staging `plugin/src/plugin.js` without a matching `plugin/dist/plugin.js` **fails** the
      hook, with a message naming `dist-sync.test.mjs` and the copy command.
- [ ] A commit staging both files, byte-identical, **passes** (no cargo invoked — the fast path still
      exits for a plugin-only change that is consistent).
- [ ] The hook stays non-mutating: it **reads** and compares, it never copies `src` over `dist` behind the
      author's back. (Its own header promises "the hook never mutates the repository".)
- [ ] `scripts/test-pre-commit.sh` gains behavioural cases for both directions (a passing pair, a
      divergent pair, and a `plugin/` path staged alongside a Rust path so the two gates compose), and
      `bash scripts/test-pre-commit.sh` still prints `PASS n/n`.
- [ ] `docs/adr/0016-pre-commit-gate.md` records the widened gate and its measured cost.

## Blocked by

None.

## Evidence

Measured 2026-11-01 while auditing the issue set: `plugin/package.json` `main`/`exports` → `dist/`;
`plugin/tests/dist-sync.test.mjs` byte-compares `src/plugin.js` to `dist/plugin.js`;
`grep -n plugin .githooks/pre-commit` returns nothing. Issue 83's approved plan names
`plugin/dist/plugin.js` as a required artifact ("without it `dist-sync.test.mjs` fails and nothing
ships") — this issue is why that sentence needs a gate behind it.
