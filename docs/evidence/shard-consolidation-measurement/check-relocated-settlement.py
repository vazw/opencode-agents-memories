"""Execute the byte-preserved checker with only its scratch-root locator relocated."""
from pathlib import Path

archive = Path(__file__).resolve().parent
checker = archive / "harness/evidence/check-settlement.py"
source = checker.read_text()
old = 'S = Path(".tmp/scratch/shard-measurement")'
assert source.count(old) == 1, "Historical checker locator changed; review adapter"
source = source.replace(old, f"S = Path({str(archive / 'harness')!r})", 1)
exec(compile(source, str(checker), "exec"), {"__name__": "__main__"})
