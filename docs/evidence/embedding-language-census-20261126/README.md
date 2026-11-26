# Evidence: embedding-language census, 2026-11-26

Issue 167 asked to swap the live embedding model from `Xenova/nomic-embed-text-v1` to
`Xenova/multilingual-e5-base`, on the strength of a Thai nDCG@10 figure (75.27 for e5, 0.19 for
nomic) measured 2026-10-31. This directory is the measurement that dissolved the request.
**Issue 167 closes as measurement, not a deliverable.**

Three findings, in order of weight:

1. **There is no non-Latin script content in the store.** Zero rows across 5,601 memories and 417
   prompts carry Thai, Han, Kana, Hangul, Cyrillic, Arabic, Hebrew or Devanagari text at any density
   the gate recognises; zero even as a single character. The corpus the swap would improve does not
   exist here.
2. **The harness behind the nDCG figure was never tracked.** It lived at
   `.tmp/scratch/search-embedding-models-20261031.md`, which is gone; no NDCG code exists in
   `crates/`, `tests/`, `scripts/` or `docs/`. The numbers cannot be reproduced or re-derived.
3. **The swap's known cost points the other way.** e5 caps at 512 tokens (head+tail truncation);
   nomic declares `n_positions: 8192`. 206 of 5,601 rows exceed 1,800 characters (longest 6,603;
   three exceed 4,096), so the swap would truncate rows the current model embeds whole. That cost
   stands independently of the language finding.

## Reproduce

```sh
python3 docs/evidence/embedding-language-census-20261126/census.py .tmp/scratch/step11/store4
```

`census.py` is stdlib-only and opens every database `mode=ro`. The input must be a *copy* of a data
directory (`projects/*.db` + `user-prompts.db`); the script refuses a path under `~/.opencode-mem/`.
`run.txt` is the verbatim stdout of one run against the §2a copy
`.tmp/scratch/step11/store4` (36 shard dbs, 5,601 memories rows, 417 prompts). Two consecutive runs
produced byte-identical output.

## Unit of measurement: character, not byte

A row is "fully ASCII" when no character has a codepoint ≥ 0x80 — the unit the model tokenizes,
not the byte. SQLite's `length()` counts bytes for a BLOB and characters for TEXT, which yields a
one-query cross-check: `length(cast(content as blob)) > length(content)` selects rows containing
multi-byte UTF-8. It returns 2,238 for the memory corpus and 20 for prompts — exactly the
"has non-ASCII" counts below.

## Memory corpus — `memories.content`, 36 shards

| Measure | Rows | Share |
| :--- | ---: | ---: |
| Total | 5,601 | 100.00 % |
| Fully ASCII (no codepoint ≥ 0x80) | 3,363 | 60.04 % |
| At least one non-ASCII character | 2,238 | 39.96 % |

Per-category counts (categories overlap; one row can hit several):

| Category | Rows | Row share | Characters |
| :--- | ---: | ---: | ---: |
| latin_accented (U+00C0–U+024F) | 214 | 3.82 % | 1,115 |
| thai (U+0E00–U+0E7F) | 0 | 0.00 % | 0 |
| han (U+3400–U+4DBF, U+4E00–U+9FFF, U+F900–U+FAFF) | 0 | 0.00 % | 0 |
| kana (U+3040–U+30FF) | 0 | 0.00 % | 0 |
| hangul (U+AC00–U+D7AF) | 0 | 0.00 % | 0 |
| cyrillic (U+0400–U+04FF) | 0 | 0.00 % | 0 |
| arabic (U+0600–U+06FF) | 0 | 0.00 % | 0 |
| hebrew (U+0590–U+05FF) | 0 | 0.00 % | 0 |
| devanagari (U+0900–U+097F) | 0 | 0.00 % | 0 |
| greek (U+0370–U+03FF) | 37 | 0.66 % | 64 |
| punctuation (U+2000–U+206F, U+00A0–U+00BF) | 1,810 | 32.32 % | 4,417 |
| symbols_math (U+2190–U+23FF, U+25A0–U+27BF, U+2900–U+297F) | 974 | 17.39 % | 2,454 |
| emoji (U+1F000–U+1FAFF) | 5 | 0.09 % | 9 |

