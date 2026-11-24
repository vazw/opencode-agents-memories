# 161 — the debug inject probe drops the block's first heading, because the block glues it to the open tag

Status: ready-for-human — 2026-11-24 (MEASURED; two candidate fixes, one of them a byte-pinned engine change)

## The defect

`OPENCODE_MEM_DEBUG_INJECT=1` adds `sections` to `inject.received` so a block's headings can be read without parsing
bytes. It under-reports: the first heading is never in the list.

`debugInjection` (`plugin/src/plugin.js:269-279`) splits the block on `"\n"` and requires `line.startsWith("## ")`.
The block's first heading is not at a line start: `render_history_body` (`crates/mem-bridge/src/inject.rs:1080-1085`)
builds `body` as `String::from(HISTORY_BLOCK_OPEN)` and then pushes the heading immediately, with no `\n` between. So the
history body literally reads:

```
<project_agent_session_history>## Project facts
```

## Measurements (2026-11-24, flag-on readings in `~/.opencode-mem/opencode-mem.log`)

- 30 `inject.received` readings carry a block prefix at all; **30 of 30** have a heading glued to the open tag.
- **30 of 30** — the glued heading is absent from that reading's `sections` array. Exactly one heading is dropped per
  reading, always the first.
- The drop is not cosmetic in effect: it hid a working feature. Reading `sections` alone, `Recent actions` looked
  permanently absent; `block_prefix` shows it as the first heading in 5 readings, including both post-restart ones
  (`18:29:24`, `18:29:34`). Issue 149's closure needed the block read directly.

## Two candidate fixes, and what decides between them

1. **Engine — a newline after the open tag.** One line in `render_history_body` (`body.push('\n')` after the
   `String::from(HISTORY_BLOCK_OPEN)`). Fixes the probe *and* the block: today the first heading is not a markdown
   heading to any consumer, and the tag line is malformed. Cost risk: `inject_budget.rs:868` pins an exact block length
   (`474`, described as "192-byte frame + history bytes") and `inject.rs:1043` computes a budget from
   `HISTORY_BLOCK_OPEN.len() + HISTORY_BLOCK_CLOSE.len() + 2`; both must move with it. `compaction_capture_chain.rs`
   byte-exact assertions are about restore bytes, not this block — verify that rather than assume it.
2. **Probe only — normalise the split.** Treat `<project_agent_session_history>` as a line boundary in
   `debugInjection`. A one-line change, no byte-pin fallout, but it leaves the injected block malformed for the model and
   only teaches the instrument to accept the defect.

Preference is (1), because the block is what the model reads and the tag/heading collision is the root cause; (2) alone
would let two producers of the same text disagree indefinitely.

## Vacuity note

A fix is only proven if a flag-on reading logs a `sections` array containing the block's first heading, on a block whose
`block_prefix` shows that heading first. Re-running the scanner over the existing log proves nothing about a new engine.

## What this does not change

Issue 153's default-off shape stays. The instrument is correct in what it reads; it is the block that hands it a heading
mid-line.

## Outcome (2026-11-24T13:35Z)

Fix (1) shipped in the engine; the probe was left untouched, as the issue's preference and its "one normaliser" rule
require.

- `crates/mem-bridge/src/inject.rs` — `render_history_body` pushes `'\n'` after `HISTORY_BLOCK_OPEN`, so the block reads
  `<project_agent_session_history>\n## …`. The greedy invariant comment moved `emitted = 192 + 64 + Σ(h+1) + Σ(line+1)
  = used − 1 − 2n <= used`; `fixed_overhead_bytes()` keeps its `+ 2` margin (`used_at_zero = 257` unmoved, so the
  accepted row set cannot change).
- `crates/mem-bridge/tests/capture_inject_e2e.rs` — positive/negative body-shape pair on the real JSON-RPC inject
  block: `contains("<project_agent_session_history>\n## ")` and `!contains("<project_agent_session_history>## ")`. RED
  before the engine change at `:262` with the glued block printed; GREEN after.
- `crates/mem-bridge/tests/inject_budget.rs` — the one exact-length pin moved `474 → 475` (arithmetic comment
  `192 + 64 + 19 + 2×100 = 475`). RED was taken UNEDITED straight after the engine change: `left: 475`, `right: 474`.
- Gates: `cargo test -p mem-bridge` exit 0, 0 failed; `cargo fmt --all --check` exit 0; both `AGENTS.md` clippy gates
  exit 0 with no output; `cd plugin && node --test tests/*.test.mjs` 271/271; `node --test tests/dist-sync.test.mjs`
  2/2. `git diff --stat` touches no `plugin/` path, so no bundle rebuild.
- Deployment: engine rebuilt (`cargo build --release`), `plugin/engine/linux-x64/opencode-agents-memory` replaced
  (`33b5c73e1a094498…` → `115bfb382d914b48…`). `plugin/scripts/stage-engine.sh` stopped at `cp` with `Text file busy`
  because the running host holds the staged binary; the built release binary was installed with an atomic `mv` instead,
  and the staged UI bundle was moved aside to `.tmp/staged-web-ui-dist-20261124-1329` and moved back unchanged.

Live confirmation still needs the owner's restart with `OPENCODE_MEM_DEBUG_INJECT=1`: only then can a flag-on
`inject.received` show `sections` beginning with the block's first heading. Re-reading the existing log proves nothing
about this build.

## Live verification — 2026-11-24T22:47:45 restart

MEASURED, and this is the wave's payoff: the probe can no longer hide a first heading.

```
pid 28153  /proc/28153/exe sha256 = 115bfb382d914b48   started 22:47:45
staged     plugin/engine/linux-x64/opencode-agents-memory sha256 = 115bfb382d914b48
```

The release build the reviewer reproduced from `target/release` is what runs. Rollback dirs untouched:
`linux-x64.old-5a4622b0/`, `linux-x64.old-6fe8acd0/`.

Three consecutive `block_prefix` readings from `~/.opencode-mem/opencode-mem.log`, the pre-restart one as the
control:

| reading | block_prefix | sections |
|---|---|---|
| 22:01:30 (pre-restart) | `<project_agent_session_history>## Project facts` | first heading absent |
| 22:48:30 (post) | `<project_agent_session_history>\n## Recent actions` | `["Recent actions","Project facts","Recent sessions"]` |
| 22:49:35 (post) | `<project_agent_session_history>\n## Project facts` | `["Project facts","Recent sessions"]` |

The first element of `sections` is now the block's first heading, which is exactly the observation the defect
denied. The 30-of-30 under-report is gone.

Scope of the claim: this verifies the block shape and the probe's agreement with it. It does not re-verify the
budget arithmetic, which the crate's tests pin.
