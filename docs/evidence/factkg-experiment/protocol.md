# FactKG transfer experiment — executable protocol

**Status:** DRAFT protocol, frozen before execution. Written 2026-11-07 by the protocol-drafting
session. **Not yet executed.** The experiment itself is run by a later coder session, after (or
alongside) the current engine silent-500 gate lands and the pre-flight gate in §14 passes.

**Authority chain:**

- Binding spec: FactKG research digest §6, `.tmp/scratch/search-20261107T060549Z.md` (accessed
  2026-11-07). §1 below reproduces the hypothesis and stop condition **verbatim** from it.
- Evidence rules: `docs/agents/workflow.md` — every substantive claim in this protocol and in the
  results carries `MEASURED` / `READ` / `REPORTED` / `INFERRED`; no measurement is ever claimed
  that was not run; "not covered by any evidence here" is an acceptable result.
- Provenance of route facts in this protocol: `READ` of
  `crates/mem-web/src/api/mod.rs:406-502` (the full `/api/*` route table) on 2026-11-07.
  Anything about the *live* engine's behaviour is `REPORTED` project context until the pre-flight
  gate measures it.

**Research-only scope:** this experiment measures whether the FactKG-style grounding premise
(checking a textual claim against explicit supporting evidence) transfers to this project's local
coding-agent memory. It is a *measurement*, not a deliverable. Both outcomes — QUEUE and CLOSE —
are successes by the repo's workflow rules.

---

## 1. Hypothesis and stop condition (VERBATIM from digest §6)

> **INFERRED smallest discriminating experiment — NOT RUN:** Before considering transfer, use
> **20 fixed, privacy-safe local claim/evidence pairs** from real coding decisions, including
> explicit contradiction, missing evidence, and two-fact/negated claims. Freeze model, prompt,
> claim wording, context size; compare **correct source evidence versus same-length unrelated
> evidence**, paired per claim. Human gold labels: Supported / Contradicted / Unknown. Measure
> paired verdict correctness, false contradiction rate on Unknown, and source-citation
> correctness. If correct evidence does not improve accuracy or unrelated evidence still gets
> confident "supported" verdicts, paper's grounding premise does not transfer under this setup;
> close as measurement, no feature justified. If correct evidence helps, only grounding relevance
> established—not graph superiority or consolidation benefit. Rationale: missing causal evidence
> ablation and missing Unknown class in original evaluation.

The stop condition above is the **decision rule** (§12). P-values are reported (§11) but never
drive the decision.

User directive framing (2026-11-07, from project memory): positive outcome ⇒ the follow-on
(claim-vs-evidence verification for the facts/consolidation tier) may be queued **after the vault
feature**. Negative outcome ⇒ close as measurement, nothing queued.

---

## 2. Design overview

- **Paired, within-claim:** every claim runs twice — once with its correct source evidence, once
  with same-length unrelated evidence from a different project's shard. 20 pairs ⇒ **40 runs**.
- **Gold labels assigned before any model run** (§8), from git/PRD/ADR ground truth, with a
  written rationale per pair. Once the first run starts, no pair may be added, edited, or
  re-labelled. Violation ⇒ matrix invalid, full rerun, both matrices recorded.
- **Frozen:** model and invocation path, both prompt templates (byte-identical system prompt
  across arms), decoding temperature, max output tokens, claim wording, evidence text, token
  budget, run order seed. §7 lists every frozen parameter; deviations discovered mid-run are
  recorded, never silently absorbed.
- **Arms differ in exactly one byte-span:** the EVIDENCE block of the user template. Everything
  else — claim text, system prompt, instructions — is byte-identical across arms.

---

## 3. Model under test and route resolution

### 3.1 What exists (READ, 2026-11-07)

`crates/mem-web/src/api/mod.rs:406-502` registers the complete API surface: `/api/tags`,
`/api/memories` (list/add/update/delete/bulk-delete/pin/unpin), `/api/search`, `/api/stats`,
`/api/cleanup`, `/api/deduplicate`, `/api/migration/*`, `/api/prompts/{id}`,
`/api/user-profile*` (incl. `/api/user-profiles`), `/api/skills*`, `/api/projects`,
`/api/projects/{project_id}/sessions`, `/api/projects/{project_id}/sessions/{session_id}`,
`/api/projects/{project_id}/milestones`. `/api/health` is token-exempt; every other `/api/*`
route is behind the token gate (READ: `crates/mem-web/tests/api_route_auth.rs:12-13`).

