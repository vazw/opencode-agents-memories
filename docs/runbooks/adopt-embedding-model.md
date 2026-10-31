# Runbook — adopt `Xenova/multilingual-e5-base` (q8) as the live embedding model

**Owner-run, OpenCode stopped.** The executable form is
`scripts/adopt-embedding-model.sh`; this file is the spec it mirrors. Where the
two disagree, the script's `--help` and its refusal messages are the ones that
ran.

## Why

Measured 2026-10-31:

| fact | value | source |
|---|---|---|
| live engine RSS on fp32 nomic | 1,152,180 kB (`VmRSS`), 1,573,644 kB `VmHWM` | `.tmp/progress/engine-rss-debug.md` |
| `nomic-embed-text-v1` Thai retrieval | **0.19 nDCG@10** on Thai MIRACL | `.tmp/scratch/search-embedding-models-20261031.md` |
| `multilingual-e5-base` Thai retrieval | **75.27 nDCG@10** (same harness) | same |
| e5-base q8 weights | `onnx/model_quantized.onnx`, 278,647,662 B, sha256 `df7a9a29…fac94` | HF API + re-hashed locally |
| dimensions | 768 = the store's current width | `crates/mem-core/src/config/normalize.rs` |
| pooling / prefixes | preset `multilingual-e5` → mean + `query: ` / `passage: ` | same, `normalize.rs:353-358` |

The store is 768-wide in every shard, so the swap needs **no store migration**
— only a re-embed of each scope's vectors into the new model space.

## Preconditions

- OpenCode (and therefore the engine) is **stopped**. Every writing mode
  (`--backup`, `--provision`, `--swap`, `--rollback`) refuses otherwise;
  `--dry-run` and `--verify` are safe while it runs. `--provision` counts as a
  writing mode because its default invocation canonicalises the store's nomic
  `tokenizer.json` in place (v1.0 → v2.0).
- The staging directory `<repo>/.tmp/scratch/e5-base-src/` holds `config.json`,
  `tokenizer.json` (v1.0 — the provisioner canonicalises the copy),
  `tokenizer_config.json`, `special_tokens_map.json` and
  `onnx/model_quantized.onnx`. Pass `--source DIR` if it lives elsewhere.
- The repo's `crates/` change is present: the engine resolves its model cache
  at `<storagePath>/.cache` (not `/tmp`). **The engine must be rebuilt and
  re-staged before `--swap`** — see step 3; `--swap` refuses an engine that
  still carries the old tmpfs path (measured: such an engine exits 74 at scope
  1 of 22 *after* the config was flipped).

## Sequence

```sh
# 0. quit OpenCode; confirm nothing is left
pgrep -af opencode

# 1. read the plan and the preflight (writes nothing outside its work dir)
scripts/adopt-embedding-model.sh --dry-run

# 2. rebuild + re-stage the engine FIRST (the cache root is compiled in).
#    exit-5 prerequisite: web-ui/dist/index.html must exist
#    exit-7 prerequisite: the staged bundle must be removed first, or
#    stage-engine builds and THEN exits 7
ls -l web-ui/dist/index.html
rm -rf plugin/engine/linux-x64/web-ui/dist
bash plugin/scripts/stage-engine.sh
LC_ALL=C grep -c opencode-mem-onnx plugin/engine/linux-x64/opencode-agents-memory   # must print 0

# 3. the cutover: backup -> provision -> config edit -> re-embed per scope
scripts/adopt-embedding-model.sh --swap --yes

# 4. if it stopped: rebuild (step 2) if you have not, then run the resume
#    command the STOPPED block prints (backup, provision and the config edit
#    are idempotent; migrated scopes short-circuit without embedding anything)

# 5. start OpenCode, then
scripts/adopt-embedding-model.sh --verify

# 6. keep the backup for a week; roll back if a rollback trigger fires
scripts/adopt-embedding-model.sh --rollback --yes --from <stamp>
```

After `--verify` passes, delete the engine's own pre-reembed copies (one
`.bak` + `.bak-wal` per re-embedded scope, inside `data/`); the summary prints
the count, the size and the exact `find … -delete`. They are excluded from the
backup's WAL check, so they never block a later `--backup`, but they do bloat
every `cp -a`.

## What each mode does

