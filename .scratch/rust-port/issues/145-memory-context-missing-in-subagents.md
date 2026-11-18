# 145 — the pre-turn `<memory_context>` block almost never reaches subagent sessions

Status: ready-for-agent — filed 2026-11-18 by owner check ("did it still work?"), measured below.

*(Related: issue 124, closed 2026-11-05 as "intermittent miss" — this issue supersedes its conclusion.)*

## The measurement

Ground truth is the query issue 124 established: a session received the block iff one of its stored
messages carries the literal `<memory_context>` marker (`session_message.data LIKE '%<memory_context>%'`).

**Today, 2026-11-18:**

| session | agent | messages | carrying `<memory_context>` |
|---|---|---|---|
| `ses_e9d5dd698ffe5lUV7W7s2KDpbh` | main (this session) | 18,948 | **180** (21 today) |
| `ses_e12b819a2ffeGsAVfmRbCN66Sg` | coder (issue 32) | 156 | **0** |
| `ses_e12c6c2f4ffe6LJN44vCpqC7Nb` | code-reviewer (141) | 80 | **0** |
| `ses_e12944535fferitJvVSfaFmSsa` | syseng (issue 28) | 79 | **0** |
| `ses_e12a283b2ffeui2vBlkdOnGrqN` | code-reviewer (32) | 62 | **0** |
| `ses_e12881697ffeSvAQxNoW5DEFah` | explore (throwaway probe) | 3 | **1** |

**Child sessions over all history in this database: 171 of 3,982 carry a block (4.3 %)**, and each of those
carries exactly **one**, at an arbitrary message position (`seq` 4, 41, 54, 84, 102, 126, 168, 183, 359, 368,
465, 2056) — never reliably on the spawn prompt, which is the turn that needs it.

### Correction, same day: the marker measures PERSISTENCE, and the debugger is a miss

The owner spotted the problem because the debugger child `ses_e130e6e2dffeyy1bGZz17qo9SM` (84 messages) does
**not** show `◈memory context` — and that session stores **0** markers, so UI and database agree there. An
earlier inject-delivery wave had recorded that ephemeral blocks are sometimes never saved in `session_message`,
so a stored marker is evidence of delivery but its absence is not proof against it; for this debugger both
signals say miss.

The decisive split is visible in the five misses and three hits spawned today, all of which attach at
**`seq 4`, the session's first user message**:

| session | agent | first user message (bytes) | block |
|---|---|---|---|
| `ses_e1283232cffeq1O3Glrm5REycu` | explore | 806 | **yes** |
| `ses_e1283232bffeqstrnlgeQ2bmHk` | explore | 806 | **yes** |
| `ses_e12881697ffeSvAQxNoW5DEFah` | explore | 878 | **yes** |
| `ses_e12b819a2ffeGsAVfmRbCN66Sg` | coder | 4,973 | no |
| `ses_e12944535fferitJvVSfaFmSsa` | syseng | 4,995 | no |
| `ses_e130e6e2dffeyy1bGZz17qo9SM` | debugger | 5,412 | no |
| `ses_e12c6c2f4ffe6LJN44vCpqC7Nb` | code-reviewer | 5,713 | no |
| `ses_e12a283b2ffeui2vBlkdOnGrqN` | code-reviewer | 6,322 | no |

Every fleet spawn prompt is 5–6 KB; every probe prompt is under 1 KB. **The size hypothesis is now dead:**
a padded `explore` probe given ~9,500 characters of inert filler plus the same four questions **received the
block** (`memory_context: yes`, 1 occurrence, profile and project facts present). Length is therefore not the
discriminator, and today's clean split was coincidence. Over all history the split never held either (children
with a ≥1 KB first prompt: 3,614 missed / 153 hit; under 1 KB: 167 missed / 12 hit).

The next single variable is **agent type** — and that is dead too: short-prompt probes on `coder` and `syseng`
(`ses_e12812aeaffe…`, `ses_e12812aeaffd…`) both answered `memory_context: yes` (1 occurrence, facts present,
~490 and ~540 prompt bytes). The **prompt shape** hypothesis is dead as well: a `coder` probe given the fleet's
exact preamble (`BLUEPRINT:`/`STAMP:`/`LEDGER:` + long task text, `ses_e12809c47ffe…`) also answered yes and
persisted 1 marker.

