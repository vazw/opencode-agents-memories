# 104 — wave ledgers reconstruct their timestamps, omit the manifest fields, and record no rework state

Status: ready-for-agent — 2026-11-01. Filed from the batched review (`.tmp/progress/batch-review-1.md`
§8 F2, F3, F5). All three are process defects in evidence that gates consume, not code defects.

## What is wrong

**1. The stamps are reconstructed, not read.** Both ledgers of the batched pass
(`.tmp/progress/blueprint-20261101-1016-capture-agent-id-r4.ledger.md` and
`.tmp/progress/20261101-1124-profile-decay-r2.ledger.md`) carry stamps that cluster:

```
capture ledger   Steps 1-8 all at 11:42:55Z
profile ledger   Steps 1-3 at 11:49:24Z, Steps 4-5 + END at 11:56:21Z
```

Those are written in batches after the gates passed, not `date -u +%FT%TZ` reads at the moment each gate
ran. This is the **same defect previously adjudicated here** — `bug-pattern` memory, log-signature wave
SPECIES D: "seven pre-rework stamps all ended in :00 seconds … they were RECONSTRUCTED". A ledger is
declared the sole authority after an interruption; a reconstructed timestamp cannot certify that.

**2. The manifest fields the stamp protocol requires are absent.** The profile-decay blueprint's stamp
protocol requires `date -u +%FT%TZ` shard START/END stamps and a rework state per shard. Measured: the
profile ledger has both stamps; the **capture ledger has no START/END lines at all**, and **neither
ledger records a rework state** (`accepted` / `rework: …`). The ordering those stamps exist to certify
happened to be true and was re-derived from git instead:

```
95857b7 committed 11:45:26Z  <  profile shard START 11:47:20Z (records HEAD 95857b7)  <  39dadf3 11:56:36Z
```

Being re-derivable from git is why this was not a rejection — and also why it should be fixed, because
the ledger's whole claim to authority is that a reader does not have to go to git.

**3. A removed assertion cannot be identified.** The coder dropped a "redundant duplicate assertion"
from the log-row test. One squashed commit and no scratch copy means the removed text exists nowhere
in the tree or in `.tmp/`; the reviewer recorded it as *unverifiable* rather than as a gap. Deleting a
test assertion is removing coverage until proven otherwise, so the removed text belongs in the ledger.

## What would settle it

Nothing here needs code. The fix is a rule plus a template:

- A ledger line carries the output of `date -u +%FT%TZ` executed **at the moment the step's gate passed**.
  If a line is written later, it says `stamp: reconstructed <time>` — a reconstructed stamp disclosed is
  honest; one backdated is not.
- Every ledger opens with `SHARD START` and closes with `SHARD END` (or `WAVE END` for a single-shard
  wave), and carries a `rework:` field per shard once a review returns.
- Any edit that **removes** an assertion or a test records the removed text verbatim in the ledger.

## Acceptance criteria

- [ ] The ledger template (wherever wave ledgers are specified) carries: live `date -u` stamps, a
      `reconstructed` opt-out that must be used when a stamp is written later, `SHARD START`/`SHARD END`,
      and a per-shard `rework:` field.
- [ ] A rule states that removed assertions/tests are recorded verbatim, with the reason.
- [ ] The next wave's ledger demonstrates all four: two live stamps at least 1 s apart that bracket its
      own commit, a rework field, and no reconstructed line without the word `reconstructed`.
- [ ] No historical ledger is rewritten — the fix is forward-only, and this issue says so, so that a
      future reader does not "repair" the record.

## Parent

Process issue, no slice parent. Its children are future waves, not code.

## Comments

**Why this is not pedantry.** Two of this session's three evidence failures were exactly this shape: a
claim (a name, a timestamp, a "measured" label) that outran its artifact. The plan gate's own adopted
rule — *a numeral that is not an operand of an executed command is either labelled with the command that
reproduces it or deleted* — is the same rule stated for numbers. This issue states it for timestamps and
for deleted coverage, so the ledger cannot become the place where claims go to look verified.
