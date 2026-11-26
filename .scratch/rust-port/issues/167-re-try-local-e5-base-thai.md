# 167 — re-try the local multilingual-e5-base now that the truncation cap ships

Status: wontfix — closed 2026-11-26 as measurement, not a deliverable. The store holds no non-Latin script content
(0 rows of 5,601) and the prompts none either (0 of 417); the 75.27-vs-0.19 bar rests on a harness that was never
tracked and is now gone. Re-entry gate, measurement and the truncation arithmetic:
`docs/evidence/embedding-language-census-20261126/`.
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

## Closure 2026-11-26 — measurement, not a deliverable

Full evidence: `docs/evidence/embedding-language-census-20261126/` (`census.py`, `run.txt`, `README.md`). Read-only
over a §2a copy of the live store; live engine and host untouched.

**The premise does not hold in this store.** Across 5,601 memories and 417 captured prompts, rows containing Thai, Han,
Kana, Hangul, Cyrillic, Arabic, Hebrew or Devanagari script: **0** (0.00 %). The 75.27 figure was measured on Thai
MIRACL, a corpus with no counterpart here. Its harness lived in `.tmp/scratch/search-embedding-models-20261031.md`,
which is gone, and no NDCG code exists anywhere in `crates/`, `tests/`, `scripts/` or `docs/` — so the bar this issue
asked to re-derive cannot be re-derived at all.

| gate | threshold | measured |
|---|---|---|
| non-Latin-script share of memories | ≥ 1 % trips | **0.00 %** |
| highest per-project non-Latin-script share | ≥ 50 % trips | **0.00 %** (all 36 shards) |
| non-Latin-script share of captured prompts | ≥ 1 % trips | **0.00 %** |

**The swap's cost is real and points the other way.** e5 caps at 512 tokens with head+tail truncation; nomic declares
`n_positions: 8192`. 206 of 5,601 rows exceed 1,800 chars, 3 exceed 4,096, longest 6,603 — the current model embeds all
of them whole, e5 would not. This argument survives a store that did contain non-Latin script.

**Two caveats stated as limits, not smoothed over.** The prompt store is pruned: its 417 rows span
`2026-10-26T07:09Z .. 2026-10-31T07:27Z` (six calendar days), so they describe recent usage, not history. And 39.96 % of
memory rows carry non-ASCII characters — but that is typography and Western European orthography (`—` 2,784, `→` 1,966,
`…` 625, `§` 436, `é` 447), not language; hence the gate is script-aware and density-filtered, and Greek is excluded
because its 37 rows are Δ/Σ/λ/µ notation.

**Method note worth keeping.** The first census run for this issue watched only non-Latin ranges and reported 99.95 %
ASCII — it could not see em dashes. The byte-vs-character cross-check
`length(cast(content as blob)) > length(content)` returned 2,238 rows and exposed the error. The second instrument
caught the first; the census now prints that cross-check on every run.

**Deliberately not built:** the nomic-vs-e5 retrieval canary harness, the model swap, the 280 MB e5 download, any
`scripts/` change, any engine restaging. Recorded so omission is not later mistaken for oversight.

**Re-entry.** Reopen when any gate trips: re-run `census.py` against a fresh copy. The gate is mechanical to check (one
command, stdlib only, no network) but not automatic — nothing runs it, because no language column is persisted. That
limitation is stated in the evidence README rather than papered over.
