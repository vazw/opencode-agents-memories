# Live swap — upstream `opencode-mem@latest` → this port

Owner-facing sequence for moving this machine's live memory store from the upstream plugin to the local
port. **This document is the procedure; issue 81 is the execution.** Every command below was rehearsed on
copies on 2026-10-30 (evidence: `docs/evidence/live-swap-2026-10-30/`); no step here has been run against
the live store.

**`scripts/live-swap.sh` is this document as a program** — same order, same expected values. `--help`
lists the modes; `--dry-run` runs the read-only preflight and prints every command; `--backup`, `--swap`,
`--verify` and `--rollback` are §3–§5, §7–§8, §9b/§10 and §12; `--rehearse <dir>` points all of it at a
scratch store copy. Every mode refuses to write while OpenCode or the engine is running. Where the two
disagree, this document wins and the script is the bug.

- Store: `~/.opencode-mem/data` — at rehearsal **22 project shards / 3,104 memories / 768 dims / 1,745
  prompts / 2 profiles + 10 changelogs / 29 DBs**, plus `data/.cache` (546 MB, the ONNX weights).
- Config: `~/.config/opencode/opencode.jsonc` (plugin entry) and `~/.config/opencode/opencode-mem.jsonc`
  (port reads the same file; unknown keys tolerated — measured 2026-10-30 with the live 17,457-byte file).
