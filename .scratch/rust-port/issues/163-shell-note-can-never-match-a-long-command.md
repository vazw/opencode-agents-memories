# Issue 163 — issue 152's shell note can never match a real command

Status: done — 2026-11-24 (landed; review APPROVED)
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

## Outcome — landed 2026-11-24

The defect was the ordering, and the ordering is what changed: `shellStamped` now runs **before** the
`MAX_TOOL_INPUT_LENGTH` clamp in `parseIdleTranscript`, so `input.includes(rec.command)` sees the whole
`command: "<cmd>"` join. The cap is untouched at 100; the note is a prefix, so the clamp cuts the command
tail and keeps the note, and the engine's end-clamp in `action_line` behaves the same way.

Comment at the call site, corrected (it argued for a match the old order could not make):

```js
      // Issue 163: BEFORE the clamp — `includes` needs every byte of the
      // command; the note is a prefix, so clamping after keeps it and cuts the tail.
```

Cases added to `plugin/tests/capture-payload.test.mjs` (and `shellTurn(command, tool = "shell")`):

| Criterion | Test | Before the fix | Mutant it survives |
| --- | --- | --- | --- |
| 1 | `test_shell_note_reaches_a_command_longer_than_the_cap` — 1143-byte command; row must start with `[pid N, alive at capture] command: ` | RED: `AssertionError: a command longer than the input cap still carries the note` (`false !== true`) | `input.includes(rec.command)` → `input.slice(0, 100).includes(rec.command)` → same assertion RED |
| 2 | `test_shell_note_is_scoped_to_its_own_session` (existing, reused) — one note, not two; ses-A's row byte-identical | GREEN | a second `shellStamped` call after the clamp → `the owning session's row carries the pid and its probed state` RED on the ses-B row |
| 3 | `test_shell_note_skips_a_non_shell_tool` — a `grep` whose input holds the live shell's command | GREEN | `if (name !== "shell" && name !== "bash") return input;` → `if (false) return input;` → `a non-shell tool is never stamped` RED |
| 3 | `test_shell_note_skips_a_shell_with_no_matching_command` — record `npm run dev`, turn `npm run build` | GREEN | `&& input.includes(rec.command)` dropped from the match → `a shell running another command is never stamped` RED |

All four mutants ran against `plugin/dist/plugin.js`; each restored and byte-compared after its run
(`RESTORED-163` ×4, `dist` pristine at sweep end). `cd plugin && node --test tests/*.test.mjs`: 281 pass,
0 fail, 0 skipped, and all three `*byte_identical_to_src` cases green.

Two ceilings survive, deliberately and named:

- **Escaped commands still never match.** The join is `key + ": " + JSON.stringify(value)`, so a command
  holding `"` or a newline is escaped and `includes(rec.command)` misses it even now. Any live count read
  after this lands is a **lower bound**. Closing it changes the predicate and needs its own discriminator.
- **Two records naming one command stack two notes.** Pre-existing, in `shellStamped`'s per-record loop; this
  change only widens the reachable set to long commands.

Row-size arithmetic is unchanged in kind: the stamped row is `7 + 103 = 110` bytes, under the engine's
160-byte `action_line` clamp and its `validation.rs` reject threshold, so no note can cause a row to drop.

No live reading was taken by this change. See issue 152's "Live state" section for the one read that closes
it, and note that a row still without a note afterwards is explained by the escaped-command ceiling.

## Review, 2026-11-24 — APPROVED

The reviewer re-ran the wave gate itself (281 pass, 0 fail, 0 skipped, exit 0, all three `byte_identical_to_src`
green) and rebuilt criterion (a)'s RED from the committed source rather than trusting the handoff: it loaded
`git show d9f9757:plugin/src/plugin.js` into `dist` and got `false !== true` on the 1143-byte fixture, whose
command is 11× the cap. Two of its own mutants each red their own assertion, parseable, restored with trap +
`cp -p` + `cmp -s`. `dist` was pristine before and after.

**One correction to this issue's own text.** The band that pays for the note is **60–89 bytes**, not the
71–100 the plan estimated: the note's length varies with the pid's digit count, so the boundary moves. A
shell command in that band previously fit the cap whole and now loses up to the note's length off its tail.
The blueprint states 71–100; the measurement says 60–89; the shipped comment does not name a band at all,
which is why the number was left to drift.

**Two ceilings survive, both recorded and neither "fixed" in passing:** an escaped command still never
matches (so any live count is a lower bound), and two records naming one command stack two notes.

**Artifact note:** the executor's first ledger used the blueprint-id filename and the delivery-path ledger was
reconstructed from the recorded run, not written live; the reviewer checked the two against each other and
against the handoff and found the evidence consistent, with the reconstruction's own DONE stamps drifting from
the file mtimes. Recorded because a ledger that contradicts its run is worse than no ledger, and this one only
nearly did.
