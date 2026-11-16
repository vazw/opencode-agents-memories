# Recovery housekeeping — 2026-11-11

## Authority and scope

READ: Blueprint `20261111-1304-recovery-housekeeping`, approved stamp
`sha256=78e05651b082`, SYS-APPROVED 2026-11-11T13:12Z. Full body SHA-256:
`78e05651b0827169c04deb77b9358bbc74d628f36a75312a79136866fc28d7d4`.
The owner authorized three scoped configuration corrections and two separate local
AgentsBrain commits, leaving the hex note untracked. Cooperative editing exclusion
was explicitly confirmed. No root commit, push, restart, encryption enablement,
consolidation or live database operation was authorized.

## Competing designs and decision

- **A — exact allowlisted edits and explicit staging (selected):** preserves
  comments, opaque credential pointers and unrelated work. Costs explicit byte,
  semantic, fingerprint and Git cohort checks. Prediction: only approved text and
  commit sets change. Refuted by any out-of-scope byte or staged path.
- **B — whole-file JSONC serialization and broad staging (rejected):** shorter
  operationally, but risks losing comments and absorbing registry/hex/unrelated
  changes. No implementation or performance measurement of B was run.

READ: The shared discriminator is exact before/after text and parsed-key comparison,
then actual commit parent/path/blob sets. A was selected for locality, not a measured
performance advantage. Inverted world: already-correct configuration or already
separately delivered exact commits would close as verification without replay.

## Configuration result

MEASURED in the approved configuration slice, then rechecked during settlement
preparation: exactly thirteen inert top-level leaves plus `memory.defaultScope`
were removed; its otherwise-empty parent and approved orphaned comments were
removed. Configured port and localhost comment changed from 4747 to 3742. Only the
approved commented encryption-key URI was corrected; encryption entries remain
inactive. All other parsed values and opaque pointers were preserved. No secret
pointer was resolved or secret key read. Config contents are not reproduced here.

- Original/private backup SHA-256:
  `290cd7a98c815f07bc023fb8cb79b2f1fe4400693245e1c81493fa124c68252b`.
- Final actual config SHA-256:
  `612d34136c549d84cb14df29dd86857be7d0ce1af4d8ee657260dc5d666ff7c1`.
- Approved exact patch SHA-256:
  `880875514348c572d1c66450013b8c712cba4e784e6712643902a25ff5bd158c`.

MEASURED: Existing production JSONC seam with recursive duplicate-property rejection
accepts original-to-final inputs, verifies exact semantic allowlist and absence of
active encryption properties; exit 0. Actual bytes equal approved prospective
bytes. Owner/mode/device/inode remain original. Independent review-2 and opsec-1
approved the configuration slice only. The original GNU/difflib hunk-alignment
mismatch was a presentation-oracle issue, not byte drift; mutation was not replayed.

## Separate local vault commits

MEASURED ingest commit:
`41cc100e821346284dc3286e3b1745bf8b5a7660`.
Parent: `011db767a8102f338b60d0cc668d4788da630578`.
Message: `docs(vault): ingest issue-118 wave 4 polish closeout`.
Exactly nine changed paths:

- `raw/issue-118-wave4-polish-closeout-20261110.md`
- `wiki/Second Brain.md`
- `wiki/concepts/erasure-posture.md`
- `wiki/concepts/fleet-review-gate-evidence.md`
- `wiki/concepts/served-bundle-provenance.md`
- `wiki/concepts/vacuous-test-oracle.md`
- `wiki/entities/opencode-mem.md`
- `wiki/log.md`
- `wiki/sources/issue-118-wave4-polish-closeout-20261110.md`

MEASURED: Each committed blob matches the independently approved staged snapshot
and expected worktree SHA-256. Only the 77-byte shorter duplicate source bullet was
deleted from the frozen entity content; the detailed three-line bullet is
byte-identical, with one issue-118 citation remaining in Sources. Fifteen distinct
added wikilink targets resolve locally, including OpenCode by explicit title/alias.
Six bounded credential-pattern classes yielded zero candidates; this is not proof
against every possible secret encoding. Historical source paths were preserved.
Actual committed cohort contains 47 occurrences of `/home/vaz`; an earlier report's
43 counted matching lines, not occurrences. This correction is additive; historical
reports were retained. Archived product figures were not remeasured by housekeeping.
Independent review-3 and opsec-2 approved the exact ingest stage before commit.

MEASURED registry commit:
`bb5aebf6e6f22104c5e292a4022fd9af4eed9a1c`.
Parent: `41cc100e821346284dc3286e3b1745bf8b5a7660`.
Message: `docs(vault): update project registry and template links`.
Sole changed path: `AGENTS.md`.
Approved/committed blob: `109e4b4bdec371dd3981c3d5ffdf38c8ef90d050`.
SHA-256: `7ce2db882677d5290e73853dd49afa31a2bc88e19ce0d68679aef7696584acee`.
Complete bytes/diff match the original registry baseline. Independent review-4 and
opsec-3 approved that exact stage and ingest parent before commit.

Both normal Git commits exited 0 without hook bypass. Both external local commits
were recorded in the trail; ingest is grouped under issue-118. No push occurred.

## Operational checks and settlement limits

MEASURED during final preparation:

- Real `check-config <private-original> <actual-config>` invocation: exit 0.
  This invokes the existing production parser, not engine startup.
- `git diff --cached --check`: exit 0 before each commit.
- `git diff-tree --no-commit-id --name-only -r <commit>`, `git rev-parse
  <commit>^`, `git rev-parse <commit>:<path>`, `git show <commit>:<path>`:
  exits 0; parent/path/blob sets and complete bytes match approved manifests.
- Vault index empty and no tracked worktree diff after registry commit. Only the
  original hex note remains untracked, unchanged SHA-256
  `858245b0dac403c1dd365c99f60fed3618ad8b54dab5df37554ea3cb6bb71d02`.
- Root's three unrelated dirty files retain original complete fingerprints;
  root HEAD remains `df34c522cd349ddb59f021fc82eae64190123e2f` and index empty.
  This evidence document is the sole intended extra root dirt and remains unstaged.
- PID 23866 retains start ticks 5762211. Read-only `ps` and `ss` exit 0;
  listener remains `127.0.0.1:3742`, owned by that PID, with no 4747 listener.
  This corroborates captured-process continuity, not a system-wide historical
  audit or future restart health. Startup-warning disappearance is not claimed.

TDD-EXEMPT: configuration, Markdown and one-off Git operations introduce no new
software behavior. No fabricated regression suite or automated RED/GREEN claim.
Exact operational acceptance replaces software tests for this scope.

Private command transcripts, original/prospective bytes, full fingerprints,
commit manifests and reviewer reports are retained under the pinned wave's
`.tmp/progress/` artifacts. They are not durable merely because referenced here.
This sanitized record is also **not durable until later committed**; no root commit
is authorized. Step 7 remains OPEN awaiting independent final settlement. Executor
checks and earlier slice approvals do not self-clear that gate. Any fresh unrelated
fingerprint/index drift blocks closure without cleanup or replay.
