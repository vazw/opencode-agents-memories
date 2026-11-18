# Issue 145 — post-restart runbook

Measures, per child, whether the `<memory_context>` block reaches a child after compaction and whether it names the
child's last action. Run cold after the next restart of host pid 20156. That restart also takes engine pid 4719 with it,
and the restart is the owner's call. This runbook does not restart, signal or write to either process.

Read-only throughout. Every command records `date -u +%FT%TZ` and its rc. The owner triggers compaction; this runbook
does not. Counts are recorded with timestamps. No count is pinned here.

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

A compacted child lists a `compaction` row. A `time_compacting` value in the session data may be recorded as a cross-check.

Worked example: `docs/evidence/issue-145/probe-compaction-observations.md` holds one probe child that compacted for real.
Its figures are one child's rows at one moment and must be re-derived before quoting; this runbook copies none of them.

Q1 (calibration, non-zero across the store):

```sh
date -u +%FT%TZ; sqlite3 -readonly ~/.local/share/opencode/opencode.db "select count(*) from session_message m join session_v2 v on v.id = m.session_id where v.parent_id is not null and m.type='synthetic' and m.data like '{\"text\":\"<memory_context>%';"; echo rc=$?
```

Zero is recorded as `uncalibrated` and the runbook stops. The `m.type='synthetic'` term is not optional — see trap A.

## Traps — stated before any Q runs

**Trap A: a `<memory_context>` content query without `m.type='synthetic'` over-counts.** Every `compaction` row carries
earlier user text verbatim, because the host summary embeds earlier turns, so the boundary row itself satisfies a bare
`like '%<memory_context>%'` match and reports presence where nothing was delivered. Any hit obtained without
`m.type='synthetic'` is discarded, never read as delivery. Every Q6, Q7 and Q8 above therefore carries `m.type='synthetic'`.
Worked example, with its own figures and its own re-derivation warning: `docs/evidence/issue-145/probe-compaction-observations.md`.

**Trap B: "the token appears after the compaction point" is not delivery evidence.** Delivery requires an injected-shape
row — `type='synthetic'` with `data` beginning `{"text":"<memory_context>` — at `seq > <seq>`. A token or shell string found
after the boundary is contamination when it sits inside a `compaction` row, because the host summary embeds earlier turns,
so a token recovered in conversation with Q6 = 0 is not delivery evidence at all. A Q7 or Q8 hit inside a `compaction` row
is contaminated and must be excluded. Contamination is not classification: the child's reading is computed from Q6 to Q9
in the precedence order below, first match wins. Worked example: `docs/evidence/issue-145/probe-compaction-observations.md`.

## R1 — host on the new graph

```sh
date -u +%FT%TZ; grep -n 'host_contract' ~/.opencode-mem/opencode-mem.log | tail -5; echo rc=$?
```

Expected: `"host_contract":"setup"` on the V2 host's inject lines. None means the host still runs the pre-change graph; the
owner restarts again. A V1 host reports `"host_contract":"server"`. Record it; that is not a wave failure.

## R2 — per child, owner setup

The owner starts 5 fresh child runs from a different directory. Each child is told to start one background shell with a
known name, then perform one known action (for example, a file read with a known path), then wait. The owner records each
child's `session_id`, the shell name and the action token. The owner then triggers compaction in the child and sends one
follow-up that requires continuing from the last action without re-running it.

## R3 — per child, after compaction (read-only, counts only)

`<child>` and the boundary `<seq>` come from R0.

Q6 (block after compaction):

```sh
date -u +%FT%TZ; sqlite3 -readonly ~/.local/share/opencode/opencode.db "select count(*) from session_message m where m.session_id='<child>' and m.type='synthetic' and m.data like '{\"text\":\"<memory_context>%' and m.seq > <seq>;"; echo rc=$?
```

Q7 (recent action present):

```sh
date -u +%FT%TZ; sqlite3 -readonly ~/.local/share/opencode/opencode.db "select count(*) from session_message m where m.session_id='<child>' and m.type='synthetic' and m.data like '{\"text\":\"<memory_context>%' and m.seq > <seq> and instr(m.data,'<action-token>')>0;"; echo rc=$?
```

