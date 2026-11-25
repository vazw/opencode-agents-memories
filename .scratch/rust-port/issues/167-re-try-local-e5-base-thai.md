# 167 — re-try the local multilingual-e5-base now that the truncation cap ships

Status: ready-for-human (owner decision: local over hosted, planned 2026-11-25)
Filed: 2026-11-25

## Why this exists

On 2026-10-31 `Xenova/multilingual-e5-base` measured **75.27 Thai nDCG@10** against nomic-embed-text-v1's
**0.19** on this repo's own Thai harness (`docs/runbooks/adopt-embedding-model.md:15-16`), and was rolled back for a
build defect, not for quality: `crates/mem-embed/src/local/tokenize.rs` did not truncate, e5 is XLM-R with a hard
512-position limit, so any input over ~512 tokens failed the ONNX graph and the reembed CLI exited `74` on the
first long scope (213 of 3,079 rows exceeded ~1,800 chars).

That blocker is gone. MEASURED 2026-11-25:

- `truncate_tokenized` caps a sequence at `max_tokens` ids, keeping head and tail — `crates/mem-embed/src/local/tokenize.rs:54`.
- `effective_max_tokens` derives the cap from the model's declared limit minus special tokens — `crates/mem-embed/src/local/model_cache.rs:203`.
- The cap is wired through `LocalLoadOptions.max_tokens` and applied per embed — `crates/mem-embed/src/local/mod.rs:48`, `:110`, `:221-243`.
- `embeddingMaxTokens` exists as a config key — `crates/mem-core/src/config/schema.rs:439`.

Owner ruling 2026-10-31 was "do not retry e5 until a truncation cap ships". It shipped.

## The bar, not the claim

The 75.27 predates truncation: it was measured on inputs that all fit. Re-derive it on the current build before
committing to the swap. Compare like for like on the same harness against the current model.

## The swap, in order

1. Re-run the Thai harness against `Xenova/multilingual-e5-base` @ `q8` on HEAD; record nDCG@10 with its date.
2. If it beats the current model on the same harness, follow `docs/runbooks/adopt-embedding-model.md`:
   `--swap` refuses while running, refuses an engine built before the cache root moved, reads the scope list from a
   copy of `metadata.db`, takes `--backup`, `--provision`s the model under `<storagePath>/.cache`, edits exactly two
   config keys, then runs one `--mode=reembed-scope` per container tag and stops at the first failure with a resume
   command.
3. `--verify`: engine count and parent, web health, add + search round-trip, store versions, the resolved cache root
   (model present under the store), the shard's stored identity (model + dimensions + fingerprint), a Thai and an
   English query canary, and the engine's RSS.
4. Keep the `--rollback` path unexercised but ready. A default rollback needs `--from` when more than one
   `adopt-*.info` exists.

Config edit, two keys plus the optional cap (the cap now defaults correctly, so the third line is optional):

```jsonc
"embeddingModel": "Xenova/multilingual-e5-base",
"embeddingDtype": "q8",
// "embeddingMaxTokens": 512,   // optional: defaults to the model's own declared limit
```

## Known costs

- The model is **not cached**: `~/.opencode-mem/data/.cache/Xenova/` holds only `nomic-embed-text-v1` (546 MB).
  e5-base q8 downloads once into the store (~280 MB) and survives reboots.
- Full re-embed of the store (~5.6k memories), locally, CPU. `--swap` backs up first and moves post-backup memories
  aside rather than deleting them.
- Same width (768) as the current model, so no store-format change — but the vector space differs, so the re-embed
  is mandatory, and `episodic.db` stays in the old space (it is never re-embedded).
- Context is 512 tokens with head+tail truncation, against 32k for a hosted 8B model. Acceptable for memory
  passages; state it so the loss is a decision, not a surprise.

## Inverted world

Closed as *measurement, not a deliverable* if the re-measured Thai nDCG on the current build does not beat the
current model on the same harness. In that case the 2026-10-31 figure was an artifact of the harness's short
inputs, and the defect is in the measurement, not the model.

## Not in scope

- Hosted embedding providers (owner selected local on cost, egress and width).
- Issue 166 (adapter re-activation wiping live blocks).
