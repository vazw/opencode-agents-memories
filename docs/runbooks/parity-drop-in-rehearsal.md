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
| live engine | pid: read argv-exact by `live_pid` in S0 and recorded verbatim (`plugin/engine/linux-x64/opencode-agents-memory --storage-root /home/opencode/.opencode-mem/data`) |
| its web port | **3742** (the only listener; `127.0.0.1:3742`) |
| storage root | `/home/opencode/.opencode-mem/data` — 703 MB, 43 `*.db`, 36 registered shards |
| memories | 6766 across 36 active shards (`/api/stats` → `data.total`) |
| `.legacy.bak` files in the live root | **0** |
| live `opencode.jsonc` plugin list | already carries the absolute `…/opencode-agents-memory/plugin` entry; **no `opencode-mem@latest`** |

**Hard constraint.** The live storage root is not the fixture. Every command below that writes takes a
path under `$REH/store` (the copy); the original appears only as a read-only source (unit copy,
`find`/`stat`) and as read-only HTTP on 3742.

- Never `rm -rf` anything; a cleanup step is `find <dir> -mindepth 1 -delete`.
- Never signal, restart or stop the live pid recorded in S0. The auto-restart budget is spent and a second
  unexpected engine death ends the session. The rehearsal engine gets its own pid, recorded in the
  transcript, and is the only process this runbook kills.
- The rehearsal binds **port 3749** (3742 is the live engine; 3750 is used in §7 for a deliberate
  second-engine refusal). Check both are free before starting.

`ps`-level proof that the live pid recorded in S0 is unchanged is taken in S0 and re-taken in §8.

### S0 — pre-state, before anything is copied or edited

Read-only; these values are the reference every later checkpoint compares against, and this block is
re-run verbatim in §8:

```sh
# The live pid is READ, never copied into this document: argv-exact, so a shell
# whose command line merely contains the pattern cannot match itself.
live_pid() {  # argv-exact: <engine> --storage-root $HOME/.opencode-mem/data
  local p a0 a1 a2
  for p in /proc/[0-9]*; do
    [ -r "$p/cmdline" ] || continue
    a0= a1= a2=
    { IFS= read -r -d '' a0; IFS= read -r -d '' a1; IFS= read -r -d '' a2; } <"$p/cmdline" || :
    case $a0 in */plugin/engine/*/opencode-agents-memory) ;; *) continue ;; esac
    [ "$a1" = --storage-root ] || continue
    [ "$a2" = "$HOME/.opencode-mem/data" ] || continue
    printf '%s\n' "${p#/proc/}"
  done
}
LIVEPID=$(live_pid); echo "live_pid=$LIVEPID"
ps -o pid=,lstart=,etime= -p "$LIVEPID"
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
[ -n "$LIVEPID" ] && echo live_engine_ok
! ss -ltn | grep -q '127.0.0.1:3749' && echo port_3749_free
! ss -ltn | grep -q '127.0.0.1:3750' && echo port_3750_free
live_pid | wc -l   # expect 1 (argv-exact; a substring scan self-matches)
```

Expected: six `*_ok` lines, two `*_free` lines, then `1`. Any missing line ⇒ stop.

**Refusal list — stop rather than proceed if:**

1. `live_pid` does not print exactly one pid, or that pid's `ps -o lstart=` start time differs from the one
   recorded in S0.
2. `plugin/engine/linux-x64/opencode-agents-memory` or `web-ui/dist/index.html` is missing (stage them
   with `plugin/scripts/stage-engine.sh` and `cd web-ui && trunk build`; do not build during the run).
3. `/api/stats` on 3742 does not answer `success:true` with a numeric `data.total` — the live engine is
   unhealthy and its numbers cannot be the reference.
4. Port 3749 or 3750 is taken by something that is not this rehearsal.
5. Any `UNITCOPY_FAILED` line appears in `$REH/evidence/copy-unit.log`: report it, do **not** retry in a
   loop, and do **not** fall back to copying a `*.db` without its `-wal` — the db and its `-wal` are one unit.
6. The copy's counts in S2 do not match S0. This is the whole reason the copy comes first: nothing has
   been uninstalled at this point, so stopping costs nothing.
