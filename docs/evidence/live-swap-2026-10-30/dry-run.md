# In-place migration dry run — 2026-10-30

Transcribed from `.tmp/progress/live-store-dryrun-20261030T154208Z.md`
(sha256 `a1b021dee578fed058a0199b3ec6f59c75a77692df3cd7d8a69d23501a1ada02`), which lived only under
`.tmp/`. Engine `72d7f64d…4d5e653` (built at `e5154bc`). The live store was copied read-only; only the
copies were checkpointed, opened or migrated.

Copies: `data` (15:21:38Z), `data-write` (from `data`), `data-final` (fresh, 15:42:08Z).

## 1. Pre-state of the copies

`data` (checkpointed copy):

| DB | `user_version` | rows / detail |
|---|---:|---|
| `metadata.db` | 1 | `shards` = **22**, `sum(vector_count)` = **3,089**, active 22 |
| `projects/*.db` (22) | **2** | **3,089** memories; every vector **768** dims; `embedding_model = Xenova/nomic-embed-text-v1` |
| `user-prompts.db` | 1 | `user_prompts` = **1,732** |
| `user-profiles.db` | 1 | `user_profiles` = **2**, `user_profile_changelogs` = **10** |
| `skills.db` | 1 | `skills` = 0 |
| `episodic.db` | 6 | 5 tables, 0 rows each |
| `ai-sessions.db` | 1 | 0 / 0 |