- Engine: `plugin/engine/linux-x64/opencode-agents-memory`, staged beside the plugin. The rehearsal used
  sha256 `72d7f64d5bdcbabd54b5c59b8a35c93712d4a6699c3557f4dbf01f8b24d5e653` (built at `e5154bc`, before
  W6.6's web routes). **Issue 81 must rebuild it from landed `main`** (`plugin/scripts/stage-engine.sh`)
  and re-check the web UI, because that build is not the one rehearsed.
- Run everything as your own user. No `sudo` anywhere in this document. Commands are **bash** (this
  host's shell); `mapfile` is bash-specific.

**Non-negotiables**

1. Stop OpenCode before checkpointing or copying the store. A live writer makes a torn copy.
2. The backup, not the old plugin, is the rollback. The port is one-way: it refuses a store whose
   `metadata.db` is newer than its own (`-32603` on every store call), and no downgrade path exists.
3. Provision the ONNX cache before the swap, or `search`/`add` degrade to `-32603` while everything else
   looks healthy.

---

## 1. Preflight

```sh
# the store the plugin actually uses (it resolves $HOME/.opencode-mem/data, not the JSONC key)
ls -d ~/.opencode-mem/data ~/.opencode-mem/data/.cache/Xenova/nomic-embed-text-v1
du -sh ~/.opencode-mem                                  # expected: 628M (2026-10-30)
df -h ~/.opencode-mem | tail -1                         # need ~2x the store: 1.5G free is ample
grep -n '"storagePath"\|"embeddingModel"\|"webServerPort"' ~/.config/opencode/opencode-mem.jsonc
#   expected: "~/.opencode-mem/data" / "Xenova/nomic-embed-text-v1" / 4747
```

Expected: all paths exist; the values are the ones above. The config's `webServerPort` is 4747, but
the plugin's engine launch does not forward it: the engine's web server binds **3742** by default
(measured 2026-10-31 on the live swap). Open `http://127.0.0.1:3742` unless `OPENCODE_MEM_WEB_PORT` is
set.

## 2. Stop OpenCode cleanly

Quit the application (or `Ctrl-C` the `opencode` session). Then confirm nothing can write:

```sh
pgrep -af 'opencode' || echo 'no opencode process'
pgrep -af 'opencode-agents-memory' || echo 'no engine process'
date -r ~/.opencode-mem/data/metadata.db; sleep 3; date -r ~/.opencode-mem/data/metadata.db
```

Expected: both `pgrep`s print nothing but their echo, and the two `date` lines are **identical**.

## 3. Checkpoint the live DBs

WAL frames are part of the store; fold them into the base files before copying.

```sh
mapfile -t dbs < <(find ~/.opencode-mem/data -type f -name '*.db' | sort)
echo "dbs=${#dbs[@]}"                                   # expected: 29
for db in "${dbs[@]}"; do
  printf '%s -> ' "${db#"$HOME"/}"; sqlite3 "$db" 'PRAGMA wal_checkpoint(TRUNCATE);'
done
find ~/.opencode-mem/data -type f \( -name '*-shm' -o -name '*-tshm' \) -delete
find ~/.opencode-mem/data -type f \( -name '*-wal' -o -name '*-shm' -o -name '*-tshm' \) | wc -l
```

Expected: 29 lines, each ending in `0|0|0` (`busy|log|checkpointed`; the frames are applied when SQLite
opens the file, so a fully-drained WAL reports zeros), then `0` — no sidecars left. `-wal`/`-shm`/`-tshm`
are derived and rebuilt; **never** delete `*.db` or a non-empty `*.db-wal`.

## 4. Dated backup of the whole root + both config files

```sh
TS=$(date -u +%Y%m%dT%H%M%SZ)
BACKUP=~/.opencode-mem-backups/$TS        # must be OUTSIDE ~/.opencode-mem
mkdir -p ~/.opencode-mem-backups
cp -a ~/.opencode-mem "$BACKUP"
mkdir -p "$BACKUP/config"
cp -a ~/.config/opencode/opencode.jsonc     "$BACKUP/config/opencode.jsonc"
cp -a ~/.config/opencode/opencode-mem.jsonc "$BACKUP/config/opencode-mem.jsonc"
du -sh "$BACKUP"; find "$BACKUP" -type f | wc -l
```

Expected: `617M` and `85` files (rehearsal, 2026-10-30T17:16:53Z); 29 of them `*.db`. The backup carries
`data/`, `data/.cache/`, `exports/`, the logs, and both config files under `config/`.
`echo "$TS" > ~/.opencode-mem-backups/LATEST`.

## 5. Manifest, verify, per-DB integrity

```sh
cd "$BACKUP"
find . -type f ! -name MANIFEST.sha256 -print0 | sort -z | xargs -0 sha256sum > MANIFEST.sha256
wc -l < MANIFEST.sha256                       # expected: 85
sha256sum -c MANIFEST.sha256 --quiet; echo "manifest_rc=$?"
```

Expected: only `manifest_rc=0` (`--quiet` prints failures only). **Any `FAILED` line ⇒ restore point is
not valid; fix the copy before going on.**

```sh
for db in $(find "$BACKUP/data" -type f -name '*.db' | sort); do
  printf '%s ' "${db#"$BACKUP/"}"; sqlite3 -readonly "file:$db?immutable=1" 'PRAGMA integrity_check;'
done | tee /tmp/integrity.txt
grep -c ' ok$' /tmp/integrity.txt; grep -v ' ok$' /tmp/integrity.txt
```

Expected: 29 lines, each ending in ` ok` (`29` from the count, and the second `grep` prints nothing).
`immutable=1` is correct here: the files were just checkpointed, so the base file *is* the store.

## 6. What the first engine open will migrate

The port migrates in place, additively, with no `.bak` files — measured twice (dry run and this
rehearsal) and reproduced on a restored copy:

| DB | before → after | when | measured cost | effect |
|---|---|---|---|---|
| `metadata.db` | 1 → **2** | first store touch (`list-shards`/`list`) | 144–408 ms | repair + partial unique index; 22 rows / 3,104 vectors unchanged; 24,576 → 28,672 B |
| `user-prompts.db` | 1 → **2** | first web read with `?includePrompts=true` | 43–237 ms | stamp only (all 12 columns already present); 1,745 rows unchanged, file size unchanged (1,540,096 B) |
| one project shard | 2 → **3** | first `add`/write to that project | 97 ms (the write itself) | `CREATE TABLE shard_stats` + one row `1|<count>`; the other 21 shards stay v2 until they are written |

Nothing readable is lost: shards, memories, vectors (768-dim, read in place, no re-embed), prompts,
profiles + changelogs, skills, episodic and ai-sessions tiers all preserve their counts. The store is
usable at every point; re-opening an already-migrated store is idempotent.

Reading the store between steps: after an engine run the newest writes sit in `*.db-wal`, so a raw
`immutable=1` read under-reports (it shows the base file only). Checkpoint first or open normally:
`sqlite3 <db> 'PRAGMA wal_checkpoint(TRUNCATE);'` on a *copy*, or just trust the RPC/web surfaces.

## 7. Provision the ONNX cache (prerequisite for `search`/`add`)

The tool embedder loads the model from `$TMPDIR/.opencode-mem-onnx/<embeddingModel>` with downloads
disabled (`crates/mem-bridge/src/composition.rs`, `hub: None`). It does **not** read
`{storagePath}/.cache`, and the `tokenizers` reader rejects Hugging Face's legacy `"version": "1.0"`
tokenizer. Measured failure modes with the engine otherwise healthy:

| cache state | `search` |
|---|---|
| absent | `-32603 Internal error` in 9–11 ms |
| upstream `.cache` copied as-is (tokenizer 1.0) | `-32603 Internal error` after 3.7–6.8 s |
| canonical v2.0 tokenizer + weights | works (first search 2.9–4.3 s; then 60–200 ms) |

```sh
# from the repo root; ~38 s cold (it compiles a 6-line canonicalizer against the
# cargo registry cache, offline). Idempotent: re-run it after any reboot.
scripts/provision-onnx-cache.sh
```

Expected output ends with:

```
tokenizer_version=2.0
model_onnx_bytes=547552426
model_onnx_sha256=347440e93b5ec979fdcf6041b72721aade7b9680c16022e2830db7115ff6fd9f
```

Verify it *works*, not just exists — start the engine against the store copy from §13 or, after the
swap, against the live store:

```sh
# replace <STORE> with the store root; PORT is the engine's web port (3742 by default;
# OPENCODE_MEM_WEB_PORT overrides; the config's webServerPort is not forwarded by the plugin launch)
STORE=~/.opencode-mem/data; PORT=3742
curl -s -o /dev/null -w 'mint=%{http_code}\n' "http://127.0.0.1:$PORT/api/stats"   # first gated call: 401
TOKEN=$(cat "$STORE/.auth-token")                                                 # minted by that call
curl -s -H "x-opencode-mem-token: $TOKEN" \
  "http://127.0.0.1:$PORT/api/search?q=W8%20brute-force%20search%20cost%20benchmark" \
  | head -c 300
curl -s -X POST -H "x-opencode-mem-token: $TOKEN" -H 'content-type: application/json' \
  -d '{"content":"live-swap canary","containerTag":"opencode_project_4182e0858e0c93a1"}' \
  "http://127.0.0.1:$PORT/api/memories"
```

Expected: `mint=401` (the token file is created lazily by the first gated request — not an error), then a
JSON body with `"items"` and a `similarity` on the top hit, then `{"success":true,"data":{"id":"mem_…"}}`.
Measured: search 200 in 3.4 s (first, model load), add 200 in 81–86 ms, canary rank-1 on the follow-up
search in 187–197 ms. The port's token is `<storage-root>/.auth-token` (`~/.opencode-mem/data/.auth-token`
once live) — **not** upstream's `~/.opencode-mem/.auth-token`, which sits one level above `data/` and
belongs to the other plugin.

`$TMPDIR` is volatile on this host (`/tmp` is a 16 GB tmpfs): **after every reboot re-run the
provisioner**, or launch OpenCode with a persistent `TMPDIR` (e.g. `TMPDIR=$HOME/.cache/opencode-tmp`)
and provision into it with `--cache-root`. A configurable cache root is a code change, not a config key.

## 8. Swap the plugin entry

The current entry is the third item of the `plugins` array in `~/.config/opencode/opencode.jsonc`
(around line 171):

```jsonc
    "plugins": [
        "@mohak34/opencode-notifier@latest",
        "opencode-antigravity-auth@latest",
        "opencode-mem@latest"          // ← replace this line
    ],
```

Replace that one string with the absolute plugin directory:

```jsonc
        "/home/vaz/Code/Personal/Rust/opencode-agents-memory/plugin"
```

An absolute path in the plugin list resolves as a local plugin (`source-directory` + supervisor
`path.isAbsolute` branch; the legacy `"plugin"` key is merged into the same list). Take the backup first
and keep the exact restore command:

```sh
cp -a ~/.config/opencode/opencode.jsonc ~/.config/opencode/opencode.jsonc.pre-port-$TS
# ... edit the single line above ...
diff ~/.config/opencode/opencode.jsonc.pre-port-$TS ~/.config/opencode/opencode.jsonc
# restore (any time):
# cp -a ~/.config/opencode/opencode.jsonc.pre-port-$TS ~/.config/opencode/opencode.jsonc
```

`~/.config/opencode/opencode-mem.jsonc` stays as it is — the port reads the same file and the same
`storagePath`. Expect a second backup with the suffix `.bak` if the config editor applies the edit
instead of `cp`.

**The local plugin does not auto-update.** `"autoUpdate": true` in `opencode-mem.jsonc` is an upstream
key with no effect on the local path; updating means rebuilding `plugin/` and re-running
`plugin/scripts/stage-engine.sh`. A stale plugin is a deliberate state — record the commit you are on.

## 9. First start + fresh-engine reopen

Start OpenCode normally; the plugin spawns the engine, which performs the migrations in §6. Expected:
the memory tool answers, the web UI loads, no engine stderr beyond the two informational lines
(`consolidation.startup_tick`, `engine.shutdown_complete`).

Then **quit and start once more** (a fresh-engine reopen). Never leave a `SIGTERM`'ed engine as the last
opener — that state was observed once answering `hello`/`health` but `-32603` on `list-shards`/`list`.

Expected on the reopen: `list-shards` reports 22 shards, `current 1 / linked 20 / orphaned 1` (the
orphaned `usbgurad-tray` project path no longer exists — pre-existing data), `list` returns the newest
memories, and the print/ask path still works.

If a fresh engine answers `-32603` on `list-shards`/`list`/`add`:

```sh
pgrep -af 'opencode' || echo 'no opencode process'     # confirm no engine runs first
find ~/.opencode-mem/data -type f \( -name '*.db-shm' -o -name '*.db-tshm' \) -delete
```

Those shared-memory files are derived and rebuilt on the next open. **Never** remove `*.db` or a
non-empty `*.db-wal` to fix an open. The clean control (ordered shutdown → immediate reopen) opened
first try; this recovery was measured on the poisoned store (see the import report §6).

## 9b. Multi-instance check (issue 82)

One host process evaluates the plugin once per instance/location, so one run can load the local
plugin several times while every instance must drive the **same** engine. Check all three facts
together:

```sh
# 1. the run loaded the local plugin at least twice (measured 26 pre-fix)
grep 'msg="loading plugin" id=/home/vaz/Code/Personal/Rust/opencode-agents-memory/plugin' \
  ~/.local/share/opencode/log/opencode.log | wc -l        # expected: ≥ 2 in one run

# 2. one engine serves the shared store, whatever the load count
ps -eo pid=,args= | grep -c '[o]pencode-agents-memory --storage-root /home/vaz/.opencode-mem/data'
#   expected: 1

# 3. that engine's parent is the host process itself (the one-process premise, D1)
ps -o ppid= -p "$(pgrep -f '[o]pencode-agents-memory --storage-root /home/vaz/.opencode-mem/data' | head -1)"
#   expected: the opencode host pid
```

If fact 3 fails — the engine's parent is not the host — the one-process premise is wrong for this
host: stop, record the measurement, and reopen the socket-service alternative (the plan's Step 10
does not paper over it). Facts 1 and 2 alone hold pre-fix as well; fact 3 plus the §10 two-workspace
`list-shards` answer are the discriminators.

## 10. Verification battery

Each check with the value it must produce. `/api/skill` and `/api/plugin` are the **OpenCode host**
endpoints, not the port's web server.

| # | check | how | expected |
|---|---|---|---|
| 1 | handshake | `memory` tool `mode:"help"` in **two different workspaces** (two agent instances/locations), or `hello` over the bridge | **both** answer: `engine_version 0.1.0`, protocol `1.0.0`, capabilities `add, compaction, export, import, list, list-shards, migrate, profile, search, skills, standalone, web`; `help` returns nine commands with `unavailable: []`. Pre-fix the second workspace answered `engine_unavailable` (issue 82). |
| 2 | health | `memory` tool `mode:"list-shards"` in **both workspaces** (or the RPC) | both `success:true`; `storagePath` = `~/.opencode-mem/data`; 22 shards; `current 1 / linked 20 / orphaned 1`. The second workspace is the fix's live discriminator: pre-fix it answers `engine_unavailable`. |
| 3 | direct tool | ask the model to call `memory` with `options.codemode: false` (the tool is exposed directly, not via `execute`) | one direct `memory` call in the session, `success:true` — the tool lane was proven on 2026-10-30 in podman (11 calls, cost 0) |
| 4 | skill listed | `curl -s http://127.0.0.1:<host-port>/api/skill` | list contains `opencode-mem.memory-usage` (`"path": ".../plugin/skills/memory-usage/SKILL.md"`); poll for it — a cold start answers `[]` first |
| 5 | skill used | ask a model to load `opencode-mem.memory-usage` and follow it | the skill tool returns its 2,781-byte body and the answer applies it |
| 6 | plugin state | `curl -s http://127.0.0.1:<host-port>/api/plugin` | `{"id":"opencode-mem","source":{"type":"local","path":"/home/vaz/.../plugin"},"state":{"status":"active"}}` |
| 7 | skills RPC | `memory` tool `mode:"skills"` | `{"skills":[]}` (the port's `skills.db` registry is empty; the plugin skill is #4, not here) |
| 8 | list | `memory` tool `mode:"list"` | newest-first, `count` 20 for the default page; `all-projects` caps at 1,000 |
| 9 | search | `memory` tool `mode:"search"` for `W8 brute-force search cost benchmark shard layout` | rank 1 `mem_1792900662953_j6e64unbf`, similarity ≈0.54 (JSON-RPC; REST measured 0.5619 on the restored copy) |
| 10 | profile | `memory` tool `mode:"profile"` | non-null profile (version 157 at 2026-10-30T14:05Z; changes as it learns) |
| 11 | web UI | open `http://127.0.0.1:3742` (the engine's default; `OPENCODE_MEM_WEB_PORT` overrides) | HTTP 200, `OpenCode Memory Explorer` shell; `/api/health` = `{"success":true,"status":"ok","authEnabled":false}`; `/api/stats` total = memory count |
| 12 | add + search | the §7 curl pair, or `memory` tool `mode:"add"` then `mode:"search"` for the canary | add 200 / `success:true`; canary is rank 1 (similarity ≈0.79–0.90) |
| 13 | capture | a real session with a prompt | a row appears in `user-prompts.db` (count grows past 1,745) and, with the provider configured, an auto-captured memory |
| 14 | injection | start a second session in the same project | memory context is prepended (the response reflects stored facts); `injectProfile: true` also injects the profile block |
| 15 | compaction | let a session compact (or trigger it) | the `experimental.session.compacting` hook runs and the compaction summary is produced; no engine error |

Checks 1–12 were measured on copies. 13–15 are the hooks, **not** covered by the tool-lane podman run
(which proved the tool lane only) — treat them as observations to record, and a failure here is a finding,
not a surprise.

## 11. Dogfood window

Run the port as the daily driver for **one week**, with the backup untouched:

- Daily: one `search`, one `add`, open the web UI, read the engine log tail
  (`tail -n 40 ~/.opencode-mem/opencode-mem.log`).
- Watch: `user-prompts.db` growth (capture works), profile `version` advancing (learning works), no
  `-32603` in the log, the store's shard versions creeping 2 → 3 only on the projects you write to.
- Record in the dogfood log: date, engine sha256, commit, memory count, prompts count, any error.
- Roll back (§12) on: any data-loss-looking read (missing memories, wrong scope), repeated `-32603` that
  the shm recovery does not fix, or capture silently stopping. A degraded `search` alone is §7.

## 12. Rollback

```sh
# 1. stop OpenCode (quit the app), then confirm:
pgrep -af 'opencode' || echo 'no opencode process'

# 2. restore both config files from the backup
TS=$(cat ~/.opencode-mem-backups/LATEST); BACKUP=~/.opencode-mem-backups/$TS
cp -a "$BACKUP/config/opencode.jsonc"     ~/.config/opencode/opencode.jsonc
cp -a "$BACKUP/config/opencode-mem.jsonc" ~/.config/opencode/opencode-mem.jsonc

# 3. restore the store: move the port-era store OUT of the live root, then copy the backup in
mv  ~/.opencode-mem/data ~/.opencode-mem-backups/data.port-$TS
cp -a "$BACKUP/data" ~/.opencode-mem/data

# 4. start OpenCode; the upstream plugin writes again
```

`mv` (not delete) keeps the failed state for diagnosis. This is the exact form of the restore rehearsed
in `docs/evidence/live-swap-2026-10-30/backup-restore.md`; `rsync` is **not installed** on this host, so
the `rsync -a --delete` form in the PRD is replaced by move-aside + `cp -a` (equivalent, and it cannot
leave stale files behind).

## 13. Export → import: the DR / portability route, not the primary swap

`~/.opencode-mem/exports/opencode-agents-memory-20261030T151445Z.json` (1,547,433 B, sha256
`f3d1b6db26a6e570f6ccf0168ed47c0c472a509499e35c012acccd3038d24370`) rebuilds **the current project's
memory tier only** into a fresh store. Measured 2026-10-30 on a scratch store:

- `917 imported / 0 rejected / 0 skipped`, all 917 rows field-for-field identical (0 mismatches across
  content/type/tags/createdAt/updatedAt/isPinned/display/project/metadata); shard v3 / metadata v2, same
  container tag `opencode_project_4182e0858e0c93a1`, search rank-1 correct.
- Cost: 627.8 s real / 667.6 s dry run for 917 rows (a second run bracketed it at 8.7–10.5 min real,
  7.2–11.1 min dry) — it re-embeds everything.
- A second import aborts the whole import in 521 ms (`import_aborted`, 917 duplicate ids) and writes
  nothing: safe, not a success.
- It carries **no** profiles, prompts, milestones/staging sessions, global memories, or the other 21
  projects. The other projects' directory paths are not recoverable from the store.

So: in-place migration is the swap; the export is what you rebuild this project from if the store is ever
lost, and the cross-machine route. Before cutover, refresh it (`memory` tool `mode:"export"`) and copy it
next to the backup — the bundle is a point-in-time snapshot of *this* project.

---

## Appendix — measured expectations

Host: Void Linux, kernel 7.3.0_1, 12 logical cores, sqlite3 3.53.4, node 24.22.1.
Store at rehearsal (`2026-10-30T17:16:53Z`): 22 shards, 3,104 memories, 1,745 prompts, 2 profiles + 10
changelogs, 29 DBs, 768 dims everywhere, `.cache` 546 MB.

| operation | measured |
|---|---|
| 29-DB checkpoint on a copy | **0.18 s**, all `0|0|0` |
| `cp -a` of the 628 MB root | 0.09–0.23 s (fast NVMe, warm page cache); expect seconds cold |
| sha256 manifest of 85 files (incl. the 547 MB `model.onnx`) | **3.5 s** |
| `integrity_check` over the 29 DBs | **0.6 s** |
| restore into an empty dir + byte compare | < 1 s; 0 differing DBs, 31 identical non-DB files |
| first open on the restored copy | `metadata.db` 1→2 in 242 ms; read battery 5–8 s wall |
| first `add` | 73–97 ms; migrates that project shard 2→3 |
| first `search` after provisioning | 2.9–4.3 s (547 MB ONNX load); then 60–200 ms |
| whole read battery (RPC + web) | 5.3 s first open, 7.6 s with the web surface |

Rehearsal artifacts and the exact commands: `docs/evidence/live-swap-2026-10-30/`.