So five structural probes hit at 14:03–14:11 (explore ×4, coder, syseng; 3 messages each, 1–2 markers each),
and the four real spawns missed at 12:55–13:50. Every hit's spawn second has its `profile inject` line in the
engine log, and so does every miss (12:55:13, 13:11:14, 13:34:49, 13:50:22) — the engine answered in both
windows.

**Live hypothesis: contention with auto-capture.** The four misses fall inside a capture-heavy window (engine
log captures at 13:05:43, 13:06:33, 13:07:06, 13:07:41, 13:09:57, 13:10:26, 13:11:42, 13:35:03, 13:35:23,
13:43:58 — the 13:11:14 and 13:34:49 spawns sit within ~30 s of a capture), while the five hits landed in a
quiet window. UNPROVEN and not to be acted on: it is the only hypothesis left, and the sampler cannot see
`inject.received {block_bytes, status}` because it goes to `process.stderr` with no sink on disk. Instrumenting
that is step 1 and it is not optional.

**The engine is not the failing end.** `~/.opencode-mem/opencode-mem.log` logs `profile inject` at the exact
spawn second of every child measured above — 13:11:14 (coder), 12:55:13 (141 reviewer), 13:34:49 (32 reviewer),
13:50:22 (syseng), 14:03:41 (probe) — and 75 times today in total, against 22 blocks that landed (21 parent,
1 probe). So the request arrives and the profile is rendered; the block is lost afterwards.

**Prompt size is not the cause:** comparing each child session's first stored message with whether it ever
received a block gives average 5,287 bytes (never blocked, n=3,811) versus 5,814 bytes (blocked, n=171) —
indistinguishable.

## Where the loss is

INFERRED, and it is the one thing this issue exists to settle by measurement: the attach is
`plugin/src/plugin.js:1402`, `output.message.parts.push({ type: "text", text: block, synthetic: true })`,
unconditional whenever the engine returned a non-empty block. Either (a) the engine's `block` is empty for
child turns despite the `profile inject` line, or (b) the host does not persist a synthetic part pushed onto
a child session's `chat.message` output, while it does for a root session. The wrapper already emits
`inject.received {session_id, block_bytes, handling_ms, status}` that would separate them — **but it goes to
`process.stderr` (`plugin/src/plugin.js:214-221`), and `~/.local/share/opencode/log/opencode.log` contains
zero `opencode-mem-plugin` lines**, so the telemetry does not exist on disk.

## What to build

1. **Instrument first, and cheaply:** `writeStructured` also appends its JSON line to a file under the
   engine's log directory (path from the plugin's existing config/env resolution — do not invent a second
   location scheme). No new format, no new event names. Then one spawn reproduces the miss and says which
   of (a) or (b) it is, with `block_bytes` and `status` in hand.
2. **Fix the winning arm**, not both:
   - if `status = "empty"` on child turns, the fault is in the engine's query construction for a child prompt
     (fix there, and say which input differed);
   - if `status = "ok"` with `block_bytes > 0` and no block in the session, the fault is the attach/persist
     path, and the fix must not depend on mutating a message object the host may replace.
3. **A test that would have caught it.** The regression test must assert a **child** session receives the
   block, not that the wrapper returns one: spawn a child in a fixture, then assert its stored messages carry
   the marker. A test of the wrapper's return value alone is the shape that let this pass in the first place.

## Acceptance criteria

- [ ] `inject.received` and `inject.requested` are readable on disk after a spawn (path recorded in the issue
      comment, with the verbatim lines from one child that missed and one that hit).
- [ ] The losing arm (a) or (b) is named with evidence, and only that arm is fixed.
- [ ] A test fails on the pre-fix tree for a **child** session and passes after, with the RED output pasted
      (`plugin/tests/*.test.mjs`, wasm-free, `node --test`).
- [ ] Post-fix measurement, same query as above: N children spawned, N blocks delivered, for N ≥ 3, in the
      issue comment.
- [ ] No behaviour change for root sessions: their block count per turn stays as it is (21 of today's 75
      injects were root-side, and those already work).

## Blocked by

Nothing. Touches `plugin/src/plugin.js` only; independent of issues 142/143 (web API) and of the issue 28
rehearsal.
