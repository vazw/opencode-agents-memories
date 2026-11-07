# FactKG transfer experiment — gold worksheet (FROZEN 2026-11-07T18:40Z)

Frozen before run 1 per protocol §8, under AMENDMENT-1 (§16): correct-arm evidence from S2 repo
ground truth only; every gold rationale cites an artifact **different** from the pair's evidence
block (anti-circularity rule; same-file different-clause rationales are noted in the row).
Model verified at freeze: `opencode-go/beam-free` (READ `~/.config/opencode/opencode-mem.jsonc`,
unchanged from the 2026-11-07T16:20Z pre-flight read).

Full verbatim blocks + sha256s: `.tmp/scratch/factkg-experiment/freeze.json` (regenerable via
`freeze.py`; hashes bind exact bytes). Token counts: Xenova/nomic-embed-text-v1 WordPiece,
content tokens (CLS/SEP excluded), both arms matched by the same tokenizer (§5.2 primary).
Donor rows are content-addressed (`project#<content-hash12>`); a donor reused across pairs is
re-lengthened per pair (§5.1) and marked `reused`.

**Stratification as frozen (§6-exact):** 10 supported / 6 contradicted / 4 unknown;
conjunction-structure: 4 (4 inside supported); negated-style: 4.
The contradicted slots are explicit entity/polarity flips of recorded decisions (FactKG §3.1
substitution style); the unknown slots are genuine repo gaps, not missing-evidence stand-ins.

