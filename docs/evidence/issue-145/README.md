# Issue 145 — post-restart runbook

Measures, per child, whether the `<memory_context>` block reaches a child after compaction and whether it names the
child's last action. Run after the next restart of the host process. That restart also takes the engine with it, and the
restart is the owner's call. This runbook does not restart, signal or write to either process. Pids are deliberately not
recorded here — read them, do not trust a copy.

Read-only on the host, the engine and both stores. The one write this runbook permits is a temporary project-memory
nonce, created and deleted inside Q1b and Q6; nothing else is written and no file under `~/.opencode-mem/data` is touched.
Every command records `date -u +%FT%TZ` and its rc. The owner triggers compaction; this runbook does not. Counts are
recorded with timestamps. No count is pinned here.

**The instrument is behavioural, not forensic.** Delivery is decided by whether a child can produce a token that exists
only inside the block, and action recall by whether it can name its own last action. The host's session store is not
interrogated for block content — see trap A for the measurement that removed that reading. Q9 remains a log reading,
because the wrapper's own log is where the engine's answer is written.

**Shell half is unmet.** `crates/mem-bridge/src/inject.rs` has no background-shell section, so a child cannot be told which
shell it left running. That is issue 146. Q8 is not run while the block has no shell section. Nothing in this runbook
measures the shell half.

## R0 — schema check, calibration, compaction boundary (one existing child, read-only)

Run S0 first. A missing column stops the runbook. Record `uncalibrated (schema drift: <missing columns>)` and do not run Q0.

S0 (schema):

```sh
date -u +%FT%TZ; sqlite3 -readonly ~/.local/share/opencode/opencode.db "pragma table_info(session_message);"; echo rc=$?
date -u +%FT%TZ; sqlite3 -readonly ~/.local/share/opencode/opencode.db "pragma table_info(session_v2);"; echo rc=$?
```

`session_message` must list `session_id`, `type`, `data`, `seq`. `session_v2` must list `id` and `parent_id`.

Q0 (compaction boundary, for one child `<child>`):

```sh
date -u +%FT%TZ; sqlite3 -readonly ~/.local/share/opencode/opencode.db "select max(seq) from session_message where session_id='<child>' and type='compaction';"; echo rc=$?
```

Expected shape: one integer `<seq>`. A null result means the child has not compacted. Record `not compacted` for that child and skip Q6 to Q9 for it. That is not a miss and not a reading.

Convert the boundary's time for Q9:

```sh
date -u +%FT%TZ; sqlite3 -readonly ~/.local/share/opencode/opencode.db "select strftime('%Y-%m-%dT%H:%M:%fZ', json_extract(data,'$.time.created')/1000.0, 'unixepoch') from session_message where session_id='<child>' and seq=<seq>;"; echo rc=$?
```

`time.created` is an integer epoch in milliseconds. Never compare the raw value with a string.

Optional cross-check, not a discovery step:

```sh
date -u +%FT%TZ; sqlite3 -readonly ~/.local/share/opencode/opencode.db "select type, count(*) from session_message where session_id='<child>' group by type;"; echo rc=$?
```

A compacted child lists one or more `compaction` rows — **count them, that is the reliable indicator**. Do not read
`session_v2.time_compacting`: MEASURED 2026-11-19 on a child that compacted three times, it stayed `NULL` throughout, so a
null there is not evidence of no compaction.

Worked example: `docs/evidence/issue-145/probe-compaction-observations.md` holds one probe child that compacted for real.
Its figures are one child's rows at one moment and must be re-derived before quoting; this runbook copies none of them.

Q1 (calibration, log side):

```sh
date -u +%FT%TZ; grep -c '"host_contract"' ~/.opencode-mem/opencode-mem.log; echo rc=$?
tail -5 ~/.opencode-mem/opencode-mem.log | grep 'inject.received'; echo rc=$?
```

Both must be non-zero and the last `inject.received` must carry `"level":"info"`. Zero is recorded as `uncalibrated` and
the runbook stops — the engine half is not running and a child's `NONE` would measure nothing.

