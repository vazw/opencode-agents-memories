import json
from pathlib import Path

e = Path(".tmp/scratch/shard-measurement/evidence")
r = json.loads((e / "results.json").read_text())
r["tests_passed"] = 8
r["rework"] = {
    "blueprint": "d90461a51f51",
    "prescription": "debug-1 items 1-4",
    "s03": {
        "mutant": "guard: both pre-guard existence clauses removed (empty root reaches create_dir)",
        "mutant_red_exit": 101,
        "mutant_red_payload": "left 'File exists (os error 17)' right root path",
        "pristine_green_exit": 0,
        "note": "mutant v1 (root.exists()-only deletion) measured GREEN: equivalent mutant, symlink_metadata().is_ok() subsumes exists(); dead-clause deletion is unkillable by construction",
    },
    "s04": {
        "red_exit": 101,
        "red_payload": "Observation.schema Null vs measured 19-column array",
        "green_exit": 0,
        "resolution": "Observation.schema/duration_ms/limitations/artifact_paths populated from the persisted artifact",
    },
    "s05": {
        "red_exit": 101,
        "red_payload": "E0599 ExactDuplicates/DivergentIds missing",
        "green_exit": 0,
        "resolution": "Case::ExactDuplicates/DivergentIds through run_case; inputs built from raw typed_rows with old1-/old2- association variants; plan 3 unique ids, 5 occurrences; duplicates.json and divergent.json persisted and carried inside Observation",
    },
    "s07": {
        "routed_test_red_exit": 101,
        "routed_test_red_payload": "Engine(case not wired)",
        "remap_mutant_red_exit": 101,
        "remap_mutant_red_payload": "Engine(persisted mismatch agent_sessions)",
        "pristine_green_exit": 0,
        "resolution": "run_case(Case::CrossDb) routes auxiliary_fixture + verify_persisted reopen oracle (typed per-table compare, no-residual bindings, shards scope/db_path) + refusal cases with system sha256sum closed-root manifests (4 sentinels, unknown encoding, divergent aux key)",
    },
    "suite_exit": 0,
    "suite_tests": 8,
    "clippy_exit": 0,
    "rustfmt_check_exit": 0,
    "immutable_baseline": "PASS",
}
r["limits"] = [
    "S08 interface only; no baseline snapshot/restore manifest",
    "S09 exact named test not implemented",
    "facts/vault presence refusal is a synthetic sentinel guard, not production support",
    "duplicate plan is a typed preflight through dedicated Case variants; no production write path exercised",
]
r["unknowns"] = [
    "ai-sessions schema",
    "production auxiliary completeness",
    "admission/drain/stale handles",
    "whole-root interrupted restore",
]
(e / "results.json").write_text(json.dumps(r, indent=2) + "\n")
print("results.json updated: rework block added, limits resolved honestly")
