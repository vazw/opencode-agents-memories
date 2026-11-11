"""S10 settlement gate (debug-2 item 2c): results.json must agree with the artifacts.

Seven assertions, all recomputed from live files at settlement time. Run from R:

    python3 .tmp/scratch/shard-measurement/evidence/check-settlement.py

Exits nonzero on any failure. The last line is `settlement: PASS` only on success.
"""

import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

S = Path(".tmp/scratch/shard-measurement")
E = S / "evidence"
TREE_FILES = ("src/lib.rs", "src/main.rs", "tests/measurement.rs")

failures = []


def check(number, name, ok, detail=""):
    status = "PASS" if ok else "FAIL"
    print(f"check {number} [{status}] {name}" + (f" — {detail}" if detail else ""))
    if not ok:
        failures.append(f"check {number}: {name} — {detail}")


results = json.loads((E / "results.json").read_text())
admission = json.loads((E / "admission.json").read_text())

# 1. executed blocks are green and no hand-maintained top-level *_executed summary remains.
check(
    1,
    "s08/s09 green and no top-level *_executed field",
    results["s08"]["green_exit"] == 0
    and results["s09"]["green_exit"] == 0
    and not [key for key in results if key.endswith("_executed")],
    f"s08={results['s08']['green_exit']} s09={results['s09']['green_exit']} "
    f"unexpected={[key for key in results if key.endswith('_executed')]}",
)

# 2. every unknown is scoped.
unknowns = results["unknowns"]
bad_scope = [u for u in unknowns if not u.startswith(("production: ", "not_exercised: "))]
check(2, "every unknowns[] entry carries production:/not_exercised:", not bad_scope, f"unscoped={bad_scope}")

# 3. not_exercised unknowns are exactly the admission paths; measured counts agree.
claimed = sorted(u[len("not_exercised: ") :] for u in unknowns if u.startswith("not_exercised: "))
actual = sorted(entry["path"] for entry in admission["phases"]["not_exercised"])
check(
    3,
    "not_exercised unknowns == admission.phases.not_exercised[].path",
    claimed == actual,
    f"claimed={claimed} actual={actual}",
)
check(
    3,
    "results.s09.measured_outcomes == admission.status_counts",
    results["s09"]["measured_outcomes"] == admission["status_counts"],
    f"results={results['s09']['measured_outcomes']} admission={admission['status_counts']}",
)

# 4. final_tree_sha256 is recomputed from the live tree, never quoted.
live = {rel: hashlib.sha256((S / rel).read_bytes()).hexdigest() for rel in TREE_FILES}
claimed_tree = results["final_tree_sha256"]
check(
    4,
    "final_tree_sha256 equals live sha256 of the three source files",
    claimed_tree == live,
    f"claimed={claimed_tree} live={live}",
)

# 5. the pristine manifest verifies itself AND maps the same tree.
manifest_path = E / "s09-pristine.sha256"
verify = subprocess.run(
    ["sha256sum", "-c", str(manifest_path.relative_to(S))],
    cwd=S,
    capture_output=True,
    text=True,
)
manifest = {}
for line in manifest_path.read_text().splitlines():
    if not line.strip():
        continue
    digest, _, rel = line.partition("  ")
    manifest[rel.strip()] = digest.strip()
check(
    5,
    "sha256sum -c evidence/s09-pristine.sha256 exit 0",
    verify.returncode == 0,
    f"exit={verify.returncode} output={verify.stdout.strip() or verify.stderr.strip()}",
)
check(
    5,
    "pristine manifest mapping == final_tree_sha256",
    manifest == claimed_tree,
    f"manifest={manifest} claimed={claimed_tree}",
)

# 6. every "S0x executed" limit has a matching block whose gate exited 0.
executed_limits = [limit for limit in results["limits"] if re.match(r"^S\d{2} executed", limit)]
unbacked = []
for limit in executed_limits:
    step = re.match(r"^(S\d{2}) executed", limit).group(1).lower()
    block = results.get(step)
    if not isinstance(block, dict) or block.get("green_exit") != 0:
        unbacked.append(step)
check(
    6,
    'every limits[] entry starting "S0x executed" has green_exit == 0',
    bool(executed_limits) and not unbacked,
    f"limits={[limit.split(':')[0] for limit in executed_limits]} unbacked={unbacked}",
)

if failures:
    for failure in failures:
        print("settlement: FAIL", failure)
    sys.exit(1)

print("settlement: PASS")