Q1b (calibration, instrument side — run once per run, before any child):

1. Write a fresh nonce to memory, project scope: `PROBE-NONCE-<8 hex>`.
2. Spawn **one throwaway child** whose prompt names only the *pattern* — "quote any string matching
   `PROBE-NONCE-<8 hex>` you find in your context, or answer `NONE`" — never the value.
3. It must quote the nonce back. Delete the memory entry afterwards.

A calibration child that answers `NONE` means the instrument is broken (memory not indexed, or the prompt named a value
the child could not see). Record `uncalibrated (instrument)` and stop. Do not record `miss-host` for any child in that
run — a zero from a broken instrument is not a reading. This is why calibration child and measured children are
separated: the same `NONE` means opposite things depending on which side of that line it falls.

## Traps — stated before any Q runs

**Trap A: this host build does not persist the block, so no database query can read delivery.** MEASURED 2026-11-19: the
predicate `type='synthetic' and data like '{"text":"<memory_context>%'` matches a handful of sessions from mid-October,
newest `2026-10-11 22:47:50`, and **none in the five weeks since** — across every `inject.received` event logged in that
window, including sessions that demonstrably received the block. An earlier revision of this runbook read delivery from
that predicate; it would have classified every child `miss-host` while delivery worked. Filed as issue 148. Do not
reintroduce it, and do not treat the October rows as a calibration signal — they are a dead build's shape, not a live one.

**Trap B: loosening the content match is worse than the dead predicate.** `type='synthetic'` on this build is mostly
subagent completion notices (`{"metadata":{"source":"subagent","childID":…}}`), and
`data like '%<memory_context>%'` returns them by the hundred because a child's *answer* quotes the tag. A loosened query
measures the conversation, not the injection. There is no database reading of delivery here; the instrument is the
nonce.

**Trap C: asking a model whether it can see a block is not a measurement.** MEASURED: a child answered
`<memory_context>` **absent** while the log for its own session already carried an `inject.received` line with a non-zero
`block_bytes` and `"status":"ok"`. Asked instead to *produce* a token that exists only inside the block, the next child
found it immediately, with provenance the prompt never contained. Every question in this runbook asks a child to output
something or to name its own last action. None asks it to describe its context.

**Trap D: "the token appears after the compaction point" is not delivery evidence by itself.** A token found inside a
`compaction` row is contamination — the host summary embeds earlier turns verbatim. Contamination is not classification:
a child's reading comes from Q6 to Q9 in the precedence order below, first match wins. Worked example, with its own
re-derivation warning: `docs/evidence/issue-145/probe-compaction-observations.md`.

## R1 — host on the new graph

```sh
date -u +%FT%TZ; grep -n 'host_contract' ~/.opencode-mem/opencode-mem.log | tail -5; echo rc=$?
```

Expected: `"host_contract":"setup"` on the V2 host's inject lines. None means the host still runs the pre-change graph; the
owner restarts again. A V1 host reports `"host_contract":"server"`. Record it; that is not a wave failure.

## R2 — per child, owner setup

The owner starts 5 fresh child runs from a different directory. Each child is told to start one background shell with a
known name, then perform one known action (for example, a file read with a known path), then wait. The owner records each
child's `session_id`, the shell name and the action token. The owner then triggers compaction in the child and **stops
there** — no follow-up turn yet. The follow-up is sent in R3, after the nonce exists, so that the post-compaction block
is the nonce's only possible source.

## R3 — per child, after compaction (read-only on the processes, writes only memory)

`<child>` and the boundary `<seq>` come from R0.

**Order matters: the nonce is written *after* the boundary, never before.** A nonce present before compaction could be
answered from the child's own still-resident pre-compaction window, which would measure nothing. Written after the
boundary, the post-compaction block is its only possible source.

Q6 (block delivered after compaction):

1. Write a fresh nonce to memory, project scope: `PROBE-NONCE-<8 hex>`.
2. Send the child one follow-up: end its wait, then ask it to quote any string matching `PROBE-NONCE-<8 hex>` it can
   find in its context, or answer `NONE`. Name the pattern, never the value.