**There is no `/api/facts` route and no claim-verification / claim-style Q&A endpoint.** Grep
over `crates/mem-web/src` for `api/facts|verify|claim` confirms the only `verify*` symbols are
shard-integrity checks (`verify_shards`, `verify_write_shard`), not claim verification. The
milestone route (`crates/mem-web/src/api/milestones.rs:1-18`) is a read-only projection that
carries promoted durable facts and the `contributors` JSON — it is the engine's closest thing to
facts retrieval, and it is retrieval, not verdict-taking.

### 3.2 Consequence — DEVIATION-1 (mandatory, pre-declared)

The engine REST API is used **only for retrieval** (claims, correct evidence, donor evidence —
§4, §5). **Verdict queries cannot go through the engine** (no such endpoint), so they go to the
same configured provider/model directly, outside the engine process. This is an explicit,
pre-declared deviation from engine-only querying, required by the route surface above.

### 3.3 Model resolution (run time, never hardcoded)

At run time the executor resolves, and records verbatim in the run header (§13):

1. Read `~/.config/opencode/opencode-mem.jsonc` (JSONC — strip comments before parsing, or grep
   the two keys). Take `opencodeProvider` and `opencodeModel`. As READ on 2026-11-07 these
   resolve to `opencode-go` / `beam-free`; the protocol does **not** hardcode them — the values
   at run time are the ones under test, recorded in the run header.
2. **Invocation path, in preference order:**
   - **(a) Preferred:** invoke through OpenCode itself (`opencode run --model
     "<opencodeProvider>/<opencodeModel>"` non-interactively, or the equivalent session-prompt
     API path). This reproduces what the engine's auto-capture path does — the plugin calls
     opencode's `session.prompt` and opencode owns auth, token refresh and provider routing
     (READ: config file §"OpenCode Provider Settings", 2026-11-07) — so no API key handling is
     added by this experiment.
   - **(b) Fallback (only if (a) cannot run non-interactively):** if the resolved config carries
     a manual fallback (`memoryProvider` `openai-chat` + `memoryApiUrl` + `memoryApiKey`
     resolvers), call that OpenAI-compatible `/chat/completions` endpoint with the `memoryModel`
     value. API material is resolved at run time (`env://`, `file://` resolvers as configured);
     **no token, key path, key value, or internal endpoint hostname is ever copied into any
     protocol, worksheet, transcript header, or results artifact** (§4.3 scrub rule P7).
3. If the model string changes between the freeze and the run (config edited), the matrix is
   invalid and must be rerun; record both values in the ledger.

### 3.4 DEVIATION-2 (minor, pre-declared)

Decoding temperature is frozen at **0** for reproducibility of scoring, rather than the engine's
`memoryTemperature: 0.3` (which governs auto-capture summarisation, not verdicts). Recorded here
as a deliberate frozen parameter; it does not affect the comparison, which is symmetric across
arms.

---

## 4. Claim and evidence sourcing procedure

### 4.1 Sources (in preference order)

- **S1 — Engine retrieval (preferred):** `GET /api/projects/{project_id}/milestones?limit=200`
  for the source project (this repo). Each row carries the milestone summary, the durable facts
  it promoted, and the `contributors` JSON. Milestones are the durable-facts tier's projection
  and are agent-anonymous where promoted facts are (REPORTED project context). Also
  `GET /api/projects/{project_id}/sessions` for episodic context when a milestone fact needs its
  decision narrative.
- **S2 — Repo ground truth:** `git log` of this repo (decision-shaped commit subjects/bodies),
  `docs/adr/*.md`, `.scratch/*/PRD.md`, `issues/NN-*.md`. This is the **gold-label authority** —
  the label comes from what the repo actually records, never from the memory tier's own summary
  of it (a memory summary that misstates the repo would otherwise make a wrong gold label).