7. The engine in S5 exits non-zero, or 3749 never answers `/api/health` 200 within 15 s.
8. Anything writes to a path outside `$REH` — stop and record it as a finding.

## 2. The copy, and the proof it is complete

Layout (one directory per run, gitignored, `.tmp/` is out of the repo by construction):

```sh
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
REH=$PWD/.tmp/rehearsal-28/$STAMP
mkdir -p "$REH"/{store,config,evidence/rest,logs}
echo "$REH"            # ← record this verbatim; every later command depends on it

# >>> s2-rehearsal-pid
# argv-exact reader: a rehearsal engine's argv is six elements, so the live
# three-element form does not transfer. Match the adjacent pair, never a
# substring, so a `bash -c` wrapper carrying the expanded root is not a match.
rehearsal_pid() {
	local p i a
	local -a args
	for p in /proc/[0-9]*; do
		[ -r "$p/cmdline" ] || continue
		args=()
		while IFS= read -r -d '' a; do args+=("$a"); done <"$p/cmdline"
		[ "${#args[@]}" -ge 3 ] || continue
		case ${args[0]} in */plugin/engine/*/opencode-agents-memory) ;; *) continue ;; esac
		for ((i = 1; i < ${#args[@]} - 1; i++)); do
			if [ "${args[i]}" = --storage-root ] && [ "${args[i+1]}" = "$REH/store" ]; then
				printf '%s\n' "${p#/proc/}"
				break
			fi
		done
	done
}
# <<< s2-rehearsal-pid
```

Copy the DBs **and their `-wal` sidecars as one unit**, then everything that is not a DB, a sidecar or a
per-process artifact:

```sh
# 2a. every *.db AND its -wal, as one unit (a db copied alone loses whatever the
#     WAL holds — measured: without the -wal the copy answers "no such table")
find "$HOME/.opencode-mem/data" -type f -name '*.db' -print0 | sort -z |
while IFS= read -r -d '' src; do
  rel=${src#"$HOME/.opencode-mem/data"/}
  mkdir -p "$REH/store/$(dirname "$rel")"
  if cp -- "$src" "$REH/store/$rel"; then
    [ -f "${src}-wal" ] && { cp -- "${src}-wal" "$REH/store/${rel}-wal" || echo "UNITCOPY_FAILED ${rel}-wal"; }
  else
    echo "UNITCOPY_FAILED $rel"
  fi
done | tee "$REH/evidence/copy-unit.log"
echo "unitcopy_failures=$(grep -c UNITCOPY_FAILED "$REH/evidence/copy-unit.log")"

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

**Why not `.backup`, and why not `immutable=1`.** The live engine holds its storage root exclusively, and
`.backup` needs a read lock that hold never grants: in the 2026-11-25 run
`sqlite3 -readonly "$src" ".backup '…'"` answered `Error: database is locked` on every db attempted
(`backup_failures=41`, `run-20261125T035152Z/` TRANSCRIPT 101-116), and a plain `SELECT count(*)` failed
identically (`s2-diagnosis.txt`). *REPORTED, with two counter-probes*: a stock WAL writer holding
`BEGIN EXCLUSIVE` lets `.backup` complete (`rc=0`), and a whole-file exclusive `fcntl` holder makes it
answer `unable to open database file`, never `database is locked` — so the blocker is the engine's root
hold, not "a writer". `immutable=1` is not a workaround either: it declares "this file cannot change and no
WAL exists", so on a copy that still carries a `-wal` it returns the stale pre-`-wal` base — *measured*:
89 rows on `project_1b5c40c3136fed58_shard_0.db`, whose `-wal` is 4,111,792 bytes and whose live count is 0
(`s2-probe-a.txt`). Hence: unit copy, then a WAL replay in `$REH/store` (the first `mode=ro` open does it),
and every later read is `?mode=ro` too.

Why those exclusions: `*-shm`/`*-tshm` are derived (SQLite rebuilds them on open and a final `mode=ro` close
removes them again); `*-wal` is copied by 2a only, as part of the db unit — never on its own, never skipped
when it exists; `.engine.lock` names a **live** pid and would make the rehearsal
engine refuse its own root (§7); `.auth-token`/`status` are per-process session artifacts — the copy mints
its own token on its first gated call, so no live secret is duplicated into a scratch tree.

**Verify the copy before anything is uninstalled.** Counts on both sides, not byte sizes:

```sh
# copy side — structural counts
echo "dbs=$(find "$REH/store" -type f -name '*.db' | wc -l)"
echo "shard_files=$(find "$REH/store" -type f -name '*_shard_*.db' | wc -l)"
sqlite3 -readonly "file:$REH/store/metadata.db?mode=ro" \
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
  printf '%s ' "${db#"$REH/store"/}"; sqlite3 -readonly "file:$db?mode=ro" 'PRAGMA integrity_check;'