| pair | claim (verbatim) | evidence ref (S2 artifact) | evidence sha256 | donor ref (project#content-hash) | gold | rationale artifact (≠ evidence) | style | scrub |
|---|---|---|---|---|---|---|---|---|
| P01 | The port's episodic sessions are deduplicated by the triple of project, session, and agent identifiers. | docs/adr/0005-episodic-storage-model.md L22-28 | `5b66d4fa40a2e247…` | `antwall#56054af564b5` (75 tok) · reused | supported | crates/mem-store/src/episodic_sql.rs:20-33 (agent_sessions DDL declares UNIQUE (project_id, session_id, agent_id)) | one-hop | P-A ok / P-B ok |
| P03 | The vault tier keeps one key per project scope, and one encrypted database file per scope. | docs/adr/0012-vault-key-custody.md L8-12 | `c37a6ae064cb9206…` | `antwall#3bba28cb34cd` (103 tok) · reused | supported | crates/mem-vault/CONTEXT.md Purpose (one key per scope hash, one encrypted vault.db per scope) | conjunction | P-A ok / P-B ok |
| P04 | The tag search repair shipped a server-side free-text filter parameter, and the alternative picker that filters projects was rejected. | .scratch/rust-port/issues/123-web-ui-tag-search-and-dashboard.md L8-9 | `b768d4d879b3a4d8…` | `void-packages#a95a2b5cdb8a` (62 tok) | supported | .scratch/rust-port/issues/123-web-ui-tag-search-and-dashboard.md Status (A1 chosen (`tags=`), merged 3476061) + owner ruling paragraph (A2 rejected) | conjunction+negated | P-A ok / P-B ok |
| P05 | The memory injection feature preserves conversation context across compaction, and the wrapper emits telemetry about inject delivery. | .scratch/rust-port/issues/124-memory-context-inject-intermittent-miss.md L3-4 | `34c847ed7ec6e0c8…` | `antwall#7a733845e297` (155 tok) | supported | commits 33e9da9 (preserve same-turn context across compaction) and 620cab9 (emit privacy-preserving chat inject telemetry) | conjunction | P-A ok / P-B ok |
| P08 | The hundred-millisecond search budget fails at both ten-times and hundred-times corpus scale, and the smaller shard-cap value does not repair the failure. | docs/adr/0024-brute-force-scan-cost-measured.md L33-38 (Result table) | `74b322b0e84c3a79…` | `opencode#f15f4308ab68` (98 tok) · reused | supported | docs/evidence/issue-43-w8-benchmark-20261030/raw/*.summary.json + README.md (the measured artifacts the ADR's Evidence line cites) | conjunction | P-A ok / P-B ok |
| P10 | The capture summarizer now runs against a stateless text-generation route instead of creating and deleting a session. | .scratch/rust-port/issues/108-capture-should-use-stateless-generate.md L3 | `98de5149da1beb88…` | `antwall#c03600d12160` (27 tok) | supported | docs/evidence/stateless-capture-2026-11-02/ (post-restart log evidence: captures persisted against the stateless route) | one-hop | P-A ok / P-B ok |
| P12 | A permanent failure class is dead-lettered on its first drain without burning the attempt ladder. | .scratch/rust-port/issues/107-deterministic-503-is-retried.md L9-17 | `474fc26a881f3a21…` | `antwall#881983c51012` (91 tok) | supported | crates/mem-bridge/src/capture.rs:1060-1075 (branch order is load-bearing: a permanent class dead-letters on the first drain before any attempt bump) | two-hop | P-A ok / P-B ok |
| P13 | The consolidation age window defaults to thirty days. | docs/adr/0006-episodic-consolidation.md L17-22 | `b9ccffcaf98fa95d…` | `antwall#a8f028dd8ef4` (110 tok) | supported | crates/mem-core/tests/config_defaults.rs:224-226 (default-config test asserts episodic.consolidation_age_days == 30) | one-hop | P-A ok / P-B ok |
| P14 | A purge scrubs a surviving milestone's retained contributor text instead of deleting the milestone row, because the deletion would destroy a co-contributor's durable knowledge. | git commit b46ef0f (subject+body) | `89f829b3785a53c3…` | `antwall#71268ab67246` (374 tok) | supported | .scratch/rust-port/issues/32-hard-delete-milestone-scrub.md (owner ruling: scrub keeps only attribution; deleted text must not survive inside a milestone) | one-hop | P-A ok / P-B ok |
| P16 | The pre-commit gate costs about thirty-five seconds on a cold target directory. | docs/adr/0016-pre-commit-gate.md L129-133 | `9f1d7bc7b22a7153…` | `antwall#47440183d9c1` (163 tok) | unknown | docs/adr/0016-pre-commit-gate.md Context (root gates measured exit 0: 8 s + 25 s; AGENTS.md Commands records the same warm figure; no cold figure is recorded anywhere) | one-hop | P-A ok / P-B ok |
| P17 | The consolidation scheduler ships in the very first consolidation wave. | docs/adr/0006-episodic-consolidation.md L30-36 (D-18) | `d79611de872dea3a…` | `antwall#663ba75a60dc` (84 tok) | contradicted | crates/mem-bridge/src/engine.rs:1655-1661 (ConsolidationScheduler::new with Duration::from_secs(CONSOLIDATION_INTERVAL_SECS), constant = 3600, added by the Q8 wave long after W4.3) | one-hop | P-A ok / P-B ok |
| P18 | The port chose the dedicated episodic database file over a memory shard or the shared auxiliary sessions database. | docs/adr/0005-episodic-storage-model.md L22-28 | `5b66d4fa40a2e247…` | `antwall#56054af564b5` (75 tok) · reused | supported | MEASURED 2026-11-07 pre-flight: the live storage root runs episodic.db (agent_sessions/episodic_milestones tables) beside the per-project shard dbs | one-hop | P-A ok / P-B ok |
| P19 | The web explorer's own engine install needs the Intel Mac binding after all. | docs/adr/0007-bridge-strategy.md L77-80 | `abbe87396463744b…` | `antwall#d8b6fe3c0e49` (139 tok) | contradicted | plugin/scripts/stage-engine.sh:60-68 (uname case; unsupported host OS/arch exits 4) + plugin/src/plugin.js degrade paths (D-38 fail-open, no download) | one-hop+negated | P-A ok / P-B ok |
| P21 | The consolidation output is one milestone row per project, covering every agent together. | docs/adr/0006-episodic-consolidation.md L17-22 | `b9ccffcaf98fa95d…` | `antwall#f64162b18828` (110 tok) | contradicted | docs/adr/0006-episodic-consolidation.md D-18 (milestones are per (project_id, agent_id), per-agent NOT per-project) — a different clause of the same ADR is not the cited block; block is the W4.3 paragraph, rationale cites D-18 | one-hop | P-A ok / P-B ok |
| P22 | The benchmark's verdict was that the narrower table layout won the comparison and was adopted. | docs/adr/0024-brute-force-scan-cost-measured.md L33-38 (Result table) | `74b322b0e84c3a79…` | `opencode#f15f4308ab68` (98 tok) · reused | contradicted | docs/adr/0024-brute-force-scan-cost-measured.md Result paragraph (first-match lands on D4 NEW-ADR, no code ships) + D-24-2 (rejected on this evidence) — distinct clauses from the D1/D2 table block | one-hop+negated | P-A ok / P-B ok |
| P23 | The upstream dependency gap means the web explorer's engine install has no Intel Mac limitation at all. | docs/adr/0007-bridge-strategy.md L77-80 | `abbe87396463744b…` | `antwall#e248233d7f22` (139 tok) | contradicted | docs/adr/0007-bridge-strategy.md D-38/g (the darwin/x64 path degrades, never improvises) + plugin/scripts/stage-engine.sh:60-68 — the amendment note block settles the opposite | one-hop+negated | P-A ok / P-B ok |
| P24 | A vault purge erases every byte of the purged content from the storage medium. | docs/adr/0012-vault-key-custody.md L525-529 (D-14) | `673e3c1a0246bfd3…` | `antwall#32ab3cb0aacb` (80 tok) | contradicted | docs/adr/0012-vault-key-custody.md D-14 residuals (page images and WAL may retain ciphertext until a checkpoint; nothing in the purge forces one) | one-hop | P-A ok / P-B ok |
| P25 | Before the calibration, the wrapper waited thirty seconds for the engine's shutdown acknowledgement. | docs/evidence/issue-105-shutdown-escalation/README.md L3 | `bdef1c2570396d61…` | `antwall#63617261cd18` (18 tok) | unknown | plugin/src/plugin.js:52-54 (SHUTDOWN_RESPONSE_TIMEOUT_MS = 10000 now; issue-105 records the measured 5-6 s acks against the old 2000 ms budget — the thirty-second figure appears nowhere) | one-hop | P-A ok / P-B ok |
| P26 | Vault key material is not stored inside the memory shards themselves. | docs/adr/0012-vault-key-custody.md L8-12 | `c37a6ae064cb9206…` | `antwall#3bba28cb34cd` (103 tok) · reused | unknown | docs/adr/0012-vault-key-custody.md D-1 custody table (the scope key is a standalone 0600 FILE at {storage_root}/vault-keys/{scope_hash}.key outside the data tree, written by the key custodian — never through the shard writers) — block is the D-rulings header, rationale cites D-1's custody clause | one-hop | P-A ok / P-B ok |
| P27 | The narrow layout's on-disk size was measured smaller than the inline layout at the largest scale. | docs/adr/0024-brute-force-scan-cost-measured.md L60-64 (D-24-2) | `db29fe982b40b154…` | `void-packages#9c2252b804e7` (136 tok) | unknown | docs/adr/0024-brute-force-scan-cost-measured.md D-24-2 (on disk it is 13.8%/14.1% LARGER than inline) — the size comparison clause settles the opposite | one-hop | P-A ok / P-B ok |

## Scrub record (P-A automated grep, P-B human read)

P-A: automated case-insensitive grep over every claim, evidence block and donor block for the
§4.3 checklist patterns (P1 credential fragments, P2 key-file paths, P3 URLs, P4 internal
hosts/addresses, P5 emails/home paths, P8 token prefixes) — zero hits across all 27 blocks and
claims (machine-checked in freeze.py; `scrub_pa_hits` empty for every row).

P-B: human read pass applied at freeze to every claim and evidence block (each block read in
full; claims authored to share no distinctive span with their block); donor blocks additionally
screened for topical unrelatedness (no shared ≥5-char alphanumeric tokens with their claim).
Sign-off: all rows `P-A ok / P-B ok`. Seven candidate pairs (P02, P06, P07, P09, P11, P15, P20
from the drafting pass) were trimmed to meet the §6 roster exactly (17 supported drafted, 10
kept); the trim and its reason are recorded here and in the ledger. No pair was dropped for
scrub or gold-ambiguity reasons.