Q8 (running shell present): not run while the block has no shell section (`grep -c -i 'shell' crates/mem-bridge/src/inject.rs` = 0 at Step 0 item 7 of the wave).
If a later build adds one, run the Q7 form with `'<shell-name>'`.

Q9 (engine answer):

```sh
date -u +%FT%TZ; grep -n '"session_id":"<child>"' ~/.opencode-mem/opencode-mem.log | grep 'inject.received'; echo rc=$?
```

Keep only the last line whose timestamp is at or after the child's compaction time. The log timestamp is ISO-8601 with a
`+07:00` offset, for example `[2026-11-18T20:55:49.558+07:00]`. Normalise it to UTC by subtracting the offset, then compare
the normalised string with the Q0 `time.created` string. Comparing raw milliseconds against a string misclassifies every
child as `miss-engine`.

Record that line verbatim with its `status` and `block_bytes`. No such line: `OPEN — engine answer not recorded`. That is not a reading.

## Reading rule — one reading per child, first match wins

1. `miss-engine`: Q9 shows `status != "ok"`, or `block_bytes` is 0. The engine answered nothing or timed out.
2. `miss-host`: Q9 shows `status:"ok"` and `block_bytes > 0`, and Q6 is 0. A block was delivered but never persisted to the child's session store.
3. `stale`: Q9 shows `status:"ok"` and `block_bytes > 0`, Q6 ≥ 1, and Q7 is 0. The block arrived but does not carry the last turn's action.
4. `recent-action-delivered`: Q9 shows `status:"ok"` and `block_bytes > 0`, Q6 ≥ 1, and Q7 ≥ 1. The block was persisted after compaction and names the last action. This reading makes no claim about background shells.

Run-level gap, outside the four readings: while the block has no shell section, record once
`OPEN — block carries no background-shell section`, with the `grep -c -i 'shell' crates/mem-bridge/src/inject.rs` output
verbatim. This opens issue 146. It is not a per-child reading.

## Expected shape and sample size

Expected per child: Q6 ≥ 1 and Q7 ≥ 1. The block's history section should name the action. The shell half is not expected
while the block has no shell section.

Sample size N = 5. Any `miss-engine`, `miss-host` or `stale` at N = 5 extends to N = 10. A failure that survives N = 10 is
an open item in the issue. Zero failures at N = 5 or N = 10 are the R3 result; issue 145 stays `OPEN` until issue 146 closes.

## Arm naming, from R3 readings only

Recorded in the issue, not decided in advance.

- `miss-engine` with `status:"error"`: the chat-path timeout (arm (a)). `CHAT_INJECT_REQUEST_TIMEOUT_MS` is a separate design.
- `miss-host`: attach/persist (arm (b) candidate).
- `miss-engine` with `status:"ok"`: neither arm; opens a new item.
- `stale`: opens a new item (the engine's block is not refreshed after compaction).
- Block-content gap: opens issue 146.

## Failure meaning

- `miss-engine`: the engine gave no block or timed out after compaction.
- `miss-host`: a block was delivered but no synthetic row landed in the child's session store.
- `stale`: the child got a block that does not reflect its last turn.

Each opens a new item in the issue. None reopens the scratch-root closure.

## Records

Every row, count, timestamp and rc goes in the issue's evidence section. The tracker status line gets the R3 totals for the
four reading classes, with N, and the run-level issue 146 gap stated beside them.

Per-run reading record, mandatory, one file per run: `docs/evidence/issue-145/run-<UTC-stamp>.md`. Creation is enforced:

```sh
( set -C; printf 'run %s\n' "$(date -u +%FT%TZ)" > "docs/evidence/issue-145/run-<UTC-stamp>.md" )
```

`noclobber` makes the redirect fail when the file already exists, so a second run in the same UTC second or a re-run on the
same stamp is refused rather than silently overwriting. A refused creation is recorded as `OPEN — run record exists` (with
the intended path) and that run records no result; choose a new stamp and start a new record. After a successful creation,
append the per-child lines with `>>`, one stamped line per child: `date -u +%FT%TZ`, `<child>`, the boundary `<seq>` from Q0,
the synthetic-row counts after the boundary (Q6, and Q7/Q8 when they are run), the Q9 `status` and `block_bytes`, and that
child's single four-reading classification. A run that writes no such file has no auditable outcome and records no result.
