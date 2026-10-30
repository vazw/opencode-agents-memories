# ONNX cache provisioning — A/B/C, executed 2026-10-30

The port's tool embedder resolves the model at `$TMPDIR/.opencode-mem-onnx/<embeddingModel>` with
downloads disabled (`crates/mem-bridge/src/composition.rs`, `embedder_from_config`: `cache_root:
std::env::temp_dir().join(".opencode-mem-onnx")`, `hub: None`), and its `tokenizers` 1.0.0-rc.2 reader
accepts **only** the canonical `"version": "2.0"` tokenizer file. Upstream's cache under
`{storagePath}/.cache` is therefore not usable as-is: it holds the legacy v1.0 tokenizer, and it is not
where the port looks.

All three cases ran against a **copy** of the backed-up store (`prov/data`, itself a copy of
`backup-20261030T171653Z/data`), with an isolated `HOME` and a dedicated `TMPDIR` per case. Engine
`72d7f64d…4d5e653`.

## 1. The three cases

| case | `$TMPDIR/.opencode-mem-onnx/Xenova/nomic-embed-text-v1` | `search` |
|---|---|---|
| **A** | absent (`tmp-a` empty) | `{"code":-32603,"message":"Internal error"}` in **11 ms** (second call 9 ms) |
| **B** | upstream `.cache` copied as-is, `tokenizer.json` `"version": "1.0"` | same `-32603` after **3,753 ms** (8 ms on the retry) — the reader rejects the legacy file |
| **C** | provisioned by `scripts/provision-onnx-cache.sh`: same weights, tokenizer canonicalized to **2.0** | works: first search **2,947 ms**, then **63 ms**; `add` **73 ms**; canary search **71 ms** |

Raw JSON-RPC frames (A and B were captured with the same three-frame exchange; C shown with its hit):

```jsonl
{"jsonrpc":"2.0","id":"s","method":"search","params":{"session_id":"ses-raw","project_id":"/home/vaz/Code/Personal/Rust/opencode-agents-memory","scope":"project","query":"provisioning probe","limit":1}}
{"error":{"code":-32603,"message":"Internal error"},"id":"s","jsonrpc":"2.0"}          # A, B
{"id":"s","jsonrpc":"2.0","result":{"count":1,"items":[{"id":"mem_1793381216171_410dee3e4","similarity":0.5452636812005998,…}]}}   # C
```

`hello` answered normally in all three cases (`engine_version 0.1.0`, capabilities include
`add, compaction, export, import, list, list-shards, migrate, profile, search, skills, standalone, web`) —
so a `-32603` on `search`/`add` is the embedder, not the store.

Commands:

```sh
S=.tmp/scratch/issue80-20261031/prov; ENGINE=$PWD/plugin/engine/linux-x64/opencode-agents-memory
# A: empty TMPDIR
TMPDIR=$S/tmp-a OPENCODE_MEM_WEB_PORT=4799 node .tmp/scratch/swap-dryrun/probe.mjs "$S/data" "$S/home" "$S/results/case-a.json" read
# B: upstream cache copied as-is
mkdir -p "$S/tmp-b/.opencode-mem-onnx" && cp -a ~/.opencode-mem/data/.cache/Xenova "$S/tmp-b/.opencode-mem-onnx/"
TMPDIR=$S/tmp-b OPENCODE_MEM_WEB_PORT=4799 node .tmp/scratch/swap-dryrun/probe.mjs "$S/data" "$S/home" "$S/results/case-b.json" read
# C: provision (tracked script), then the same probe in write mode (add + canary search)
TMPDIR=$S/tmp-c scripts/provision-onnx-cache.sh
TMPDIR=$S/tmp-c OPENCODE_MEM_WEB_PORT=4799 node .tmp/scratch/swap-dryrun/probe.mjs "$S/data" "$S/home" "$S/results/case-c.json" write
```

## 2. The provisioner (`scripts/provision-onnx-cache.sh`, tracked)

What it does, and why it is a script rather than a copy:

1. copies the model directory (weights + config + tokenizer) from `--source`
   (default `~/.opencode-mem/data/.cache/Xenova/nomic-embed-text-v1`) into
   `<cache-root>/<model>` (default `${TMPDIR:-/tmp}/.opencode-mem-onnx/Xenova/nomic-embed-text-v1`),
   dropping upstream's partial downloads (`*.tmp.<pid>.<rand>` — 24 MB of them in the live cache);
2. if the copied tokenizer is not `"version": "2.0"`, compiles a 6-line `canonicalize_file` helper on the
   spot and rewrites the **copy** (the source is never touched), then re-asserts v2.0;
