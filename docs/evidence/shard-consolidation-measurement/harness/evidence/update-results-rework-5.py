"""Review-5 rework (debug-2 items 2-3): rebuild the settlement artifacts from measured data.

Derives evidence/admission.json from the newest green S09 run and rewrites the
hand-maintained fields of evidence/results.json that drifted (R2), recording the
superseded pristine snapshot (R3b). Run from R:

    python3 .tmp/scratch/shard-measurement/evidence/update-results-rework-5.py
"""

import glob
import hashlib
import json
import os
from collections import Counter
from pathlib import Path

S = Path(".tmp/scratch/shard-measurement")
E = S / "evidence"

runs = sorted(glob.glob(str(S / "runs" / "s09-admission-*")), key=os.path.getmtime)
assert runs, "no S09 run artifact found"
run_dir = Path(runs[-1])
artifact = json.loads((run_dir / "admission-observations.json").read_text())

paths = artifact["paths"]
counts = Counter(entry["status"] for entry in paths.values())
not_exercised = artifact["not_exercised"]
status_counts = {
    "completed": counts["completed"],
    "refused": counts["refused"],
    "blocked": counts["blocked"],
    "not_exercised": len(not_exercised),
}

timings = artifact["timings_us"]
assert "scope_same_scope_negative_poll_us" in timings, "poll timing missing from artifact"
assert "scope_positive_control_us" in timings, "control timing missing from artifact"
assert "contender_blocked_poll_1s" not in timings, "bool poll witness must be gone"

# ---- evidence/admission.json: green/suite/clippy pointers and the measured phases ----
admission = json.loads((E / "admission.json").read_text())
admission["green"] = {
    "exit": 0,
    "result": "1 passed (1.04s)",
    "log": "evidence/s09-green-after-fix.log",
}
admission["m3_scope_poll_after_fix"] = {
    "callsite_window_0s": {
        "exit": 101,
        "observable": "negative poll returned after 1.081608ms; a degenerate window proves nothing",
        "log": "evidence/s09-m3-callsite-after-fix.log",
    },
    "constant_window_0s": {
        "exit": 101,
        "observable": "persisted gate: the persisted negative poll must show the wait on the live contender, got 1081us",
        "log": "evidence/s09-m3-after-fix.log",
    },
    "latch_without_release": {
        "exit": 101,
        "observable": "same-scope contender must not complete while the holder is latched",
        "log": "evidence/s09-m1-after-fix.log",
    },
    "refused_to_completed": {
        "exit": 101,
        "observable": 'assertion `left == right` failed: hardcoded all-blocked/all-completed ledgers cannot pass (left "completed" / right "refused")',
        "log": "evidence/s09-m2-after-fix.log",
    },
}
admission["suite"] = {
    "exit": 0,
    "result": "10 passed; 0 failed (2.72s)",
    "log": "evidence/s10-suite-after-fix.log",
}
admission["clippy"] = {"exit": 0, "log": "evidence/s09-rework-clippy.log"}
admission["status_counts"] = status_counts
admission["observation_source"] = str(run_dir / "admission-observations.json")
admission["phases"] = artifact
(E / "admission.json").write_text(json.dumps(admission, indent=2) + "\n")

# ---- evidence/results.json: delete hand-maintained duplicates, regenerate unknowns ----
results = json.loads((E / "results.json").read_text())
for key in ("restore_executed", "online_admission_executed"):
    results.pop(key)
assert not [k for k in results if k.endswith("_executed")], "top-level *_executed fields must be gone"

results["unknowns"] = [
    "production: ai-sessions schema (never opened by this wave)",
    "production: auxiliary completeness beyond the synthetic fixtures",
    "production: power-loss / filesystem atomicity for interrupted restore (process-kill harness only)",
] + [f"not_exercised: {entry['path']}" for entry in not_exercised]

results["final_tree_sha256"] = {
    rel: hashlib.sha256((S / rel).read_bytes()).hexdigest()
    for rel in ("src/lib.rs", "src/main.rs", "tests/measurement.rs")
}

