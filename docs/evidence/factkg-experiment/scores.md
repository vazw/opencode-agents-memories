# FactKG transfer experiment — scoring sheet + decision-rule application

Scoring per protocol §10 from the raw transcripts (`.tmp/scratch/factkg-experiment/transcripts/`,
one JSON per run; machine-scored by `score.py`, re-derived from transcripts — never from logs).
The table below is generated from the authoritative machine sheet (`scores.json` `rows`).

## Per-run verdicts (raw; correctness judged against the frozen gold)

| order | pair | arm | raw verdict | parse ok? | citation | conf | gold | correct? |
|---|---|---|---|---|---|---|---|---|
| 1 | P21 | correct | contradicted | yes | EV-P21-correct | high | contradicted | ✓ |
| 2 | P14 | correct | supported | yes | EV-P14-correct | high | supported | ✓ |
| 3 | P27 | correct | contradicted | yes | EV-P27-correct | high | unknown | ✗ |
| 4 | P16 | correct | contradicted | yes | EV-P16-correct | high | unknown | ✗ |
| 5 | P21 | unrelated | unknown | yes | none | high | contradicted | ✗ |
| 6 | P19 | unrelated | unknown | yes | none | high | contradicted | ✗ |
| 7 | P10 | unrelated | unknown | yes | none | high | supported | ✗ |
| 8 | P03 | unrelated | unknown | yes | none | high | supported | ✗ |
| 9 | P08 | correct | unknown | yes | EV-P08-correct | medium | supported | ✗ |
| 10 | P01 | unrelated | unknown | yes | none | high | supported | ✗ |
| 11 | P01 | correct | unknown | yes | EV-P01-correct | high | supported | ✗ |
| 12 | P25 | correct | unknown | yes | none | high | unknown | ✓ |
| 13 | P13 | correct | supported | yes | EV-P13-correct | high | supported | ✓ |
| 14 | P17 | unrelated | unknown | yes | none | high | contradicted | ✗ |
| 15 | P24 | unrelated | unknown | yes | none | high | contradicted | ✗ |
| 16 | P24 | correct | contradicted | yes | EV-P24-correct | high | contradicted | ✓ |
| 17 | P17 | correct | contradicted | yes | EV-P17-correct | high | contradicted | ✓ |
| 18 | P18 | unrelated | unknown | yes | none | high | supported | ✗ |
| 19 | P05 | unrelated | unknown | yes | none | high | supported | ✗ |
| 20 | P23 | correct | contradicted | yes | EV-P23-correct | high | contradicted | ✓ |
| 21 | P18 | correct | unknown | yes | EV-P18-correct | medium | supported | ✗ |
| 22 | P10 | correct | unknown | yes | EV-P10-correct | high | supported | ✗ |
| 23 | P04 | unrelated | unknown | yes | none | high | supported | ✗ |
| 24 | P22 | unrelated | unknown | yes | none | high | contradicted | ✗ |
| 25 | P26 | unrelated | unknown | yes | none | high | unknown | ✓ |
| 26 | P12 | correct | contradicted | yes | EV-P12-correct | high | supported | ✗ |
| 27 | P05 | correct | supported | yes | EV-P05-correct | high | supported | ✓ |
| 28 | P23 | unrelated | unknown | yes | none | high | contradicted | ✗ |
| 29 | P27 | unrelated | unknown | yes | none | high | unknown | ✓ |
| 30 | P25 | unrelated | unknown | yes | none | high | unknown | ✓ |
| 31 | P14 | unrelated | unknown | yes | none | high | supported | ✗ |
| 32 | P04 | correct | supported | yes | EV-P04-correct | high | supported | ✓ |
| 33 | P12 | unrelated | unknown | yes | none | high | supported | ✗ |
| 34 | P13 | unrelated | unknown | yes | none | high | supported | ✗ |
| 35 | P16 | unrelated | unknown | yes | none | high | unknown | ✓ |
| 36 | P08 | unrelated | unknown | yes | none | high | supported | ✗ |
| 37 | P03 | correct | unknown | yes | EV-P03-correct | high | supported | ✗ |
| 38 | P22 | correct | unknown | yes | EV-P22-correct | high | contradicted | ✗ |
| 39 | P19 | correct | supported | yes | EV-P19-correct | medium | contradicted | ✗ |
| 40 | P26 | correct | unknown | yes | none | high | unknown | ✓ |

## Metrics (exact §10 definitions)

1. **Paired verdict correctness** — both-correct = 2 (P25, P26); **b (correct-arm only right)
   = 8** (P04, P05, P13, P14, P16, P17, P21, P23 — order rows 4, 13, 16, 17, 20, 23, 27, 32);
   **c (unrelated-arm only right) = 2** (P25, P26 — order rows 12, 25); both-wrong = 8 (P01,
   P03, P08, P10, P12, P18, P19, P22). Parse-failed runs score incorrect: none occurred.
2. **False-contradiction rate on Unknown pairs** (n=4 per arm): correct arm 2/4 (P16, P27 —
   both `confidence: high`); unrelated arm 0/4.
3. **Source-citation correctness** — correct arm: 18/20 responses cite the supplied block id
   exactly (`EV-<pair>-correct`); 2 cite `none`. Unrelated arm: 0/20 cite the block id; 20/20
   cite `none`.
4. **Prompt-refusal / parse-failure rate** — 0/40 (0.00%); no prose refusals.

**Confident false-supported (stop-condition input)** — Unknown pairs,
(verdict=supported, confidence=high): correct arm 0, unrelated arm 0 (raw counts, n=4 per arm).

## Statistics (§11)

Paired design, n=20; McNemar exact two-sided on the discordant counts (b=8, c=2):
**p = 0.1094**. Pre-declared: p-values never drive the decision (§12). No rate inference from
the small classes (Unknown n=4 per arm; citation and refusal counts reported raw only).

## Decision rule (§12, applied exactly to this frozen sheet)

- Stop-condition 1: correct-evidence paired accuracy **not greater** than unrelated (c ≥ b)?
  c=2, b=8 → **c ≥ b is FALSE**.
- Stop-condition 2: any confident false-supported on the unrelated arm? 0 → **FALSE**.

### **DECISION: QUEUE (positive) — the stop condition does not fire.**

Only **grounding relevance** is established: correct source evidence improved paired verdict
correctness (b=8 vs c=2) and the model demonstrably grounded its verdicts in the supplied
block (18/20 exact block-id citations on the correct arm; 20/20 `none` on the unrelated arm).
Per §12 and the digest: graph superiority and consolidation benefit are **NOT** established.
Per the 2026-11-07 directive, the follow-on (claim-vs-evidence verification for the
facts/consolidation tier) may be queued **after the vault feature**.
