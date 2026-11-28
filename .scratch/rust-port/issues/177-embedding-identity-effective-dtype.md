# 177 — the store identity names the configured dtype while the loader loads the cache-resolved one

Status: **done** — landed in `596aadd` (2026-11-28). Work: `crates/mem-bridge/src/embedding_identity.rs`
(`resolve_effective_embedding(config) -> (LocalLoadOptions, EmbeddingIdentity)` resolving `None`/`Some(Auto)`
through the cache), wired at `crates/mem-bridge/src/composition.rs`, `crates/mem-bridge/src/reembed.rs`,
`src/shard_consolidation.rs` and `crates/mem-web/src/api/store_adapter.rs`; `ModelAuthority::Mismatch` and
`ShardModelMismatchEntry` carry stored+expected fingerprints and `Display` names the differing fields.
Tests: `identity_dtype_resolves_from_cache` / `identity_dtype_resolves_auto_from_cache` (mem-bridge) and
`mismatch_display_names_differing_fields` (mem-store), each with a recorded mutant. Residual OPEN (not this
issue): `claimable_fingerprint` still gates on dimensions/model only, not dtype — stated in code.
Filed: 2026-11-28 from the live write-refusal diagnosis (`docs/evidence/embedding-identity-2026-11-28/MEASUREMENT.md`),
which found the config-side origin and scoped this code hardening. The config fix (`e99bcc0`) is the
unblock; this issue is the seam that let a one-line regression become a 38-shard refusal.

## The defect

The store identity and the loader disagree on the effective dtype:

- `crates/mem-bridge/src/composition.rs:236` builds `EmbeddingIdentity::from_config(config)` — the
  **configured** value.
- `local_load_options_from_config` (`composition.rs:514-525`) passes `dtype: config.embedding_dtype`;
  when it is `None`, the loader resolves the effective dtype from the cache
  (`crates/mem-embed/src/local/mod.rs:171-177` → `resolve_local_onnx_dtype`,
  `crates/mem-embed/src/local/model_cache.rs:123`, which probes `model_quantized.onnx`).

So a config with no `embeddingDtype` stamps `"dtype":null` into the fingerprint while the runtime loads
q8 weights. `judge_model_facts` compares the full fingerprint string
(`crates/mem-store/src/shard_facts.rs:577-597`), so every write in the scope is refused with a message
that names only the model:

```rust
// crates/mem-store/src/error.rs:46-71
write!(formatter, "shard {} (stored model '{model}', expected '{}')", shard.id, shard.expected)
```

Two identical model names for a dtype-only mismatch. The same identity-from-config seam appears at
`crates/mem-bridge/src/reembed.rs:158` and `src/shard_consolidation.rs:1301,1666`.

The refusal path lives in `prepare_write_shard → judge_model_facts`
(`crates/mem-store/src/shard_manager.rs:1360` → `shard_facts.rs:271`), the same one the capture tier-1
write traverses (`crates/mem-bridge/src/capture.rs:180,216`), so capture dead-letters under the same
defect.

## Impact

A dtype-only config drift (exactly the `48194aa → f0e122e → c7d2540` chain that dropped
`embeddingDtype`) makes every write and every capture in a 38-shard scope fail, with a log line that
cannot distinguish "different model" from "same model, different dtype". The store and the runtime were
never actually in different vector spaces; only the identity document differed.

## Fix (one seam, four sites)

Resolve the effective dtype **once** — `config.embedding_dtype`, else
`resolve_local_onnx_dtype(model, cache_root)` — and feed that same `Option<LocalOnnxDtype>` into both
`LocalLoadOptions.dtype` and the `EmbeddingIdentity` construction at all four sites. The document can
then never name a dtype the loader does not load.

Second half: `judge_model_facts` must carry the **stored fingerprint** out of
`ModelAuthority::Mismatch` (it carries only `stored_model` today, `shard_facts.rs:588-592`), and
`ShardModelMismatchEntry` (`error.rs:22-29`) must carry the stored and expected fingerprints (or the
differing fields) so `Display` can print `dtype`, `pooling` and the prefixes. Today the stored
fingerprint is discarded at judgement time, so changing `Display` alone cannot fix the message.

## Options

**A — one helper returns both halves.** `resolve_effective_embedding(config) -> (LocalLoadOptions,
EmbeddingIdentity)`; every site calls it and passes each half on. Deletion test: four ad-hoc
`from_config` + `local_load_options_from_config` pairs collapse to one call each.

**B — make `EmbeddingIdentity::from_config` consult the cache.** Rejected: it needs a `cache_root` it
does not have, turns a pure document constructor into I/O, and couples `mem-core` to `mem-embed`. The
identity must stay a value derived from resolved inputs.

**Discriminating measurement.** Point a config with `embeddingDtype` unset at a cache holding only
`model_quantized.onnx`; assert the constructed `LocalLoadOptions.dtype` **and** the
`EmbeddingIdentity.dtype` are both `Some(Q8)`, and the fingerprint says `"dtype":"q8"`. Under the
current code the loader resolves Q8 while the identity says `null` — the assertion reds. Second row:
a mismatch differing only in dtype must render two different strings and name `dtype`; today both
sides print the same model name.

## Residuals (do not claim more safety than exists)

- Fingerprints are the **only** provenance record of what produced the vectors; nothing re-checks the
  weights behind a stored shard.
- `claimable_fingerprint` (`crates/mem-store/src/shard_facts.rs:202-220`) gates on dimensions and
  model only — **not dtype** — so a shard whose vectors came from a different effective dtype but was
  stamped q8 is accepted silently. This issue does not close that; state it in the code comment.
- With the dtype key absent, any newly created shard is stamped `"dtype":null` by the same claim path,
  splitting a scope's identity. Resolving the effective dtype (A) removes that too; the record's
  second residual should be closed here, not by the config line.

## Acceptance criteria

1. All four sites (`composition.rs:236`, `reembed.rs:158`, `shard_consolidation.rs:1301,1666`) derive
   identity and loader options from one resolved dtype value; no site calls `EmbeddingIdentity::from_config`
   on the raw config beside a separately-resolved loader dtype.
2. A unit test pins: `config.embedding_dtype = None` + a cache with only `model_quantized.onnx` ⇒
   identity dtype `Q8` and loader dtype `Q8`, fingerprint `"dtype":"q8"`. Non-vacuous: revert the
   identity half to `from_config` and the assertion reds (`null` vs `q8`).
3. `ShardModelMismatchEntry` carries the differing fingerprint fields; `Display` names `dtype` (and
   any other differing field it prints). A test with stored `"dtype":"q8"` vs expected `"dtype":null`
   asserts the output contains two different strings and the token `dtype`; revert the field and the
   test reds as two identical model names.
4. The `claimable_fingerprint` residual is stated in a code comment and in this issue; no criterion
   claims dtype validation.
5. `cargo test -p mem-bridge -p mem-store -p mem-embed` green; no count or duration pinned.
6. No live store, live log or host touched; the test cache is a fixture directory under a temp root.

## Inverted world — what would close this as "not a defect"

Every shipped config sets `embeddingDtype` explicitly (it does not: `opencode-mem.example.jsonc`
ships the key commented, and the live incident was exactly the unset case), **or** the loader never
resolves a dtype from the cache (it does, `model_cache.rs:123`). Neither holds. A second falsifier: if
the identity's dtype and the loader's dtype were structurally the same value (they are two independent
reads of `config`/cache today), there would be no seam to fix.

## Dependencies

None. Independent of 174–176. No new ADR: this is an internal seam and reversible; the storage-format
and identity-document decisions are unchanged.
