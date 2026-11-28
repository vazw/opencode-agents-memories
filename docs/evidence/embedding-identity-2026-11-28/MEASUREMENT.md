# DEBUG — live write refusal: "embedding model mismatch" on identical model names

Date: 2026-11-28 · repo `/home/opencode/Code/Personal/Rust/opencode-agents-memory` @ `b23e161` (clean)
Live engine pid 7278 (read-only; never signalled). Live store `~/.opencode-mem/` never opened in place.

## [REPRO]

Logger line (live, `~/.opencode-mem/opencode-mem.log:6566`, 2026-11-28T16:27:38):

```
tools.add.write_failed: {"component":"tools","detail":"embedding model mismatch: shard 14003
(stored model 'Xenova/multilingual-e5-base', expected 'Xenova/multilingual-e5-base');
re-embed the scope: opencode-agents-memory --mode=reembed-scope --scope
opencode_project_fd2bfb63318dfd29","level":"error"}
```

Capture path, same window (`:6518`, `:6523`, `:6542`):

```
Auto-capture final error after 5 attempts: {"attempts":5,"captureId":"…","outcome":"dead_lettered",
"projectId":"/home/opencode/Code/Personal/Rust/opencode-agents-memory","reason":"tier1_write_failed"}
```

Runnable, read-only red-capable check (verified below; exit 1 = refusal reproduced, 0 = clear):

```sh
cd /home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/scratch/model-mismatch
S=$PWD/projects/project_fd2bfb63318dfd29_shard_0.db
./check_identity.sh $PWD/home-live  "$S"   # MISMATCH, exit 1  <- the bug
./check_identity.sh $PWD/home-fixed "$S"   # MATCH,    exit 0  <- with "embeddingDtype":"q8" restored
```

`check_identity.sh` resolves a config with the current build (`EmbeddingIdentity::from_config`,
`crates/mem-core/src/embedding.rs:36`), prints `fingerprint()`, and compares it byte-for-byte with
`shard_metadata.embedding_fingerprint` read from the copied shard — exactly what
`judge_model_facts` (`crates/mem-store/src/shard_facts.rs:577`) compares.

Environment: config `/home/opencode/.config/opencode/opencode-mem.jsonc` (read-only copy),
`storagePath ~/.opencode-mem/data`, shard copy of `~/.opencode-mem/data/projects`
(`.tmp/scratch/model-mismatch/projects`, 70 MB, 38 `*.db` + their `-wal`, copied before any open;
no `-shm` was created in the live root).

## [MINIMAL CASE]

One field, one byte-difference:

```diff
-{"v":1,"model":"Xenova/multilingual-e5-base","dims":768,"pooling":"mean","dtype":"q8", ...}
+{"v":1,"model":"Xenova/multilingual-e5-base","dims":768,"pooling":"mean","dtype":null, ...}
```

(rest identical: `task_prefixes:false, query_prefix:"query: ", document_prefix:"passage: "`, no
`max_tokens` key on either side). Configured identity of the live config, MEASURED by the probe:

```
model=Xenova/multilingual-e5-base dimensions=768 pooling=Mean dtype=None
use_task_prefixes=false query_prefix=Some("query: ") document_prefix=Some("passage: ")
max_tokens=None
FINGERPRINT={"v":1,"model":"Xenova/multilingual-e5-base","dims":768,"pooling":"mean","dtype":null,"task_prefixes":false,"query_prefix":"query: ","document_prefix":"passage: "}
```

Eliminated: model, dims, pooling, both prefixes, task-prefix flag, max_tokens — all identical on
both sides and all resolved from the `Xenova/multilingual-e5-base` preset
(`crates/mem-core/src/config/normalize.rs:432-437`). `dtype` is the only input that differs.

## [FINDINGS] — ranked hypotheses

