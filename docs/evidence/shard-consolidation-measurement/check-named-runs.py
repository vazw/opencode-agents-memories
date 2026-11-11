"""Read-only source-to-archive closure; enumerate originals, never open databases.

Relocated from the diagnostic checker: repository is archive.parents[2].
Run with the original synthetic scratch runs present in that repository.
"""
import json
from pathlib import Path

ARCHIVE = Path(__file__).resolve().parent
ROOT = ARCHIVE.parents[2]
RUNS = ('divergent-12955', 'divergent-4807', 'divergent-5210',
        'duplicates-12955', 'duplicates-4564', 'raw-copy-12955', 'raw-copy-2952')
mapping = json.loads((ARCHIVE / 'relocation-map.json').read_text())
references = json.loads((ARCHIVE / 'reference-index.json').read_text())
errors = []
total = 0
for name in RUNS:
    directory = ROOT / '.tmp/scratch/shard-measurement/runs' / name
    assert directory.is_dir() and not directory.is_symlink(), f'missing/linked original run: {directory}'
    entries = sorted(directory.rglob('*'))
    assert not any(p.is_symlink() for p in entries), f'symlink in {name}'
    files = [p for p in entries if p.is_file()]
    assert files, f'empty original run: {name}'
    source_keys = {p.relative_to(ROOT).as_posix() for p in files}
    mapped_keys = {key for key in mapping if key.startswith(directory.relative_to(ROOT).as_posix() + '/')}
    errors.extend(f'EXTRA SOURCE MAPPING {key}' for key in sorted(mapped_keys - source_keys))
    archived_run = ARCHIVE / 'harness/runs' / name
    archived_entries = list(archived_run.rglob('*'))
    assert not archived_run.is_symlink() and not any(p.is_symlink() for p in archived_entries), f'archive symlink in {name}'
    expected_names = {p.relative_to(directory).as_posix() for p in files}
    archived_names = {p.relative_to(archived_run).as_posix() for p in archived_entries if p.is_file()}
    errors.extend(f'EXTRA ARCHIVED FILE {name}/{extra}' for extra in sorted(archived_names - expected_names))
    for source in files:
        key = source.relative_to(ROOT).as_posix()
        destination = mapping.get(key)
        if key in references and references[key] != 'included':
            errors.append(f'EXCLUDED RUN FILE {key}: {references[key]}')
        if destination is None:
            errors.append(f'UNMAPPED {key}')
            continue
        target = (ARCHIVE / destination).resolve()
        if not target.is_relative_to(ARCHIVE.resolve()) or not target.is_file():
            errors.append(f'MISSING/ESCAPING DESTINATION {key}: {destination}')
        elif source.read_bytes() != target.read_bytes():
            errors.append(f'BYTE MISMATCH {key}: {destination}')
    total += len(files)
    print(f'closure: {name}: original_files={len(files)}')
for error in errors:
    print(error)
assert not errors, f'named-run closure failed: {len(errors)} errors'
print(f'named-run closure: PASS; independently enumerated files={total}; runs={len(RUNS)}')
