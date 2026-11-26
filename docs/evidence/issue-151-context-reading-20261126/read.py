import json, re, glob, collections, sys

FILES = sorted(glob.glob("/home/opencode/.opencode-mem/opencode-mem-2026-11-*.log"))
LINE = re.compile(r"^\[([^\]]+)\] ([a-z0-9_.]+): (\{.*\})\s*$")

ctx = []          # (ts, session, has_block, turn)
inj = []          # (ts, session, turn)
capture = collections.Counter()
for path in FILES:
    for raw in open(path, encoding="utf-8", errors="replace"):
        m = LINE.match(raw)
        if not m:
            if "Auto-capture memory persisted" in raw:
                mm = re.search(r'"session_id":"([^"]+)"', raw)
                if mm:
                    capture[mm.group(1)] += 1
            continue
        ts, event, body = m.groups()
        try:
            obj = json.loads(body)
        except Exception:
            continue
        sid = obj.get("session_id") or obj.get("sessionID")
        if event == "context.hook":
            ctx.append((ts, sid, obj.get("has_block"), obj.get("turn_index")))
        elif event == "inject.requested":
            inj.append((ts, sid, obj.get("turn_index")))

ctx_sessions = {c[1] for c in ctx if c[1]}
inj_sessions = {i[1] for i in inj if i[1]}

print(f"files                {len(FILES)}  ({FILES[0].split('/')[-1]} .. {FILES[-1].split('/')[-1]})")
print(f"context.hook lines   {len(ctx)}")
print(f"inject.requested     {len(inj)}")
print(f"sessions w/ context  {len(ctx_sessions)}")
print(f"sessions w/ inject   {len(inj_sessions)}")
print()
dist = collections.Counter((c[2], c[3]) for c in ctx)
print("context.hook (has_block, turn_index) distribution:")
for (hb, t), n in sorted(dist.items(), key=lambda kv: (kv[0][1] is None, kv[0][1] or 0, str(kv[0][0]))):
    print(f"  has_block={hb!s:<5} turn={t!s:<4} n={n}")
print()
never = sorted(s for s in ctx_sessions if s not in inj_sessions)
print(f"PREMISE CANDIDATES (context.hook lines, zero inject.requested): {len(never)}")
for s in never:
    lines = [c for c in ctx if c[1] == s]
    print(f"  {s}  context_lines={len(lines)} has_block={sorted({c[2] for c in lines})} turns={sorted({c[3] for c in lines})} auto_capture_rows={capture.get(s, 0)}")
print()
print("never-prompted by the rule (no inject AND no auto-capture row):",
      [s for s in never if capture.get(s, 0) == 0])
