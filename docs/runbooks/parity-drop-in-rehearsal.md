# Parity drop-in rehearsal — issue 28

The owner-facing runbook for the **manual drop-in rehearsal** half of issue 28
(`.scratch/rust-port/issues/28-parity-matrix-rehearsal.md`). The matrix half is
issue 71 and is done (110/110); nothing here re-opens it.

**What it proves**, in the parity gate's own words: the TypeScript plugin is uninstalled, this port is
installed into the same project, the existing memories are still readable, the UI works against the Rust
server, and the REST surface still answers the upstream Svelte client.

**Shape is fixed by the issue** (`## Owner rulings 2026-10-29`) and is not redesigned here: copy first, a
re-derivable text transcript, and the live swap as a **separate owner-executed repeat** (§11). No
screenshots, no scripted suite — a manual install property is the point.

- Transcript scaffold to fill: `docs/evidence/issue-28-dropin-rehearsal/TRANSCRIPT.md`
- Filled transcript lands in: `docs/evidence/issue-28-dropin-rehearsal/`
- Commands are **bash** (this host's shell). Run as your own user. No `sudo`.
- Every command form below was verified on a throwaway root on 2026-11-18 (see §10); the *rehearsal
  itself* has **not** been run, so every cell in the scaffold says `NOT RUN`.

---

## 0. Live state, and the one thing this runbook must never do

Measured on this host, 2026-11-18, before writing this document:

| fact | value |
|---|---|
| live engine | pid **4719**, `plugin/engine/linux-x64/opencode-agents-memory --storage-root /home/opencode/.opencode-mem/data` |
| its web port | **3742** (the only listener; `127.0.0.1:3742`) |
| storage root | `/home/opencode/.opencode-mem/data` — 703 MB, 43 `*.db`, 36 registered shards |
| memories | 6766 across 36 active shards (`/api/stats` → `data.total`) |
| `.legacy.bak` files in the live root | **0** |
| live `opencode.jsonc` plugin list | already carries the absolute `…/opencode-agents-memory/plugin` entry; **no `opencode-mem@latest`** |

**Hard constraint.** The live storage root is not the fixture. Every command below that writes takes a
path under `$REH/store` (the copy); the original appears only as a read-only source (`cp -a`, `.backup`
source, `find`/`stat`) and as read-only HTTP on 3742.

- Never `rm -rf` anything; a cleanup step is `find <dir> -mindepth 1 -delete`.
- Never signal, restart or stop pid 4719. The auto-restart budget is spent and a second unexpected engine
  death ends the session. The rehearsal engine gets its own pid, recorded in the transcript, and is the
  only process this runbook kills.
- The rehearsal binds **port 3749** (3742 is the live engine; 3750 is used in §7 for a deliberate
  second-engine refusal). Check both are free before starting.

`ps`-level proof that 4719 is unchanged is taken in S0 and re-taken in S8.

### S0 — pre-state, before anything is copied or edited

Read-only; these values are the reference every later checkpoint compares against, and this block is
re-run verbatim in §8:

```sh
ps -o pid=,lstart=,etime= -p 4719
sha256sum "$HOME/.opencode-mem/data/.engine.lock"
stat -c '%n %Y' "$HOME/.opencode-mem/data"
find "$HOME/.opencode-mem/data" -type f -name '*.db' | wc -l
find "$HOME/.opencode-mem/data" -type f -name '*.legacy.bak' | wc -l
T=$(cat "$HOME/.opencode-mem/data/.auth-token")
curl -s -H "x-opencode-mem-token: $T" http://127.0.0.1:3742/api/stats |
  jq -r '.data | "live_total=\(.total) live_shards=\(.shards|length)"'
```

Expected (2026-11-18): one pid line, one sha256, one `stat` line, `43`, `0`, and
`live_total=6766 live_shards=36`. Your numbers differ — record them verbatim; they are the reference.
Never sqlite against this root: the live engine holds it.

## 1. Preconditions

Everything here must hold, or the run is refused:

```sh
cd /home/opencode/Code/Personal/Rust/opencode-agents-memory || exit 1
test -x plugin/engine/linux-x64/opencode-agents-memory && echo engine_ok
test -f plugin/index.js && echo plugin_ok
test -f web-ui/dist/index.html && echo web_ok
test -f "$HOME/.opencode-mem/data/.auth-token" && echo token_ok
ps -o pid= -p 4719 >/dev/null && echo live_engine_ok
! ss -ltn | grep -q '127.0.0.1:3749' && echo port_3749_free
! ss -ltn | grep -q '127.0.0.1:3750' && echo port_3750_free
pgrep -af 'opencode-agents-memory --storage-root /home/opencode/.opencode-mem/data' | wc -l   # 1
```

Expected: six `*_ok` lines, two `*_free` lines, then `1`. Any missing line ⇒ stop.

**Refusal list — stop rather than proceed if:**

1. pid 4719 is not running, or its `ps -o lstart=` start time differs from the one recorded in S0.
2. `plugin/engine/linux-x64/opencode-agents-memory` or `web-ui/dist/index.html` is missing (stage them
   with `plugin/scripts/stage-engine.sh` and `cd web-ui && trunk build`; do not build during the run).
3. `/api/stats` on 3742 does not answer `success:true` with a numeric `data.total` — the live engine is
   unhealthy and its numbers cannot be the reference.
4. Port 3749 or 3750 is taken by something that is not this rehearsal.
5. Any `.backup` in S2 prints `BACKUP_FAILED` (SQLITE_BUSY on the live store: report it, do **not** retry
   in a loop, and do **not** fall back to `cp -a` of a live `*.db` — that is how a torn copy is made).
6. The copy's counts in S2 do not match S0. This is the whole reason the copy comes first: nothing has
   been uninstalled at this point, so stopping costs nothing.
7. The engine in S5 exits non-zero, or 3749 never answers `/api/health` 200 within 15 s.
8. Anything writes to a path outside `$REH` — stop and record it as a finding.

## 2. The copy, and the proof it is complete

Layout (one directory per run, gitignored, `.tmp/` is out of the repo by construction):

```sh
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
REH=$PWD/.tmp/rehearsal-28/$STAMP
mkdir -p "$REH"/{store,config,evidence,logs}
echo "$REH"            # ← record this verbatim; every later command depends on it
```

Copy the DBs with SQLite's online backup API (the live engine keeps writing; `cp -a` on a live `*.db`
would tear), then everything that is not a DB, a sidecar or a per-process artifact:

```sh
# 2a. every *.db, consistently, from a read-only open of the live file
find "$HOME/.opencode-mem/data" -type f -name '*.db' -print0 | sort -z |
while IFS= read -r -d '' src; do
  rel=${src#"$HOME/.opencode-mem/data"/}
  mkdir -p "$REH/store/$(dirname "$rel")"
  sqlite3 -readonly "$src" ".backup '$REH/store/$rel'" || echo "BACKUP_FAILED $rel"
done | tee "$REH/evidence/copy-backup.log"
echo "backup_failures=$(grep -c BACKUP_FAILED "$REH/evidence/copy-backup.log")"

# 2b. everything else: store markers, exports, logs — not WAL sidecars, not liveness artifacts
(
  cd "$HOME/.opencode-mem/data" || exit 1
  find . -type f ! -name '*.db' ! -name '*-wal' ! -name '*-shm' ! -name '*-tshm' \
       ! -name '.engine.lock' ! -name '.auth-token' ! -name 'status' -print0 |
  while IFS= read -r -d '' f; do
    mkdir -p "$REH/store/$(dirname "$f")"
    cp -a -- "$f" "$REH/store/$f"
  done
)
```

Why those three exclusions: `*-wal`/`*-shm`/`*-tshm` are derived (the `.backup` already carries their
content in the consistent base file); `.engine.lock` names a **live** pid and would make the rehearsal
engine refuse its own root (§7); `.auth-token`/`status` are per-process session artifacts — the copy mints
its own token on its first gated call, so no live secret is duplicated into a scratch tree.

**Verify the copy before anything is uninstalled.** Counts on both sides, not byte sizes:

```sh
# copy side — structural counts
echo "dbs=$(find "$REH/store" -type f -name '*.db' | wc -l)"
echo "shard_files=$(find "$REH/store" -type f -name '*_shard_*.db' | wc -l)"
sqlite3 -readonly "file:$REH/store/metadata.db?immutable=1" \
  'SELECT count(*) FROM shards WHERE is_active=1; SELECT coalesce(sum(vector_count),0) FROM shards;'

# copy side — actual rows per shard (the registry can drift; rows cannot)
find "$REH/store" -type f -name '*_shard_*.db' -print0 | sort -z |
while IFS= read -r -d '' db; do
  n=$(sqlite3 -readonly "file:$db?mode=ro" 'SELECT count(*) FROM memories' 2>&1) || n=-1
  printf '%s\t%s\n' "${db#"$REH/store"/}" "$n"
done | tee "$REH/evidence/copy-rows.tsv" |
awk -F'\t' '$2 ~ /^[0-9]+$/ {s+=$2; k++} END {printf "shards=%d memories=%d\n", k, s}'

# copy side — every DB opens and is internally consistent
for db in $(find "$REH/store" -type f -name '*.db' | sort); do
  printf '%s ' "${db#"$REH/store"/}"; sqlite3 -readonly "file:$db?immutable=1" 'PRAGMA integrity_check;'
done | tee "$REH/evidence/copy-integrity.txt"
grep -c ' ok$' "$REH/evidence/copy-integrity.txt"
grep -v ' ok$' "$REH/evidence/copy-integrity.txt"   # must print nothing
```

Then compare against the pre-state numbers recorded in S0 (§0).

**Checkpoint (STOP if it fails):** `dbs` and the live `*.db` count are equal (43 at authoring);
`shards=N` equals `live_shards`; `memories` equals `live_total` (6766 at authoring); `shard_files` equals
the shard row count; `grep -c ' ok$'` equals `dbs`; the failure greps print `0` and nothing. Your numbers
will differ from these — the comparison is copy-vs-live in the same run, never against this document.

## 3. Pre-register the memory that must survive (before any uninstall)

Pick one existing memory **from the copy** and write its id and a text field into the transcript. This is
the concrete meaning of "still readable": after the swap, that id comes back, carrying that text.

```sh
SHARDFILE=$(sqlite3 -readonly "file:$REH/store/metadata.db?immutable=1" \
  'SELECT db_path FROM shards WHERE is_active=1 ORDER BY vector_count DESC LIMIT 1;')
REFDB=$(find "$REH/store" -type f -name "$(basename "$SHARDFILE")" | head -1)
sqlite3 -readonly "file:$REFDB?mode=ro" \
  'SELECT id, created_at, substr(content,1,120) FROM memories ORDER BY created_at ASC LIMIT 1;'
```

Record verbatim: the shard file name, the memory `id`, its `created_at`, and the first 120 characters of
`content`. Choose the 3–5 most distinctive consecutive words of that text as `$Q_REF` (the recall probe) —
write `$Q_REF` down too, so it is fixed before the swap and cannot be tuned afterwards.

Expected: one row, `mem_<digits>_<hex>`, and non-empty content. No row ⇒ stop (the copy is wrong).

## 4. The swap, as one recorded transition

The live host already carries the port, so the rehearsal **constructs the pre-state explicitly** instead
of pretending to find it: the scratch config starts with the TypeScript entry, that entry is removed
("uninstall"), and the absolute local plugin path is put in its place ("install"). Three diffs, no
ambiguity.

```sh
# 4a. the pre-state: TS entry present, local path absent  (scratch config only)
cp -a "$HOME/.config/opencode/opencode.jsonc" "$REH/config/opencode.jsonc.live-reference"
cp -a "$HOME/.config/opencode/opencode.jsonc" "$REH/config/opencode.jsonc"
sed -i 's#"/home/opencode/Code/Personal/Rust/opencode-agents-memory/plugin"#"opencode-mem@latest"#' \
  "$REH/config/opencode.jsonc"
grep -c '"opencode-mem@latest"' "$REH/config/opencode.jsonc"          # expect: 1

# 4b. uninstall TS
sed -i '/"opencode-mem@latest",\?$/d' "$REH/config/opencode.jsonc"
grep -c 'opencode-mem@' "$REH/config/opencode.jsonc"                  # expect: 0

# 4c. install the port: edit the `plugins` array of $REH/config/opencode.jsonc and add this element
#     "/home/opencode/Code/Personal/Rust/opencode-agents-memory/plugin",
grep -n 'opencode-agents-memory/plugin' "$REH/config/opencode.jsonc"   # expect: 1 line
diff "$REH/config/opencode.jsonc.live-reference" "$REH/config/opencode.jsonc"
sha256sum plugin/index.js plugin/dist/plugin.js web-ui/dist/index.html \
          plugin/engine/linux-x64/opencode-agents-memory | tee "$REH/evidence/artifacts.sha256"
```

