# Archive reference-closure diagnosis

Blueprint: `.tmp/blueprints/blueprint-20261111-1304-lossless-shard-consolidation.md`, sha256 header `d71b540b11fd`. Authority: prerequisite Steps 1–3 only; this diagnosis concerns rejected Step 2 preservation. Ledger Step 1 DONE was not replayed. Step 2 remains OPEN; no implementation, archive repair, commit, DB open/recovery, live snapshot, shutdown or production change performed.

## [REPRO]

MEASURED: from repository root, Python 3 on Linux:

```sh
python3 .tmp/scratch/debug-archive-closure/check-named-runs.py
```

Runnable read-only regression check retained at that path; captured final output: `.tmp/scratch/debug-archive-closure/red.log`. It enumerates original run directories directly, not relocation-map/reference-index/manifest as its expected file-set oracle. It checks every source file has a mapping, mapped destination exists within archive, bytes agree, explicit file classifications do not exclude it, and no extra source mappings/archive run files exist. No DB library is imported; files are read as bytes only.

Expected current failure: exit 1, `AssertionError: named-run closure failed: 70 errors`. This is 35 independently enumerated missing mappings plus 35 incorrect explicit exclusion classifications, not 70 missing files. Before adding the classification assertion, the same complete file-set check failed twice with exactly 35 unmapped files. Final augmented check also ran RED.

MEASURED source enumeration:

| Named run | Original files | Mapped files | Missing files |
|---|---:|---:|---:|
| divergent-12955 | 18 | 13 | 5 |
| divergent-4807 | 18 | 13 | 5 |
| divergent-5210 | 18 | 13 | 5 |
| duplicates-12955 | 18 | 13 | 5 |
| duplicates-4564 | 18 | 13 | 5 |
| raw-copy-12955 | 18 | 13 | 5 |
| raw-copy-2952 | 18 | 13 | 5 |
| **Total** | **126** | **91** | **35** |

Each independently walked run has this actual file set (not a filename whitelist used by the checker):

```text
.wal-mode
raw-copy.json
source1-schema.json
source1.db
source1.db-wal
source2-schema.json
source2.db
source2.db-wal
target-schema.json
target.db
target.db-wal
work/.wal-mode
work/source1.db
work/source1.db-wal
work/source2.db
work/source2.db-wal
work/target.db
work/target.db-wal
```

MEASURED missing set in EVERY run: `target-schema.json`, `target.db`, `target.db-wal`, `work/target.db`, `work/target.db-wal`. Their aggregate original logical size is **2,327,584 bytes**. The other 91 mapped named-run files compare byte-identically. Counts were derived anew from originals, not inherited from review's 35.

## [MINIMAL CASE]

MEASURED predicate-only reduction, using the actual failing input:

```python
from pathlib import Path
src = '.tmp/scratch/shard-measurement/runs/divergent-12955/target-schema.json'
assert any('target' in part or part == 'debug' for part in Path(src).parts)
assert not any(part in ('target', 'debug') for part in Path(src).parts)
```

One existing original schema file suffices to trigger the erroneous build classification. This eliminates Turso, DB recovery, WAL semantics, Rust compilation, settlement, audit tooling and recursive reference discovery from the immediate cause. Removing the substring match removes this false positive. This minimal predicate probe supplements, not replaces, the complete original-input regression check.

## [FINDINGS]

Ranked falsifiable hypotheses were recorded before probing the filter:

1. **Substring build filter deletes persisted artifacts. CONFIRMED (READ/MEASURED).** Prediction: every omitted file matches the trim predicate, while ordinary source tuples survive; exact-component matching rejects none of these omitted files. `.tmp/trim-consolidation-archive.py:9` uses `any('target' in part or part == 'debug' for part in Path(src).parts)`. Lines 10–14 unlink destination, delete mapping, and overwrite reference classification. Direct read-only predicate evaluation established this for all 35 missing files; every one has that script's exact exclusion string. No contrary observation.
2. **Named-run discovery omitted these runs. FALSIFIED as explanation (READ/MEASURED).** Prediction: undiscovered runs would not retain complete-run classifications and 13 sibling mappings each. All seven retain those entries. `.tmp/prepare-consolidation-archive.py:59–69` recursively copies entire referenced named runs, without the substring filter. Preparation's separate filter at lines 50–51 tests exact path components, not `target` substrings. The destructive narrowing was introduced in trim, not initial run enumeration.
3. **Missing originals/copy I/O failure. FALSIFIED as explanation (MEASURED/READ).** Prediction: absent/unreadable files or failed copies rather than systematic build exclusions. Every omitted source is an existing readable regular file; all are exactly the names selected by trim and explicitly reclassified by it. Preparation `copy` uses `shutil.copyfile` and does not swallow copy exceptions. Historical execution trace is not reconstructed; present artifacts and code establish the classification cause without replaying mutation.

