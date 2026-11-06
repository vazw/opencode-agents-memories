# Issue 129 residuals — evidence index (r9 wave)

Blueprint: `.tmp/blueprints/blueprint-20261106-1011-issue-129-residuals-r9.md` (stamp
`sha256=3ebd5268bd6f`, SYS-APPROVED 2026-11-06T10:30Z). Review-2: APPROVED
(`.tmp/progress/20261106-1011-issue-129-residuals-r9.review-2.md`). Base `eda2efd`,
branch `issue-129/wave-residuals`.

## Files

| file | step | owner | content |
| --- | --- | --- | --- |
| `stats-breakdown.md` | Step 2 | A | stats timing measurement (2000 memories, ≥5 requests populated/empty/probe), A/B on the same db handle (2.06× candidate), measurement-not-deliverable |
| `sanitation-actor-error.md` | Steps 2b/3d | A | storage inventory matrix N=8, measured fresh-root listing, O1 in-process capture of the hostile-root composition error, RED/GREEN table incl. the facts-token alignment |
| `sanitation-mutations.md` | Step 3f | A | G1/G3/G2 mutation verdicts (KILLED, own labels) + M2 presence mutant (KILLED, review-2's own run) |
| `partial-store-route-matrix.md` | Step 4 | A | 9-route reachability matrix; Decision block (:70-81): closed as diagnosis per syseng Q2=ACCEPT-STOP 2026-11-06, inverted world stated |
| `pill-baseline.md` | Step 5 | B | PILL_CONTRACT_ONLY pre-change table, bundle `72f5eaea5edb020c`: 4 RED FAIL + 7 CONTROL PASS |
| `pill-post.md` | Step 5 | B | post-change table, 11/11 PASS |
| `dialog-baseline.md` | Step 6 | B | DIALOG_ONLY pre-change table (REGENERATED), bundle `72f5eaea5edb020c`: 8 RED FAIL + 6 CONTROL PASS (14 rows) |
| `dialog-post.md` | Step 6 | B | post-change table 14/14 PASS on `259e2ecdd2554e09` + six control mutants (m1–m5, m3 measured negative) and the shared-target-dir build trap |
| `scope-amendments.md` | rework | B/integrator | Amendment 1: web-ui/Cargo.toml web-sys feature grant (binding scope: this wave only); Amendment 2: Step 4 diagnosis close (self-contained appendix quoting the Decision block) |

Bundles: pre `web-ui-72f5eaea5edb020c_bg.wasm` (sha256 `1cb2c66974848c285279e82b2d9631ebc1fe476a9a36d1df2e7fb7d1de25142a`),
post `web-ui-259e2ecdd2554e09_bg.wasm` (sha256 `38dedc58bcc80d62975be0fb270220899a526130c0f6f73e7e5bc6dfa6a5d455`).

## Integration (Step 7, integrator)

Final gates re-run at integration from the worktree (logs `.tmp/progress/r9-final/`,
worktree `.tmp/progress/20261106-1011-issue-129-residuals-r9.ledger.md`):

| gate | result |
| --- | --- |
| `cargo fmt --all --check` | rc=0 |
| `cargo test --test workspace_deps` | rc=0, 1 passed |
| `cargo test --workspace --no-fail-fast` | see ledger (expected 2478 passed = 2470 baseline + 8 new; known flake `test_measurement_disconnected_provider_terminal_deadletter` reruns isolated before attribution) |
| root clippy lib + all-targets | rc=0 / rc=0 |
| class census `--min-emitted=175 --self-test` | SELFTEST 9/9, emitted=175 hooks=175 missing=0 dead=0 unresolved=0 |
| token census `--self-test` | SELFTEST 2/2, missing=0 dead=0 |
| DIALOG post lane | rc=0, 14/14, engine serving the worktree's built dist |
| PILL post lane | rc=0, 11/11, same engine |
| `.githooks/pre-commit` | `bash -n` + `shellcheck` clean; floor `--min-emitted=175` both lines |