done | tee "$REH/evidence/copy-integrity.txt"
grep -c ' ok$' "$REH/evidence/copy-integrity.txt"
grep -v ' ok$' "$REH/evidence/copy-integrity.txt"   # must print nothing
```

The copy is evidence rather than assumption on three independent readings, the same three the 2026-11-25
run used: `PRAGMA integrity_check` `ok` on every db, per-shard row counts equal to the live API's total as
a multiset, and one byte-exact by-id read of the pre-registered memory (§3). The two `cp` calls are **not
atomic** with the engine's writes — pairing a db with its `-wal` is best-effort, which is exactly why those
three readings, and not an atomicity claim, are the evidence.

Then compare against the pre-state numbers recorded in S0 (§0).

**Checkpoint (STOP if it fails):** `dbs` and the live `*.db` count are equal (43 at authoring);
`shards=N` equals `live_shards`; `memories` equals `live_total` (6766 at authoring); `shard_files` equals
the shard row count; `grep -c ' ok$'` equals `dbs`; the failure greps print `0` and nothing. Your numbers
will differ from these — the comparison is copy-vs-live in the same run, never against this document.

## 3. Pre-register the memory that must survive (before any uninstall)

Pick one existing memory **from the copy** and write its id and a text field into the transcript. This is
the concrete meaning of "still readable": after the swap, that id comes back, carrying that text.

```sh
SHARDFILE=$(sqlite3 -readonly "file:$REH/store/metadata.db?mode=ro" \
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
# >>> s5-engine-start
ENGINE=$PWD/plugin/engine/linux-x64/opencode-agents-memory
(
  "$ENGINE" --mode=standalone --storage-root "$REH/store" --web-port 3749 \
    > "$REH/logs/engine.log" 2>&1 &
  STARTED_PID=$!
  sleep 3
  RPID=$(rehearsal_pid)
  [ "$RPID" = "$STARTED_PID" ] || {
    kill -TERM "$STARTED_PID" || printf 'note: started pid=%s was already gone\n' "$STARTED_PID" >&2
    printf 'REHEARSAL-PID: started pid=%s but the argv-exact reader returned "%s"\n' "$STARTED_PID" "$RPID" >&2
    exit 3
  }
  printf '%s\n' "$RPID" > "$REH/evidence/engine-pid.txt"
  printf 'rehearsal_pid=%s started_pid=%s\n' "$RPID" "$STARTED_PID"
  ps -o pid=,lstart=,etime= -p "$RPID"
  curl -s -o /dev/null -w 'health=%{http_code}\n' http://127.0.0.1:3749/api/health
)
# <<< s5-engine-start
```

Expected: one `rehearsal_pid=<n> started_pid=<n>` line with equal values, one `ps` line, `health=200`, and
`$REH/evidence/engine-pid.txt` holding that one pid — **this pid is the only one you may signal**. The start
block runs in a subshell, so a start failure ends the cell with rc 3 rather than the operator's shell.
(Verified form: `--mode=standalone --storage-root <root> --web-port <port>` binds and
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
# 6a. the Leptos UI against the Rust server.
#     This build emits root-relative hrefs (`href="/web-ui-<hash>.js"`), so the
#     old `assets/*.js` grep matched nothing and an empty ASSET made curl refetch
#     `/` — a vacuous pass. Read the index ONCE into a temp file, take the asset
#     path from that file's own markup, and require the fetched bytes to DIFFER
#     from it. The index embeds `window.__OPENCODE_MEM_TOKEN__`, so both temp
#     files live under $REH/tmp — the one tree §13 never copies — and are
#     deleted below, never under evidence/ or logs/.
mkdir -p -- "$REH/tmp"
IDX="$REH/tmp/index.html"; ASSET_BODY="$REH/tmp/asset.body"; ASSET_OK=1
curl -s -o /dev/null -w 'index=%{http_code}\n' http://127.0.0.1:3749/          # expect: index=200
curl -s http://127.0.0.1:3749/ > "$IDX"
grep -o -m1 '<title>[^<]*</title>' "$IDX"                                      # expect: <title>OpenCode Memory Explorer</title>
ASSET=$(grep -o -m1 'href="/[^"]*\.js"' "$IDX" | sed 's/^href="//; s/"$//')
[ -n "$ASSET" ] || { echo 'ASSET_EXTRACT_FAILED STOP: the index markup carries no href="/…js"' >&2; ASSET_OK=0; }
if [ -n "$ASSET" ]; then
  ASSET_CODE=$(curl -s -o "$ASSET_BODY" -w '%{http_code}' "http://127.0.0.1:3749$ASSET")
  printf 'asset=%s bytes=%s\n' "$ASSET_CODE" "$(wc -c < "$ASSET_BODY")"
  [ "$ASSET_CODE" = 200 ] || { echo "ASSET_HTTP STOP: $ASSET returned HTTP $ASSET_CODE" >&2; ASSET_OK=0; }
  [ -s "$ASSET_BODY" ] || { echo "ASSET_EMPTY STOP: $ASSET returned 0 bytes" >&2; ASSET_OK=0; }
  cmp -s "$ASSET_BODY" "$IDX" && { echo "ASSET_IS_INDEX STOP: $ASSET served the index bytes" >&2; ASSET_OK=0; }
fi
[ "$ASSET_OK" = 1 ] && echo asset_differs_from_index=yes
rm -f -- "$IDX" "$ASSET_BODY"
[ "$ASSET_OK" = 1 ]

# 6b. the pre-registered REST surface (read-only, 41 rows; refuses non-GET)
# >>> s6b-smoke
scripts/smoke-rest-surface.sh \
  --live-base-url http://127.0.0.1:3749 \
  --live-token-file "$REH/store/.auth-token" \
  --evidence-dir "$REH/evidence/rest" > "$REH/evidence/rest-summary.log" 2>&1
smoke_rc=$?
cat "$REH/evidence/rest-summary.log"
echo "smoke_rc=$smoke_rc"
[ "$smoke_rc" = 0 ]
# <<< s6b-smoke

# 6c. one add round-trip on the copy, then find it back
CANARY=rehearsal-28-canary-$STAMP
curl -s -X POST -H "x-opencode-mem-token: $T" -H 'content-type: application/json' \
  -d "{\"content\":\"$CANARY zqx traffic light\",\"containerTag\":\"opencode_project_0123456789abcdef\"}" \
  http://127.0.0.1:3749/api/memories | tee "$REH/evidence/canary-add.json"
curl -s -H "x-opencode-mem-token: $T" \
  "http://127.0.0.1:3749/api/search?q=$CANARY" |
  jq -r '.data.items[0] | "id=\(.id) similarity=\(.similarity)"'
```

Expected: `index=200`, the exact title, `asset=200 bytes=<n>` and `asset_differs_from_index=yes` (the
fetched asset's bytes differ from the index's; equal bytes print `ASSET_IS_INDEX` and the cell ends
non-zero); a non-200 asset answers `ASSET_HTTP STOP: <asset> returned HTTP <code>` and the cell ends
non-zero — the status is checked before the byte arms, because a 500 body is non-empty and differs from the
index and would otherwise pass both of them; the add answers
`{"success":true,"data":{"id":"mem_…"}}` and the follow-up search returns that same id
(`similarity` ≈ 0.6–0.9 — measured 0.606 on a fresh root). `containerTag` must be
`opencode_<scope>_<16 hex>`; anything else is refused with `Invalid container tag`.

The cell's exit status is the script's own, captured with `smoke_rc=$?` from the redirect and then required
with `[ "$smoke_rc" = 0 ]`; the script's log is printed in full either way. A non-zero `smoke_rc` ends the
cell non-zero — a green cell here means the 41 rows ran and passed, not that some downstream writer exited 0.

`smoke-rest-surface.sh` separates a path failure from a data failure by exit code, and names the case in its
message — distinguishable **by code across classes, by message within one** — a path failure is never
reported as a data failure:

- **`rc=0` with a PASS line tallying 0 FAIL** — the recorded pass.
- **`rc=3` with `EVIDENCE-DIR: <dir> does not exist`** — path class: the `--evidence-dir` path is absent,
  **or one of its components is not a directory** (`[ -e ]` is false under `ENOTDIR`, so a parent that is a
  regular file prints this same message; `ls -d <dir>` names the offending component). §0 creates
  `"$REH"/{store,config,evidence/rest,logs}`, so this means a wrong `$REH`, a typo, or §0 was skipped; the
  script refuses and creates nothing. Record it verbatim as a path finding, not a port finding.
- **`rc=3` with `EVIDENCE-DIR: <dir> is not a directory`** — the `--evidence-dir` path itself exists and is
  not a directory (a regular file, or a symlink to one).
- **`rc=3` with `EVIDENCE-DIR: <dir> is not writable`** — it exists as a directory and cannot be written.
  These two are the rest of the path class; same rule: refused, recorded, never retried in a loop and never
  reported as an empty roster.
- **`rc=2` with `STOP: live /api/projects roster is empty`** — data class: the copied store genuinely has
  no project rows (measured on an empty scratch root). On a populated copy this should not appear; any
  `STOP:` is recorded verbatim as a finding and is **not** reported as a pass. A second data STOP,
  `populated arm memories rows=0`, means the roster resolved but the memory arm is empty.

The script prints the token's length and a 4-character prefix only, never the token. An `rc=3` with
`EVIDENCE-DIR:` that still appears after §0 has run is a **path** finding — do not work around it with a
manual `mkdir` and do not record it as a roster result.

A populated copy is why this step targets `$REH/store` and not a fresh root: the 41 rows assert real
answers, and an empty store cannot satisfy them.

The other pipes into `tee` in this runbook are display, never the oracle: §2a's unit-copy log (its oracle is
the `grep -c UNITCOPY_FAILED` count), §2's per-shard rows (the printed `shards=`/`memories=` compared to S0),
§2's integrity list (two `grep`s on the integrity file), §4's sha256 list (the five-line count), and §6c's
canary body (`success:true` plus the follow-up search). §5's process listing is gone — the argv-exact reader
replaced it — and §6b above is the one cell whose oracle *is* the left-hand command's status. The rule: a
pipe to `tee` is verdict-bearing only when the left-hand command's exit status alone is the cell's oracle.

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

Stop the rehearsal engine — its pid only, read back as an argv-exact pair match on `$REH/store` and checked
against the pid §5 wrote to `$REH/evidence/engine-pid.txt` — then restore the scratch config from the copy
taken before the swap — an exact restore of the live port-installed config (`live-reference`), not an inverse
edit:

```sh
# >>> s8-engine-stop
(
  # The engine §5 started is the only process this block may signal. The reader is
  # argv-exact on "$REH/store", so a `bash -c` wrapper carrying the expanded root is
  # not a match. Refusals print on stderr and are not a green cell.
  [ -s "$REH/evidence/engine-pid.txt" ] || {
    printf 'REHEARSAL-PID: no engine was ever started on %s (no engine-pid.txt; §5 did not run)\n' "$REH/store" >&2
    exit 3
  }
  REH_PID=$(cat "$REH/evidence/engine-pid.txt")
  RPID=$(rehearsal_pid)
  if [ -z "$RPID" ]; then
    printf 'rehearsal_engine_already_gone pid=%s (recorded at §5; the rollback did not stop it)\n' "$REH_PID" >&2
    exit 4
  fi
  [ "$RPID" = "$REH_PID" ] || {
    printf 'REHEARSAL-PID: argv-exact match pid=%s is not the pid §5 recorded (%s)\n' "$RPID" "$REH_PID" >&2
    exit 5
  }
  kill -TERM "$RPID"
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    sleep 0.5
    [ -z "$(rehearsal_pid)" ] && break
  done
  [ -z "$(rehearsal_pid)" ] || {
    printf 'REHEARSAL-PID: pid=%s still matches after SIGTERM\n' "$RPID" >&2
    exit 6
  }
  echo "rehearsal_engine_stopped pid=$RPID"
  ! ss -ltn | grep -q '127.0.0.1:3749' && echo port_3749_free_again
)
# <<< s8-engine-stop

cp -a "$REH/config/opencode.jsonc" "$REH/config/opencode.jsonc.post-install"
cp -a "$REH/config/opencode.jsonc.live-reference" "$REH/config/opencode.jsonc"
grep -c '"opencode-mem@latest"' "$REH/config/opencode.jsonc"    # expect: 0 (port-installed live config)
diff "$REH/config/opencode.jsonc.live-reference" "$REH/config/opencode.jsonc"   # expect: no output
cmp "$HOME/.config/opencode/opencode.jsonc" "$REH/config/opencode.jsonc" \
  && echo restore_is_identity_with_live_config
```

Expected: `rehearsal_engine_stopped pid=<n>` and `port_3749_free_again` on the stopped path, **zero**
`opencode-mem@latest` entries, an empty diff, `restore_is_identity_with_live_config`, and
`opencode.jsonc.post-install` kept so the installed state is still on record.

The stop block ends in one of five outcomes, distinguishable by exit code across classes and by message
within one. Only the first is a green stop:

- **rc 0, `rehearsal_engine_stopped pid=<n>` then `port_3749_free_again`** — the pid §5 recorded was matched
  by the argv-exact reader, signalled, and is gone within 5 s.
- **rc 3, `REHEARSAL-PID: no engine was ever started on <root>`** — `engine-pid.txt` is absent or empty, so
  §5 never ran. Nothing was signalled.
- **rc 4, `rehearsal_engine_already_gone pid=<recorded>`** — the sentinel is there but the reader matches
  nothing: the engine died without this block stopping it. Distinct from rc 3 precisely because the
  transcript must be able to say "already gone", not "never started", and neither is a pass.
- **rc 5, `REHEARSAL-PID: argv-exact match pid=<found> is not the pid §5 recorded (<recorded>)`** — something
  else now matches the scratch root. Nothing is signalled.
- **rc 6, `REHEARSAL-PID: pid=<n> still matches after SIGTERM`** — the signal was sent and the process
  outlived it. It is still running; stop it yourself and record the finding.

The block is a subshell, so a refusal's `exit` ends the cell without closing the operator's shell. A refusal
prints neither `rehearsal_engine_stopped` nor `port_3749_free_again` — those bytes belong to rc 0 alone. The
recorded pid is the only process this block may signal; the former substring scan read the whole command
line, so a `bash -c` wrapper carrying the expanded root could be selected instead of the engine.

What is compared, precisely: the restored scratch `opencode.jsonc` against `live-reference`, and
`live-reference` is a `cp -a` of the **live port-installed** `~/.config/opencode/opencode.jsonc` (4a). On
this host that file already carries the absolute local `plugin/` path and **no** `opencode-mem@latest`, so
the correct post-restore state is *no TS entry and an empty diff* — the restore puts back the port-installed
state; it does not resurrect an upstream entry that is not there. "TS reinstalled = 1" therefore applies
only where the TS entry *was* the live state: here it never was, and expecting `1` fails on success.

**Original-untouched proof.** The live store is written by its own engine, so bit-for-bit identity is not
available and must not be claimed. What *is* provable, and what this run takes before the first step and
after the last:

```sh
# re-take the S0 block verbatim
LIVEPID=$(live_pid)
ps -o pid=,lstart=,etime= -p "$LIVEPID"
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

1. the live pid recorded in S0 alive with the **same `lstart`** — the live engine was never restarted, so
   the migrator never ran against the live root.
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
# landing gate (issue 164): a capture still carrying the index token is never
# staged — and never copied, so nothing tokened reaches a tracked path either.
# §6a's temp files now live under $REH/tmp, which this block does not copy, so a
# hit here means some cell wrote one into logs/ or evidence/ — redact that file
# and re-run; the scratch tree is kept on refusal.
# The scan fails closed: grep's exit status is kept, and rc > 1 (a path it could
# not read) is reported as `scan-failed` and refused exactly like a hit. Without
# that, a mode-000 capture passes the gate, the cp -a below fails on it, and a
# partial copy would still be staged and committed.
TOKEN_HIT=$( {
  for d in "$REH/logs" "$REH/evidence"; do
    [ -d "$d" ] || continue
    scan_rc=0; grep -rlF -- '__OPENCODE_MEM_TOKEN__' "$d" || scan_rc=$?
    [ "$scan_rc" -le 1 ] || printf 'scan-failed: %s (grep rc=%s)\n' "$d" "$scan_rc"
  done
} )
if [ -n "$TOKEN_HIT" ]; then
  printf 'TOKEN-IN-EVIDENCE STOP: nothing was copied or staged — a capture below still carries the index token, or the scan could not read a path (`scan-failed`); redact or fix it, then re-run this block:\n%s\n' "$TOKEN_HIT" >&2
  false
else
  cp -a "$REH/evidence/." docs/evidence/issue-28-dropin-rehearsal/raw/
  cp -a "$REH/logs" docs/evidence/issue-28-dropin-rehearsal/logs
  # fill TRANSCRIPT.md (it names these files), then:
  git add docs/evidence/issue-28-dropin-rehearsal docs/runbooks/parity-drop-in-rehearsal.md
  git commit -m "docs(evidence): issue-28 drop-in rehearsal transcript"
  find "$REH" -mindepth 1 -delete     # only after the copy is in the repo and committed
fi
```

The scratch tree is disposable; the transcript is not. Winners and losers of any decision during the run
live in the transcript or an issue comment, never only in `.tmp/`.

## Appendix — command forms verified while writing this document

On 2026-11-18, against throwaway roots under `.tmp/verify-28/` (never the live store):

| form | result |
|---|---|
| unit copy of a db **plus its `-wal`** into `$REH/store`, then `mode=ro` | the live engine holds the storage root exclusively, so `.backup` answers `database is locked` on every db and `immutable=1` returns the stale pre-WAL base (`run-20261125T035152Z/s2-probe-a.txt`); the unit copy + scratch replay is the read path — 43/43 `integrity_check ok`, per-shard rows equal to the live API multiset, one byte-exact by-id read |
| `engine --mode=standalone --storage-root <root> --web-port 3760` | `/api/health` 200 in < 3 s, `/` 200 with `<title>OpenCode Memory Explorer</title>`, exit 0 on SIGTERM |
| `/api/stats` ungated, then `cat <root>/.auth-token` | `401` first (mint), token length 64 |
| `POST /api/memories` with `containerTag=opencode_project_0123456789abcdef` | `{"success":true,"data":{"id":"mem_…"}}`; a non-hex-16 tag → `Invalid container tag` |
| `GET /api/search?q=…` | `data.items[0]` = `{type,id,content,tags,createdAt,similarity,metadata,isPinned}` |
| `GET /api/stats` | `data.total`, `data.shards[].memoryCount`, `data.generatedAt` (camelCase) |
| second engine on the same root | **rc 75**, `{"event":"lock_lost","holder_pid":…}`, `status` = `degraded: engine lock held (pid=…, ts=…)` |
| per-shard row walk (`find … *_shard_*.db` → `SELECT count(*) FROM memories`) | sums correctly; a DB without that table exits 1 and is excluded by the numeric filter |
| `scripts/smoke-rest-surface.sh --live-base-url … --live-token-file … --evidence-dir …` on an empty root | flags accepted, token printed as length + 4-char prefix only, **`rc=2`** with `STOP: live /api/projects roster is empty` (the populated-arm gate; a copy of the real store is populated) |
| live 3742 read-only | `/api/stats` → `total=6766`, 36 shards; `/` → 200, `OpenCode Memory Explorer` |
