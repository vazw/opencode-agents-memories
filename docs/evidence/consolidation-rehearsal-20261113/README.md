# Step 10 rehearsal evidence — 2026-11-13

Durable, sanitized copies of the Step 10 rehearsal and settlement for wave
`20261111-1304-lossless-shard-consolidation`. The originals lived in
`.tmp/progress/`, which is gitignored, so the settlement would have been one
cleanup away from gone.

Everything below was produced on a `cp -a` COPY of the frozen recovery snapshot.
No live store was opened, no `/home/vaz` data was read, nothing was stopped,
signalled or mutated.

## Files and their hashes

| file | sha256 | what it is |
|---|---|---|
| `rehearsal-3.md` | `320e3ed7e018703e9025934de8a1c2b0fc2bedeec6fafae2a44aba2f5f917589` | the clean-copy run: anomaly verdict, F5 marker-plant, F3 carried-legacy check, clean-copy integrity/FK, the `§C.5a` oracle |
| `rehearsal-3b.md` | `36e8a3b82767efffd4d1916b7c121e38734e91cf035370036cc2e81295bd611e` | rollback, repeat-apply at both phases, registry, cosine smoke — and the F1' contradiction, raised rather than papered over |
| `rehearsal-3c-settlement.md` | `ac0dc65ffa22a6bb4216d44fd7dd976fd5c816783146baf4ce64fbdb391d34bc` | the settlement: `SETTLEMENT: PASSED`, F1–F6 each closed by a named measurement |
| `release-rehearsal.md` | `e19e36d5a4a4dd1b2ca641090eb67f7cf2e923e81fe2d2effb54d3c7138c65e1` | the deployable-artifact gate: the 13-step sequence on release bytes, build determinism, the plan generator verified on a fresh snapshot |

## What the settlement rests on

- **Frozen snapshot intact.** `manifest.final.sha256` and `manifest.baseline.sha256`
  both `e0fe0276fe5cf769a649f742bbc18ba26e3383a4575a8e3edac9fab6f53d258e`
  before and after every run.
- **F1–F6 closed**, each by a measurement on a clean copy — not by a fix's own claim.
- **Independent oracle.** `oracle3.py` was written from §C.5a of the architecture
  revision, not from the implementation's output, and agrees with the sidecar on
  both the reparent set and the row count (34/34 memory targets exact;
  `agent_sessions` 888 → 887; one 300-row reparent tuple).
- **Rollback proven.** Kill at L4 → exit 134 → `consolidate-recover` 0 →
  `consolidate-rollback` 0, restored digest equal to the custody baseline and to
  the pre-apply digest, restored `(mode, kind, path)` matching `custody/mode-table`.
- **The deployable file is the rehearsed file.** `.tmp/cutover-artifact/opencode-agents-memory`,
  sha256 `24b02b3566bc5a31e8b6bc8ca7d2ed99bcef33eecf63f53542a38c1d427e7756`,
  83097040 B, `cmp`-identical to the binary the 13 steps ran on. The build is
  deterministic per build path (three identical relinks) but not commit-pinned,
  so the rule is: never rebuild between rehearsal and staging.

## Non-blocking residuals, carried deliberately

- **F10** — `consolidate-shards --apply` and `consolidate-staged-verify` accept a
  custody dir with no baseline, producing a staged tree that `consolidate-install`
  refuses (77, `baseline unreadable`). Fail-closed: no store mutation, no baseline
  written, both trees intact, recoverable by restarting the custody dir. The
  design makes the S3 baseline the first action of S3, so this is a missing
  precondition check, not a missing guard.
- **Documentation defect** — design line 333 and the blueprint's
  `test_verified_repeat_apply_is_noop` row describe repeat apply as a no-op.
  Measured: `consolidate-shards --apply` at `Verified` refuses 77
  `window_not_applyable` by the rev-13 pin, while `consolidate-verify`
  (settlement) is the true no-op. Both statements are true at different levels;
  the stale wording is corrected at the next revision rather than voiding the
  dual approval on body `2763cdf6…` for a docs-only change.
- **F5 marker** — the planted marker is a byte copy of the engine's `.wal-mode`,
  not the product of a live writable open. The refusal is measured for that byte
  content; a live-open-generated marker was not separately measured.
- **Cosine** — a three-query smoke with 0 unexplained rows, not a recall
  measurement.

## What this evidence does NOT establish

The stop ladder, custody release, plan derivation in a live window, and the
service restart are unmeasured. The restart command is not derivable from this
repository: no systemd unit exists, and the service (`opencode serve --service`,
PID 16181 at the time of writing) is a manual daemonized launch. The operator
must supply it.
