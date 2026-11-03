# Child-session compaction restore — live observation (2026-11-03)

The opt-in restore landed as `be005b4` + `89a704c` (blueprint
`blueprint-20261103-0724-compaction-child-restore-r3`, stamp `fd875731e949`; ADR-0017 amendment).
This file records the one thing the tests could not: that the block actually appears in a **child**
session after a compaction, on the live host.

## What was deployed when it was seen

```
engine        pid 11724, sha256 d6223037… (built from the tree at 89a704c)
wrapper       plugin/dist/plugin.js sha256 d8441fc9…  — identical to src
host restart  2026-11-03T16:21:07+07 (host) / 16:21:13 (engine) — the restart that loaded both
config        ~/.config/opencode/opencode-mem.jsonc:291  "captureChildSessions": true
store         episodic.db user_version 7 (unchanged by this wave)
```

## Observation

After the 16:21 restart, the owner reports seeing **`◈ memory context` in a subagent compaction** —
the part the TUI renders from `session.synthetic({ description: "memory context" })`
(`plugin/src/v2/legacy-client.js`). Before this wave the same action produced nothing, because
`plugin.js` returned early for any child.

The three conditions the gate requires were all true at once, which is what makes the observation
attributable rather than coincidental:

1. the config key resolved `true` (it defaults `false`, and a `false` would have kept the gate shut);
2. the **wrapper** was new — the host reloads `plugin/dist/plugin.js` only on restart, so a staged
   engine alone would not have changed this;
3. the engine advertised the flag, so the fail-closed arm (`=== true` required) did not swallow it.

The pre-compaction **ingest** is untouched by this change and stays ungated — that is D-114-2, and it
is why children keep feeding memory regardless of the flag.

## What would have falsified it

If the block had *not* appeared, the candidates in order were: the key resolving `false` on this host;
a stale wrapper (host not restarted, so `dist/plugin.js` still the old build); or a mixed-version pair
(new wrapper, pre-change engine) where absence of the advertised key keeps the gate closed by design.
A **parent** session losing its restore would have been a regression rather than a feature gap; none
was observed.
