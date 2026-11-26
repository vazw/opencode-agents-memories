#!/usr/bin/env python3
"""Issue 171 §4 — post-deploy observation, from the live plugin log.

The blueprint's [TEST PLAN & ACCEPTANCE CRITERIA] demanded one observation, explicitly
not an acceptance oracle:

    a session whose first `context.hook` in an activation reads `has_block:false,
    turn_index:0` is followed in the same activation by `context.hook has_block:true,
    turn_index:0` for that session, plus an `inject.requested` line carrying
    `query_chars:0` and no `turn_index`.

Inverted world: zero such sequences => `ctx.session.context` returns no messages
mid-turn => the fallback is dead code => close §4 as measurement.

Reads only the live log. Prints; writes nothing.

Usage: python3 extract.py [YYYY-MM-DDTHH:MM:SS]   (default: the deploy window)
"""
import glob
import json
import re
import sys
from collections import defaultdict

LOGS = sorted(glob.glob("/home/opencode/.opencode-mem/opencode-mem*.log"))
LINE = re.compile(r"^\[([^\]]+)\] ([a-z0-9_.]+): (\{.*\})\s*$")
SINCE = sys.argv[1] if len(sys.argv) > 1 else "2026-11-26T16:43:00"

hooks, inj = [], []
for path in LOGS:
    for raw in open(path, encoding="utf-8", errors="replace"):
        m = LINE.match(raw)
        if not m:
            continue
        ts, event, body = m.groups()
        if ts < SINCE or event not in ("context.hook", "inject.requested"):
            continue
        try:
            obj = json.loads(body)
        except Exception:
            continue
        (hooks if event == "context.hook" else inj).append((ts, obj))

print(f"window: every context.hook / inject.requested line at or after {SINCE}")
print(f"logs read: {len(LOGS)}")
print()
for ts, o in sorted(hooks + inj):
    sid = o.get("session_id", "?")
    if "has_block" in o:
        print(f"{ts}  hook  {sid}  has_block={str(o['has_block']).lower():5s} turn_index={o.get('turn_index')}")
    else:
        ti = o.get("turn_index", "-")
        print(f"{ts}  inject {sid}  query_chars={o.get('query_chars')} turn_index={ti} host_contract={o.get('host_contract')}")

# The observation, derived rather than eyeballed: per session, the first false/0
# followed later in the same window by a true/0, and a query-less inject between.
print()
print("observation per session:")
by_s = defaultdict(list)
for ts, o in sorted(hooks):
    by_s[o.get("session_id", "?")].append((ts, o.get("has_block"), o.get("turn_index")))
found = 0
for sid, evs in by_s.items():
    false0 = [e for e in evs if e[1] is False and e[2] == 0]
    if not false0:
        continue
    t0 = false0[0][0]
    true0 = [e for e in evs if e[1] is True and e[2] == 0 and e[0] > t0]
    qless = [i for i in inj if i[1].get("session_id") == sid
             and i[1].get("query_chars") == 0 and "turn_index" not in i[1] and i[0] > t0]
    print(f"  {sid}  false/0 at {t0}  ->  true/0 at {true0[0][0] if true0 else 'NONE'}"
          f"  query-less inject at {qless[0][0] if qless else 'NONE'}")
    if true0 and qless:
        found += 1
print(f"  complete sequences: {found}")