3. prints the cache root, model dir, tokenizer version, and the weights' size + sha256.

The helper crate pins `tokenizers =1.0.0-rc.2` **and** `tk-{encode,convert,serialize} =0.1.0-rc.2`.
Without the pins a fresh resolver picks `tk-encode 0.1.0-rc.3`, and `tokenizers 1.0.0-rc.2` no longer
compiles (`unresolved import tk_encode::utils::ProgressFormat`) — measured, and the reason the workspace
rlib was used first time round. `cargo build --offline` keeps the provisioning step off the network.

Measured run (clean cache, cold helper build):

```
canonicalizing …/tmp-c/.opencode-mem-onnx/Xenova/nomic-embed-text-v1/tokenizer.json (version 1.0 -> 2.0)
canonicalized …/tokenizer.json
cache_root=…/tmp-c/.opencode-mem-onnx
model=Xenova/nomic-embed-text-v1
model_dir=…/tmp-c/.opencode-mem-onnx/Xenova/nomic-embed-text-v1
tokenizer_version=2.0
model_onnx_bytes=547552426
model_onnx_sha256=347440e93b5ec979fdcf6041b72721aade7b9680c16022e2830db7115ff6fd9f
real 0m37.563s            # ~35 s of that is the helper build; the copy is page-cached
```

Resulting tokenizer sha256 `cd2ae06ba2731bb72f0d2728e4ce13e2cdbadbfd0374567d2b079d410b5362c3` — identical
to the canonical file the earlier dry run produced, so the transformation is deterministic. Re-running
refreshes the destination from the source (the legacy tokenizer arrives again) and rebuilds; same bytes
out. The destination holds no `*.tmp.*` leftovers.

The weights are byte-identical to the live cache's (`347440e9…`), so no model was downloaded and no
embedding space changed.

`$TMPDIR` is volatile here (`/tmp` is a 16 GB tmpfs): re-run the provisioner after a reboot, or point the
host at a persistent `TMPDIR` and provision with `--cache-root`. A configurable cache root is a code
change (recorded as a follow-up in the issue), not a config key.

## 3. Owner-facing verification through the web API (the runbook's check)

Same provisioned cache, engine on 4799, plain `curl` — this is the exact procedure the runbook gives:

```
web_ready=1
first_gated_call_status=401          # the token file is minted lazily by the first gated request
token_len=64                         # <storage-root>/.auth-token
search_http=200 search_ms= 3427      # GET /api/search?q=W8%20brute-force… , first call = model load
add_http=200 add_ms= 86              # POST /api/memories {content,containerTag,tags}
search_canary_http=200 search_canary_ms= 197
stats_http=200
ui_http=200 ui_bytes=1238            # GET /  → "OpenCode Memory Explorer" shell
{"id":"1","jsonrpc":"2.0","result":{"ok":true}}   # ordered shutdown, exit 0
```

- `GET /api/search?q=…` → rank 1 `mem_1792900662953_j6e64unbf`, similarity **0.5619** (the JSON-RPC path
  measured 0.5369 for the same query in the dry run — same embedder, different surface; both are
  recorded, neither is normalised against the other).
- `POST /api/memories` → `{"success":true,"data":{"id":"mem_…"}}`; the canary then ranks 1 at **0.7877**.
- `GET /api/health` → `{"success":true,"status":"ok","authEnabled":false}`; `GET /api/stats` → total
  matches the store count.
- The token file is `<storage-root>/.auth-token`, **not** upstream's `~/.opencode-mem/.auth-token`
  (which sits one level above `data/` and belongs to the other plugin).

## 4. The live config runs the port unchanged

The live `~/.config/opencode/opencode-mem.jsonc` (17,457 B) was copied into an isolated `HOME` with only
`storagePath` rewritten to the scratch store (byte-identical otherwise — verified by `diff`). The engine
started, migrated the store, served the RPC and web surfaces, and reported
`list-shards.storagePath = <scratch root>`: `--storage-root` wins over the JSONC key. Unknown keys
(`autoUpdate`, `userProfile*`, `opencodeProvider`, …) are tolerated; `autoCleanupEnabled`,
`deduplicationEnabled`, `userProfileAutoCleanupEnabled` all `true` caused no startup scan
(`consolidation.startup_tick projects_scanned=0 runs=0`) and no surprise write. The live store's mtimes
were unchanged afterwards.

## 5. Limits

- One machine, CPU inference; the first search cost (2.9–5.9 s across runs) is the 547 MB model load.
- No remote-embedding path was exercised (`embeddingApiUrl` unset in the live config).
- The canonicalizer is the workspace's own `tokenizers::canonicalize_file`; no hand-rolled format
  conversion was attempted.