3. Record the child's answer verbatim. Echo ⇒ Q6 ≥ 1. `NONE` ⇒ Q6 = 0.
4. Delete the memory entry.

Q7 (last action named by the child):

Ask the same follow-up, in the same turn, what the child's last action before compaction was — the file it read, the
command it ran — **without stating it**. Compare with the owner's `<action-token>` from R2. Q7 ≥ 1 on a match, Q7 = 0
otherwise. Do not read action presence out of the block's text: with M1 deferred the block carries no recent-action
section, and asking the child is the behaviour the complaint is about.

The question must be about an action that **predates the boundary**. Asked "what did you just do", a child answers from
its own retained post-compaction window and is right for no reason connected to the block — MEASURED 2026-11-19, where
such an answer matched the recorded tool call and still proved nothing (`run-20261118T215648Z`, Q7, recorded
inconclusive). For the same reason the nonce in Q6 must be written **after** the boundary, never before.

Q8 (running shell present): not run while the block has no shell section
(`grep -c -i 'shell' crates/mem-bridge/src/inject.rs` = 0 at the wave's Step 0 item 7). If a later build adds one, ask
the child to name the shell it left running and compare with R2's `<shell-name>`.

Q9 (engine answer):

```sh
date -u +%FT%TZ; grep -n '"session_id":"<child>"' ~/.opencode-mem/opencode-mem.log | grep 'inject.received'; echo rc=$?
```

Keep only the last two lines and compare them with the boundary time, not with `<seq>` — the log is not sequenced. The
log timestamp is ISO-8601 with a `+07:00` offset, for example `[2026-11-19T01:16:26.707+07:00]`. Normalise it to UTC by
subtracting the offset, then compare against the Q0 `time.created` string. Comparing raw milliseconds against a string
misclassifies every child as `miss-engine`.

Record verbatim the lines at or after the boundary, with `date`, `status`, `block_bytes` and `host_contract`. A session
with no inject line at all: `OPEN — engine answer not recorded`. That is not a reading. A session whose inject lines
all precede the boundary: `no-inject-after-compaction`, reading 3 below — a real outcome with a named mechanism.

## Reading rule — one reading per child, first match wins

1. `miss-engine`: Q9 shows `status != "ok"`, or `block_bytes` is 0. The engine answered nothing or timed out.
2. `miss-host`: Q9 shows `status:"ok"` and `block_bytes > 0`, and Q6 = 0 — the engine returned a block and the child
   could not produce a token that exists only inside it. Delivery failed downstream of the engine.
3. `no-inject-after-compaction`: Q9 shows at least one `inject.received` with `status:"ok"` and `block_bytes > 0`, all
   strictly **before** the Q0 boundary, and none at or after it. The session got a block at some point and got nothing
   once it compacted. MEASURED 2026-11-19 (`run-20261118T215648Z`): with `inject_on: "first"` — the built-in default
   (`crates/mem-core/src/config/schema.rs:749`, asserted at `crates/mem-core/tests/config_defaults.rs:122`) — the
   turn-0 block is cached and the adapter's `context` hook replays it every turn
   (`plugin/src/v2/adapter.js:86-89`), while `:67` returns before the refresh when the cache is non-empty
   (`plugin/src/plugin.js:1507`). Nothing invalidates the cache at compaction (`adapter.js:122`). This reading
   neither refutes nor confirms a recent-actions section: it says the section cannot be measured until the cached
   block is dropped at compaction. Inverted world: a post-compaction `inject.requested` line, which would mean the
   refresh path ran and the child belongs to reading 4 or 5 instead.
4. `stale`: Q6 ≥ 1 — the block demonstrably reached the child after compaction — and Q7 = 0. The child received the
   block and still could not name its last action. This was the predicted reading before the 2026-11-19 baseline and
   did not survive it; it becomes reachable again only once the cached block is invalidated at compaction. The
   inverted world is a `recent-action-delivered`, which would refute the deferral's premise and is a finding, not a pass.
5. `recent-action-delivered`: Q6 ≥ 1 and Q7 ≥ 1. The block was delivered after compaction and the child named its last
   action. This reading makes no claim about background shells.

Run-level gap, outside the five readings: while the block has no shell section, record once
`OPEN — block carries no background-shell section`, with the `grep -c -i 'shell' crates/mem-bridge/src/inject.rs` output
verbatim. This opens issue 146. It is not a per-child reading.

## Expected shape and sample size

Expected per child, as of the 2026-11-19 baseline (`run-20261118T215648Z`): **`no-inject-after-compaction`** — the block
arrives before the boundary and nothing arrives after it. `stale` (Q6 ≥ 1 and Q7 = 0) was the pre-baseline prediction and
did not survive it; it returns only once the cached block is invalidated at compaction (issue 149, amendment 2026-11-19).
The shell half is not expected while the block has no shell section. A `recent-action-delivered` is the surprise, not the
target — record it and re-derive what produced it.

Sample size N = 5. Any `miss-engine` or `miss-host` at N = 5 extends to N = 10. A failure that survives N = 10 is an
open item in the issue. `stale` and `no-inject-after-compaction` need no extension — the latter is the predicted reading
as of the 2026-11-19 baseline, and its count at N = 5 is the R3 result. Issue 145 stays `OPEN` until issue 146 closes.

## Arm naming, from R3 readings only

Recorded in the issue, not decided in advance.

- `miss-engine` with `status:"error"`: the chat-path timeout (arm (a)). `CHAT_INJECT_REQUEST_TIMEOUT_MS` is a separate design.
- `miss-host`: attach/deliver (arm (b) candidate) — the engine returned a block that never reached the child.
- `miss-engine` with `status:"ok"`: neither arm; opens a new item.
- `stale`: the M1 gap; feeds issue 146's sibling rather than opening a new arm. A `stale` count above zero
  is evidence for building recent-action memory, not a defect in what shipped.
- `no-inject-after-compaction`: a delivery-path gap upstream of any section the block carries, measured at
  `adapter.js:67`/`plugin.js:1507`. Opens its own item — it is why a recent-actions section cannot be measured until the
  cached block is dropped at compaction. Not evidence for or against the section itself.
- Block-content gap: opens issue 146.

## Failure meaning

- `miss-engine`: the engine gave no block or timed out after compaction.
- `miss-host`: the engine returned a block and the child could not produce a token that exists only inside it.
- `no-inject-after-compaction`: the engine produced a block earlier in the session and produced none after the boundary,
  because the cached turn-0 block is replayed rather than refreshed.
- `stale`: the block reached the child and it still could not name its last action.

Each opens a new item in the issue. None reopens the scratch-root closure.

## Records

Every row, count, timestamp and rc goes in the issue's evidence section. The tracker status line gets the R3 totals for the
five reading classes, with N, and the run-level issue 146 gap stated beside them.

Per-run reading record, mandatory, one file per run: `docs/evidence/issue-145/run-<UTC-stamp>.md`. Creation is enforced:

```sh
( set -C; printf 'run %s\n' "$(date -u +%FT%TZ)" > "docs/evidence/issue-145/run-<UTC-stamp>.md" )
```

`noclobber` makes the redirect fail when the file already exists, so a second run in the same UTC second or a re-run on the
same stamp is refused rather than silently overwriting. A refused creation is recorded as `OPEN — run record exists` (with
the intended path) and that run records no result; choose a new stamp and start a new record. After a successful creation,
append the per-child lines with `>>`, one stamped line per child: `date -u +%FT%TZ`, `<child>`, the boundary `<seq>` from Q0,
the child's verbatim Q6 answer (the nonce it quoted, or `NONE`), its verbatim Q7 answer (the action it named, or `NONE`),
the Q9 `status`, `block_bytes` and `host_contract`, and that child's single five-reading classification. Also append the
Q1b calibration child's verbatim answer once per run — a run whose calibration failed records no per-child readings at
all. A run that writes no such file has no auditable outcome and records no result.
