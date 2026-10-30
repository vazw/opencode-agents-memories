# Export → import dry run — 2026-10-30

Transcribed from `.tmp/progress/upstream-export-to-port-import-20261030T163933Z.md`
(sha256 `b1d8de24d8fdcaf21e5f742028bcab88fb1cad243c9459346d9a5aab3c290dcf`), which lived only under
`.tmp/`. Port tip `e5154bc`; the same engine `72d7f64d…4d5e653`. Run on a scratch store; the live store
was never a root.

- Export: `~/.opencode-mem/exports/opencode-agents-memory-20261030T151445Z.json`, 1,547,433 B, sha256
  `f3d1b6db26a6e570f6ccf0168ed47c0c472a509499e35c012acccd3038d24370`
- `schemaVersion: 1`, `plugin: {package: "opencode-mem", version: "2.30.0"}`, source
  `opencode_project_4182e0858e0c93a1` (`scope: project`), `embedding: {model:
  "Xenova/nomic-embed-text-v1", dimensions: 768}`, **917 memories, no per-row vectors**
- Top-level keys: `schemaVersion, exportedAt, plugin, source, embedding, memories` — no
  `globalMemories`, `profiles`, `milestones`, `staging`

## 1. Import results

| call | result | elapsed |
|---|---|---:|
| `import` with `dry_run: true` | `{"imported":917,"rejected":[],"skipped":[]}` | **667,615 ms** |
| `import` (`dry_run` false) | `{"imported":917,"rejected":[],"skipped":[]}` | **627,800 ms** |

Re-embedding volume: 917 content + 917 tags embeddings = **1,834 embeddings per pass** (≈342 ms each on
this host). Run A (same inputs, concurrent cargo builds) measured dry 429,394 ms / real 521,454 ms. Two
runs bracket the cost at **7.2–11.1 min dry / 8.7–10.5 min real per 917 memories**.

## 2. Fidelity — all 917 rows, not a sample

`diff_rows.py` compared every export row with the corresponding `list` item on `content, type, tags,
createdAt, updatedAt, isPinned, displayName, projectPath, projectName`, plus the metadata rewrite:

```
export rows: 917   listed: 917
missing from export: []      missing from list: []
field mismatches: NONE       metadata mismatches: 0
type histogram equal: True   list sorted desc by created_at: True
container tags: ['opencode_project_4182e0858e0c93a1']
```

The only semantic rewrite is metadata provenance: `source` becomes `"import"` (all 420 capture rows had
`"auto-capture"`), and `importedAt`, `exportSchemaVersion`, `originalContainerTag`, `originalProjectPath`
are added. Every other original metadata key is preserved. Row-level `session_id` is populated for the
420 capture rows (upstream kept it only inside `metadata.sessionID`) — a gain.

## 3. Store produced (scratch root, WAL not checkpointed)

| file | size (B) | notes |
|---|---:|---|
| `projects/project_4182e0858e0c93a1_shard_0.db` | 7,655,424 | 917 rows, one active shard |
| `metadata.db` | 4,096 | registry, `user_version = 2` |
| `episodic.db` | 4,096 | `user_version = 6`, 0 rows |
| `user-profiles.db` | 4,096 | `user_version = 1`, 0 profile rows |
| `user-prompts.db` | — | **absent** (the bundle carries no prompts) |

Shard: 917 memories / 917 distinct ids / 1 container tag; `vector NOT NULL` 917, `tags_vector NOT NULL`
917, `length(vector)` 3,072 B = 768 × f32; `shard_stats` `1|917`; registry `vector_count = 917`;
`shard_metadata` `embedding_model=Xenova/nomic-embed-text-v1`,
`embedding_dimensions=768`, truthful fingerprint (the shard was created empty, so v3's "claim only on an
empty shard" rule stamps the configured identity).

Search rank-1 after import: `mem_1793371620572_g2bkoyelv` (0.8239) for the issue-79 query,
`mem_1793373146840_sz6ax4ey1` (0.7135) for the shard-migration query.

## 4. Second import — idempotency contract

```json
{"code":-32602,"message":"Import aborted: 917 memory id(s) already exist",
 "data":{"reason":"import_aborted","kind":"memory","count":917,"rejected":[ … 917 … ]}}
```

Elapsed **521 ms** — the duplicate-id pre-scan is phase A and returns before any embedding or write. The
store is unchanged (917 rows, 917 vectors, registry `vector_count 917`). The JSON-RPC `import` is
merge-only; the overwrite mode exists only on the REST surface. So a re-import is safe but not a success.

## 5. What the export does not carry

1. **Profiles** — no `profiles` tier; profile reads `null`, 0 rows on disk.
2. **The other 21 projects** — the export is `scope: project`; each other project must be exported and
   imported separately (and re-embedded).
3. **Prompts** (`user-prompts.db`) — not part of an upstream export.
4. **Milestones / staging / sessions** — port-additive; upstream writes neither.
5. **Global/user-tier memories** — `all-projects` still lists 917 (user scope 0).
6. **Vectors** — deliberately absent; the port re-embeds (the 10–11 min above).
7. **Metadata fidelity is not 100 %** — the five provenance keys are added and an existing
   `metadata.source` is overwritten, by design.

## 6. Verdict

The upstream export imports cleanly into a fresh port store with all 917 memories: `917 imported / 0
rejected / 0 skipped` in both dry and real runs, 0 field mismatches across all 917 rows, shard v3 /
metadata v2 with the same container tag, a truthful embedding fingerprint, 917 + 917 vectors, correct
rank-1 search, and a whole-import abort (521 ms, zero writes) on re-import.

It is **not a full-store migration**: one project's memory tier only, no profiles/prompts/milestones/
global memories/other projects, and every row is re-embedded. Use it as the portability/backup route and
the rebuild path for this project if the in-place migration ever fails — not as the swap.