The 39.96 % non-ASCII share is typography and Western European orthography, not language:
`latin_accented` (214 rows) is accented Latin; `punctuation` (1,810 rows) is em dashes, arrows,
ellipses, section signs; `symbols_math` (974 rows) is the same plus notation. The 37 Greek-letter
rows hold 64 characters total — that is Δ, Σ, λ, µ used as mathematical notation, not Greek prose.

Per-project, density-filtered non-Latin-script share is 0.00 % for all 36 shards; the full table is
in `run.txt`. The highest share is therefore 0.00 %, and it comes from a shard with rows, not from
an empty one (the largest shard holds 3,068 rows; the 36 shard row counts sum to 5,601).

## Prompt corpus — `user-prompts.db`, `user_prompts.content`

| Measure | Rows | Share |
| :--- | ---: | ---: |
| Total | 417 | 100.00 % |
| Fully ASCII | 397 | 95.20 % |
| At least one non-ASCII character | 20 | 4.80 % |
| Non-Latin script, raw "contains any" | 0 | 0.00 % |
| Non-Latin script, density ≥ 5 % | 0 | 0.00 % |

Per-category: `punctuation` 19 rows / 117 chars, `symbols_math` 13 / 67, `latin_accented` 3 / 9,
`greek` 1 / 1. No Thai, Han, Kana, Hangul, Cyrillic, Arabic, Hebrew, Devanagari, or emoji.

`created_at` range: **2026-10-26T07:09:11.726Z .. 2026-10-31T07:27:35.368Z** — five days and twenty
minutes, six calendar days. The prompt store is **pruned**, so these 417 rows describe recent usage,
not history; a low non-Latin share here is evidence about the last six days only.

## The gate, and why it is script-aware and density-filtered

Typographic characters (`—`, `→`, `…`, `§`, `×`) and Western European accented letters (`é`, `ü`)
are not language. A gate of "contains any non-ASCII character" would sit at 39.96 % on this corpus
and mean nothing. A gate of "contains any Greek" would sit at 37 rows and drift by several rows on a
handful of extra `Δ`s. So a row counts as non-Latin script only when **one** of
thai/han/kana/hangul/cyrillic/arabic/hebrew/devanagari supplies **at least 5 % of its non-whitespace
characters**. `latin_accented`, `punctuation`, `symbols_math`, `emoji` and `greek` are excluded from
the gate for the reasons above; raw "contains any" counts are still printed beside the filtered ones.

```
GATE memory_share: 0.00 % — clear
GATE project_majority: 0.00 % — clear
GATE prompt_share: 0.00 % — clear
```

Thresholds: `memory_share` ≥ 1 % trips, `project_majority` ≥ 50 % trips (highest per-project share),
`prompt_share` ≥ 1 % trips. The gate exits 0 either way; a tripped gate is a result, not an error.

**Limitation:** the gate is mechanical to *check* — one command, stdlib only, no network — but not
*automatic*: nothing runs it. No language column is persisted (`user_profiles` has no such column in
this store; this copy holds zero profile rows), and wiring one would be a feature, not a gate. If a
persisted language column is independently confirmed to exist, correct this sentence.

## Method note: the second instrument caught the first

The first census produced for this issue watched only non-Latin ranges and therefore classified a
row full of em dashes as "pure ASCII". It reported **99.95 % ASCII**. The byte-vs-character check
`length(cast(content as blob)) > length(content)` returned **2,238 rows**, exposing that the
remainder was not ASCII at all, and the instrument was rewritten around the character and the
script categories above. The reusable lesson: a census that looks for one thing cannot see what it
is missing; cross-check the unit before trusting the ratio.

## Truncation arithmetic (independent of the language finding)

| Measure | Value |
| :--- | ---: |
| Rows > 1,800 characters | 206 (3.68 %) |
| Rows > 4,096 characters | 3 |
| Longest content | 6,603 characters |

nomic's declared `n_positions: 8192` covers the longest row whole. e5's 512-token limit applies
head+tail truncation well below that, so the swap would lose text the current model embeds. This
cost does not depend on any language argument and would survive a corpus that did contain
non-Latin script.

## Deliberately not built

- The nomic-vs-e5 retrieval canary harness — the nDCG script was never tracked, so there is nothing
  to re-run and no honest way to compare models this session.
- The model swap — no corpus-axis benefit in this store, a real truncation cost.
- The 280 MB e5 download — nothing would consume it.
- Any `scripts/` change — the census lives in this evidence directory; no operational hook was
  requested or is justified.
- Any engine restaging — no behaviour change, so no artifact to stage.
