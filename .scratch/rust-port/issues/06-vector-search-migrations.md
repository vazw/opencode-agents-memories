# 06 — W1.3 · Vector search (exact fallback) & migrations (US 2.2)

Status: ready-for-agent

## What to build

`mem-store::vector` — vector search with exact/keyword fallback, vector utils; schema/engine/dimension re-embedding migrations. Vectors bind as JSON text into `vector32(?)` and read back via `vector_extract()` — a straight SQL transliteration of upstream (`vectorToJson`/`parseExtractedVector`), exact-scan `ORDER BY vector_distance_cos(...) LIMIT k`, no ANN (roadmap §0b D2). Wave detail: `.scratch/rust-port/roadmap.md` §W1.3.

## Acceptance criteria

- [ ] Behaviors ported: `turso-vector-search`, `turso-vector-utils`, `turso-exact-fallback`, `turso-reembed-migration`, `turso-migrate-dims-preflight`, `turso-engine-migrator`, `schema-migrations`, `turso-legacy-migrator`.
- [ ] Exact/keyword fallback triggers when vector search fails or returns insufficient hits.
- [ ] Golden-transliteration test: verbatim upstream statements run unchanged on the `turso` crate; any mismatch is a blocking defect, not an adaptation.
- [ ] Dimension preflight blocks re-embedding when preconditions fail.

## Blocked by

- 05 · W1.2
