# FactKG transfer experiment — results

**Run:** 2026-11-07 → 2026-11-08 (UTC). **Decision: QUEUE (positive) — the grounding premise
transfers under this setup.** One page, every figure MEASURED with its artifact.

- Protocol (frozen + AMENDMENT-1): [protocol.md](protocol.md) — §16 records the orchestrator's
  S2-only amendment (§14 check 3 waived; 0 milestones in the live root, root cause measured in
  the blocker edition of this page, superseded by this file).
- Frozen inputs: [gold-worksheet.md](gold-worksheet.md) (20 pairs, gold labels + rationales,
  scrub sign-off), [run-matrix.md](run-matrix.md) (model resolution, seeds, deviations, engine
  state), [scores.md](scores.md) (per-run sheet + decision rule).
- Raw transcripts (gitignored): `.tmp/scratch/factkg-experiment/transcripts/run-<order>-<pair>-<arm>.json`
  (40 files, one per run: raw request, raw JSONL response, prompt sha256s, UTC timestamps).
- Wave ledger: `.tmp/progress/factkg-experiment.ledger.md` (live stamps, Rules 1–7).

## Setup (all MEASURED, see run-matrix.md)

- Model under test: `opencode-go/beam-free` via `opencode run --format json` (invocation path
  (a), §3.3). 40 runs = 20 pairs × {correct evidence, token-length-matched unrelated evidence}.
- Correct-arm evidence: verbatim S2 repo artifacts (ADR/issue/evidence-dir/AGENTS extracts,
  one commit subject+body). Unrelated arm: donor memories from **other projects' shards** via
  the engine's store-backed `/api/memories` route, scrub-clean, topically unrelated,
  length-matched with the cached Xenova/nomic-embed-text-v1 tokenizer (18/20 token-exact,
  2/20 within 7 tokens, never over target).
- Stratification as frozen: 10 supported / 6 contradicted (4 explicit flips) / 4 unknown;
  4 conjunction-style; 4 negated-style. Gold labels frozen before run 1; every rationale cites
  an artifact different from the evidence block (anti-circularity).
- Scoring: parse-fail = incorrect (none occurred); verdicts re-derived from raw transcripts;
  decision by §12 only.

## Counts (MEASURED: scores.md, scores.json)

| Metric | Correct arm | Unrelated arm |
|---|---|---|
| Paired verdict correctness (of 20) | **b = 8** correct-arm-only-right; both-correct 2; both-wrong 8 | **c = 2** unrelated-arm-only-right |
| False contradiction on Unknown (n=4) | 2/4 (both confidence high) | **0/4** |
| Confident false-supported on Unknown | 0/4 | **0/4** |
| Citation = supplied block id | **18/20** | 0/20 (20/20 cite `none`) |
| Refusal / parse failure | 0/20 | 0/20 |

- **McNemar exact two-sided** on (b=8, c=2): **p = 0.1094** — computed per §11, supporting
  only, never decision-driving.
- **§12 applied exactly:** stop-condition 1 (c ≥ b) false; stop-condition 2 (any confident
  false-supported on unrelated) false → **QUEUE**. The follow-on (claim-vs-evidence
  verification for the facts/consolidation tier) may queue **after the vault feature**.

## Reading the two arms (MEASURED, transcripts)

The unrelated arm's behaviour is uniform: 20/20 verdicts `unknown` with `citation: none` — the
model reliably recognized that unrelated evidence grounds nothing (no confident
false-supported, the digest's failure mode). The correct arm grounded 19/20 (block-id
citations) and its two errors on Unknown pairs were **false contradictions** (P16, P27): it
treated adjacent-but-different figures in the real evidence block as conflicts — e.g. for P16
(claim: the gate costs ≈35 s **cold**; block: the published ≈35 s **warm** figure) it answered
`contradicted, high`, the exact "missing evidence treated as contradiction" behaviour the
digest critiques. Correct-evidence helps where it exists (b=8: three of the four explicit
flips and two of the four conjunction pairs landed correct) but also *activates* over-reading
that unrelated evidence never triggers (2/4 Unknown false-contradictions).

## Exemplary transcripts (verbatim `assistant_text`)

1. Grounding on the correct arm — run 1 (order 1, P21 correct; gold `contradicted`):
   `{"verdict":"contradicted","citation":"EV-P21-correct","confidence":"high"}` — correct
   verdict with the exact block id cited.
2. Refusal to ground on the unrelated arm — run 30 (order 30, P25 unrelated):
   `{"verdict": "unknown", "citation": "none", "confidence": "high"}` — the uniform
   unrelated-arm shape; no hallucinated grounding.
3. The false contradiction on Unknown — run 4 (order 4, P16 correct; gold `unknown`):
   `{"verdict": "contradicted", "citation": "EV-P16-correct", "confidence": "high"}` — the
   warm-vs-cold cost figure read as a conflict; high confidence.

## Boundary statement (§15, verbatim obligations)

This experiment establishes **grounding relevance only**. It does **not** establish that this
project needs graph storage, triple stores, or a FactKG-style KG; it measures **nothing**
about consolidation benefit (fact preservation, provenance, privacy, currency). No benchmark
rerun or dataset-quality claims are made. The live engine was used read-only (retrieval +
health probes); no code, endpoints, or config changed; the only written files are these
§13 artifacts and gitignored scratch. No significance claims beyond the pre-declared McNemar
statement; no rate inference from the n=4 Unknown class; no post-hoc subset analyses (the
correct-arm-Unknown false-contradiction reading above is a raw-count observation, not a rate
claim).

**Result: measurement closed, outcome QUEUE.** The smallest discriminating experiment's stop
condition did not fire: correct evidence improved paired verdict correctness and no confident
false-supported verdicts occurred on the unrelated arm.
