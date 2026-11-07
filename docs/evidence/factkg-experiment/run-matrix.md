# FactKG transfer experiment — run matrix + run header (frozen matrix, executed)

**Executed:** 2026-11-07T16:20Z (pre-flight) through 2026-11-08T00:39Z (final run). All stamps
UTC. Engine state at matrix start AND end: `GET /api/health` = 200 (MEASURED both ends); no
engine restart occurred during the matrix (the wrapper-launched engine process, PID seen at
pre-flight, was never touched; health 200 at the final scoring check confirms continuity).

## Run header (§7.1 frozen parameters, resolved/verified at run time)

| Parameter | Value |
|---|---|
| Model / provider | `opencode-go/beam-free` (READ `~/.config/opencode/opencode-mem.jsonc` at 2026-11-07T16:20Z pre-flight and re-verified at freeze — unchanged between freeze and run; no config edit occurred) |
| Invocation path | §3.3 (a): `opencode run --model opencode-go/beam-free --format json --archived` (non-interactive; opencode owns auth/routing; no key material handled) |
| Decoding temperature | not settable through path (a); opencode defaults apply (DEVIATION-3, recorded; symmetric across arms) |
| Max output tokens | provider default (opencode path (a) exposes no token cap flag; responses observed ≤ 1 sentence) |
| System prompt | byte-identical across all 40 runs; sha256 `41de30a702d89821dd5d6f498fd2b4be72998d06acb722fb8e2d095c22ff0515` (verified in every transcript) |
| User template | §7.3, two arm instances differing only in the EVIDENCE block; per-run user-prompt sha256 in each transcript |
| Claim wording | frozen per pair in `gold-worksheet.md` + `freeze.json` (sha256 per claim) |
| Evidence text | frozen per pair per arm (`freeze.json`; sha256 per block) |
| Evidence cap | 512 tokens — max block used 431 tokens (P14), all under cap |
| Length-match method | §5.2 PRIMARY: Xenova/nomic-embed-text-v1 WordPiece tokenizer (cached copy content-matched to HF upstream; full hash provenance in `freeze.json` and the worksheet), content tokens (CLS/SEP excluded), one method for all 20 pairs |
| Run order | Fisher–Yates over the 40 (pair, arm) tuples, seed 20261107, recorded in `run-order.json` BEFORE run 1 and never edited |
| Engine state | start: health 200 (2026-11-07T16:20Z); end: health 200 (2026-11-08T00:4xZ, scoring check) |
| Pre-flight | §14: checks 1, 2, 4, 5 PASS (measured 2026-11-07T16:20–16:40Z); check 3 WAIVED under AMENDMENT-1 (§16); check 6 model smoke PASS (2026-11-07T16:56Z, dummy pair parsed to the exact JSON object, discarded from all metrics) |

## Matrix execution (§9)

- 40 runs executed in the seeded order, one `opencode run` per (pair, arm); each run's full
  transcript (raw request, raw JSONL response, hashes, UTC timestamps, exit code) is in
  `.tmp/scratch/factkg-experiment/transcripts/run-<order>-<pair>-<arm>.json` (gitignored;
  referenced by path from the tracked summary per §13).
- Executed in two sessions of the same process (runs 1–24, then 25–40 after a console timeout):
  the matrix itself was never restarted, no run was retried, and the engine was not touched
  between or during sessions.
- Refusals/parse failures: **0 of 40** (metric 4 = 0.00%). No prompt-v2 rerun triggered (§9
  invalidity condition (iii) not reached).
- Model string: identical all 40 runs (`opencode-go/beam-free`); no mid-matrix change (§9
  invalidity condition (i) not reached).

## DEVIATION-3 (minor, recorded)

`opencode run` (invocation path (a)) exposes no temperature or max-output-token flags; the
provider's defaults applied. The frozen §7.1 values (temperature 0, max output 256) are
enforceable only on fallback path (b), which was not needed. The comparison is symmetric
across arms, so the deviation cannot bias the paired contrast; it does mean decoding
parameters were engine-identical to auto-capture's defaults rather than the protocol's
temperature-0 intent. Recorded per §2 ("deviations discovered mid-run are recorded, never
silently absorbed") — identified at smoke time, before run 1, and accepted in preference to
falling to path (b) (which would have put API-key handling into the experiment).

## Sourcing + donor construction (AMENDMENT-1)

- Correct-arm evidence: S2 repo artifacts only, verbatim contiguous extracts (git-log pairs =
  one commit's subject+body). Every gold rationale cites an artifact different from the pair's
  evidence block (§16 anti-circularity). Full provenance: `gold-worksheet.md`, `freeze.json`.
- Donor blocks: engine store-backed `/api/memories` (the §14-check-4 route), projects ≠ source
  (donor pools: a work project, a personal JS project, a packages repo), P-A scrub-clean, no
  shared ≥5-char alphanumeric token with the paired claim, one block per pair; six donor rows
  reused across exactly two pairs each, re-lengthened per pair (§5.1, recorded in the
  worksheet). Token-exact matching: 18/20 pairs Δ=0; P14 Δ=−7 (shortfall, within the ±8
  acceptance recorded pre-run). The matcher never exceeded the target.
- Scrub P-A (automated grep) machine-run over every claim, evidence block and donor block:
  zero hits. P-B (human read) signed per row in the worksheet.

## Stratification as frozen (§6)

10 supported (conjunction-structure: P03, P05, P08, P15) / 6 contradicted (explicit flips:
P21 per-agent→per-project, P22 rejected→adopted, P23 limitation→none, P24 residuals→full
erasure) / 4 unknown (P16, P25, P26, P27). Negated-style claims: 4 (P04, P19, P22, P23).
Counts met exactly; no unfilled slots.