s09 = results["s09"]
s09["green_log"] = "evidence/s09-green-after-fix.log"
s09["measured_outcomes"] = status_counts
s09["scope_poll_evidence"] = {
    "window": "S09_SCOPE_POLL = 1s, shared by the negative poll and the positive control",
    "negative_poll_us": timings["scope_same_scope_negative_poll_us"],
    "positive_control_us": timings["scope_positive_control_us"],
    "channel_state": "try_recv() == Empty (queued, sender alive) asserted while the holder is latched",
    "mutants": admission["m3_scope_poll_after_fix"],
    "note": "the relative lower bound (window/2) scales with a mutated constant; the absolute 500000us persisted gate kills the constant mutant, the live lower bound kills the call-site mutant",
}
s09["key_observations"]["scope_barrier"] = (
    "admitted holder latched per (scope,hash); same-scope contender stayed queued "
    f"({timings['scope_same_scope_negative_poll_us']}us measured against a {timings['scope_positive_control_us']}us "
    "uncontended same-key control); distinct scope completed while held; queued contender drained only after release"
)
for log in (
    "s09-green-after-fix.log",
    "s09-m3-callsite-after-fix.log",
    "s09-m3-after-fix.log",
    "s09-m1-after-fix.log",
    "s09-m2-after-fix.log",
    "s09-rework-clippy.log",
    "s09-pristine-check.log",
    "s10-suite-after-fix.log",
    "s10-settlement.log",
):
    if log not in s09["logs"]:
        s09["logs"].append(log)
s09["pristine_snapshot_superseded"] = (
    "evidence/s09-pristine.sha256 first captured 2026-11-11T06:05:49Z, before the final rustfmt write pass "
    "(src/lib.rs 06:06:10Z) and the three bool_assert_comparison fixes (tests/measurement.rs 06:06:29Z); "
    "superseded values tests a745c97da91f5994217a695a27c9cbc75977c67e73be9f153f4ddc1362716e4a / "
    "lib fe6033033b4eda55c5a4b76fe96bcc8224b58c18e317ea4a3752f78f2a5cf227; regenerated after the final "
    "edit, three files, verified by check-settlement.py"
)

results["s10"]["settlement_check"] = "evidence/s10-settlement.log (literal EXITCODE recorded in the log)"
results["s10"]["pristine_manifest"] = "evidence/s09-pristine.sha256 (three files, post-format)"

results["rework_5"] = {
    "blueprint": "d90461a51f51",
    "prescription": "debug-2 items 1-5 (review-5 R1/R2/R3, R4/R5)",
    "r1": {
        "constant": "S09_SCOPE_POLL = 1s",
        "live_assertions": "poll.is_err(); try_recv() == Empty; poll_elapsed >= S09_SCOPE_POLL/2",
        "positive_control": (
            f"uncontended same key user:scope-a completed in {timings['scope_positive_control_us']}us inside the same window"
        ),
        "persisted_gate": "scope_same_scope_negative_poll_us >= 500000",
        "mutants": {
            "callsite_window_0s": 101,
            "constant_window_0s": 101,
            "latch_without_release": 101,
            "refused_to_completed": 101,
        },
    },
    "r2": (
        "top-level restore_executed/online_admission_executed deleted; unknowns regenerated with "
        "production:/not_exercised: prefixes from admission.json; settlement machine-gated by "
        "evidence/check-settlement.py"
    ),
    "r3": "evidence/s09-pristine.sha256 regenerated after the final source edit, three files, sha256sum -c exit 0",
    "r4": "S09/S10 gate logs re-captured with literal EXITCODE:<n>",
    "r5": "S09 ledger row carries the authorization/release note; S10 history de-duplicated",
}
(E / "results.json").write_text(json.dumps(results, indent=2) + "\n")

print("admission.json rebuilt from", run_dir)
print("status_counts", status_counts)
print("poll_us", timings["scope_same_scope_negative_poll_us"], "control_us", timings["scope_positive_control_us"])
print("final_tree_sha256", results["final_tree_sha256"])
print("unknowns", len(results["unknowns"]))
