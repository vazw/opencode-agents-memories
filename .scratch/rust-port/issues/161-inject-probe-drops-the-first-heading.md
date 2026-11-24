# 161 — the debug inject probe drops the block's first heading, because the block glues it to the open tag

Status: needs-triage — 2026-11-24 (MEASURED; two candidate fixes, one of them a byte-pinned engine change)

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
