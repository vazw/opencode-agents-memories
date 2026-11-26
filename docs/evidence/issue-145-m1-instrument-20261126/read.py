"""Issue 145 M1 — can either arm be named from what is on disk today?

Arm (a) = miss-engine: the engine gave no block or timed out.
Arm (b) = miss-host:   the engine returned a block that never reached the child.

Reads only: ~/.local/share/opencode/opencode.db (mode=ro) and ~/.opencode-mem/opencode-mem*.log.
Prints; writes nothing.
"""
import glob
import json
import re
import sqlite3
from collections import Counter, defaultdict

DB = "/home/opencode/.local/share/opencode/opencode.db"
LOGS = sorted(glob.glob("/home/opencode/.opencode-mem/opencode-mem*.log"))
LINE = re.compile(r"^\[([^\]]+)\] ([a-z0-9_.]+): (\{.*\})\s*$")
TODAY = "2026-11-26"
STRICT = "type='synthetic' and data like '{\"text\":\"<memory_context>%'"


def fixture(sid):
    """The plugin's own test fixtures use non-`ses_` ids and write to the same log."""
    return not sid.startswith("ses_")


hooks, inj = defaultdict(list), defaultdict(list)
for path in LOGS:
    for raw in open(path, encoding="utf-8", errors="replace"):
        m = LINE.match(raw)
        if not m:
            continue
        ts, event, body = m.groups()
        if event not in ("context.hook", "inject.received"):
            continue
        try:
            obj = json.loads(body)
        except Exception:
            continue
        sid = obj.get("session_id")
        if not sid:
            continue
        if event == "context.hook":
            hooks[sid].append((ts, obj.get("has_block"), obj.get("turn_index")))
        else:
            inj[sid].append((ts, obj.get("status"), obj.get("block_bytes"), obj.get("turn_index")))

con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
parent = {}
for r in con.execute("select id, parent_id from session_v2"):
    parent[r[0]] = r[1]

n, s, lo, hi = con.execute(
    f"select count(*), count(distinct session_id), datetime(min(time_created)/1000,'unixepoch'), "
    f"datetime(max(time_created)/1000,'unixepoch') from session_message where {STRICT}").fetchone()
strict_by_month = list(con.execute(
    f"select strftime('%Y-%m',time_created/1000,'unixepoch') m, count(*) from session_message "
    f"where {STRICT} group by m order by m"))
loose_by_type = list(con.execute(
    "select type, count(*), count(distinct session_id) from session_message "
    "where instr(data,'<memory_context>')>0 group by type order by 2 desc"))
loose = {r[0] for r in con.execute(
    "select distinct session_id from session_message where instr(data,'<memory_context>')>0")}
strict = {r[0] for r in con.execute(f"select distinct session_id from session_message where {STRICT}")}
con.close()

print("=" * 72)
print("1. What the stored marker actually records")
print("=" * 72)
print("STRICT (an injected block):")
print(f"  rows {n}  sessions {s}  {lo} .. {hi}")
print(f"  by month {dict(strict_by_month)}")
print("LOOSE (issue 145's headline predicate), by message type:")
for t, c, sess in loose_by_type:
    print(f"  {t:12s} rows={c:5d} sessions={sess}")
print("  => dominated by assistants quoting the tag back (runbook Trap B).")
print()

print("=" * 72)
print("2. Arm (a): is there a live status:\"error\" witness?")
print("=" * 72)
err_by_sid, err_by_day = Counter(), Counter()
for sid, evs in inj.items():
    for ts, status, nbytes, turn in evs:
        if status == "error":
            err_by_sid[sid] += 1
            err_by_day[ts[:10]] += 1
print(f"error lines total   {sum(err_by_sid.values())}   by day {dict(err_by_day)}")
print(f"fixture ids         {sum(c for k, c in err_by_sid.items() if fixture(k))}"
      f"   non-fixture sessions: {sum(1 for k in err_by_sid if not fixture(k))}")
print("fixture lines per log file (non-`ses_` session id):")
for path in LOGS:
    cnt = 0
    for raw in open(path, encoding="utf-8", errors="replace"):
        if "inject." not in raw:
            continue
        m = LINE.match(raw)
        if not m:
            continue
        try:
            o = json.loads(m.group(3))
        except Exception:
            continue
        if o.get("session_id") and fixture(o["session_id"]):
            cnt += 1
    if cnt:
        print(f"  {path.split('/')[-1]}  {cnt}")
print()

print("=" * 72)
print("3. Arm (b): can a host-side drop be distinguished at all?")
print("=" * 72)
pre = {sid: [e for e in evs if e[0] < "2026-11-25"] for sid, evs in inj.items()}
pre = {sid: e for sid, e in pre.items() if e and not fixture(sid)}
children = [sid for sid in pre if parent.get(sid)]
tab = Counter()
for sid in children:
    st = Counter(e[1] for e in pre[sid])
    tab[("loose marker" if sid in loose else "NO loose marker",
         "has error" if st.get("error") else "no error")] += 1
print(f"child sessions with inject.received before 2026-11-25   {len(children)}")
for k, c in sorted(tab.items()):
    print(f"  {c:5d}  {k[0]:16s} | {k[1]}")
print(f"  of those, sessions with the STRICT injected shape     {sum(1 for sid in children if sid in strict)}")
print()
today_hooks = {k: v for k, v in hooks.items() if any(h[0].startswith(TODAY) for h in v)}
true_today = {k for k, v in today_hooks.items() if any(h[1] is True for h in v)}
ch_today = [k for k in today_hooks if parent.get(k)]
ok_today = sum(1 for evs in inj.values() for e in evs
               if e[0].startswith(TODAY) and e[1] == "ok" and (e[2] or 0) > 0)
print(f"today: context.hook sessions {len(today_hooks)} (children {len(ch_today)}), "
      f"has_block:true {len(true_today)}, inject ok>0 {ok_today}")
print()
print("Neither log line can fail on a host-side drop (READ):")
print("  plugin.js:1457  writes inject.received BEFORE the parts push at :1472")
print("  adapter.js:121  logs context.hook BEFORE event.system.push at :123")