**H2 (in the brief) — CONFIRMED, narrowed to one input: `dtype`.** Stored `"dtype":"q8"`;
resolved config `embedding_dtype = None` (key absent) → `"dtype":null`. Evidence:
`~/.config/opencode/opencode-mem.jsonc` has no live `embeddingDtype` (line 27 holds only the
commented example `// "embeddingDtype": "q8",`); `normalize_embedding_dtype`
(`normalize.rs:258-273`) returns `None` when the key is absent; nothing else fills it
(`config/mod.rs:287`); `EmbeddingIdentity::from_config` maps it verbatim (`embedding.rs:42`).
Measured consequence: **0/38** shards match the current build's fingerprint; **38/38** match it
with `"embeddingDtype": "q8"` restored.

**H1 — fingerprint algorithm changed after the shards were written — FALSIFIED.**
`git log -S 'FingerprintV1'` and `-S 'fn fingerprint'` on `crates/mem-core/src/embedding.rs` return
exactly one commit, `6bd8eed` (2026-10-28). The only later touch, `11465e0` (2026-11-01), added
`max_tokens` with `#[serde(skip_serializing_if = "Option::is_none")]`; `crates/mem-core/tests/
embedding_identity.rs:38-42` pins the default literal as byte-identical "to the pre-change
literal". `judge_model_facts` history: `94e4261`, `13d3f29` — both before the shards were stamped
and before HEAD; the comparison (`configured == stored`) is unchanged.

**H3 — only shard 14003 has a fingerprint — FALSIFIED.** All 38 `*.db` carry
`embedding_fingerprint` and `embedding_model`; a single distinct fingerprint value across all 38
(`SELECT … GROUP BY`-equivalent count: 38/38 identical). Registry `metadata.db:shards` maps
id 14003 → `project_fd2bfb63318dfd29_shard_0.db` (3154 `memories`), not a filename.

**H4 — truncated/corrupt fingerprint — FALSIFIED.** Byte-identical, well-formed JSON in all 38,
including keys introduced by the current schema (`query_prefix`, `document_prefix`).

**H5 — the capture `tier1_write_failed` is a different failure — FALSIFIED, same judge.**
`write_capture_summary` (`crates/mem-bridge/src/capture.rs:180`) → `get_write_shard` →
`resolve_write_shard` → `prepare_write_shard` (`crates/mem-store/src/shard_manager.rs:1360`) →
`judge_model_facts` (`shard_facts.rs:271`). Anything that is not `MigrationLocked` becomes
`CaptureWriterError::Failed` (`capture.rs:216`) → `Failure::of("tier1_write_failed")`
(`capture.rs:946`, `:1119`). Same refusal, coarser label.

**H6 — added: the config regression is a lost line in the revert chain — CONFIRMED as the origin.**
Config git (`~/.config/opencode` is versioned):
`48194aa` (2026-11-28 14:52:10) `+ "embeddingDtype": "q8",` alongside
`+ "embeddingModel": "Xenova/multilingual-e5-base",`;
`f0e122e` (14:52:19) reverted **both** lines back to nomic;
`c7d2540` (15:01:38) restored **only** `embeddingModel` — the dtype line was left deleted. The
shards had already been stamped by the adoption run (`scripts/adopt-embedding-model.sh:70`
`TARGET_DTYPE=q8`, which sets the key in place, `:1018-1036`) while the working-tree config carried
it; shards `14005`/`14006` were created 2026-11-26/27, before the commit dance, carrying the same
q8 document.

**H7 — added: the vectors really are q8, so the refusal is a false positive — CONFIRMED.**
`~/.opencode-mem/data/.cache/Xenova/multilingual-e5-base/onnx/` contains only
`model_quantized.onnx` (278 647 662 B; no `model.onnx`). With `dtype: None`,
`LocalOnnxEmbedder` calls `resolve_local_onnx_dtype` (`crates/mem-embed/src/local/mod.rs:173`),
which falls to `detect_cached_dtype` (`model_cache.rs:114-127`) and returns **Q8** from the cached
`model_quantized.onnx`. The running engine therefore loads q8 weights — the same dtype the shards
record. Store and runtime agree on the numbers; only the identity document differs.

