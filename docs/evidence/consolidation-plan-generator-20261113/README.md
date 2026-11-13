# Consolidation plan generator — preserved from scratch

`build_plan.py` derives the private `consolidate-shards --plan <abs-json>` document
from a storage root. It lived only in `.tmp/scratch/` — one scratch cleanup from
gone — while being the single artifact able to produce a production plan. This
directory is its durable copy.

`sha256 924b1dcf8fc4661840b8b247879887e4c933ffdb0eca6088686be9581322e343`

## Why it exists

`src/main.rs` consumes the plan as an INPUT (`--plan <abs-json>`, modes at
`:602`–`:796`); no shipped mode derives one. `consolidate-shards` cannot run
without it, so the cutover window has to produce it, and the only tool that does
is this script.

## Contract it relies on (MEASURED 2026-11-13, `src/shard_consolidation.rs`)

The three hash fields in the plan are **bound into the plan digest and never
verified against the tree**:

- `source_snapshot_sha256`, `schema_manifest_sha256`, `baseline_manifest_sha256`
  are read from the JSON at `:289`/`:291`/`:292`, required non-empty at
  `:366`–`:368`, and concatenated into `render_plan_digest` at `:535`–`:537`.
- `schema_manifest_sha256` has **no** recomputation and **no** comparison site.
- The plan's content binding is `source_hash` per row, matched against the shard
  files found on disk (`:442`–`:481`), plus the custody `baseline.sha256`
  written at `:3432` and compared at `:3448` and `:4404`.

Consequence: the hashes above decide the `--confirm <plan_sha256>` string, not
safety. Safety comes from the per-row identity census and from `baseline.sha256`
in custody. That is why the generator may hash whichever baseline file the
snapshot step actually wrote.

## Inputs and outputs

```
python3 build_plan.py <storage-root> <out-plan-path>
```

Reads, read-only, from `<storage-root>`:

- `projects/project_<hash>_shard_0.db` — the shards
- `metadata.db` — the registry

and, for the baseline hash, the first `manifest.sha256` then `baseline.sha256`
found in `<parent-of-storage-root>`, then `<parent>/custody`, then the
grandparent. The layout matters: `consolidate-snapshot --custody <dir>` writes
`<dir>/data` (the snapshot of the storage root) and `<dir>/custody/baseline.sha256`
(`BASELINE = "baseline.sha256"`, `:3034`), so the baseline sits one level BELOW
the storage root's parent — a first revision of this fix looked only in
`<parent>` and failed on a real snapshot. Older rehearsal tooling instead put a
`sha256  ./path` `manifest.sha256` beside `data/`, which the wider search still
accepts. `baseline_source` in the summary says which file was used.

Writes `<out-plan-path>` with `O_EXCL` at mode 0600, and prints a JSON summary
(`shards_on_disk`, `sources`, `unresolved`, `not_sources`, `registry_ok`,
`schema_manifest_sha256`, `baseline_manifest_sha256`, `baseline_source`,
`plan_mode`).

**Pass absolute paths.** A relative `<storage-root>` lands `source_path` in the
plan as a relative string and the binary rejects the plan with
`source_path_mismatch` (exit 72).

## Derivation, and what it deliberately does not read

Identity candidate comes from the shard's own association columns
(`container_tag`, `project_path`, `project_name`, `git_repo_url`), confirmed by
`sha256(identity)[:16]` equalling the registry/scope hash. Target identity is the
plain LEADING-prefix swap `/home/vaz` → `/home/opencode`, with no exceptions
(`SPECIAL = {}` — the rev-12/13 user ruling of 2026-11-12 removed the last one).

**No implementation output is read.** The script opens only the storage root it
is given, so it stays an independent oracle of the mapping rather than a
transcription of what the consolidator produced.

## Verified end-to-end

The first revision of the `baseline.sha256` fix FAILED on a real snapshot: it
searched `<parent>/baseline.sha256` while `consolidate-snapshot` writes
`<custody>/custody/baseline.sha256`, so it exited 1 with
`no manifest.sha256 or baseline.sha256 beside …/cust/data`. The search now
covers the real layout. That failure is recorded here because it is the shape of
mistake this file exists to prevent: a claim about where an artifact lands, made
without running the step that produces it.

MEASURED against a freshly `consolidate-snapshot`-ed copy, with the baseline
copied beside `data/` (the pre-fix workaround): 34 mappings,
`source_snapshot_sha256` equal to `sha256(custody/baseline.sha256)`, and
`consolidate-shards --dry-run` accepting it (exit 0, `source_count` 34,
`group_count` 34). The corrected search path was re-verified separately; the
committed script's own end-to-end run on an untouched fresh snapshot is the
check to re-derive before the cutover window, and it is cheap.

Caveat carried from that run: a `sha256  ./path` manifest regenerated from a
fresh copy differs from the frozen one by 26 lines, all `-shm` files. `-shm` is
SQLite shared memory and is not stable across a copy, so a plan bound to such a
manifest would differ per copy. Hashing `baseline.sha256` — what the snapshot
step writes, once, in-window — avoids that instability entirely.

## Operating rule for the cutover window

Derive the plan **in the same window as the snapshot**, from the snapshot's own
tree, and record the plan's sha256. Do not reuse a plan derived from an earlier
snapshot: the per-row census is plan→disk, so a shard on disk that the plan does
not list is silently left unconsolidated (no refusal fires). Deriving from the
fresh snapshot is complete by construction — the script enumerates every
`*_shard_*.db` on disk.
