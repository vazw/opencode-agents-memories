# Issue 163 — issue 152's shell note can never match a real command

Status: ready-for-agent
Filed: 2026-11-24, found by live verification after the 22:47:45 restart
Severity: high for the feature — the note is unreachable for 97.5 % of measured shells

## The defect

`plugin/src/plugin.js` clamps the tool input to 100 bytes **before** asking `shellStamped` to find the
shell record that names it:

```js
if (input.length > MAX_TOOL_INPUT_LENGTH) {          // :1894, MAX_TOOL_INPUT_LENGTH = 100 (:1838)
  input = input.slice(0, MAX_TOOL_INPUT_LENGTH) + "...";
}
input = shellStamped(name, input, sessionID, shells); // :1899
```

and `shellStamped` matches on the **whole** command being present in that already-truncated string:

```js
if (rec.sessionID === sessionID && rec.command && input.includes(rec.command)) { // :1934
```

Any shell command longer than 100 bytes is truncated before the match, so `input.includes(rec.command)` is
false and the note is never applied. The note is not "cut by a clamp" — it is never produced.

## MEASURED, 320 shell records since the 2026-11-24T22:47:45 restart

```
n = 320    min = 32    median = 529    max = 1144
commands <= 100 bytes:   8   (2.5 %)
commands  > 100 bytes: 312   (97.5 %)
```

Corroborating store evidence: `SELECT count(*) FROM agent_sessions WHERE metadata LIKE '%[pid %'` over
`~/.opencode-mem/data/episodic.db` (read-only, `?immutable=1&mode=ro`) returns **0**, and no injected block
in the live log contains `[pid `. The row-level observations needed to attribute that to this defect rather
than to timing are in issue 152's "Live state after the 2026-11-24T22:47:45 restart".

## Why the tests missed it

`plugin/tests/capture-payload.test.mjs`'s C4..C7 and the mutation sweep all use commands well under 100
bytes, so the fixture never reaches the clamp. This is the same class as issue 161's gap: a criterion that is
green because the input is small, not because the path works.

## The fix shape

Move the `shellStamped` call **above** the length clamp, so the match runs on the untruncated command and the
note is prefixed before the tail is cut. That satisfies both constraints at once: the match needs the whole
command, and the note is a prefix, so both clamps (this one and the engine's whole-line end-clamp in
`crates/mem-bridge/src/capture.rs`) keep it. The comment at `:1897-1898` argues for "after the clamp, so the
stamp is never what gets cut" — the reasoning is inverted, and the comment must be corrected with the code.

Do not widen the cap to fix it: that changes row bytes for every tool, and the defect is the ordering.

## Acceptance criteria

1. A shell command longer than `MAX_TOOL_INPUT_LENGTH` produces a row whose text begins with the note
   (`[pid N, alive at capture]` or `[pid N, exited]`), RED on today's code as an assertion failure.
2. A shell command at or under the cap still produces exactly one note, not two, and no `input` grows beyond
   the cap by more than the note's own bytes.
3. A non-shell tool and a shell with no matching record are byte-identical to today's output.

## Inverted world

Closed as *measurement, not a deliverable* if moving the call above the clamp makes the note displace so much
of a long command that the row stops naming the shell usefully — measured by reading the emitted rows, not
assumed.

## Not in scope

- Issue 162 (the `injectStatus` seam's missing wire case).
- Issue 152's residuals (a) and (b).