`data-final` (15:42Z): same shape, **3,091** memories, **1,733** prompts (`pre-final.txt`; the report's
§4 table prints 1,732 because it reuses the 15:21Z figure — see the README's deviation 4).

W8 baseline drift: W8 recorded 22 shards / 3,030 rows; the live store was still being written
(3,089 at 15:21Z, 3,091 at 15:42Z). Shard count and dims unchanged.

## 2. Migrations the engine performed (copy only)

| DB | before | after | when | cost / side effect |
|---|---:|---:|---|---|
| `metadata.db` | 1 | **2** | first store touch (`list-shards`) | repair + partial unique index; **no backup** (`db_path: None`); 24,576 → 28,672 B; 22 rows / 3,089 vectors unchanged |
| `user-prompts.db` | 1 | **2** | web `GET /api/memories?includePrompts=true` | probe-then-ALTER; the live file already had all 12 columns, so only the `user_version` stamp changed; 1,732 rows and every value identical; no `.bak` |
| `project_4182…_shard_0.db` | 2 | **3** | first `add` on `data-write` | `CREATE TABLE shard_stats` (+ row `1|920`); no `.bak` |
| other 21 shards | 2 | 2 | not touched | migrate lazily on their first write |
| `user-profiles.db` | 1 | 1 | read only | max known = 1 |
| `episodic.db` | 6 | 6 | startup | max known = 6 |
| `skills.db` | 1 | 1 | read only | — |
| `ai-sessions.db` | 1 | 1 | never opened by the port | — |

No refusal or error occurred. There are no backup files: every migration runs with `db_path: None` or a
no-backup v2 step. Re-opening an already-migrated copy is idempotent (`metadata.db` sha256 stayed
`0accf6ea…` across two later opens).

Integrity fingerprints after migration:

- prompts v1 vs v2 — identical over all pre-existing columns:
  `1732|48496|51960|51960|64885|610248|3104795242790930|1475|1475`.
- 21 untouched shards — `shard=` aggregate lines identical (rows, content bytes, tags bytes, vector
  counts, max `created_at`); only the canary shard differs, by +1 row, +32 content bytes, +11 tag bytes.
- `metadata.db` — 22 shards, 3,089 vectors, 22 active before and after.
- Final copy: **29/29 DBs `integrity_check = ok`**.

## 3. Post-migration parity (`data-final`, after the read set)

| signal | before | after |
|---|---:|---:|
| shards | 22 | 22 (`current 1, linked 20, orphaned 1, missing 0`; the orphan is `usbgurad-tray`, whose project path no longer exists — pre-existing data) |
| memories | 3,091 | **3,091** (`stats.total` and `sum(vector_count)`) |
| `list` (project, limit 20) | — | 20, newest first |
| `list` (all-projects, limit 1000) | — | 1,000 (the cap) |
| prompts | 1,733 | **1,733** |
| profiles / changelogs | 2 / 10 | **2 / 10** |
| skills / episodic / ai-sessions | 0 / 0 / 0 | 0 / 0 / 0 |
| vectors | 768 | **768** in every row (min = max) |

Search (vectors read in place; only the query was embedded):

| query | top hit | similarity | content |
|---|---|---:|---|
| `W8 brute-force search cost benchmark shard layout` | `mem_1792900662953_j6e64unbf` | 0.5369 | "Measured (issue 43.10): a brute-force vector scan at 768 dims costs ~4µs/row…" |
| `owner rulings grilling session issue 43` | `mem_1793215863170_hk2azm1cr` | 0.6312 | "OWNER RULINGS from a grilling session 2026-10-28/29…" |
| canary content (write copy) | `mem_1793374381820_0a1ab9fe2` | 0.8991 | rank 1, exactly the added row |

Profile: JSON-RPC `profile` returned version **157**, `last_analyzed_at = 1793369145518`
(2026-10-30T14:05:45Z), 2 preferences / 2 patterns / 1 workflow, 32,508 B on the wire. Web
`GET /api/user-profile?userId=vaz@siamstr.com` → `exists: true`, displayName `vazw`, version 157,
`totalPromptsAnalyzed: 1410`.

Timing (copy, warm page cache):

| call | first run | later |
|---|---|---|
| `hello` / `health` | 29–140 ms / 1–4 ms | — |
| `list-shards` (with the metadata migration) | 144–285 ms | 208–285 ms |
| `list` (project / all-projects) | 26–113 ms / 120–328 ms | — |
| `search` (first ONNX load) | 3.6–5.9 s | 63–203 ms |
| `profile` / `skills` | 11–156 ms / 11–81 ms | — |
| `add` (embed + insert) | 97 ms | — |
| web `memories?includePrompts=true` (prompts migration) | 160–237 ms | — |
| whole read set, wall clock | **5.3 s** (first open) / **7.6 s** (with web) | — |

Engine stderr on every run, 2 lines (migrations are not logged — no tracing subscriber on that path):

```json
{"busy":0,"component":"consolidation","event":"consolidation.startup_tick","failed":0,"level":"info","projects_scanned":0,"runs":0}
{"component":"engine","elapsed_ms":0,"event":"engine.shutdown_complete","level":"info"}
```

## 4. Embedding compatibility

Store vectors are 768-dim f32 (`length(vector)/4 = 768` in all 3,091 rows); `shard_metadata.embedding_model
= Xenova/nomic-embed-text-v1`; the live config sets the same model, no task prefixes, no remote URL —
byte-identical intent to the port defaults. A new `add` embedded with the same model produced a
3,072-byte f32 vector (768 dims, plus a 768-dim tags vector) and the query that found it was embedded the
same way. **No re-embed is needed for the stored data.** The provisioning prerequisite is in
`provisioning.md`.

If a future swap changed `embeddingModel`/dimensions, the existing vectors would be compared against a
different space and the write-once `shard_metadata` identity would flag it rather than fix it; that needs
`--mode=reembed-scope`.

## 5. One-way cost (measured on throwaway copies)

| stamp | read path | write path |
|---|---|---|
| `metadata.db` `user_version = 3` (max known 2) | `list-shards`/`list`/`add` all `-32603`; `hello`/`health` still answer | refused |
| one shard `user_version = 4` (max known 3), metadata back at 2 | `list-shards` and `list` **work** (920 rows) | `add` → `-32603` |

The vendored upstream source does **not** refuse a higher `user_version`
(`example/opencode-mem/src/services/turso/schema-migrations.ts:38-39` filters `version > current` and
returns `current`) — READ, not executed. Return-trip risks remain unmeasured: upstream never updates
`shard_stats`, and metadata v2's partial unique index could reject an upstream registration. The backup,
not the older plugin, is the rollback.

Nothing readable was lost: `ai-sessions.db` (0 rows) is never opened by the port; `episodic.db` (0 rows,
v6) is unchanged; `skills.db` is empty; `recovery/` and the `*.turso-migrate.json` markers are inert;
`.cache` (546 MB) is not reused by the tool embedder.

Known non-blocker: the web `user_id` is hardcoded to `"unknown"` (`src/main.rs:207`), so
`GET /api/user-profile` **without** `?userId=` shows no profile although the JSON-RPC `profile` path
resolves the git email. Recorded as a follow-up, not a swap blocker.

## 6. Limits of the dry run

- Upstream was not run against the migrated store (Bun/upstream tooling is off-limits here).
- LLM-dependent paths (`capture`, `inject`, `recall`, consolidation) were not exercised: empty episodic
  store, no provider key. Only read paths plus one `add`/`search` pair.
- The web UI frontend was not exercised, only the HTTP API it calls.
- Timings are one machine with the page cache warm; the first search is dominated by loading the 547 MB
  ONNX model.
