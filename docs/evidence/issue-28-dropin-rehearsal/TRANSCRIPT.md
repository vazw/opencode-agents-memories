# Issue 28 — drop-in rehearsal transcript (scaffold)

**Status: NOT RUN.** No cell below has been executed. Each one says `NOT RUN — <reason>`; replace a cell
only with verbatim captured output and its exit code. A cell that holds a computed or remembered value
instead of captured output is treated as unrun.

Runbook: `docs/runbooks/parity-drop-in-rehearsal.md` (§ numbers below are its sections).
Rehearsal root (`$REH`, printed by §2): `NOT RUN — rehearsal not executed`
Live root, never the subject: `/home/opencode/.opencode-mem/data` (engine pid 4719, port 3742).
Rehearsal port: **3749** (second-engine probe: 3750).

## Pre-registered values — fill these before S0

| item | value |
|---|---|
| `$STAMP` / `$REH` | NOT RUN — rehearsal not executed |
| expected occurrence count of `127.0.0.1:3742` in this transcript | NOT RUN — set from the filled transcript |
| expected occurrence count of `/home/opencode/.opencode-mem/data` in this transcript | NOT RUN — set from the filled transcript |

The two counts are declared *before* the run; §8's target-absence check compares the finished transcript
against them. They are the only place a computed number is allowed, and the value is the count of
legitimate read-only mentions (S0's live reads + the copy source lines).

---

## S0 — pre-state (read-only)

```
$ NOT RUN — rehearsal not executed
NOT RUN — rehearsal not executed
rc=NOT RUN — rehearsal not executed
```

Expected: one `ps` pid line, one `.engine.lock` sha256, one `stat` line, `43`, `0`, and
`live_total=6766 live_shards=36`. Recorded verbatim, whatever it is:

| capture | value |
|---|---|
| `ps -o pid=,lstart=,etime= -p 4719` | NOT RUN — rehearsal not executed |
| `.engine.lock` sha256 | NOT RUN — rehearsal not executed |
| live root dir `%Y` | NOT RUN — rehearsal not executed |
| live `*.db` count | NOT RUN — rehearsal not executed |
| live `*.legacy.bak` count | NOT RUN — rehearsal not executed |
| `live_total` / `live_shards` | NOT RUN — rehearsal not executed |

## S1 — preconditions

```
$ NOT RUN — rehearsal not executed
NOT RUN — rehearsal not executed
rc=NOT RUN — rehearsal not executed
```

| precondition | result |
|---|---|
| `engine_ok` | NOT RUN — rehearsal not executed |
| `plugin_ok` | NOT RUN — rehearsal not executed |
| `web_ok` | NOT RUN — rehearsal not executed |
| `token_ok` | NOT RUN — rehearsal not executed |
| `live_engine_ok` | NOT RUN — rehearsal not executed |
| `port_3749_free` / `port_3750_free` | NOT RUN — rehearsal not executed |
| live-engine process count (`1`) | NOT RUN — rehearsal not executed |

If any line is missing, the run stops here — and that is the whole record.

## S2 — copy, then verify the copy

```
$ NOT RUN — rehearsal not executed
NOT RUN — rehearsal not executed
rc=NOT RUN — rehearsal not executed
```

| copy-side measure | value | S0 reference | equal? |
|---|---|---|---|
| `dbs` | NOT RUN — rehearsal not executed | NOT RUN | NOT RUN |
| `shard_files` | NOT RUN — rehearsal not executed | NOT RUN | NOT RUN |
| `shards` (active rows) | NOT RUN — rehearsal not executed | NOT RUN | NOT RUN |
| `memories` (sum over shard files) | NOT RUN — rehearsal not executed | NOT RUN | NOT RUN |
| `integrity_check` `ok` count | NOT RUN — rehearsal not executed | NOT RUN | NOT RUN |
| `BACKUP_FAILED` count (must be 0) | NOT RUN — rehearsal not executed | 0 | NOT RUN |
| integrity failures (`grep -v ' ok$'`) | NOT RUN — rehearsal not executed | (empty) | NOT RUN |

Per-shard rows: `copy-rows.tsv` — NOT RUN — rehearsal not executed.
Per-DB integrity: `copy-integrity.txt` — NOT RUN — rehearsal not executed.

**Checkpoint:** every `equal?` cell is `yes`, or the run stops before S4. Uninstalling before this table is
filled is the failure this ordering exists to prevent.

## S3 — the memory that must survive (pre-registered)

```
$ NOT RUN — rehearsal not executed
NOT RUN — rehearsal not executed
rc=NOT RUN — rehearsal not executed
```

| item | value |
|---|---|
| shard file chosen (`db_path`, basename) | NOT RUN — rehearsal not executed |
| memory `id` | NOT RUN — rehearsal not executed |
| `created_at` | NOT RUN — rehearsal not executed |
| stored text, first 120 chars | NOT RUN — rehearsal not executed |
| `$Q_REF` (3–5 distinctive words, fixed here, never tuned later) | NOT RUN — rehearsal not executed |

## S4 — swap: TS out, port in (scratch config only)

```
$ NOT RUN — rehearsal not executed
NOT RUN — rehearsal not executed
rc=NOT RUN — rehearsal not executed
```

| evidence | value |
|---|---|
| `grep -c '"opencode-mem@latest"'` pre-state (expect 1) | NOT RUN — rehearsal not executed |
| `grep -c 'opencode-mem@'` after uninstall (expect 0) | NOT RUN — rehearsal not executed |
| `grep -n 'opencode-agents-memory/plugin'` after install (expect 1 line) | NOT RUN — rehearsal not executed |
| `diff live-reference → post-install` | NOT RUN — rehearsal not executed |
| `artifacts.sha256` (5 lines) | NOT RUN — rehearsal not executed |

## S5 — engine against the copy, then recall the pre-registered memory

```
$ NOT RUN — rehearsal not executed
NOT RUN — rehearsal not executed
rc=NOT RUN — rehearsal not executed
```

| observation | value | must equal |
|---|---|---|
| rehearsal engine pid (the only pid this run may signal) | NOT RUN — rehearsal not executed | — |
| `health` | NOT RUN — rehearsal not executed | `200` |
| `mint` (first gated call) | NOT RUN — rehearsal not executed | `401` |
| `token_len` | NOT RUN — rehearsal not executed | `64` |
| `total` / `shards` after migration | NOT RUN — rehearsal not executed | S2 copy numbers |
| recalled `id` | NOT RUN — rehearsal not executed | S3 id |
| recalled `similarity` | NOT RUN — rehearsal not executed | (record only) |
| recalled `content` prefix | NOT RUN — rehearsal not executed | S3 text |

## S6 — UI, REST surface, add round-trip

```
$ NOT RUN — rehearsal not executed
NOT RUN — rehearsal not executed
rc=NOT RUN — rehearsal not executed
```

| observation | value | must equal |
|---|---|---|
| `index` | NOT RUN — rehearsal not executed | `200` |
| `<title>` | NOT RUN — rehearsal not executed | `<title>OpenCode Memory Explorer</title>` |
| `asset` | NOT RUN — rehearsal not executed | `200` |
| `smoke-rest-surface.sh` tally + exit code | NOT RUN — rehearsal not executed | `rc=0`, 0 FAIL (a `STOP:`/`rc=2` is a finding, not a pass) |
| canary add response | NOT RUN — rehearsal not executed | `{"success":true,"data":{"id":"mem_…"}}` |
| canary search hit `id` | NOT RUN — rehearsal not executed | the id the add returned |

`rest-summary.log`, `rest/rows.tsv`, `canary-add.json`, `recall.json` — NOT RUN — rehearsal not executed.

## S7 — §0b D3 hatch: the second-engine refusal

```
$ NOT RUN — rehearsal not executed
NOT RUN — rehearsal not executed
rc=NOT RUN — rehearsal not executed
```

| observation | value | must equal |
|---|---|---|
| `second_engine_rc` | NOT RUN — rehearsal not executed | `75` |
| stderr `lock_lost` `holder_pid` | NOT RUN — rehearsal not executed | S5 engine pid |
| `<copy>/status` line | NOT RUN — rehearsal not executed | `degraded: engine lock held (pid=…, ts=…)` |

This cell records the **deferral** of the hatch, not its closure: the port is single-process by
construction, trigger (b) stays material, and the corruption / supported-path observation that would open
it has not appeared.

## S8 — rollback, and the proof the original was never written

```
$ NOT RUN — rehearsal not executed
NOT RUN — rehearsal not executed
rc=NOT RUN — rehearsal not executed
```

| proof | before (S0) | after | pass? |
|---|---|---|---|
| pid 4719 alive, same `lstart` | NOT RUN | NOT RUN | NOT RUN |
| `.engine.lock` sha256 | NOT RUN | NOT RUN | NOT RUN |
| live `*.db` count | NOT RUN | NOT RUN | NOT RUN |
| live `*.legacy.bak` count | NOT RUN | NOT RUN | NOT RUN |
| live root dir `%Y` | NOT RUN | NOT RUN | NOT RUN |
| live `live_total` / `live_shards` | NOT RUN | NOT RUN | NOT RUN |
| `cfg_cmp_rc` (live config vs `live-reference`) | — | NOT RUN | NOT RUN |

| target-absence check | count | pre-registered | pass? |
|---|---|---|---|
| `grep -c '127.0.0.1:3742'` over this file | NOT RUN | NOT RUN | NOT RUN |
| `grep -c '/home/opencode/.opencode-mem/data'` over this file | NOT RUN | NOT RUN | NOT RUN |

Rollback of the scratch config (`cp -a …live-reference` back), `rehearsal_engine_gone`,
`port_3749_free_again` — NOT RUN — rehearsal not executed.

A `NOT RUN` in an `after` cell means the proof was not taken; it does not mean the original was safe.

## S9 — Svelte dist against the Rust server

| cell | value |
|---|---|
| REST endpoints the Svelte client calls answer on 3749 | NOT RUN — recorded by S6 |
| built upstream dist exists on this host | NOT RUN — rehearsal not executed |
| dist served via `--web-dist`, `/` = 200, no failed `/api/*` in the console | NOT RUN — no built upstream dist on this host |

Split deliberately: endpoint parity is S6's evidence; "the Svelte dist loads against the Rust server" needs
a dist, and until one is built this cell stays `NOT RUN`. It is never reported as passed on endpoint
shapes alone.

## S10 — what this rehearsal does not prove

| claim | status |
|---|---|
| Windows | NOT RUN — no runner on this host |
| macOS | NOT RUN — no runner on this host |
| multi-user / shared store | NOT RUN — every path is single-owner |
| host plugin capture (prompt → memory) | NOT RUN — needs an OpenCode session; owner repeat (S11) |
| host plugin injection (`memory_context` frame) | NOT RUN — needs an OpenCode session; owner repeat (S11) |
| power loss during migration | NOT RUN — not simulated |
| the live swap itself | NOT RUN — owner repeat (S11) |

## S11 — the owner repeat (separate)

| step | value |
|---|---|
| `scripts/live-swap.sh --dry-run` | NOT RUN — owner-executed |
| one capture in a real session (`user-prompts.db` count grows) | NOT RUN — owner-executed |
| one injection in a real session | NOT RUN — owner-executed |
| live `live_total` + one recall-by-content on 3742 | NOT RUN — owner-executed |
| `--backup` stamp + `sha256sum -c MANIFEST.sha256` rc=0 | NOT RUN — owner-executed |

---

## Verdict cells (fill last, after every cell above is either captured or `NOT RUN — reason`)

| question | answer |
|---|---|
| memories still readable after the swap (S5 id + text) | NOT RUN |
| UI functional against the Rust server (S6) | NOT RUN |
| REST parity holds (S6) | NOT RUN |
| Svelte dist usable (S9) | NOT RUN |
| original root never written (S8) | NOT RUN |
| D3 hatch confirmed deferred (S7) | NOT RUN |
| issue 28 closable (needs S11 too) | NOT RUN |

Rules for this file: no blanks, no paraphrases, no computed value standing in for captured output, and no
edit to an expectation after the fact — a mismatch is recorded as a mismatch, then explained underneath it.
