# 147 — the issue-145 probe harness has four dead-path defects

Status: needs-triage — filed 2026-11-18 by the issue 145 close-out review (harness defects, none gating a delivered result)

*(Filed from `.tmp/progress/blueprint-20261118-0715-issue-145-inject-sink-r32.review-1.md` findings 1–4. The reviewer
approved the wave; these are latent defects in the test harness, not in the shipped sink.)*

## The defects

The probe and its guard were written during issue 145 to gate the delivery evidence. They measure correctly on the
checkout they were run from, but three of them resolve paths against the wrong thing and one accepts too few headings.

1. **`plugin/tests/harness/probe-145.mjs:90-91` writes outside `.tmp/`.** The block path is
   `path.resolve(".tmp/progress/…")` — cwd-relative. Run as issue 145's blueprint instructs (`cd plugin`), it creates
   `plugin/.tmp/progress/`, and `.gitignore`'s `/.tmp/` (root-anchored) does not cover it. The wave's handoff moved
   the artifacts and deleted the directory, which fixes the symptom, not the cause: the next probe run recreates an
   unignored tree. Fix: resolve from `import.meta.url` (`path.resolve(here, "..", "..")`), as
   `plugin/tests/stage-engine.test.mjs:22-23` already does for `repoRoot`.

2. **`plugin/tests/harness/probe-145.mjs:60-62` reads the seed COUNT with one uncaught attempt.** Step 8 item 5 of
   the blueprint asks for up to three attempts and the failure reason `OPEN — inbox state unreadable: <message>`. A
   lock failure here dies as an uncaught throw — stack and rc, no reason line — which is the failure mode the reason
   string exists to prevent. It did not fire in this run (rc 0).

3. **`plugin/tests/harness/probe-145.mjs:95` checks three of the four acceptable headings.** `## Related older
   sessions` is absent from the accepted list, so a block carrying only that heading reports `OPEN — history heading
   absent` — a false failure. It cannot produce a false pass; this run's block carried `## Recent sessions`.

4. **`plugin/tests/failure-toast-145.test.mjs:105` leaves temp directories behind, cwd-dependent.** It calls
   `mkdtempSync(path.join(process.cwd(), "..", ".tmp", …))`: run from the repo root, that writes `.tmp` in the repo's
   *parent*. It also never cleans up — 192 `.tmp/toast-145-*` directories accumulated before they were removed at
   close-out. Same class at `plugin/tests/probe-145-guard.test.mjs:14` (`os.tmpdir()`). Fix: `mkdtempSync` under
   `.tmp/` resolved from the module, with cleanup on exit.

## Consequence

None of the four alters a delivered result or a gate — the reviewer approved the wave with all four open, and issue
145's closure rests on the scratch-root evidence plus the post-restart runbook. They matter because the probe is the
instrument a later run will trust: an instrument that writes outside `.tmp/` or reports a false `OPEN` costs more than
the code it checks.

## Ask

Fix all four in one pass when the probe is next touched, and re-run `node --test tests/*.test.mjs` plus a probe run from
`plugin/` to confirm no `plugin/.tmp` appears.