**Secondary defect (message) — CONFIRMED.** `ShardModelMismatch` Display
(`crates/mem-store/src/error.rs:46-71`) prints only `stored model` vs `expected` model name, which
is why a dtype-only mismatch is reported as two identical strings. The fingerprint is never
printed, so the operator cannot see which field differs.

## [ROOT CAUSE]

CONFIRMED. `~/.config/opencode/opencode-mem.jsonc` lost its explicit `"embeddingDtype": "q8"`
during the revert `f0e122e`; `c7d2540` restored only `embeddingModel`. The build therefore
resolves `embedding_dtype = None`, serializes the identity as `"dtype":null`, and
`judge_model_facts` (`shard_facts.rs:577-597`) compares the full fingerprint string — so all 38
shards, which all store `"dtype":"q8"`, judge `ModelAuthority::Mismatch` and every write in the
scope is refused. The refusal is a false positive: with `dtype` unset, the loader auto-detects and
loads the same q8 weights (`model_quantized.onnx`) that produced the stored vectors. Capture
dead-letters because its tier-1 write traverses the same `prepare_write_shard → judge_model_facts`
seam.

## [INVERTED WORLD] — what would prove this is NOT a bug

The refusal is legitimate if store and runtime genuinely use different weights. Observation that
would settle it: `~/.opencode-mem/data/.cache/Xenova/multilingual-e5-base/onnx/model.onnx` present
(fp32 weights selected by the dtype-less loader) **and** shards stamped `"dtype":"q8"`. Measured:
that file is absent; only `model_quantized.onnx` exists, so the loader resolves q8 — the same
dtype the shards record. Second falsifier: if the stored fingerprint for shard 14003 had read
`"dtype":null` (a shard stamped by a dtype-less build), the current identity would match and there
would be nothing to diagnose.

## [FIX PRESCRIPTION]

**(a) The fix is a data operation (one config line), not a code fix, and NOT a re-embed.**

`~/.config/opencode/opencode-mem.jsonc`, insert as a new line 16 (immediately after line 15
`"embeddingModel": "Xenova/multilingual-e5-base",`):

```jsonc
    "embeddingDtype": "q8",
```

which is exactly what the adoption script wrote (`scripts/adopt-embedding-model.sh:1018-1036`,
`TARGET_DTYPE=q8`) and what `f0e122e` deleted. This is a config-repo change; commit it in the
config repo next to `c7d2540` so the same revert cannot drop it again.

Do **not** run `--mode=reembed-scope`: with the current dtype-less identity it would re-embed
3154 vectors into the same q8 space and re-stamp a shard whose fingerprint no other shard shares —
work and drift with no benefit. The refusal is an identity-document mismatch, not a vector-space
mismatch.

The live process reads its config once at startup (`src/main.rs:811` "loaded SYNCHRONOUSLY"), so
the engine must be restarted by the operator after the edit; no code rebuild is needed.

**(b) Exact file:line if the hardening is taken as code.**
- `crates/mem-bridge/src/composition.rs:236` — builds the store identity with
  `EmbeddingIdentity::from_config(config)` while `local_load_options_from_config` (`:514-525`) lets
  the loader resolve the *effective* dtype from the cache (`mem-embed/src/local/mod.rs:173`,
  `model_cache.rs:123`). Resolve the dtype once (config value, else
  `resolve_local_onnx_dtype`) and feed both the loader options and the identity, so the document
  can never name a dtype the runtime does not load. Same seam in `crates/mem-bridge/src/
  reembed.rs:158` and `src/shard_consolidation.rs:1301,1666`.
- `crates/mem-store/src/error.rs:46-71` — print the differing fingerprint fields (at minimum
  `dtype`/prefixes) instead of only the model names; a dtype-only mismatch currently renders as
  two identical strings.

