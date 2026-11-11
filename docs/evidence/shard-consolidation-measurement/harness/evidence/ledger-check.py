"""Blueprint ledger-shape gate, verbatim from blueprint-20261111-0310 step 1.

Run: LEDGER=<path> python3 evidence/ledger-check.py   (LEDGER_CLOSE=1 at close)
"""
import os, re
from pathlib import Path

text = Path(os.environ['LEDGER']).read_text()
ts = r'\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z'
starts = re.findall(r'^WAVE START: (.+)$', text, re.M)
ends = re.findall(r'^WAVE END: (.+)$', text, re.M)
assert len(starts) == len(ends) == 1
assert re.fullmatch(ts, starts[0]) and (ends[0] == '-' or re.fullmatch(ts, ends[0]))
lines = [x for x in text.splitlines() if x.startswith('- [')]
pattern = r'- \[([ ~x!])\] (S\d{2}) \| name: (.+?) \| gate: (.+?) \| started: (.+?) \| ended: (.+?) \| state: (pending|STARTED|REWORK|DONE|BLOCKED) \| proof: (.+?) \| history: (.+)'
rows = [re.fullmatch(pattern, x) for x in lines]
assert len(rows) == 10 and all(rows), 'one valid row per step required'
values = [m.groups() for m in rows]
assert sorted(x[1] for x in values) == [f'S{i:02}' for i in range(1, 11)]
assert sum(x[0] == '~' for x in values) <= 1, 'multiple open cursors'
for mark, ident, name, gate, start, end, state, proof, history in values:
    assert (mark, state) in [(' ', 'pending'), ('~', 'STARTED'), ('~', 'REWORK'), ('x', 'DONE'), ('!', 'BLOCKED')]
    assert start == '-' or re.fullmatch(ts, start)
    assert end == '-' or re.fullmatch(ts, end)
    if mark == ' ': assert start == end == proof == '-'
    if mark == '~': assert start != '-' and end == '-'
    if state == 'REWORK': assert history != '-' and proof != '-', 'retain prior cycle and rework reason'
    if mark == 'x': assert start != '-' and end != '-' and proof != '-'
    if mark == '!': assert end != '-' and proof != '-'
closing = os.environ.get('LEDGER_CLOSE') == '1' or ends[0] != '-'
if closing:
    assert ends[0] != '-' and all(x[0] in ('x', '!') for x in values)
print('ledger-shape: PASS')