Expected: one grep hit; the diff shows exactly two changed lines (TS entry gone, local path added);
five sha256 lines. The live config is compared against `live-reference` at the end (§8) — it must be
byte-identical, because nothing in this run may have written it.

## 5. Start the engine against the copy, then read the memory back

```sh
ENGINE=$PWD/plugin/engine/linux-x64/opencode-agents-memory
"$ENGINE" --mode=standalone --storage-root "$REH/store" --web-port 3749 \
  > "$REH/logs/engine.log" 2>&1 &
sleep 3
pgrep -af -- "--storage-root $REH/store" | tee "$REH/evidence/engine-proc.txt"
curl -s -o /dev/null -w 'health=%{http_code}\n' http://127.0.0.1:3749/api/health
```

Expected: exactly one process line, whose pid you record — **this pid is the only one you may signal**;
`health=200`. (Verified form: `--mode=standalone --storage-root <root> --web-port <port>` binds and
serves; the default `stdio` mode serves too but ends on stdin EOF.)

First gated call mints the copy's token (a 401 here is the mint, not an error):

```sh
curl -s -o /dev/null -w 'mint=%{http_code}\n' http://127.0.0.1:3749/api/stats     # expect: mint=401
T=$(cat "$REH/store/.auth-token"); echo "token_len=${#T}"                         # expect: 64
```

**Still readable, concretely** — counts unchanged, and the pre-registered id recalled by its own content:

```sh
curl -s -H "x-opencode-mem-token: $T" http://127.0.0.1:3749/api/stats |
  jq -r '.data | "total=\(.total) shards=\(.shards|length)"'

curl -s -H "x-opencode-mem-token: $T" \
  "http://127.0.0.1:3749/api/search?q=$(printf '%s' "$Q_REF" | jq -sRr @uri)" \
  > "$REH/evidence/recall.json"
jq -r '.data.items[0] | "id=\(.id) similarity=\(.similarity)"' "$REH/evidence/recall.json"
jq -r '.data.items[0].content' "$REH/evidence/recall.json" | head -c 200
```

**Checkpoint:** `total` and the shard count equal §2's copy numbers (the migration is additive — it must
not change either), `id` equals the S3 id, and the printed `content` starts with the recorded 120-char
prefix. A count that grew is explained only by an `add` this run made, and must be recorded as such.

## 6. UI, REST surface, add round-trip

```sh
# 6a. the Leptos UI against the Rust server
curl -s -o /dev/null -w 'index=%{http_code}\n' http://127.0.0.1:3749/          # expect: index=200
curl -s http://127.0.0.1:3749/ | grep -o -m1 '<title>[^<]*</title>'            # expect: <title>OpenCode Memory Explorer</title>
ASSET=$(curl -s http://127.0.0.1:3749/ | grep -o 'assets/[^"]*\.js' | head -1)
curl -s -o /dev/null -w "asset=%{http_code}\n" "http://127.0.0.1:3749/$ASSET"   # expect: asset=200

# 6b. the pre-registered REST surface (read-only, 41 rows; refuses non-GET)
scripts/smoke-rest-surface.sh \
  --live-base-url http://127.0.0.1:3749 \
  --live-token-file "$REH/store/.auth-token" \
  --evidence-dir "$REH/evidence/rest" | tee "$REH/evidence/rest-summary.log"

# 6c. one add round-trip on the copy, then find it back
CANARY=rehearsal-28-canary-$STAMP
curl -s -X POST -H "x-opencode-mem-token: $T" -H 'content-type: application/json' \
  -d "{\"content\":\"$CANARY zqx traffic light\",\"containerTag\":\"opencode_project_0123456789abcdef\"}" \
  http://127.0.0.1:3749/api/memories | tee "$REH/evidence/canary-add.json"
curl -s -H "x-opencode-mem-token: $T" \
  "http://127.0.0.1:3749/api/search?q=$CANARY" |
  jq -r '.data.items[0] | "id=\(.id) similarity=\(.similarity)"'
```

