# opsec audit — the W6 erasure design (2026-10-25), preserved 2026-10-27

`GATE: REJECTED` — 10 confirmed, 3 needs_validation, 4 rejected. The originals of these seven files lived
only under `.tmp/scratch/opsec-opencode-agents-memory-run-1/`, which is gitignored. **They are preserved
here because `REPORT.md` §9 is the gate that keeps ADR-0019 and ADR-0025 unwritten**, and this repository's
own rule is that a ruling which lives only in `.tmp/` is one `rm -rf` from gone. Nothing here is
re-generated, re-scored or paraphrased: every file is byte-identical to the run's output.

## Provenance

| | |
|---|---|
| Run | `opsec-opencode-agents-memory-run-1` |
| Date | 2026-10-25 |
| Target | `/home/vaz/Code/Personal/Rust/opencode-agents-memory` |
| Source ref | `b42905c` (parent `7390de1`) — **verified reachable in this repo's history** on 2026-10-27 |
| Profile | standard (scoped) |
| Origin | `.tmp/scratch/opsec-opencode-agents-memory-run-1/` (gitignored) |
| Preserved | 2026-10-27, byte-identical (`cmp`-verified per file) |

## Files

| file | bytes | sha256 (first 16) | what it is |
|---|---|---|---|
| `REPORT.md` | 26 078 | `ca74b2350254b4b0` | the audit report — verdict, per-question sections, **§9's twelve blockers**, coverage, terminal state |
| `FINDINGS-DETAIL.md` | 82 318 | `9413eb1a0b0d342a` | per-finding evidence and traces |
| `findings.json` | 97 619 | `82baeafe37cf6720` | the machine-readable findings |
| `NEEDS-VALIDATION.md` | 5 968 | `dc4843e129eaf9df` | the three findings that could not be settled statically |
| `coverage-ledger.json` | 46 922 | `c1420eef0beacc36` | coverage 21/21 |
| `architecture.md` | 8 869 | `d4ac0a9120517fc6` | the target's architecture as the audit read it |
| `run-metadata.json` | 2 336 | `960f0c9ecae1b7f3` | run parameters and terminal state |

## What §9 gates

`REPORT.md:306` opens *"What must be settled before ADR-0025 and ADR-0019 are written"* and lists twelve
blockers in unblocking order: items 1–3 are code/probe work, items 4–12 are ADR text. The audit states
that no item requires re-litigating a pinned measurement.

**State of the twelve, as of 2026-10-27** — recorded here rather than inferred by the next reader:

- **Blocker 1 closed** — `aa0dcc4`, `fix(mem-store): close the fail-open in the process-liveness probe`.
  Recorded in issue 43 with its review outcome (APPROVED, five non-blocking advisories, A3/A4/A5 recorded
  as follow-ups and not done).
- **Blockers 2–12 open.** None is closed in the tree; items 4–12 are ADR text that has not been written.

This README records the gate's location and state. It does **not** claim the gate is satisfied, and it does
not re-derive the audit: the audit is a static property of source at `b42905c`, never a runtime result —
no sandbox ran, and the report's own §0 disclosure says so at length.

## Why the rest of the run's scratch was not copied

The `.tmp/` run directory also held intermediate tooling output. Only the seven artifacts that constitute
the audit's record are preserved; nothing was curated away from `REPORT.md`'s own evidence chain.
