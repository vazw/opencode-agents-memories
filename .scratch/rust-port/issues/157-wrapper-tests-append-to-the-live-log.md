# 157 — every test run appends to the owner's live wrapper log, so log-derived evidence mixes suite traffic with production traffic

Status: fixed — 2026-11-19 (harness-only, option 1. Blueprint `blueprint-20261119-1404-…-r11`, body hash `ed5ae751a5d9`, syseng `GATE: APPROVED`, code-reviewer `STATUS: APPROVED` after independent re-execution: suite green, live-log non-real delta 0, t2p 0 violations across all bridge-default files, product byte-identical.)

## What was measured

`plugin/src/plugin.js:307-308` resolves the wrapper log from `homedir()`, not from the storage root:

```js
export function engineLogPath(home = homedir()) {
  return path.join(home, ".opencode-mem", "opencode-mem.log");
}
```

The test harness sets only `OPENCODE_MEM_STORAGE_ROOT` (`plugin/tests/harness/bridge.mjs:432`, `:1109`) and never
redirects the log, so every driver the suite spawns writes its structured lines into the **live** log.

Reading the live file after the issue-156 wave (`~/.opencode-mem/opencode-mem.log`, the `turn_index` field added by that
wave):

```
lines with turn_index: 71
real session ids (ses_…, 26 chars): 3   — all turn 0
test fixture ids: 68                     — ses-1 (62), ses-OTHER (3), ses-2 (3)
```

`ses-1` is the fixture session id in `plugin/tests/v2-inject-delivery.test.mjs`. The 0/20 histogram that looked like
issue 150's live `[0, 20]` reading was one suite run: turns 0 and 20 arrive 39 ms apart (`18:28:07.108` and
`.147`), which no human session produces.

## Why it matters

Issue 150's live reading, this wave's post-restart triage, and issue 151's "16 apparent witnesses" were all read from
this file. A test run that happens to touch the same events can manufacture the exact shape the reading is looking
for, and nothing in the log marks a line as test traffic. The defect is silent in both directions: a contaminated
reading looks like a clean one, and a real production line is indistinguishable from a fixture line.

## Candidate fixes (not chosen here)

1. Resolve the log under the storage root (`engineLogPath(storageRoot)`), so a harness with a temp root logs to the
   temp root. Cost: the owner's file moves with `OPENCODE_MEM_STORAGE_ROOT` — check nothing else assumes `$HOME`.
2. Keep the path and have the harness set the log destination explicitly. `plugin/src/plugin.js:303` already names
   an `OPENCODE_MEM_LOG_FILE` variable that `childEnvironment()` does not forward, so this may be one forward plus
   one read.
3. Keep both and add a `test_run: true` field on every line a driver emits, so contamination is at least visible.

Preference, on the repo's own rule of deleting before adding: whichever option touches one read and one harness line,
not a new field.

## What would settle it as measurement

Re-run the wrapper suite, then count lines carrying a fixture session id in `~/.opencode-mem/opencode-mem.log` before
and after: the fix is `0` new, today's behaviour is roughly one fixture line per driven request.

Inverted world: if the drivers already resolve the log under the temp storage root, the fixture-id count in the live
file is zero and this issue is void.

## Resolution — 2026-11-19

Fix is candidate 1's inverse of the harness's blind spot: every driver spawn now gets a temp `HOME`, so the wrapper (`engineLogPath`, `plugin/src/plugin.js:307-308`) and the engine child it spawns both log under that temp home. `runDriver` (`plugin/tests/harness/bridge.mjs`) replaces an absent, empty, or owner-resolving `HOME` — `realpathSync` on both sides, `path.resolve` only on ENOENT, other errors rethrown — and returns `run.home`. Four spawn sites that bypass `runDriver` (`e2e-shutdown`, `wrapper-glue`, `host-config`, `shutdown`) pass `driverHome(tmp)` explicitly. `e2e-shutdown` gets no `DRIVER_STORAGE_ROOT` guard on purpose: it passes `OPENCODE_MEM_STORAGE_ROOT` directly at `:116`.

Candidates 2 and 3 were rejected on measurement, not taste. Storage-root anchoring moves the log for every user who sets `OPENCODE_MEM_STORAGE_ROOT` and has to reconcile two different canonicalisation rules. An `OPENCODE_MEM_LOG_FILE` override flips `engine-stderr-drain.test.mjs` row 13, which pins the current behaviour deliberately.

Measured close: the suite with owner `HOME` reproduced the leak (non-real line delta > 0) before the edits and adds zero after; `.tmp/157/probe/t2p.mjs` reports zero violating spawns across all bridge-default files; `service.json` is untouched, which also settles that the engine never writes it. The engine's own `HOME` forwarding is covered by a mandatory real-engine test rather than a stub, because stub engines never reach `LogSink`.

Known residual, recorded rather than hidden: the spawn probe cannot see the `node --test` runner's own worker spawn (attribution only, outcome covered by the log count), and three test files call the wrapper in-process with no spawn at all (`engine-stderr-drain.test.mjs:31`, `fixtures.test.mjs:18`, `memory-tools.test.mjs:25`) — the live-log delta is their compensating control. Temp `om-home-<pid>-` roots are removed on normal exit; a SIGKILL leaks one.

Hygiene note from the review: four pre-edit per-file runs wrote to the owner's log before the edits landed. They sit before the Step 5 baseline so no criterion was weakened, but re-running the suite four times against owner `HOME` is not the discipline this issue is about.