Expected: `index=200`, the exact title, `asset=200`; the add answers
`{"success":true,"data":{"id":"mem_…"}}` and the follow-up search returns that same id
(`similarity` ≈ 0.6–0.9 — measured 0.606 on a fresh root). `containerTag` must be
`opencode_<scope>_<16 hex>`; anything else is refused with `Invalid container tag`.

`smoke-rest-surface.sh` has two outcomes, and they are not interchangeable:

- **`rc=0` with a PASS line tallying 0 FAIL** — the recorded pass.
- **`rc=2` with a `STOP:` line** — a populated-arm gate could not pass (measured on an empty scratch root:
  `STOP: live /api/projects roster is empty`). On a populated copy that specific STOP should not appear;
  any STOP is recorded verbatim as a finding and is **not** reported as a pass. The script prints the
  token's length and a 4-character prefix only, never the token.

A populated copy is why this step targets `$REH/store` and not a fresh root: the 41 rows assert real
answers, and an empty store cannot satisfy them.

## 7. The §0b D3 hatch — confirming the deferral, not re-opening it

The issue records the hatch as **re-confirmed deferred**; this runbook confirms that branch. Trigger (b)
"multi-process DB access" is material (the engine documents *no multi-process access* while this port
depends on concurrent processes), so the hatch stays named and deferred. Nothing here reverses that.

Two observations are recorded, both cheap:

**Closure would have required** — neither switch trigger materializing by Phase 7 (roadmap §0b D3), i.e.
no SQLCipher-at-rest need beyond turso's experimental encryption and no functional need for multi-process
access on a supported path. That is not the state: (b) is material.

**Confirmation that the hatch is still needed** — a single engine per store root, enforced and observable:

```sh
"$ENGINE" --mode=standalone --storage-root "$REH/store" --web-port 3750 > "$REH/logs/second-engine.log" 2>&1
echo "second_engine_rc=$?"
cat "$REH/store/status"
```

Expected (measured 2026-11-18 on a throwaway root): `second_engine_rc=75`, a stderr line
`{"component":"engine","event":"lock_lost","holder_pid":<first engine pid>,"level":"warn"}`, and
`status` reading `degraded: engine lock held (pid=<first engine pid>, ts=…)`. The second engine never
opens the DB. That is the deferral's justification in one command: the port is single-process **by
construction**, so multi-process support is a real future requirement rather than a suppressed one — and
the observed corruption / supported-path requirement that *would* open the hatch has not appeared.

Run this **after** §5 and **before** §8 (it needs the first rehearsal engine alive). It writes only inside
`$REH/store`.

## 8. Rollback, and the proof the original was never written

Stop the rehearsal engine (its pid only), then restore the scratch config from the copy taken before the
swap — the "reinstall TS" half, as an exact restore rather than an inverse edit:

```sh
kill -TERM "$(pgrep -f -- "--storage-root $REH/store" | head -1)"
sleep 2; pgrep -af -- "--storage-root $REH/store" || echo rehearsal_engine_gone
! ss -ltn | grep -q '127.0.0.1:3749' && echo port_3749_free_again

cp -a "$REH/config/opencode.jsonc" "$REH/config/opencode.jsonc.post-install"
cp -a "$REH/config/opencode.jsonc.live-reference" "$REH/config/opencode.jsonc"
grep -c '"opencode-mem@latest"' "$REH/config/opencode.jsonc"    # expect: the pre-state count (1)
diff "$REH/config/opencode.jsonc.live-reference" "$REH/config/opencode.jsonc"   # expect: no output
```

Expected: `rehearsal_engine_gone`, `port_3749_free_again`, one TS entry back, an empty diff, and
`opencode.jsonc.post-install` kept so the installed state is still on record.

**Original-untouched proof.** The live store is written by its own engine, so bit-for-bit identity is not
available and must not be claimed. What *is* provable, and what this run takes before the first step and
after the last:

