# Probe observations — a child agent after real compaction (issue 145)

Recorded 2026-11-18T16:43Z. Read-only measurements against the live OpenCode store
`~/.local/share/opencode/opencode.db`. Nothing here is a projection and nothing is
pinned as a general count: every figure is one child's rows at one moment, and it
will drift. Re-run the queries before quoting any number.

Purpose: this is the durable home for the probe figures. The wave ledger lives under
`.tmp/progress/`, which is gitignored (`.gitignore:6`), so a measurement kept only
there is one `rm -rf` from gone. Figures also do not belong in the blueprint — a plan
that pins a live count contradicts its own "no count is pinned" rule.

**Historical reading — do not follow it as procedure.** This file records a probe run on
2026-11-18 whose delivery discriminator was a `type='synthetic'` row of injected shape. By
2026-11-19 that reading was measured dead on the current host build: it returns rows only
from mid-October and nothing since, so it cannot distinguish delivery from a miss. See issue
148 (`../../../.scratch/rust-port/issues/148-runbook-delivery-reading-dead-on-current-host.md`)
and the current runbook `README.md` beside this file, whose instrument is a nonce the child
must quote back. The figures below remain valid as one child's rows at one moment; the
*classification* they were used for is superseded. The behavioural finding — a compacted
child kept its token and shell id but forgot the nine files it had read — stands and does
not depend on the dead query.

## Subject

The probe child of this wave: `ses_e11216314ffe32Ksh7sE4QE7OX`. It was asked to write a
token line, start a long background sleep, read a queue of large files, and then answer
four questions from memory only once it compacted.

## Queries and readings

Boundary, taken as the maximum `seq` of the child's `type='compaction'` rows:

```sh
sqlite3 -readonly "$DB" "select max(seq), count(*) from session_message
  where session_id='ses_e11216314ffe32Ksh7sE4QE7OX' and type='compaction';"
```

Result, 2026-11-18: a boundary at `seq 1654`, with 14 compaction rows in the session.

Rows after the boundary:

```sh
sqlite3 -readonly "$DB" "select seq, type, length(data) from session_message
  where session_id='<child>' and seq > <boundary> order by seq;"
```

Result: two rows — one `assistant`, one `idle`. **Zero rows of `type='synthetic'`**, so
the narrower injected-shape query (`data like '{"text":"<memory_context>%'`) is also 0.

Whether the boundary row itself carries the tested strings:

```sh
sqlite3 -readonly "$DB" "select seq, length(data),
  instr(data,'PROBE-TOKEN-1')>0,
  instr(data,'<shell id>')>0,
  instr(data,'<memory_context>')>0
  from session_message where session_id='<child>' and type='compaction'
  order by seq desc limit 3;"
```

Result: the compaction row carried the token, the shell id **and** `<memory_context>`,
because the host's compaction summary embeds earlier turns of the conversation.

## What the child actually recalled

Asked from memory only, after the compaction, with reading forbidden:

- the exact token line — recovered
- the background shell's id and command — recovered
- the nine files it had read in the earlier phase — **forgotten**; the summary preserved
  only the count ("9 files via 10 read calls"), and it refused to guess or re-read
- a `<memory_context>` block after the compaction point — **none**; only the
  `<conversation-checkpoint>` summary was present

## Trap A — a `<memory_context>` content query over-counts

Every `compaction` row carries the text of earlier user messages, so a content query for
`<memory_context>` **without** `type='synthetic'` returns the compaction rows themselves.
Any such hit must be discarded, not counted as delivery.

## Trap B — a token after the boundary is not delivery evidence

The host's summary can carry a token that memory injection never delivered. The
discriminating condition is a `type='synthetic'` row of injected shape **after** the
boundary; a hit inside a `compaction` row is contaminated. Delivery therefore requires
the synthetic row, and nothing less.

## Why this matters to the wave

The child would have re-read nine files and re-checked a shell it could not name. The
host summary is lossy and unselective: it kept a literal token and a shell id and dropped
the work record. Which arm this is — the engine never composed a block, or the host never
attached one — **cannot be named** from these readings, because the live host log has no
`inject.received` line at all. That instrument is what the sink in this wave ships; the
live measurement of it is the post-restart runbook, not this file.

The empty-inject ruling is likewise not exercised here: an empty block is a normal branch
and never toasts.
