# Issue 145 — M1: neither arm can be named from what is on disk (2026-11-26)

M1 was deferred pending the post-restart measurement. The restart happened (2026-11-25T04:21:39Z; the engine was
restaged again 2026-11-26T06:59:57Z). The measurement is now possible, and its answer is **negative for both arms,
for a measured reason**: every reading that could have named one is either fixture noise or structurally unable to
fail. Nothing here claims delivery fails or succeeds — it is a claim about what these readings can decide.

Labels: **MEASURED** = executed, output in `run.txt`; **READ** = read from source.

## 1. The stored marker has never carried an injection since 2026-10-11 (MEASURED)

Issue 145's ground truth was: *a session received the block iff a stored message carries `<memory_context>`*.

Re-derived on this database with the shape that actually represents an injection
(`type='synthetic' AND data LIKE '{"text":"<memory_context>%'`):

| quantity | value |
|---|---|
| rows | **8** |
| sessions | 8 |
| date range | **2026-10-11 13:29:17 … 2026-10-11 15:47:50** |
| rows after 2026-10-11 | **0** |

That re-derives the runbook's Trap A and issue 148 from this database. The injection shape died on 2026-10-11.

The predicate the issue's headline table used is the *loose* one, `data LIKE '%<memory_context>%'`, and it is 1,645
rows made up of:

| message type | rows | sessions |
|---|---|---|
| `assistant` | 1,500 | 276 |
| `synthetic` | 57 | 11 |
| `user` | 52 | 45 |
| `compaction` | 36 | 8 |

So the loose count is dominated by **assistants quoting the tag back** — the runbook's Trap B. It can support
"this session had the tag in its context at some point". It cannot date a delivery, cannot attribute one to an
`inject.received` line, and says nothing when it is absent. It was never a per-delivery instrument.

## 2. Arm (a), the engine timeout, has zero live witnesses (MEASURED)

All 353 `status:"error"` lines in the entire log history fall on 2026-11-19, and **all 353 belong to three
plugin-test fixture ids** (`ses-1` 224, `sess-1` 128, `e2e-sess` 1). Non-fixture sessions carrying an error line:
**0**.

`opencode-mem-2026-11-19.log` carries 5,046 fixture inject lines; the other 24 log files carry none. The
"chat-path timeout" population the issue's arm table describes was test output written into the production log.

## 3. Arm (b) cannot be separated from persistence noise (MEASURED)

Child sessions with `inject.received` before 2026-11-25, split by the loose match:

| loose match | error lines | ok lines | sessions |
|---|---|---|---|
| present | none | yes | 67 |
| **absent** | none | yes | 179 |

No child session in the window has an error line, so the split is not arm (a); and per §1 the loose match neither
dates a delivery nor makes its absence mean anything. There is no third signal on disk.

Today's population, for scale (moving target — these are the committed run's values): 48 sessions produced
`context.hook` lines (47 with `has_block:true`, 41 of the 48 children), 72 `inject.received` lines were
`status:"ok"` with `block_bytes > 0`, and strict marker rows are 0 — the expected permanent state since
2026-10-11, not a fresh break.

## 4. Why no log line replaces the marker (READ)

Both candidate log signals are written **before** the mechanism they would have to indict:

- `inject.received` is emitted at `plugin/src/plugin.js:1457`; the `output.message.parts.push` that would have to
  fail is at `:1472`.
- `context.hook` is emitted at `plugin/src/v2/adapter.js:121`; the `event.system.push` that would have to fail is
  at `:123`.

A mutant deleting either push leaves both logs byte-identical. Under the vacuity rule they attest that the engine
answered and the cache held — never that the child received anything.

## 5. What does bite

A token that exists only inside the block and must be echoed by the model — the `PROBE-NONCE-<8 hex>` design the
runbook already specifies for R3 (Q6: the child quotes the nonce). That instrument fails when any of the three
pushes is deleted, because the nonce then never reaches the model. It is blocked on the owner's R2 (five fresh
children, each starting one named background shell plus one named action, compaction triggered by the owner).
That is the only open item on this issue.

## Reproduce

```sh
cd /home/opencode/Code/Personal/Rust/opencode-agents-memory
python3 docs/evidence/issue-145-m1-instrument-20261126/read.py
```

Read-only: opens `~/.local/share/opencode/opencode.db` with `mode=ro` and reads `~/.opencode-mem/*.log`.
Writes nothing.