```sh
# re-take the S0 block verbatim
ps -o pid=,lstart=,etime= -p 4719
sha256sum "$HOME/.opencode-mem/data/.engine.lock"
find "$HOME/.opencode-mem/data" -type f -name '*.db' | wc -l
find "$HOME/.opencode-mem/data" -type f -name '*.legacy.bak' | wc -l
stat -c '%n %Y' "$HOME/.opencode-mem/data"
T=$(cat "$HOME/.opencode-mem/data/.auth-token")
curl -s -H "x-opencode-mem-token: $T" http://127.0.0.1:3742/api/stats |
  jq -r '.data | "live_total=\(.total) live_shards=\(.shards|length)"'
cmp "$HOME/.config/opencode/opencode.jsonc" "$REH/config/opencode.jsonc.live-reference"; echo "cfg_cmp_rc=$?"
```

Pass conditions, all four:

1. `pid 4719` alive with the **same `lstart`** — the live engine was never restarted, so the migrator
   never ran against the live root.
2. `.engine.lock` sha256 unchanged — no second engine ever took that root.
3. `*.db` count unchanged and `.legacy.bak` count still `0` — nothing was added to or renamed inside the
   live root by this run.
4. `stat -c %Y` of the live root directory unchanged — no file was created or removed in it.
   `live_total` may differ; if it does, it is the owner's own session writing, and the delta is recorded
   as such (never silently).

Plus the target-absence check over the transcript itself, which is where a leak would show:

```sh
# the transcript is edited in place, in the repo — not in $REH
T28=docs/evidence/issue-28-dropin-rehearsal/TRANSCRIPT.md
grep -c '127.0.0.1:3742' "$T28"                    # expected total, pre-registered
grep -c '/home/opencode/.opencode-mem/data' "$T28" # expected total, pre-registered
```

