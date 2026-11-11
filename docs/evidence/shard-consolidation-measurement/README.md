# Shard-consolidation measurement archive

Prepared for independent review under blueprint `d71b540b11fd`, prerequisite Step 2 only. This directory is not yet integrated by a reviewed docs-only commit. No production implementation, live snapshot, shutdown or cutover is authorized. Step 3 requires source inspection, architecture revision and a fresh gate; Steps 4–13 remain unapproved proposals.

## Retained ruling

READ: `report.md`, `review-6.md`, `handoff-6.md`, `ledger.md` and `ab-draft.md` retain the historical record verbatim. **INCONCLUSIVE** remains the result. A's synthetic raw-copy and owned-child restore prerequisites passed. B's barrier sufficiency remains unproven; no bypass was measured either way. Preference for offline A is **policy only**, based on accepted downtime, not a measured victory. Phase C remains blocked.

Five `not_exercised` paths remain:

- `skills-manager/list-get-create`
- `mem-episodic/inbox-store-direct-sql`
- `mem-bridge/capture-promotion-real-callers`
- `connection/close-drain-stale-cloned-handle`
- `power-loss-durability`

Production auxiliary completeness, ai-sessions schema and power-loss/filesystem atomicity remain unknown. Synthetic fixture tuples are not production coverage. Existing review-6 advisories remain unchanged; no historical assertion was silently corrected.

## Layout and reference closure

`relocation-map.json` maps each original repository-relative path to its byte-identical archived path. It is authoritative for historical links: originals deliberately retain their old absolute/relative links. `reference-index.json` classifies explicit scratch references as included, contextual/build exclusions, directory pointers or unavailable/abbreviated historical references. These are provenance links, not a claim that every shell output path still exists.

The archive includes all measurement handoffs/reviews/debug prescriptions (including blocked/rejected history), the immutable closed ledger, all available measurement blueprints, the report and A/B draft, harness manifest/lock/src/tests, evidence/checkers/source-map, and explicitly referenced named synthetic runs with their complete tuple/sidecar/marker closure. It does not copy the 147 MB run tree wholesale. Unreferenced runs, mutant workspaces, targets, executable build outputs, caches, downloaded tools, unrelated recovery-housekeeping artifacts and private production roots are excluded. No live database was opened. DB/WAL files retained here are synthetic fixture bytes from the isolated measurement harness, copied as files without opening them.

Historical references such as `debug2-m3`, truncated run names and predecessor summaries are absent or abbreviated in the original record. They are classified explicitly, not manufactured. Contextual references to the older post-migration wave do not expand this measurement archive into that wave. In particular, tracked-source names and baseline hash lists refer to the repository at READ revision `df34c522cd349ddb59f021fc82eae64190123e2f`; production sources are not duplicated here. `fetch-before.sha256` and `tracked-before.sha256` are historical manifests, not manifests of the archive.

## Relocated acceptance

From any directory:

```sh
python3 /absolute/path/to/shard-consolidation-measurement/check-relocated-settlement.py
```

The adapter reads `harness/evidence/check-settlement.py` unchanged and substitutes exactly one locator expression in memory: `S = Path(".tmp/scratch/shard-measurement")` becomes the absolute archived `harness` directory. It asserts one exact match. All original checks and subprocess behavior remain intact, including the checker's own `sha256sum -c` with harness cwd. No historical artifact is rewritten. This executes settlement only, not Rust tests or database operations.

With the original scratch inputs present, run `python3 docs/evidence/shard-consolidation-measurement/check-named-runs.py` from the repository root (or invoke the script by absolute path). Its explicitly relocated locator uses the archive's repository ancestor; it enumerates all regular files independently in the seven named source runs, checks mapping/bytes/classifications and extra archived run files, and refuses symlinks. It does not derive completeness from the relocation map or the manifest. Existing historical destinations under `originals/` remain valid; they are checked through their mappings rather than silently moved.

Review-1 rejected the initial archive: a substring build filter removed 35 synthetic `target` files. Rework restored those original bytes without DB opening or recovery; the filter now tests excluded directory components only. `target.db`, `target.db-wal` and `target-schema.json` are measurement artifacts, not compiler target directories. `rework-1/` preserves the rejected handoff/review, diagnostic prescription, prior manifest and actual RED/GREEN closure logs. The old manifest authenticates the old incomplete selection only; it is not the current manifest. Source-enumerated closure now gates validation before manifest generation. No assertion of completeness is based on hashes alone.

From this directory, `sha256sum -c archive.sha256` verifies every archive payload after the final content pass. The manifest excludes itself to avoid a circular hash; its own digest is recorded in the handoff. `relocation-map.json` supports independent byte equality against available originals; reference closure and baseline checks are recorded in the handoff.

## Harness reproduction (not executed in this archive-preparation wave)

The original harness is intentionally not rewritten to look relocatable: `harness/Cargo.toml` has relative dependencies `../../../crates/mem-store` and `../../../crates/mem-core`, and `harness/src/lib.rs` pins its original absolute synthetic `RUNS` directory. Tests create exclusive fresh synthetic roots. Do not run the archived manifest directly: its relative dependencies no longer point to the repository, and its RUNS locator would target historical scratch.

For a future separately admitted reproduction, copy only Cargo.toml/Cargo.lock/src/tests to a new isolated scratch directory. In that disposable copy, change the two dependency paths to absolute paths under the pinned repository revision and change only the `RUNS` locator to a new nonexistent, project-local synthetic run parent. Preserve the archived originals, record the exact adapter diff and resulting hashes, and use an isolated target directory. Then run offline and locked:

```sh
cargo test --offline --locked --manifest-path /absolute/disposable/harness/Cargo.toml -- --test-threads=1
cargo clippy --offline --locked --manifest-path /absolute/disposable/harness/Cargo.toml --all-targets -- -D warnings
rustfmt --edition 2024 --check /absolute/disposable/harness/src/lib.rs /absolute/disposable/harness/src/main.rs /absolute/disposable/harness/tests/measurement.rs
```

Run cargo through background shells, set the isolated target only for the disposable command, and do not fetch missing dependencies. The locked dependency cache and pinned repository source are prerequisites, not bundled dependencies. These commands are instructions, not claims of execution in this wave. Source-locator adapters necessarily change source hashes; retain historical settlement separately rather than altering archived results to match a new run.