- **`--dry-run`** — read-only. Not-running check, the live config values
  (active keys only: the file's commented alternatives are not values), the
  target's cache or `--source`, the dimension resolution (768 required —
  384/1024 are refused because a re-embed cannot change a shard's width), free
  disk (2× weights + two store copies), the scope count read from a **copy** of
  `metadata.db`, and every command the real modes run. Exit 3 on a FAIL line.
- **`--backup`** — delegates to `scripts/live-swap.sh --backup`: checkpoint,
  dated `cp -a`, sha256 manifest + verify, per-DB `integrity_check`, `LATEST`.
  The backup root must be outside the store root; nothing is overwritten.
- **`--provision`** — `scripts/provision-onnx-cache.sh --model
  Xenova/multilingual-e5-base --weights onnx/model_quantized.onnx --source …`,
  into `<storagePath>/.cache`, then the pinned sha256/byte count are verified
  and a mismatch refuses. Idempotent. Not needed after a reboot any more: the
  cache is under the store, not in `/tmp`. A **writing** mode (it canonicalises
  the store's nomic tokenizer in place), so it refuses while OpenCode runs.
- **`--swap`** — the cutover, in order: refuse while running → refuse an engine
  built before the cache root moved → read the scope list from a copy of
  `metadata.db` → `--backup` → `--provision` → config edit → one
  `--mode=reembed-scope` per container tag, printing each exit code and
  stopping at the first failure (the STOPPED block prints the resume command
  and the rebuild it needs).
- **`--verify`** — after OpenCode starts: `live-swap.sh --verify` (engine count
  and parent, web health, add + search round-trip, store versions) plus the
  resolved cache root (must be under the store, model present), the shard's
  stored identity (model + dimensions + fingerprint), a Thai and an English
  query canary, and the engine's RSS. Then the manual checks it cannot do.
- **`--rollback`** — refuses while running. Default: restore the config and the
  store from the backup that `--swap` took (minutes; post-backup memories are
  moved aside, never deleted). `--config-only`: restore only the config and
  re-embed every scope with nomic (minutes to tens of minutes; the brief's
  40-80 min figure is unverified); until a scope finishes, the store answers
  reads for it with a typed `ShardModelMismatch`. Both routes then
  **canonicalise the restored nomic `tokenizer.json`** (the upstream cache is
  v1.0, which this engine's reader rejects — measured rc 74 without it) and
  report whether it worked. A default rollback **needs `--from`** when more
  than one `adopt-*.info` exists: a resumed `--swap` writes a second record
  whose restore point is the mid-migration store.

## The config edit

Two keys, in place, nothing else:

```jsonc
"embeddingModel": "Xenova/multilingual-e5-base",
"embeddingDtype": "q8",
```

- The model value is replaced on its existing line; if no active
  `embeddingDtype` exists, one line is inserted directly below. Comments,
  commas and every other line are byte-preserved, and the diff is verified to
  be exactly one replaced line plus one inserted (or two replaced) line.
- `embeddingPooling` / `embeddingDimensions` are **refused** if active: an
  explicit key beats the `multilingual-e5` preset, so the adoption would
  silently run with the wrong pooling or width.
- `embeddingApiUrl` / `embeddingApiKey` active is refused: the engine would use
  the remote lane and never load a local model.
- A copy is kept as `opencode-mem.jsonc.pre-model-swap-<TS>` and the exact
  restore command is printed.

## Failure semantics (do not soften these)

- Re-embed is **per-scope atomic**. A mid-run failure leaves the scopes already
  done on e5 and every remaining scope on nomic; the store then answers reads
  for those scopes with a typed `ShardModelMismatch` — a designed refusal, not
  corruption. The script stops, says exactly that (including "no scope has been
  migrated" when none was), prints the remaining tags, the resume command with
  the flags actually in use (`--rehearse` included), and the engine rebuild the
  resume needs. A stale engine fails every scope the same way, so the rebuild
  comes before the resume.
- `episodic.db`'s `agent_sessions.embedding` is **never re-embedded**. Rows
  written before the adoption keep nomic-space vectors beside e5-space ones
  (same 768 width, so nothing errors; the similarity for those rows is
  meaningless). `--swap` and `--verify` both warn; the summary repeats it.
- The live store is never opened: `metadata.db` and the shard DBs are copied
  (`{db,-wal}`) into the work dir and only the copies are queried. A new
  `-shm`/`-wal`/`-tshm` in the live root aborts the run.
- A scope enumeration that fails (an unreadable `metadata.db`, a copy that
  cannot be written) **refuses the run** — in `--swap` before the first write,
  in `--dry-run` as a FAIL line, in `--verify` as a FAIL naming the copy, in a
  `--config-only` rollback as a refusal instead of an empty command list. It is
  never "no active scopes".
- The engine's own `*.pre-reembed-*.bak` / `.bak-wal` files live inside `data/`.
  `live-swap.sh` excludes them from its WAL check (they are not a torn copy),
  so they never block a later `--backup`. They do bloat every `cp -a`: delete
  them once `--verify` passes — the summary prints the count, the size and the
  exact `find … -delete`.

## Rollback triggers

- Thai retrieval is not better after a week of dogfood.
- Engine RSS does not fall below the fp32 baseline.
- Any `-32603` from `search`/`add` that the cache check in `--verify` cannot
  explain.
