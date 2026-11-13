# Consolidation plan generator — preserved from scratch

`build_plan.py` derives the private `consolidate-shards --plan <abs-json>` document
from a storage root. It lived only in `.tmp/scratch/` — one scratch cleanup from
gone — while being the single artifact able to produce a production plan. This
directory is its durable copy.

`sha256 b564a83a1b818d0502a864000b64869f571bb05a23235e6d5c9fd2ae674905d8`

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

and, for the baseline hash, the first of `<parent-of-storage-root>/manifest.sha256`
then `baseline.sha256`. `consolidate-snapshot` writes the second
(`BASELINE = "baseline.sha256"`, `:3034`); the first was this script's original
assumption and nothing in the shipped code produces it. Either is accepted.

Writes `<out-plan-path>` with `O_EXCL` at mode 0600, and prints a JSON summary
(`shards_on_disk`, `sources`, `unresolved`, `not_sources`, `registry_ok`,
`schema_manifest_sha256`, `baseline_manifest_sha256`, `plan_mode`).

## Derivation, and what it deliberately does not read

Identity candidate comes from the shard's own association columns
(`container_tag`, `project_path`, `project_name`, `git_repo_url`), confirmed by
`sha256(identity)[:16]` equalling the registry/scope hash. Target identity is the
plain LEADING-prefix swap `/home/vaz` → `/home/opencode`, with no exceptions
(`SPECIAL = {}` — the rev-12/13 user ruling of 2026-11-12 removed the last one).

**No implementation output is read.** The script opens only the storage root it
is given, so it stays an independent oracle of the mapping rather than a
transcription of what the consolidator produced.

## Verified end-to-end (2026-11-13, syseng, MEASURED)

Against a freshly `consolidate-snapshot`-ed copy: 34 mappings, `source_snapshot_sha256`
equal to the hash of the baseline file beside `data/`, and
`consolidate-shards --dry-run` accepting it (exit 0, `source_count` 34).

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