Both counts are written into the transcript **before** the run (the number of legitimate read-only
mentions: S0's live reads plus the copy source lines). A count above it means a command addressed the
live root — a finding, not a rounding error.

## 9. The Svelte dist — REST parity today, the dist as a deliberate owner build

REST parity (the upstream Svelte client's endpoints answering this server) is recorded by §6b/§6c, and the
upstream client is same-origin by construction (`example/opencode-mem/web/src/lib/stores/memories-explorer.svelte.ts:159`
fetches `/api/health` with `credentials: "same-origin"`).

**There is no built upstream dist on this machine** (`example/opencode-mem/web/dist` does not exist;
`web/package.json`'s `build` is `bunx tsc && bun run web:build`). Building it means invoking Bun inside
the vendored `example/` subtree, which this repo forbids as routine tooling — so it is not part of the
run. If you choose to do it deliberately, one command and one flag:

```sh
( cd example/opencode-mem/web && bun install && bun run build )   # writes example/opencode-mem/web/dist
"$ENGINE" --mode=standalone --storage-root "$REH/store" --web-port 3749 \
  --web-dist example/opencode-mem/web/dist
```

Then `curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:3749/` and the browser console must show
no failed `/api/*` request. **Until that dist exists, the "Svelte dist loads against the Rust server" cell
is `NOT RUN — no built upstream dist on this host`, and the transcript says so.** Endpoint parity (§6b) is
not a substitute for that cell and is not reported as one.

## 10. What this rehearsal does not prove

- **Windows and macOS**: no runner exists here. One Linux host, one glibc, one arch. A rehearsal on this
  laptop is *native-platform* evidence for Linux only — never for another platform.
- **Multi-user**: every path is `$HOME`-relative and single-owner; nothing here exercises two users, two
  shells with different `$HOME`, or a shared store.
- **The host plugin's own hooks**: capture (a prompt becoming a memory) and injection (the `memory_context`
  frame inside a live turn) need an OpenCode session and are the owner's repeat (§11), not this phase.
  The §6c add round-trip proves the memory tier, not the host hook.
- **Power loss / crash during migration**: not simulated; the store copy is disposable.
- **The live swap itself**: that is §11. This document's copy-first phase proves the property; the owner's
  repeat is what closes the issue.

## 11. The owner repeat (separate, after this phase)

`scripts/live-swap.sh` is the executable form of `docs/runbooks/live-swap.md` and covers the real swap,
the backup and the rollback. The repeat adds exactly the observations this phase cannot make:

1. `scripts/live-swap.sh --dry-run` — read-only preflight, prints every command.
2. A real session in the same project: one capture (a prompt row in `user-prompts.db`, count grows) and
   one injection (the `memory_context` frame in a live turn), both recorded as observations.
3. The live `/api/stats` total and one recall-by-content on 3742, the same shape as §5.
4. Rollback readiness only — a `--backup` stamp and its `MANIFEST.sha256` verified (`sha256sum -c`), not a
   restore.

## 12. Transcript format

One block per step, in `docs/evidence/issue-28-dropin-rehearsal/TRANSCRIPT.md` (scaffold already contains
every block, pre-labelled `NOT RUN`):

```
### S5 — start the engine against the copy
$ <verbatim command, exactly as typed>
<stdout and stderr, verbatim, not summarised>
rc=<exit code>
```

Rules:

- **A cell that cannot be filled is `NOT RUN — <reason>`.** Never blank, never a paraphrase, never a
  computed value standing in for captured output — an evidence cell holding a computed value is treated as
  unrun (repo rule).
- A derived line is allowed only **after** the raw output it derives from, in the same cell, labelled
  `derived:`.
- Commands are recorded as typed. If you typed something else, the transcript says what you typed.
- Anything that surprises you goes in the cell, not in your memory of the run.

Step labels map onto the sections one-to-one: `S0`→§0 (pre-state), `S1`→§1 (preconditions), `S2`→§2
(copy + verify), `S3`→§3 (pre-register), `S4`→§4 (the swap), `S5`→§5 (engine + recall), `S6`→§6
(UI/REST/canary), `S7`→§7 (D3), `S8`→§8 (rollback + untouched proof), `S9`→§9 (Svelte), `S10`→§10,
`S11`→§11 (owner repeat).

## 13. Landing the evidence

```sh
# the transcript is filled in place and is already where it belongs
cp -a "$REH/evidence/." docs/evidence/issue-28-dropin-rehearsal/raw/
cp -a "$REH/logs" docs/evidence/issue-28-dropin-rehearsal/logs
# fill TRANSCRIPT.md (it names these files), then:
git add docs/evidence/issue-28-dropin-rehearsal docs/runbooks/parity-drop-in-rehearsal.md
git commit -m "docs(evidence): issue-28 drop-in rehearsal transcript"
find "$REH" -mindepth 1 -delete       # only after the copy is in the repo and committed
```

The scratch tree is disposable; the transcript is not. Winners and losers of any decision during the run
live in the transcript or an issue comment, never only in `.tmp/`.

## Appendix — command forms verified while writing this document

On 2026-11-18, against throwaway roots under `.tmp/verify-28/` (never the live store):

| form | result |
|---|---|
| `sqlite3 -readonly <live-db> ".backup '<dst>'"` with a writer holding the WAL | rc 0, row present in the copy, `integrity_check` `ok`, source sidecars untouched |
| `engine --mode=standalone --storage-root <root> --web-port 3760` | `/api/health` 200 in < 3 s, `/` 200 with `<title>OpenCode Memory Explorer</title>`, exit 0 on SIGTERM |
| `/api/stats` ungated, then `cat <root>/.auth-token` | `401` first (mint), token length 64 |
| `POST /api/memories` with `containerTag=opencode_project_0123456789abcdef` | `{"success":true,"data":{"id":"mem_…"}}`; a non-hex-16 tag → `Invalid container tag` |
| `GET /api/search?q=…` | `data.items[0]` = `{type,id,content,tags,createdAt,similarity,metadata,isPinned}` |
| `GET /api/stats` | `data.total`, `data.shards[].memoryCount`, `data.generatedAt` (camelCase) |
| second engine on the same root | **rc 75**, `{"event":"lock_lost","holder_pid":…}`, `status` = `degraded: engine lock held (pid=…, ts=…)` |
| per-shard row walk (`find … *_shard_*.db` → `SELECT count(*) FROM memories`) | sums correctly; a DB without that table exits 1 and is excluded by the numeric filter |
| `scripts/smoke-rest-surface.sh --live-base-url … --live-token-file … --evidence-dir …` on an empty root | flags accepted, token printed as length + 4-char prefix only, **`rc=2`** with `STOP: live /api/projects roster is empty` (the populated-arm gate; a copy of the real store is populated) |
| live 3742 read-only | `/api/stats` → `total=6766`, 36 shards; `/` → 200, `OpenCode Memory Explorer` |
