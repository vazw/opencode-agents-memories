# Approved architecture bodies

A dual-approved architecture body existed only in gitignored `.tmp/`, so no hash-bound
record of what was actually approved survived a cleanup. The opsec rev-15 audit named
this as a process gap: rev 15's non-§E.5 bytes could not be hash-verified against the
rev-14 approval because that body was archived nowhere.

| file | sha256 | lines | approval |
|---|---|---|---|
| `rev14-approved-body.md` | `2763cdf626890e066cadadaa490607250c570659802416327842738baa1f289f` | 383 | syseng gate-21 `GATE: APPROVED` + opsec-21 `VERDICT: APPROVED`, both fresh, 2026-11-13 |

Each file here is the CANONICAL BODY of the draft at
`.tmp/drafts/20261111-shard-consolidation-architecture-revision.md`, i.e. the output of
`sed '1,/^---$/d'` — the bytes an approval covers, with the header trail excluded.