- **S3 — Evidence artifact per pair:** the verbatim evidence block is a short extract (target
  ≤ 300 tokens, cap 512, §7) of exactly one artifact: a milestone's promoted-fact text, a commit
  message, an ADR paragraph, an issue `Status:` block, or a docs sentence. Each block gets an
  id `EV-<pair>-<arm>` and its sha256 is recorded in the worksheet (§8). Evidence text is
  verbatim (no paraphrase) so the citation and gold rationale are checkable.

### 4.2 Pair construction

- **Claim:** one single declarative sentence about a real coding decision recorded in S2.
  Present tense, self-contained, no pronouns without antecedents, no wording copied from the
  evidence block (so "supported" requires reading the evidence, not string matching — note this
  is a style choice, pre-declared; FactKG's written style likewise paraphrases).
- **Contradicted pairs:** claim asserts the opposite of the recorded decision (entity/polarity
  flip in the manner of FactKG §3.1 substitutions — e.g. decision chose A; claim says A was
  rejected in favour of B).
- **Unknown pairs:** claim asserts something the evidence does **not** state — prefer things the
  repo explicitly leaves unmeasured or unrecorded (the strongest Unknowns are figures the repo
  says were "not re-measured" or decisions with no recorded rationale). Missing evidence is
  never made into contradiction (digest §4 critique 3 is the reason this class exists).
- **Conjunction pairs:** claim conjoins two facts, both individually present in the evidence
  block; gold Supported requires both (FactKG conjunction semantics).
- **Negated pairs:** claim wording contains explicit negation ("not", "never", "does not") — a
  style axis spread across gold labels (see §6), not a label of its own.
- **One/two-hop:** claim requires one fact (restated) or two facts linked (e.g. "X was chosen
  because Y").

### 4.3 Privacy scrub checklist — MANDATORY, applies to every claim, evidence block, donor
block, and transcript header. Two passes per item: (P-A) automated grep, (P-B) human read.

| # | Automated grep (case-insensitive) | Rule |
|---|---|---|
| P1 | `sk-[a-zA-Z0-9]{8,}`, `Bearer `, `token`, `secret`, `passwd`, `password` | No credential material or fragments, in any form. |
| P2 | `\.key`, `\.pem`, `id_rsa`, `\.env` | No key/cert file paths. |
| P3 | `https?://` (whitelist none) | No URLs at all — private URLs, internal hosts, webhooks. Rewrite as "the configured endpoint" if needed. |
| P4 | `10\.\d+\.\d+\.\d+`, `192\.168\.\d+`, `127\.0\.0\.1:\d+`, `[a-z0-9-]+\.(local|lan|internal)` | No internal/loopback addresses or hostnames; ports tied to internal services are stripped. |
| P5 | `@gmail|@.*\.(com|org|dev|io|me)|/home/[a-z0-9_]+` | No email addresses; no user home paths — replace with `~`. |
| P6 | git author **names/emails**, `whoami`, hostname | No personal data. Contributors/attribution is described structurally ("the milestone's contributors JSON") without copying identifying values into artifacts. |
| P7 | account/provider identifiers, client or employer names, third-party **private** repo names | No client names, no private third-party identifiers. Public repo names (e.g. upstream open-source projects) are allowed when needed by the claim. |
| P8 | `AKIA`, `ghp_`, `gho_`, `xoxb`, `eyJ` (JWT prefix) | Known token prefixes. |
| P9 | `/api/` + any token literal, any value read from config secret keys | Never copy values of `memoryApiKey`-style keys or the engine API token into artifacts; refer to them by key name only. |

(P-B) Human read pass: for each item, read the full text and ask "could a stranger learn
anything about people, machines, accounts, or private endpoints from this?" If yes → rewrite or
drop the pair. Each pair's worksheet row gets a `scrub` cell marked `P-A ok` / `P-B ok` /
signed-off. **A pair that cannot pass both passes is dropped and replaced before freeze** —
replacement is by the same stratification slot (§6), recorded in the ledger.

### 4.4 Gold labelling

For every pair: gold label (`supported` / `contradicted` / `unknown`) + 1–3 sentence rationale
citing the S2 artifact (path, commit, or issue line) that settles it. Ambiguity at gold time ⇒
drop the pair (a pair whose gold is arguable contaminates every metric). Labels are frozen into
§8 before the first run.

---

## 5. Unrelated-evidence construction (the unrelated arm)

### 5.1 Donor shard

- Donor evidence comes from a **different project's shard** than the source project. Enumerate
  candidates with `GET /api/projects` (READ: roster route, `mod.rs:488`). Retrieve donor text
  with `GET /api/memories?page=N&pageSize=200` and filter rows by the DTO's `project_name` /
  `project_path` fields (READ: `crates/mem-web/src/api/memories.rs:62-63`) to the donor project,
  or with `GET /api/search?q=<neutral-term>` verifying each hit's project fields. The donor's
  content passes the §4.3 scrub checklist too — it is other local-project content, treated with
  the same privacy rules.
- Donor text must be **topically unrelated** to its paired claim: it may share the domain
  (software engineering) but not the specific decision, file set, or feature. Operationally: no
  shared named entities (file paths, tool names, flags) with the claim's subject matter; when in
  doubt, pick a different donor row. This keeps the arm "unrelated evidence", not "wrong but
  adjacent evidence" — the digest wording is *unrelated*.
- One donor block per pair (paired per claim); donor blocks may be reused across pairs only if
  re-lengthened per pair, and reuse is recorded in the worksheet.

### 5.2 Length matching — exact method (pre-declared, one method for ALL pairs)

- **Primary method (token-exact):** tokenize both the correct-evidence block and the donor
  block with the HuggingFace tokenizer of the configured embedding model — at freeze time
  `Xenova/nomic-embed-text-v1` (READ: `~/.config/opencode/opencode-mem.jsonc:15`; resolve at
  run time like §3.3). Any tokenizer loading path is acceptable (`tokenizers` Rust crate,
  Python `tokenizers`, or the same model id on a local HF hub cache) as long as the same
  tokenizer is used for both arms and the method is recorded. Target: donor token count ==
  correct-evidence token count.
- **Truncate:** if the donor candidate exceeds the target, truncate at the nearest **sentence
  boundary** at or below the target token count (never mid-sentence, never mid-word); if no
  sentence boundary exists at or below target, take the first sentence and continue with the
  next donor row (don't pad a single sentence beyond its end).
- **Pad:** if short, append verbatim continuation sentences **of the same donor document** until
  the token count matches exactly. Never insert filler words or repeated sentences.
- **Fallback (only if no tokenizer is available at run time):** UTF-8 **character length**
  matching within ±2%, same sentence-boundary truncate/pad rules, using the bytes/4
  approximation noted in the engine config for token intuition. The chosen method (primary or
  fallback) is recorded **once** in the run header and applies to all 20 pairs — no mixing.
- Both arms of a pair are then token-equal (or char-equal under fallback), so length cannot
  confound the verdict comparison. Cap: no evidence block exceeds **512 tokens** (§7); claims
  whose correct evidence exceeds the cap are shortened at extraction time (§4.1) before
  matching.

---

## 6. Stratification targets (binding counts)

20 pairs, gold labels distributed:

| Gold label | Count | Composition constraints |
|---|---:|---|
| `supported` | 10 | ≥4 conjunction-structure (two-fact) claims; ≥1 negated-style; remainder mixed one/two-hop. |
| `contradicted` | 6 | ≥4 explicit contradiction (polarity or entity flip of a recorded decision); ≥1 negated-style; ≤1 may be conjunction-structure. |
| `unknown` | 4 | ≥4 missing-evidence claims (this class is exactly these); ≥1 negated-style. |

Style axis cross-check: **≥2 negated claims** overall (spread across the three labels as above);
**≥4 conjunction** overall (all inside `supported` per the table); remainder are mixed
one/two-hop coding decisions. If sourcing cannot fill a slot with a scrub-clean, gold-unambiguous
pair, the pair is dropped and the slot left **empty and recorded as unfilled** in the worksheet
— never filled with a weaker pair that bends the gold label. Metrics report denominators per
class as filled.

---

## 7. Frozen parameters and prompt templates

### 7.1 Frozen parameter table (record values at freeze, verify unchanged at run)

| Parameter | Frozen value |
|---|---|
| Model / provider | resolved at run time per §3.3; recorded in run header; must be unchanged between freeze and run |
| Invocation path | §3.3 (a), fallback (b); recorded |
| Decoding temperature | `0` (DEVIATION-2) |
| Max output tokens | `256` |
| System prompt | byte-identical across arms; sha256 recorded at freeze |
| User template | two instances (§7.3) differing **only** in the EVIDENCE block; sha256 of each recorded at freeze |
| Claim wording | verbatim per pair in §8 worksheet; sha256 per claim |
| Evidence text | verbatim per pair per arm; sha256 per block; token count recorded |
| Evidence cap | 512 tokens per block (correct and donor alike) |
| Length-match method | §5.2 primary or fallback; recorded once |
| Run order | randomized once with seed `20261107` (Fisher–Yates over the 40 (pair, arm) tuples); the ordered list is recorded before the first run |
| Engine state | recorded at matrix start and end (§9) |

### 7.2 System prompt (identical text for both arms — copy byte-for-byte)

```text
You are a careful fact-verification assistant. You will be given one CLAIM and one EVIDENCE block. Judge the claim using ONLY the evidence block. Do not use outside knowledge, do not judge by plausibility, and do not fill gaps from memory.

Answer with exactly one JSON object and nothing else:
{"verdict": "supported" | "contradicted" | "unknown", "citation": "<evidence id>", "confidence": "high" | "medium" | "low"}

Rules:
- "supported": the evidence block states everything the claim asserts.
- "contradicted": the evidence block states something that conflicts with the claim.
- "unknown": the evidence block neither states nor conflicts with what the claim asserts. Missing evidence is not contradiction.
- "citation": the evidence id from the block header if your verdict rests on any part of the block; "none" if it does not.
- "confidence": how certain you are in the verdict given only this block.
- If you cannot comply, still answer the JSON object using the closest verdict. Never refuse in prose.
```

### 7.3 User templates (the two arms — identical except the EVIDENCE block)

Arm `correct`:

```text
CLAIM:
{claim}

EVIDENCE [id: {evidence_id_correct}]:
{evidence_block_correct}

Judge the claim against this evidence block only. Respond with the JSON object now.
```

Arm `unrelated`:

```text
CLAIM:
{claim}

EVIDENCE [id: {evidence_id_unrelated}]:
{evidence_block_unrelated}

Judge the claim against this evidence block only. Respond with the JSON object now.
```

`{claim}` is byte-identical across arms. Evidence ids are distinct per arm
(`EV-<pair>-correct`, `EV-<pair>-unrelated`) so the citation field is checkable literally (§10,
metric 3). No other differences exist between the arms.

---

## 8. Gold-label worksheet (EMPTY — filled by the coder before the first run)

Freeze rule: every row complete (including `gold` + `rationale` + scrub sign-off) before run 1.
Shipped copy goes to `docs/evidence/factkg-experiment/gold-worksheet.md` with the results (§13).

| pair id | claim (verbatim) | evidence ref (S2 artifact: path/commit/issue) | evidence block sha256 (correct) | donor ref (project, row) | donor sha256 | gold label | rationale (cites ground truth) | style (one-hop/two-hop/conjunction/negated) | scrub sign-off (P-A/P-B) |
|---|---|---|---|---|---|---|---|---|---|
| P01 | | | | | | | | | |
| P02 | | | | | | | | | |
| P03 | | | | | | | | | |
| P04 | | | | | | | | | |
| P05 | | | | | | | | | |
| P06 | | | | | | | | | |
| P07 | | | | | | | | | |
| P08 | | | | | | | | | |
| P09 | | | | | | | | | |
| P10 | | | | | | | | | |
| P11 | | | | | | | | | |
| P12 | | | | | | | | | |
| P13 | | | | | | | | | |
| P14 | | | | | | | | | |
| P15 | | | | | | | | | |
| P16 | | | | | | | | | |
| P17 | | | | | | | | | |
| P18 | | | | | | | | | |
| P19 | | | | | | | | | |
| P20 | | | | | | | | | |

Illustrative **shapes only** (from repo-public artifacts; these are *not* the frozen pairs and
must not be copied into the worksheet as-is — the coder sources and scrubs its own):

- Contradicted shape: evidence = `.cargo/config.toml` (pins the wild linker for host links);
  claim = "the host-linker pin in this repo's cargo config selects rust-lld" → contradicted.
- Unknown shape: evidence = ADR-0016's gate-cost passage (≈35 s warm, measured 2026-10-25, "not
  re-measured since"); claim = "the pre-commit gate costs about 35 seconds on a cold target
  directory" → unknown (the measured figure is warm; no cold figure is recorded).
- Conjunction shape: a milestone's promoted fact pair (a route exists AND is token-gated); claim
  conjoins both.

---

## 9. Run matrix and execution procedure

- **40 runs:** 20 pairs × 2 arms. Order: the §7.1 seeded randomized list of (pair, arm) tuples,
  generated and recorded before run 1; executed in exactly that order. Full randomization
  (interleaved arms) prevents order/position confounds from aligning with the arm split.
- **Per run, record:** order index, pair id, arm, UTC timestamp (live, per ledger Rule 1),
  resolved model string, raw request, raw response, sha256 of both prompt instances.
- **Engine state:** recorded at matrix start and matrix end — `GET /api/health` status, engine
  build (commit if available), the pre-flight results of §14, and the resolved base URL. The
  engine is used read-only during the matrix; **no engine restart mid-matrix** — if one happens,
  the matrix is invalid and rerun in full (both matrices recorded).
- **Refusals/parse failures are runs too:** a refused or unparseable response is recorded, not
  retried silently. No retry per run. (Retry would bias the refusal metric; §10 metric 4 exists
  precisely to count these.)
- **Invalidity conditions (pre-declared):** (i) model/config changed mid-matrix; (ii) engine
  restart mid-matrix; (iii) refusal/parse-failure rate > 20% (> 8 of 40) ⇒ prompt-format
  diagnosis, freeze prompt **v2** with the minimal fix, rerun the **full** 40-run matrix once;
  at most one rerun; both attempts recorded, scoring uses the final complete matrix.

---

## 10. Scoring sheet

One row per run; aggregates below. Shipped copy: `docs/evidence/factkg-experiment/scores.md`.

| order | pair | arm | raw verdict | parse ok? | citation field | confidence | gold | verdict correct? | notes |
|---|---|---|---|---|---|---|---|---|---|
| | | | | | | | | | |

**Metric definitions (exact):**

1. **Paired verdict correctness** — per pair: `correct-arm verdict == gold` vs `unrelated-arm
   verdict == gold`. Aggregates: both-correct, both-wrong, b (correct-arm only right), c
   (unrelated-arm only right). Primary comparison input (§11). Parse-failed runs score as
   **incorrect** for this metric and are separately visible in metric 4.
2. **False-contradiction rate on Unknown pairs** — of the 4 Unknown pairs, the fraction answered
   `contradicted`, **per arm**. Also recorded: the fraction answered `contradicted` with
   `confidence: high` (the digest's "confident" qualifier, operationalized as the `high` band of
   the frozen confidence field).
3. **Source-citation correctness** — `citation` field equals the id in the supplied block header
   (strict string match), per arm. On the correct arm this distinguishes grounded verdicts from
   parametric-memory verdicts (a model answering from its own knowledge would cite `none` or
   hallucinate an id); on the unrelated arm it measures whether the model grounds at all. Also
   recorded: count of `citation: "none"` responses per arm.
4. **Prompt-refusal / parse-failure rate** — responses that are not exactly one parseable JSON
   object with the three required fields and legal enum values, plus explicit prose refusals,
   per arm and overall.

**Confident false-supported (stop-condition input):** on Unknown pairs, count of
`(verdict=supported, confidence=high)` responses, per arm. Recorded with raw counts — no rate
inference (n=4).

---

## 11. Statistics

- **Primary comparison: McNemar's test** on the paired per-pair correctness outcomes
  (correct-evidence vs unrelated-evidence) — exact two-sided binomial version on the discordant
  counts (b, c) from §10.1. Raw counts (n=20, both-correct, both-wrong, b, c) are reported
  alongside.
- **Pre-declared:** the design is paired; only McNemar on the paired outcomes supports any
  statistical statement here. **Without the paired design, no significance claim is possible at
  n=20** — an unpaired comparison of 20 vs 20 verdicts could not support one. Equally
  pre-declared: p-values **do not drive the decision** (§12 does).
- **No inference from small classes:** the Unknown false-contradiction rate (n=4), citation
  correctness, and refusal rates are reported as raw counts only; no rate comparisons, intervals,
  or significance claims are drawn from them.
- McNemar is computed but cannot rescue a negative result: if the stop condition fires, the
  result is **CLOSE as measurement** regardless of any p-value; if the stop condition does not
  fire, a significant McNemar is *supporting* evidence in the QUEUE report, not the trigger.

---

## 12. Decision rule (exact, applied to the frozen scoring sheet)

From the digest stop condition (§1, verbatim), operationalized:

- **CLOSE (negative) — stop condition fires** if **either**:
  1. correct-evidence paired accuracy (§10.1) is **not greater** than unrelated-evidence paired
     accuracy (i.e. c ≥ b in the §10.1 notation — including ties), **or**
  2. on Unknown pairs, the unrelated arm produced **any confident false-supported** verdict
     (§10 confident false-supported > 0 on the unrelated arm).
  ⇒ The grounding premise does not transfer under this setup. **Close as measurement, no feature
  justified, nothing queued.** Record per workflow.md as a measurement, not a deliverable.
- **QUEUE (positive) — stop condition does not fire** (b > c **and** no confident
  false-supported on the unrelated arm): only **grounding relevance** is established. Queue the
  follow-on (claim-vs-evidence verification for the facts/consolidation tier) **after the vault
  feature**, per the 2026-11-07 directive. The report must state explicitly that graph
  superiority and consolidation benefit are **not** established (digest §6).
- **INVALID:** matrix invalid per §9 ⇒ no decision; fix and rerun (once), then re-apply this rule.

The rule is computed from the frozen scoring sheet only; no post-hoc re-scoring, no re-labelling,
no subset selection. Every judgement call that would change a score is an INVALID-matrix event,
not an adjustment.

---

## 13. Artifacts and recording (workflow.md compliance)

| Artifact | Path | Tracked? |
|---|---|---|
| This protocol (frozen copy, shipped with results) | `docs/evidence/factkg-experiment/protocol.md` | yes |
| Gold worksheet (frozen) | `docs/evidence/factkg-experiment/gold-worksheet.md` | yes |
| Run matrix + run header (model resolution, seeds, engine state) | `docs/evidence/factkg-experiment/run-matrix.md` | yes |
| Scoring sheet + decision-rule application | `docs/evidence/factkg-experiment/scores.md` | yes |
| Results summary (one page: counts, McNemar b/c, decision, quotes of 2–3 exemplary transcripts) | `docs/evidence/factkg-experiment/results.md` | yes |
| Raw transcripts (one JSON per run: request, response, hashes) | `.tmp/scratch/factkg-experiment/transcripts/run-<order>-<pair>-<arm>.json` | no (gitignored) — referenced by path from the tracked summary |
| Wave ledger (live stamps, Rules 1–7) | `.tmp/progress/factkg-experiment.ledger.md` | no (gitignored), referenced from the summary |

Rules applied: never claim a measurement that was not run; label every figure in
`results.md` MEASURED with the artifact path; the tracked summary must be self-sufficient without
the `.tmp/` trees (a ruling that lives only in `.tmp/` is one `rm -rf` from gone). The experiment
records to `docs/evidence/factkg-experiment/` — it does **not** create ADRs, issues, or code.

---

## 14. Pre-flight gate (engine health — the experiment runs only on a healthy engine)

All checks measured live at run time, recorded in the run header. Any failure ⇒ experiment does
not start; record the blocker and stop (this enforces the ordering after the engine silent-500
gate lands). No check is assumed from prior sessions.

1. `GET /api/health` (token-exempt) → 200, on the engine the wrapper launches. Base URL is a
   run-time input (project context REPORTS `127.0.0.1:3742` for the Rust engine); confirm by
   probing, do not silently fall back to another port.
2. `GET /api/projects` → 200 with ≥ 2 distinct projects (source + donor). If only one project
   exists, the donor shard requirement (§5.1) cannot be met ⇒ blocker.
3. `GET /api/projects/{source}/milestones?limit=200` → 200, non-empty.
4. `GET /api/memories?page=1&pageSize=200` → 200 (store-backed route, exercises the path the
   silent-500 gate covers).
5. Token gate: `/api/*` calls carry the engine's token, resolved at run time (never written into
   any artifact; scrub rule P7/P9).
6. Model smoke: one invocation of the frozen system+user templates on a **dummy** pair (not one
   of the 20; discarded) — must parse to the JSON object. Recorded as a smoke row, excluded from
   all metrics.

---

## 15. NOT-IN-SCOPE (explicit)

- **No graph storage claims.** A positive result establishes grounding relevance only — never
  that this project needs graph storage, triple stores, or a FactKG-style KG (digest §6).
- **No consolidation benefit claims.** Nothing here measures whether consolidation preserves
  facts, provenance, privacy, or currency (digest §6, §4 critiques).
- **No FactKG benchmark rerun, no dataset quality claims** beyond the digest's own reading.
- **No engine code changes, no new endpoints, no config changes** — read-only against the engine
  and the repo; the only written files are the §13 artifacts.
- **No vault work** — vault is a separate queued feature; this experiment only gates whether the
  follow-on may queue after it.
- **No significance claims** beyond the pre-declared McNemar statement (§11); no rate inference
  from the small classes; no post-hoc subset analyses.
- **No claims from this protocol about the live engine** beyond what §14 actually measures —
  route facts here are READ of source; engine behaviour is measured, not assumed.

---

## 16. AMENDMENT-1 (2026-11-07, orchestrator ruling — recorded before any run)

**Trigger:** §14 pre-flight check 3 failed at 2026-11-07T16:31–16:40Z — the live storage root
holds 0 milestones (`episodic_milestones` = 0 rows, no `facts.db`, `milestoneCount: 0` on all
22 projects). Root cause measured and recorded in `results.md` blocker edition: the
consolidator's age window (`episodic.consolidationAgeDays`, default 30 days) has never elapsed
for any session (oldest session 2026-11-01). Expected engine behaviour, not a fault.

**Waiver:** §14 check 3 (source milestones non-empty) is **waived** for this run by the
orchestrator's amendment. Checks 1, 2, 4, 5 stand as PASSED (measured 2026-11-07T16:20–16:40Z,
recorded in the run header). Check 6 (model smoke) executes before the matrix under this
amendment.

**Changes:**

1. **S1 (engine milestones) is dropped as an evidence source for this run.** Correct-arm
   evidence blocks come exclusively from **S2 repo ground truth**: `git log` decision-shaped
   commits (verbatim subject+body extracts), `docs/adr/*.md` paragraphs, `.scratch/*/PRD.md`
   and `issues/NN-*.md` Status/decision blocks, `docs/*.md` sentences — same §4.1 S3 rules
   (verbatim, ≤512-token cap, sha256 per block).
2. **Gold-label authority is unchanged** (S2 was always the gold authority) — but S2 is now
   also the evidence source, which removes the previous separation. **Consequence
   (anti-circularity rule):** a pair whose evidence block and gold rationale cite the **same
   single artifact** is unacceptably circular and is dropped. A pair is acceptable only when
   the gold rationale cites a **different artifact** than the evidence block (e.g. evidence =
   an ADR paragraph, rationale cites the commit/PR that implemented or refuted it).
3. **Donor blocks (unrelated arm) still come from a different project's shard via the engine**
   — §14 checks 2 and 4 passed; donors are available; §5 rules unchanged (scrub + tokenizer
   length matching).
4. **Everything else in the frozen protocol stands:** §6 stratification, §7 frozen parameters
   and prompts, §8 worksheet discipline, §9 seeded order (seed 20261107) and invalidity
   conditions, §10 scoring, §11 statistics, §12 decision rule, §13 artifacts, §15 boundaries.

---

*End of protocol. Frozen 2026-11-07. Executor: later coder session, after the engine silent-500
gate lands and §14 passes. Amended same-day per §16 (AMENDMENT-1) before any run.*