**(c) Risk to the other 37 shards: none, given the measured state.** All 38 carry a byte-identical
q8 fingerprint (38/38 match the fixed identity), so restoring `"embeddingDtype": "q8"` clears
every shard at once and changes no vector. Residual, state it: the fingerprints are the only
provenance record (the claim path `claimable_fingerprint`, `shard_facts.rs:202-224`, gates on
dimensions/model only, not dtype), so a shard whose vectors were produced with a different
effective dtype but stamped q8 would now be accepted silently. Nothing in the cache or logs
indicates that happened — only one weights file exists. Second residual: with the dtype key still
absent, any *newly created* shard would be stamped `"dtype":null` by
`claimable_fingerprint`, splitting the scope's identity; restoring the line removes that too.

**(d) Runnable non-vacuity check.** `.tmp/scratch/model-mismatch/check_identity.sh` — verified
above: exit 1 (MISMATCH) against the live config, exit 0 (MATCH) against the same config with the
one line added. It drives the exact comparison `judge_model_facts` performs, on the exact failing
input (real resolved config vs shard 14003's stored bytes), so it cannot pass while the bug is
present. After the operator applies (a) and restarts the engine, `memory add` must succeed; re-run
the script against `~/.config/opencode` to confirm MATCH without touching the store.

## [POST-MORTEM NOTE]

The `memory add` bug-pattern write for this diagnosis was refused with the same `internal`
(harmlessly self-referential: the add path is the bug). The full lesson — config chain
`48194aa → f0e122e → c7d2540`, the q8/null fingerprint pair, the cache-only-quantized-weights
measurement, the shared capture seam and the two residual risks — is captured above and must be
replayed into `memory add` (type `bug-pattern`) once writes are green. Falsified routes worth their
own memory: (1) not a fingerprint-algorithm change — `6bd8eed` is the only `FingerprintV1` commit
and `11465e0`'s `max_tokens` is `skip_serializing_if = "None"`; (2) not a per-shard anomaly — 38/38
shards carry one identical fingerprint.

## [STATUS]

`DIAGNOSED-NEEDS-CODER` — root cause confirmed by measurement; the unblock is a one-line config
restoration owned by the operator (plus a restart of pid 7278), and the code hardening above is
scoped but not implemented.

## [CORRECTION 2026-11-28T17:0xZ — append-only]

The reproducer section above claims `home-live` exits 1 (MISMATCH). That stopped being true at
16:50: `home-live` was synced from the live config *after* `e99bcc0` restored `"embeddingDtype":
"q8"`, so `home-live` and `home-fixed` are now identical and both MATCH (ARCHITECT cross-check,
16:5xZ). The root cause is unaffected — it rests on the measured 38/38 `"dtype":"q8"` shard set,
the pre-fix fingerprint preserved in `now.txt` (`"dtype":null`), and the config chain `48194aa ->
f0e122e -> c7d2540 -> e99bcc0`. To reproduce RED, remove `embeddingDtype` from a copy of the
config (or use the pre-fix `now.txt`) and rerun: exit 1.

## [STATUS UPDATE 2026-11-28T17:0xZ — append-only]

Unblock applied, not by a coder: config-main restored the line (`e99bcc0` on `c7d2540`, one added
line, `embeddingModel` untouched), and the owner restarted the engine. MEASURED after restart: pid
5446 started 16:55:32, `/proc/5446/exe` -> staged `opencode-agents-memory` (no deleted inode),
`.engine.lock` pid 5446; `Auto-capture memory persisted` at 16:55:40 and 16:55:48, i.e. writes are
accepted again. Residual dead letters after restart are the unrelated `host_model_not_found`
(`opencode-go/beam-free`, 404) from pre-restart queued captures. The code hardening is scoped as
issue 177 in `.scratch/rust-port/issues/`; no re-embed was run and none is needed.
