# mem-portability

**Purpose:** the portable, versioned memory bundle: a JSON document that
carries the durable memory tiers across machines, with a round-trip
guarantee, an opt-in raw-staging tier and a tolerance for missing or
extra tiers.

## Ubiquitous language

| Term | Meaning |
|---|---|
| **bundle** | the whole portable document — `schemaVersion: 1`, the export stamp, the plugin identity, the source identity, the embedding identity and the tiers. |
| **tier** | one section of the bundle: `memories` (the project-scope facts, always present), `globalMemories`, `profiles`, `milestones` and `staging` (the optional siblings). |
| **source identity** | the `containerTag` / `scopeHash` / `projectPath` / `projectName` / `gitRepoUrl` the bundle was written from. It is **advisory provenance only** — the importer re-resolves the target scope from the target directory. |
| **fact** | one durable memory row. The bundle never carries a vector: the importer re-embeds the content with the target's embedder. |
| **replace mode** | the import mode in which an id that already exists in the target is **replaced**. The default (**merge**) is upstream's rule: a duplicate aborts the whole import with **zero writes**. |
| **dry run** | the import mode that writes nothing and reports the counts it would have applied — computed with the **same** skip predicate as the write path. |
| **staging tier** | the raw agent-session rows. It is **opt-in** (`ExportOptions::include_staging`) and there is no `included` flag inside the payload. |
| **extra** | the unknown top-level fields, preserved through the flattened map and re-emitted — the reason a bundle written by a newer version is never truncated. |
| **prepare phase** | the import phase that computes every embedding, every tags embedding, every provenance metadata object and every milestone encoding **before the first insert**. A failure there is a **zero-write** failure. |
| **compensating delete** | the write phase's rollback: the ids it CREATED are recorded, and a later failure deletes exactly those rows. A row that **replaced** a pre-existing row is never compensated (the replace mode is destructive by definition). |

## Rules the crate owns

- `schemaVersion` is the literal `1`; a higher version is refused with
  upstream's exact "Unsupported export schemaVersion N" message.
- The importer touches **only** the tiers the bundle carries: an omitted
  tier never deletes anything in the target.
- The import aborts on a duplicate id **before any write**. The milestone
  id is the table's primary key, so a milestone collision is **global**
  and aborts the import the same way (before the first write).
- The import runs in four phases — the pre-scan, the **prepare phase**,
  the dry run and the write phase — and only the last one writes.
- `FactStore` has **no transaction seam**, so the import is not atomic at
  the store level: the guarantee is "every fallible computation happens
  before the first write" plus the **compensating delete** of the rows the
  run created.
- A staging session's parent link is a **source row id**; the target mints
  its own row ids, so the import inserts every session first and links the
  parents afterwards (`EpisodicStore::link_parent`). A link whose parent
  row is absent is **recorded** as a skipped entry, never silently
  dropped.
- The exported memory's wire key for the type is upstream's **`type`**
  (never `memoryType`): an upstream bundle's `type` must not be dropped
  on import.
- The exported memory's `<private>` regions are stripped and a fully
  private row is dropped — the privacy pass lives in the production
  `FactStore` adapter (`mem-web`), and a bundle is a file on disk.
- The imported memory's `container_tag`, `project_path`, `project_name` and
  `display_name` are rewritten to the target's; the metadata is stamped
  with `source: "import"`, `importedAt`, `exportSchemaVersion`,
  `originalContainerTag` and `originalProjectPath`.
- The import file is capped at 100 MiB (checked on the file's metadata
  before the read) and is parsed as JSON only — nothing in a bundle is ever
  executed.
- The export writer creates the file `0600` and renames it into place
  atomically; it never follows a symlink at the destination.
- The milestone and staging listings are bounded by the public constants
  `MILESTONE_EXPORT_LIMIT` and `SESSION_EXPORT_LIMIT` (10 000 rows). The
  store's listing has no offset, so a larger project is **truncated** — the
  truncation is **logged** (never silent) and the bound is public.
- The staging **compensation boundary** is exact: `import_staging` tracks
  the triples `upsert_session` actually `Inserted` and resolves their target
  row ids from the post-import listing, so `created.sessions` contains
  exactly the rows this run created — never pre-existing rows. Because
  inserted rows carry `created_at = now()`, they sort first in the listing,
  so they stay within the `SESSION_EXPORT_LIMIT` window as long as a single
  run inserts no more than 10 000 staging sessions (the 100 MiB bundle cap
  makes a larger single run implausible in practice). The `>10 000`
  compensation case is therefore bounded by the bundle cap, not by the
  listing limit.

## Boundaries

- `mem-core` — the tag/scope algebra (`TagResolver`,
  `extract_scope_from_container_tag`, `format_tags_for_embedding`).
- `mem-store` — the SQL locus and the vector text encoding.
- `mem-profile` — the profile tier (`ProfileStore::import_profile`).
- `mem-episodic` — the milestone and staging tiers (`EpisodicStore`).
- `mem-embed` — the `Embedder` seam the import re-embeds through.

The crate owns NO web surface and NO CLI: the durable-fact seam
(`facts::FactStore`) is implemented by `mem-web`'s production adapter, and
the CLI belongs to issue 22.
