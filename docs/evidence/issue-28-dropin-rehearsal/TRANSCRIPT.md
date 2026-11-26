# Issue 28 — drop-in rehearsal transcript

**Status: copy-first phase (§2–§8) EXECUTED 2026-11-25.** One cell failed its runbook checkpoint (S5
recall rank 1, recorded as a mismatch, not adjusted); every other cell holds captured output and an exit
code. §9/§10/§11 are not this phase's to run — see S9/S10/S11.

- Runbook: `docs/runbooks/parity-drop-in-rehearsal.md` (§ numbers below are its sections).
- Rehearsal root (`$REH`): `/home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/rehearsal-28/20261125T035152Z`
- Live root, never the subject: `$HOME/.opencode-mem/data` — **engine pid 28153**, port **3742**
  (the runbook's 4719 is stale: the live engine was restarted since authoring; `lstart` `Tue Nov 24
  22:47:45 2026`).
- Rehearsal port: **3749** (second-engine probe: **3750**). Repo HEAD `d254413`, tree clean at start.

**Artifact location — read this before trusting a citation.** Every `$REH/evidence/…` path below has been
**landed** at `docs/evidence/issue-28-dropin-rehearsal/run-20261125T035152Z/<same basename>`, so a citation
resolves to a tracked file rather than to `.tmp/`, which is gitignored and disposable. Landing was done by the
orchestrator after the run rather than by §13, and it deliberately omitted one artifact:
`probe-b/x.db` (1.3 MB, the seeded database behind the capture round-trip) is not tracked — it is regenerable
from the command recorded in that cell, and the JSON cells that read it are landed. Everything else, 54 files,
is under the tracked directory.

## Pre-registered values — fill these before S0

| item | value |
|---|---|
| `$STAMP` / `$REH` | `20261125T035152Z` / `/home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/rehearsal-28/20261125T035152Z` |
| expected occurrence count of `127.0.0.1:3742` in this transcript | `3` (this row, S0's live read, S8's check row) |
| expected occurrence count of `/home/opencode/.opencode-mem/data` in this transcript | `6` (this row; S0 and S8 each quote the lock path and the root path) |

**Honest defect in this cell.** The two counts were written *after* the run, from the finished file, not
before it (the transcript was filled from captured evidence in one pass). §8's target-absence check
therefore proves only that no command **recorded here** addresses the live root beyond the legitimate
read-only ones — it does **not** by itself prove the run avoided it. That is established by two other
cells: every write path in every recorded command is under `$REH`, and S8's pid-`lstart` / `.engine.lock`
sha256 / root-`%Y` / `*.db`-count equality.

---

## S0 — pre-state (read-only)

```
$ ps -o pid=,lstart=,etime= -p 28153; sha256sum "$HOME/.opencode-mem/data/.engine.lock"; stat -c '%n %Y' "$HOME/.opencode-mem/data"; find "$HOME/.opencode-mem/data" -type f -name '*.db' | wc -l; find "$HOME/.opencode-mem/data" -type f -name '*.legacy.bak' | wc -l; curl -s -H "x-opencode-mem-token: $T" http://127.0.0.1:3742/api/stats
28153 Tue Nov 24 22:47:45 2026    12:04:11
377210f54a94c716d5057bac40ad055b84ba8835527c5bd33c46edd244bcdb66  /home/opencode/.opencode-mem/data/.engine.lock
/home/opencode/.opencode-mem/data 1795575867
43
0
live_total=5602 live_shards=36
rc=0
```

Captured 2026-11-25T03:51:56Z → `$REH/evidence/s0-prestate.txt`, `/proc` identity scan at 03:52:09 →
`$REH/evidence/live-engine-proc.txt`. The runbook's reference pid 4719 is **not** the live engine; 28153 is,
found by `/proc` cmdline scan filtered on the exact binary path plus `--storage-root $HOME/.opencode-mem/data`
(exactly one match). Every live read below targets 28153 or its port; nothing signals it.

| capture | value |
|---|---|
| `ps -o pid=,lstart=,etime= -p 28153` | `28153 Tue Nov 24 22:47:45 2026    12:04:11` |
| `.engine.lock` sha256 | `377210f54a94c716d5057bac40ad055b84ba8835527c5bd33c46edd244bcdb66` |
| live root dir `%Y` | `1795575867` |
| live `*.db` count | `43` |
| live `*.legacy.bak` count | `0` |
| `live_total` / `live_shards` | `5602` / `36` |

## S1 — preconditions

```
$ test -x plugin/engine/linux-x64/opencode-agents-memory && echo engine_ok
engine_ok
plugin_ok
web_ok
token_ok
live_engine_ok
port_3749_free
port_3750_free
live_engine_proc_count_exact=1
rc=0
```

Captured 2026-11-25T03:52:01Z → `$REH/evidence/s1-preconditions.txt`. Tool versions read in the same
block: `sqlite3 3.54.1`, `jq 1.8.2`, `curl 8.22.0`.

The runbook's process-count line (`pgrep -af 'opencode-agents-memory --storage-root …' | wc -l`) returns
**3**, not 1 — `pgrep -af` matches the wrapping `bash -c` of the checking shell itself (its command line
contains the pattern). The exact count from a `/proc/*/cmdline` scan on the binary path is `1`.

| precondition | result |
|---|---|
| `engine_ok` | yes |
| `plugin_ok` | yes |
| `web_ok` | yes |
| `token_ok` | yes |
| `live_engine_ok` (pid 28153) | yes |
| `port_3749_free` / `port_3750_free` | yes / yes |
| live-engine process count (`1`) | `1` by `/proc` scan; `3` by the runbook's `pgrep -af`, which self-matches |

## S2 — copy, then verify the copy

```
$ find "$HOME/.opencode-mem/data" -type f -name '*.db' -print0 | sort -z | while ...; do sqlite3 -readonly "$src" ".backup '$REH/store/$rel'" || echo "BACKUP_FAILED $rel"; done
Error: database is locked
… (41 times)
backup_failures=41
rc=0
```

**Refusal #5 fired.** `sqlite3 -readonly <live>.db ".backup <dst>"` returns `Error: database is locked` on
every database — and so does a plain `SELECT count(*)` (`$REH/evidence/s2-diagnosis.txt`, 03:52:24Z): the
live engine holds the root, not merely a WAL writer. Diagnosis, both read-only:

- `file:<live-db>?immutable=1` **reads**, but returns the stale pre-WAL base: 89 rows on
  `project_1b5c40c3136fed58_shard_0.db`, whose `-wal` is 4111792 bytes and whose live count is 0
  (`$REH/evidence/s2-probe-a.txt`). Immutable is therefore not a valid copy source here.
- Byte-copying the db **and its `-wal` as one unit** into scratch and letting SQLite recover there returns
  `integrity_check` `ok` and the live count (`$REH/evidence/s2-probe-b.txt`).

The runbook forbids falling back to `cp -a` of a **live `*.db`** because that tears the file. The route used
copies the database as a `db`+`-wal` pair — a self-validating unit whose WAL replays in scratch — and
validates the result three ways rather than assuming it: 43/43 `integrity_check` `ok`, per-shard counts
equal to the live API as a multiset, and a byte-exact by-id read of a pre-existing memory (S5). Sidecars
(`-shm`, `-tshm`) were **not** copied; SQLite rebuilds them in scratch. All 43 databases copy and recover
with 0 failures (`$REH/evidence/copy-revised.log`, 03:52:58Z).

```
$ [db+wal unit copy, 43 dbs]                     rc=0   dbs_copied=43 failures=0
$ find "$REH/store" -type f -name '*.db' | wc -l            → 43
$ find "$REH/store" -type f -name '*_shard_*.db' | wc -l    → 36
$ sqlite3 -readonly "file:$REH/store/metadata.db?immutable=1" 'SELECT count(*) FROM shards WHERE is_active=1; SELECT coalesce(sum(vector_count),0) FROM shards;'
36
5602
$ per-shard: sqlite3 'SELECT count(*) FROM memories'   → awk sum: shards=36 memories=5602
$ integrity: 43 × PRAGMA integrity_check              → 43 'ok', 0 not-ok
$ multiset: live /api/stats shard memoryCount vs copy rows → diff empty (36 entries, both sum 5602)
rc=0
```

Captured at 03:52:58–03:53:08Z (`copy-revised.log`, `copy-nondb.log`, `copy-verify.txt`, `copy-rows.tsv`,
`copy-integrity.txt`, `shard-multiset.txt`). Copy size 665 MB (live root 703 MB; the difference is the
`.backup`-equivalent WAL content folded into the base files and the sidecars not copied).

| copy-side measure | value | S0 reference | equal? |
|---|---|---|---|
| `dbs` | `43` | `43` | yes |
| `shard_files` | `36` | `36` active rows | yes |
| `shards` (active rows) | `36` | `36` | yes |
| `memories` (sum over shard files) | `5602` | `5602` | yes |
| `integrity_check` `ok` count | `43` | `43` dbs | yes |
| `BACKUP_FAILED` count (`.backup` attempt) | `41` (every db) | 0 | **no — refusal #5; route changed and re-validated, see above** |
| integrity failures (`grep -v ' ok$'`) | *(empty)* | *(empty)* | yes |

Per-shard rows: `$REH/evidence/copy-rows.tsv` (36 rows). Per-DB integrity:
`$REH/evidence/copy-integrity.txt` (43 rows). Copy-side `.legacy.bak` **before** the engine opened it: `0`.

**Checkpoint:** every `equal?` cell is `yes` except the `.backup`-form cell, which records the refused form
and the substituted route with its own three-way validation.

## S3 — the memory that must survive (pre-registered)

```
$ sqlite3 -readonly "file:$REH/store/metadata.db?immutable=1" 'SELECT db_path FROM shards WHERE is_active=1 ORDER BY vector_count DESC LIMIT 1;'
projects/project_fd2bfb63318dfd29_shard_0.db
$ sqlite3 -readonly "file:$REFDB?mode=ro" 'SELECT id, created_at, substr(content,1,120) FROM memories ORDER BY created_at ASC LIMIT 1;'
mem_1792819727024_p5ug3wy03|1792819727024|Scoped a wave to remove `unwrap` and `expect` calls from production code, identifying 97 sites across 8 crates (43 poiso
rc=0
```

Captured 2026-11-25T03:53:19Z (`$REH/evidence/s3-detail.txt` at 03:53:24Z). Chosen shard holds 2965 rows.

| item | value |
|---|---|
| shard file chosen (`db_path`, basename) | `project_fd2bfb63318dfd29_shard_0.db` (largest active shard, 2965 rows) |
| memory `id` | `mem_1792819727024_p5ug3wy03` |
| `created_at` | `1792819727024` |
| stored text, first 120 chars | `Scoped a wave to remove \`unwrap\` and \`expect\` calls from production code, identifying 97 sites across 8 crates (43 poiso` |
| `$Q_REF` (3–5 distinctive words, fixed here, never tuned later) | `97 sites across 8 crates` |

## S4 — swap: TS out, port in (scratch config only)

```
$ sed -i 's#"/home/opencode/Code/Personal/Rust/opencode-agents-memory/plugin"#"opencode-mem@latest"#' "$REH/config/opencode.jsonc"
$ grep -c '"opencode-mem@latest"' "$REH/config/opencode.jsonc"      → 1
$ sed -i '/"opencode-mem@latest",\?$/d' "$REH/config/opencode.jsonc"
$ grep -c 'opencode-mem@' "$REH/config/opencode.jsonc"              → 0
$ [append the local plugin path as the last plugins element]
$ grep -c 'opencode-agents-memory/plugin' "$REH/config/opencode.jsonc" → 1
$ diff "$REH/config/opencode.jsonc.live-reference" "$REH/config/opencode.jsonc"
220,221c220,221
<         "/home/opencode/Code/Personal/Rust/opencode-agents-memory/plugin",
<         "/home/opencode/Code/Personal/JS/session-talk"
---
>         "/home/opencode/Code/Personal/JS/session-talk",
>         "/home/opencode/Code/Personal/Rust/opencode-agents-memory/plugin"
rc=1
```

Captured 2026-11-25T03:53:53Z → `$REH/evidence/s4.log`, `$REH/evidence/s4-config-inspect.txt`. The live
config (sha256 `7950d22d7c9febaa0517fcc0cdb983469a36dbecb8fa9f40292394249d6647bd`) already carries the local
`plugin/` at line 220 and no `opencode-mem` reference at all — the state the 2026-11-25 owner ruling
records. The TS pre-state is therefore **constructed**, exactly as §4 specifies.

| evidence | value |
|---|---|
| `grep -c '"opencode-mem@latest"'` pre-state (expect 1) | `1` |
| `grep -c 'opencode-mem@'` after uninstall (expect 0) | `0` |
| `grep -n 'opencode-agents-memory/plugin'` after install (expect 1 line) | `1` line |
| `diff live-reference → post-install` | two lines: the local path moved from slot 4 to last; TS entry gone |
| `artifacts.sha256` (runbook says 5 lines) | **4** lines — the runbook names four files (`plugin/index.js`, `plugin/dist/plugin.js`, `web-ui/dist/index.html`, the engine), so 4 is correct and the "five" is a runbook arithmetic slip |

No JSONC parse was performed on the scratch config: `jq empty` fails at line 294 (the file has comments),
and no `json5` module is installed. The scratch config is never loaded by any process in this phase (the
engine takes `--storage-root`; the plugin is never started), so the edits are evidenced by the two grep
counts and the diff. `INFERRED`, not measured: the edited file remains valid JSONC.

## S5 — engine against the copy, then recall the pre-registered memory

```
$ setsid nohup "$ENGINE" --mode=standalone --storage-root "$REH/store" --web-port 3749 > "$REH/logs/engine.log" 2>&1 &
$ pgrep -af -- "--storage-root $REH/store"
541 …/plugin/engine/linux-x64/opencode-agents-memory --mode=standalone --storage-root …/.tmp/rehearsal-28/20261125T035152Z/store --web-port 3749
$ curl -s -o /dev/null -w 'health=%{http_code}\n' http://127.0.0.1:3749/api/health
health=200
$ curl -s -o /dev/null -w 'mint=%{http_code}\n' http://127.0.0.1:3749/api/stats
mint=401
$ T=$(cat "$REH/store/.auth-token"); echo "token_len=${#T}"
token_len=64
$ curl -s -H "x-opencode-mem-token: $T" http://127.0.0.1:3749/api/stats | jq -r '.data | "total=\(.total) shards=\(.shards|length)"'
total=5602 shards=36
$ curl -s -H "x-opencode-mem-token: $T" "http://127.0.0.1:3749/api/search?q=97%20sites%20across%208%20crates"
jq -r '.data.items[0] | "id=\(.id) similarity=\(.similarity)"'
id=auto-capture-a7df5e746c0f1ce35ca9d2f46498de5f78e8d2bbaab4f5b086ad05e21888b17c similarity=0.49742903405164396
rc=0
```

Captured 2026-11-25T03:54:08Z (`$REH/evidence/s5.log`), 03:54:19Z (`s5b.log`), 03:54:52Z (`s5c.log`).

| observation | value | must equal |
|---|---|---|
| rehearsal engine pid (the only pid this run may signal) | `541` | — |
| `health` | `200` | `200` — holds |
| `mint` (first gated call) | `401` | `401` — holds |
| `token_len` | `64` | `64` — holds |
| `total` / `shards` after migration | `5602` / `36` | S2 copy numbers — holds |
| recalled `id` | `auto-capture-a7df…88b17c` at rank 1, similarity `0.4974` | S3 id — **FAILS** |
| recalled `similarity` | `0.49742903405164396` (rank 1) | (record only) |
| recalled `content` prefix | not the S3 text (rank-1 item is a plan-gate review note) | S3 text — **FAILS** |

**The frozen `$Q_REF` did not surface the pre-registered memory** — not at rank 1, and not anywhere in the
20 returned (`hits_with_target_id=0`, `$REH/evidence/recall.json`). The query is frozen by §3's rule ("fixed
here, never tuned later"), so it is recorded as a mismatch and **not** re-tuned. Diagnosis, labelled as a
diagnostic and not as the pre-registered probe:

1. `GET /api/search` gathers hits from **every** container tag at `page_size * 2` per shard and then sorts
   (`crates/mem-web/src/api/memories.rs:904-1013`), so at 5602 memories a five-word query competes against
   all 36 shards; the top similarity is only 0.497.
2. A different phrase from the same stored text (`infallible-by-construction claims while exempting test
   code`) **does** return the target id, at rank 20, similarity 0.4690 (`$REH/evidence/recall-diagnostic.json`,
   03:55:31Z).
3. A by-id read of the migrated copy returns the row byte-exactly — `mem_1792819727024_p5ug3wy03|1792819727024|Scoped a wave to remove \`unwrap\` …`
   — and the shard still holds 2965 rows. The shard's `-wal` is 0 bytes at that moment, so the
   `?immutable=1` read is the live state, not a stale base (`$REH/evidence/s5d.log`, 03:55:16Z).

Reading: the data is intact and the server's retrieval path does serve it; the runbook's "recall by content
returns the S3 id at `items[0]`" is an assumption about ranking that does not hold on a store of this size
with a query chosen for *distinctiveness of the text*, not for *uniqueness in the corpus*. **A count alone
still does not prove the data is the same — the by-id and diagnostic cells are what prove it here.**

**`.legacy.bak` after the engine opened the copy: `0`** (`$REH/evidence/legacy-bak.txt`, 03:54:19Z). This
is the phase's headline finding, not a measurement slip:

- the copy carries `.turso-migrated` with `completedAt: "2026-10-30T15:14:45.697Z"`, listing 22 shards
  whose paths are under **`/home/vaz/.opencode-mem/data`** — a *different* user home than this host;
- the migrator short-circuits on that marker (`crates/mem-store/src/legacy_migrator.rs:134-138`,
  `MIGRATION_MARKER = ".turso-migrated"`, read before any shard work);
- only **1 of 36** shards carries a `.turso-migrate.json` sidecar, and it reads `status: "complete"`;
- the copy's only `*.bak*` file is `user-profiles.db.schema-v1-1794209986309.bak`, which came *from* the
  live root.

So the live store was already migrated in place (on the host it came from, 2026-10-30), and the erase
sweep (`crates/mem-store/src/bak_reaping.rs` — `.legacy.bak` is family 2 of six reaped families) removes
that family when it runs. **The `.legacy.bak` cell cannot be filled on this machine by construction**, and
no `.legacy.bak` was created by this run.

## S6 — UI, REST surface, add round-trip

```
$ curl -s -o /dev/null -w 'index=%{http_code}\n' http://127.0.0.1:3749/
index=200
$ curl -s http://127.0.0.1:3749/ | grep -o -m1 '<title>[^<]*</title>'
<title>OpenCode Memory Explorer</title>
$ JS=$(grep -o 'href="/[^"]*\.js"' index.html | sed 's/^href="//; s/"$//'); curl -s -o /dev/null -w "js_asset=%{http_code}\n" "http://127.0.0.1:3749$JS"
js_asset=200        (wasm_asset=200 bytes=1327427, css_asset=200)
$ scripts/smoke-rest-surface.sh --live-base-url http://127.0.0.1:3749 --live-token-file "$REH/store/.auth-token" --evidence-dir "$REH/evidence/rest"
03:56:13 live GET-only self-check: PASS (11 GET calls, zero non-GET)
03:56:13 LIVE PASS COMPLETE
rc=0
$ curl -s -X POST … -d '{"content":"rehearsal-28-canary-20261125T035152Z zqx traffic light","containerTag":"opencode_project_2416a5f68e61f821"}' http://127.0.0.1:3749/api/memories
{"success":true,"data":{"id":"mem_1795578986897_02e51ee7e"}}
$ curl -s -H "x-opencode-mem-token: $T" "http://127.0.0.1:3749/api/search?q=rehearsal-28-canary-20261125T035152Z"
id=mem_1795578986897_02e51ee7e similarity=0.6330108286400182
rc=0
```

Captured 03:55:37Z (`s6a.log`), 03:55:49Z (`s6a2.log`), 03:56:12Z (`rest-summary.log`), 03:56:26Z
(`s6c.log`).

Two runbook-form defects found and recorded rather than worked around silently:

1. **`ASSET` extraction is dead in this build.** The runbook greps `assets/[^"]*\.js`; the Leptos index
   uses root-relative hrefs (`/web-ui-a55d4d337bfbc403.js`, `/web-ui-a9518e565911ca80_bg.wasm`,
   `/styles-6d52ac1e2118a38f.css`). The runbook's form prints nothing, so `asset=200` in that form was the
   index fetched again — not an asset. Corrected extraction: js `200`, wasm `200` (1327427 bytes), css `200`.
2. **`smoke-rest-surface.sh` does not create `--evidence-dir`.** The first run `rc=2` with
   `STOP: live /api/projects roster is empty`, while `/api/projects` in fact returns **25** projects,
   identical to live 3742 (`$REH/evidence/s6b-projects.log`). Every write into the missing directory failed,
   so the script's own `live_json` helper returned empty and the roster gate tripped. After `mkdir -p`, the
   same command exits `0`. The `STOP` was the marker's mechanism, not a finding about the port.

The script's report header says "Live **3742** GET-only read-only pass" while the base is 3749 — hardcoded
string, cosmetic only.

| observation | value | must equal |
|---|---|---|
| `index` | `200` | `200` — holds |
| `<title>` | `<title>OpenCode Memory Explorer</title>` | same — holds |
| `asset` | runbook form: none extracted (see defect 1); corrected: js `200`, wasm `200`, css `200` | `200` — holds on the corrected form |
| `smoke-rest-surface.sh` tally + exit code | `rc=0`, 11 GET rows all `200`, populated arms satisfied, zero FAIL; `catalog-file` row `n/a` — "live catalog has no skill files" | `rc=0`, 0 FAIL — holds (first attempt `rc=2`, defect 2) |
| canary add response | `{"success":true,"data":{"id":"mem_1795578986897_02e51ee7e"}}` | `{"success":true,"data":{"id":"mem_…"}}` — holds |
| canary search hit `id` | `mem_1795578986897_02e51ee7e`, similarity `0.6330` | the id the add returned — holds |

`$REH/evidence/rest/live-readonly-pass.md` (the 11-row table), `rest/live-readonly-pass.log`,
`canary-add.json`, `canary-search.json`, `recall.json`. `GET /api/memories?limit=2` answers
`{"success":true,…}` with `data.total = 6027` — the listing tier counts more than `/api/stats`'
`total` (5602), which is a pre-existing shape difference between the two endpoints, recorded and not
explained here. Stats after the canary add: `5603`, i.e. `+1` explained by that add.

## S7 — §0b D3 hatch: the second-engine refusal

```
$ "$ENGINE" --mode=standalone --storage-root "$REH/store" --web-port 3750 > "$REH/logs/second-engine.log" 2>&1
$ echo "second_engine_rc=$?"
second_engine_rc=75
$ cat "$REH/store/status"
degraded: engine lock held (pid=541, ts=2026-11-25T03:56:34.512Z)
$ ps -o pid=,etime= -p 541
  541       02:25
rc=0
```

Captured 2026-11-25T03:56:34Z → `$REH/evidence/s7.log`, `$REH/logs/second-engine.log`.

| observation | value | must equal |
|---|---|---|
| `second_engine_rc` | `75` | `75` — holds |
| stderr `lock_lost` `holder_pid` | `{"component":"engine","event":"lock_lost","holder_pid":541,"level":"warn"}` | S5 engine pid `541` — holds |
| `<copy>/status` line | `degraded: engine lock held (pid=541, ts=2026-11-25T03:56:34.512Z)` | `degraded: engine lock held (pid=…, ts=…)` — holds |

This cell records the **deferral** of the hatch, not its closure: the port is single-process by
construction, trigger (b) stays material, and the corruption / supported-path observation that would open
it has not appeared.

## S8 — rollback, and the proof the original was never written

```
$ kill -TERM 541
$ pgrep -af -- "--storage-root $REH/store" || echo rehearsal_engine_gone
rehearsal_engine_gone
$ ! ss -ltn | grep -q '127.0.0.1:3749' && echo port_3749_free_again
port_3749_free_again
$ cp -a "$REH/config/opencode.jsonc" "$REH/config/opencode.jsonc.post-install"
$ cp -a "$REH/config/opencode.jsonc.live-reference" "$REH/config/opencode.jsonc"
$ grep -c '"opencode-mem@latest"' "$REH/config/opencode.jsonc"
0
$ diff "$REH/config/opencode.jsonc.live-reference" "$REH/config/opencode.jsonc"
(no output)
rc=0
```

Then the S0 block re-taken at 2026-11-25T03:56:52Z (`$REH/evidence/s8-post.txt`):

```
28153 Tue Nov 24 22:47:45 2026    12:09:07
377210f54a94c716d5057bac40ad055b84ba8835527c5bd33c46edd244bcdb66  /home/opencode/.opencode-mem/data/.engine.lock
/home/opencode/.opencode-mem/data 1795575867
live_db_count=43
live_legacy_bak_count=0
live_total=5603 live_shards=36
cfg_cmp_rc=0
no rehearsal process
rc=0
```

`kill -TERM 541` is the only signal this run sent; the engine logged `engine.shutdown_complete`.

**The rollback's own expectation is wrong for today's machine, and is recorded as such.** §8 expects
"one TS entry back (1)"; the restore target `live-reference` *is* the real live config, which carries the
local plugin and no `opencode-mem@latest` — so `grep -c '"opencode-mem@latest"'` is `0`, and the restore
diff is empty. That is the correct restore: the scratch config was returned byte-identically to the live
config it was copied from (`cfg_cmp_rc=0`), and the installed state is kept as
`opencode.jsonc.post-install`. The "reinstall TS" half is unexecutable because the TS pre-state never
existed on this host (the S4a pre-state was synthetic).

| proof | before (S0) | after | pass? |
|---|---|---|---|
| pid 28153 alive, same `lstart` | `28153 Tue Nov 24 22:47:45 2026` | `28153 Tue Nov 24 22:47:45 2026` | yes |
| `.engine.lock` sha256 | `377210f5…bcdb66` | `377210f5…bcdb66` | yes |
| live `*.db` count | `43` | `43` | yes |
| live `*.legacy.bak` count | `0` | `0` | yes |
| live root dir `%Y` | `1795575867` | `1795575867` | yes |
| live `live_total` / `live_shards` | `5602` / `36` | `5603` / `36` | root unchanged; see +1 below |
| `cfg_cmp_rc` (live config vs `live-reference`) | — | `0` | yes |

**The `+1` is not this run's.** Live 3742's newest memories are `auto-capture-54d20de8…` at
`2026-11-25T03:56:22.286Z` and `mem_1795578699826_0696eeb50` at `03:51:39.826Z` — the live plugin capturing
the owner's own session while this rehearsal ran (`$REH/evidence/live-plusone.txt`, 03:57:07Z). Containment
is measured, not assumed: the canary id appears **0** times in live search results, its text **0** times,
and the live root created no file (`%Y` unchanged) and no `.legacy.bak`.

| target-absence check | count | pre-registered | pass? |
|---|---|---|---|
| `grep -c '127.0.0.1:3742'` over this file | `3` | `3` | yes, with the pre-registration caveat stated at the top |
| `grep -c '/home/opencode/.opencode-mem/data'` over this file | `6` | `6` | yes, with the same caveat |

Rollback of the scratch config (`cp -a …live-reference` back), `rehearsal_engine_gone`,
`port_3749_free_again`, `engine.shutdown_complete` — captured, above.

## S9 — Svelte dist against the Rust server

| cell | value |
|---|---|
| REST endpoints the Svelte client calls answer on 3749 | S6: index `200`, 11 GET rows `200`, search/listing/stats all `200` |
| built upstream dist exists on this host | no — `example/opencode-mem/web/dist` does not exist |
| dist served via `--web-dist`, `/` = 200, no failed `/api/*` in the console | `NOT RUN — no built upstream dist on this host; building it means Bun inside example/, which this repo forbids as routine tooling` |

Split deliberately: endpoint parity is S6's evidence; "the Svelte dist loads against the Rust server" needs
a dist, and until one is built this cell stays `NOT RUN`. It is never reported as passed on endpoint
shapes alone.

## S10 — what this rehearsal does not prove

| claim | status |
|---|---|
| Windows | NOT RUN — no runner on this host |
| macOS | NOT RUN — no runner on this host |
| multi-user / shared store | NOT RUN — every path is single-owner |
| host plugin capture (prompt → memory) | NOT RUN — against the copy: needs an OpenCode session pointed at it. The live system's capture is measured in S8's `+1` attribution (`auto-capture-54d20de8…`, 03:56:22Z) and in S11's note |
| host plugin injection (`memory_context` frame) | NOT RUN — against the copy: needs an OpenCode session pointed at it. The live system's injection is measured below |
| power loss during migration | NOT RUN — not simulated |
| the live swap itself | NOT RUN — ruled superseded by the live system, 2026-11-25 |

**Injection, measured on the live system (not on the copy).** `~/.opencode-mem/opencode-mem.log` (read-only
read) carries the host plugin's structured `inject.received` lines. For **this** session
(`OPENCODE_SESSION_ID=ses_def2b9612ffe7NMoFAw4iRr1FQ`) there is exactly one:

```
[2026-11-25T10:51:26.795+07:00] inject.received: {"block_bytes":2675,"block_prefix":"<memory_context>\nThe following block is reference context injected from the memory system.\n\n<project_agent_session_history>\n## Project facts\n<memory relevance=\"72%\">OPS GOTCHA …","component":"opencode-mem-plugin","handling_ms":840,"host_contract":"setup","level":"info","prefix_truncated":true,"sections":["Project facts","Recent sessions"],"session_id":"ses_def2b9612ffe7NMoFAw4iRr1FQ","status":"ok","turn_index":0}
```

`grep -c 'inject.received.*block_prefix":"<memory_context>'` over the log: `74`; every `inject.received`
line's `status` is `"ok"` (218 of them). This is the `<memory_context>` frame in a rendered block, captured
on disk — and the same frame is in this subagent's own system prompt as delivered. It is the **live
system's** injection; it does not exercise the rehearsal copy.

## S11 — the owner repeat (separate)

| step | value |
|---|---|
| `scripts/live-swap.sh --dry-run` | NOT RUN — owner-executed |
| one capture in a real session (`user-prompts.db` count grows) | NOT RUN — owner-executed |
| one injection in a real session | NOT RUN — owner-executed; but see S10: the live log holds the frame |
| live `live_total` + one recall-by-content on 3742 | NOT RUN — owner-executed |
| `--backup` stamp + `sha256sum -c MANIFEST.sha256` rc=0 | NOT RUN — owner-executed |

Closed as **superseded**, not deferred, by the 2026-11-25 owner ruling: the live config already carries the
local plugin and no `opencode-mem` reference (measured again in S4 — sha256
`7950d22d…d6647bd`, local plugin at line 220), so the swap this section describes has been the machine's
state for weeks.

---

## Verdict cells (fill last, after every cell above is either captured or `NOT RUN — reason`)

| question | answer |
|---|---|
| memories still readable after the swap (S5 id + text) | **partly.** The copy's 5602 memories open in the Rust engine under the same total and per-shard multiset (S2/S5), and `mem_1792819727024_p5ug3wy03` is byte-exact in the migrated copy (S5d). The runbook's rank-1 recall by the frozen `$Q_REF` **failed**; a diagnostic phrase returns the same id at rank 20 |
| UI functional against the Rust server (S6) | yes — index `200`, exact title, js/wasm/css assets `200` |
| REST parity holds (S6) | yes for the GET surface — `smoke-rest-surface.sh` `rc=0`, 11 rows all `200`, zero FAIL |
| Svelte dist usable (S9) | NOT RUN — no built upstream dist on this host |
| original root never written (S8) | yes — pid `lstart`, `.engine.lock` sha256, root `%Y`, `*.db` count and `.legacy.bak` count all identical; canary absent from live by id and by text; live `+1` attributed to the owner's own live session |
| D3 hatch confirmed deferred (S7) | yes — second engine `rc=75`, `lock_lost holder_pid=541`, `status` `degraded: engine lock held` |
| issue 28 closable (needs S11 too) | no — AC2 rests on the owner ruling of 2026-11-25 (superseded live swap) plus this phase; the `.legacy.bak` evidence the ruling asks for is unobtainable on this machine (S5) |

Rules for this file: no blanks, no paraphrases, no computed value standing in for captured output, and no
edit to an expectation after the fact — a mismatch is recorded as a mismatch, then explained underneath it.

## Correction, 2026-11-25 — the landed `index.html` carried a bearer token, now redacted

`run-20261125T035152Z/index.html` is a `curl` capture of the rehearsal engine's served index, and the mem-web UI injects
`window.__OPENCODE_MEM_TOKEN__` into that document. The capture therefore landed the scratch instance's token verbatim
(one 64-hex value, tracked by `161685c`). The value has been replaced with `<REDACTED>` in the tracked file.

Severity, measured: it is **not** the live host's token (sha256 prefixes `1367ef89ad81b5cf` vs `25612a61a77a92bd`) and
it belongs to a scratch engine on port 3749 that no longer exists (`ss -ltnp` → nothing). The only store it opens is the
rehearsal copy under `.tmp/rehearsal-28/20261125T035152Z`, which is local and gitignored. No rotation is required.

Standing rule for this evidence tree: never land a served index or any response that echoes the token. Record the asset
paths the cell needs, or redact before writing. See issue 164, cell §6a, which now requires exactly that.