READ validation gap: `.tmp/validate-consolidation-archive.py:7–10` compares only declared mappings. Lines 24–27 require mapping only when classification is exactly `included`; neither falsely excluded files nor directory entries saying `included named synthetic run with complete tuple/sidecar closure` are enumerated. It then generates a manifest from the already incomplete archive (lines 36–40), and emits an unsupported named-run-closure claim (line 56). This explains why byte/hash checks passed without reference closure. Do NOT execute this validator during diagnosis: it writes archive logs/manifests and handoff/ledger.

READ synthetic provenance: historical harness `src/lib.rs:405–422` writes `{origin}-schema.json` and clones baseline DB/sidecar files into `work`; lines 426–452 include origin `target` and open `work/target.db` for persisted measurement output. These are synthetic storage-engine tuples and sidecars in the mem-store vocabulary, not compiler output. `CONTEXT-MAP.md` identifies the root as composition root; `crates/mem-store/CONTEXT.md:36` defines WAL companions. ADR-0001 provides workspace context; no architecture change is needed.

## [ROOT CAUSE]

**CONFIRMED:** a post-copy cleanup broadened Cargo build-directory exclusion into an arbitrary filename substring test. `target-schema.json`, `target.db` and `target.db-wal` contain `target` but are not inside a Cargo `target` directory. Trim removed their archive bytes and mappings, then mislabeled them contextual exclusions. Validation trusted the narrowed map/classification, so it could prove byte preservation only for survivors and not complete named-run closure.

## [FIX PRESCRIPTION]

Coder/doc owner; minimum repair, no original mutation:

1. Keep the runnable regression check above RED before editing anything. Put its independently enumerated named-run check into the archive-validation workflow before manifest generation and before claiming closure. Keep all seven explicit run names; do not replace it with count-only checks or the five known missing filenames. If preserving the check durably, copy it into an approved evidence-checker path and adjust its root locator explicitly; do not change historical harness/checker bytes.
2. In `.tmp/trim-consolidation-archive.py:9`, replace filename-substring filtering with actual excluded directory components, e.g. `build = any(part in ('target', 'debug') for part in Path(src).parent.parts)`. This is the smallest correction for this script's existing policy: only directory components are candidates. Keep `allowed` scope restrictions. Do not broaden collection to unrelated scratch trees or remove legitimate Cargo/cache exclusions. Preparation already uses exact components; no change to historical harness is necessary.
3. The archive already exists and trim already deleted entries: changing the predicate alone cannot restore them. **Do not rerun preparation**, whose `A.exists()` guard intentionally forbids overwrite. Independently enumerate regular files in the seven original `.tmp/scratch/shard-measurement/runs/<name>/` trees. For each absent mapping, byte-copy its original to `docs/evidence/shard-consolidation-measurement/harness/runs/<name>/<relative-file>`, record the source-relative key and destination-relative value in `relocation-map.json`, and set its `reference-index.json` file classification to exactly `included`. Reject symlinks/path escape; if an existing destination differs, stop rather than overwrite unexplained bytes. Existing mappings/bytes remain unchanged. Preserve source bytes, including WAL; no open/checkpoint/recovery/normalization.
4. Correct the validation seam in `.tmp/validate-consolidation-archive.py`: execute the full named-run enumeration/byte/classification check before line 36, not merely the `included` loop. It must fail for a dropped mapping or file, a falsely excluded run file, or a byte mismatch. Correct README/handoff claims only after that passes. Preserve previous rejected handoff/review as history; report repaired metrics in the next handoff.
5. Run the exact regression command again; require GREEN with all **126** original files mapped byte-for-byte and no exclusions. This is a measured expectation for the present originals, not a hardcoded completeness oracle. Then perform the already scoped independent closure/relocation acceptance. No historical Rust suite or unrelated audit rerun is needed for this diagnosis.
6. After the final content/log write, regenerate `archive.sha256` from final payload bytes and independently verify actual archive payload set equals manifest entries. Recompute mappings/file count/logical bytes/manifest digest for the new handoff. Do not inherit earlier 277/276 or predict final sizes: map/index/log/manifest text changes affect totals. Recheck all four baseline hashes and independent review; Step 2 remains open until its gate closes. No staging/commit authorization is implied.

## Cleanup and post-mortem

MEASURED: all four protected baseline SHA-256 values still equal ledger lines 19–22 (`.cargo/config.toml`, `AGENTS.md`, `crates/mem-web/src/api/deduplication/mod.rs`, `docs/evidence/recovery-housekeeping-20261111.md`). Only diagnostic files under `.tmp/scratch/debug-archive-closure/` and this requested report were written. No instrumentation was added to originals/archive/product code; `[DEBUG-closure]` output remains only in clearly marked scratch reproduction. No fix was applied, so GREEN and original symptom resolution are intentionally deferred to coder and independent review.

Prevention: enumerate source-to-destination closure independently of any copied-set manifest or self-written classification. A cryptographic manifest authenticates the selected bytes, not the completeness of selection. No architecture refactor or new dependency is warranted.

Memory recorded: `mem_1794408493752_0940f1555` (confirmed pattern), `mem_1794408501661_06b7d3d4c` (falsified discovery route and validator gap). No external evidence or advisory decision was needed; predicates and current artifacts directly settle the cause.

## [STATUS]

DIAGNOSED-NEEDS-CODER
