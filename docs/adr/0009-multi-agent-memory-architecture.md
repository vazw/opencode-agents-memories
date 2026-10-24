# 0009 — Multi-Agent Memory: Tiered Architecture with Context Preservation and Agent Isolation

- **Status:** Proposed (draft for review)
- **Date:** 2026-10-22
- **Wave:** Design (pre-W5.3, feeds issues 21, 24, 26, 29, 31, 34, 35)
- **Supersedes:** nothing. **Amends:** ADR-0005 (episodic storage) and ADR-0006 (consolidation) — extends the three-tier model to five tiers and adds cross-tier promotion.
- **Source:** `wiki/synthesis/multi-agent-memory-design-opencode-rust-port.md` (synthesis of G-Memory, MIRIX, E-mem research)

## Context

The rust port's three-tier memory model (profile / facts / episodic) was designed
for single-agent operation. The multi-agent memory research (G-Memory, MIRIX,
E-mem) identifies that:

1. **Tier granularity is too coarse.** MIRIX demonstrates that 6 specialized
   memory types (Core, Episodic, Semantic, Procedural, Resource, Knowledge Vault)
   outperform flat 3-tier models. The port's "Facts" tier conflates semantic
   facts, procedural skills, and resource references.

2. **Context is destroyed by preprocessing.** E-mem shows that compressing
   episodic context into embeddings/graphs severs sequential dependencies
   ("destructive de-contextualization"), hurting System 2 reasoning. The port's
   consolidation pipeline (issue 16/17) may over-compress.

3. **Cross-trial learning is absent.** G-Memory shows that insights from one
   session should inform future sessions through cross-tier promotion. The port
   has no promotion pipeline from episodic → facts → profile.

4. **Agent isolation is undefined.** Multiple agents (main, explorer, reviewer,
   worker) share the same memory store with no access control. G-Memory and
   MIRIX both support per-agent memory views.

5. **No consistency protocol.** Concurrent writes to shared tiers have no
   conflict resolution strategy. No standard exists in the literature.

## Decision

### D-1 — Five-Tier Memory Model

Extend the three-tier model to five tiers:

| Tier | Name | Content | Lifecycle | Sharing |
|------|------|---------|-----------|---------|
| 1 | **Profile** | User preferences, identity, cross-session insights | Long-lived, slowly evolving | Shared across all sessions/agents |
| 2a | **Facts (Semantic)** | Project facts, conventions, decisions | Durable, updated on new evidence | Shared within project |
| 2b | **Facts (Procedural)** | Skills, workflows, tool patterns | Durable, versioned | Shared within project |
| 2c | **Facts (Resource)** | Code snippets, file refs, diagrams | Durable, content-addressed | Shared within project |
| 3 | **Episodic** | Session trajectories, raw interactions | Raw, segment-preserving | Per-session, promoted to Facts |
| 4 | **Vault** | Verbatim critical data (configs, exact code) | Immutable, content-addressed | Per-project scope, encrypted (W7.2; see ADR-0012) |

**W7.2 amendment (2026-10-23; UTC 2026-10-22) — vault sharing is per project.**
The Tier-4 cell read `Per-user, encrypted`; it now reads `Per-project scope,
encrypted`. The owner's vault-key ruling (one key per memory scope, at
`{storage_root}/vault-keys/{scope_hash}.key`) makes the per-user reading
unbuildable: libSQL's encryption is a whole-file codec with one key per file, so
a per-user vault shared across projects could not be opened by the per-scope
key, and per-project containment is the property the key scheme was chosen for.
Key custody, keyed content ids and rotation are decided in **ADR-0012
(Accepted)**; nothing else in this ADR changes.

**Rationale:** MIRIX's 6-type taxonomy provides finer granularity than 3 tiers.
The port's "Facts" tier splits into Semantic/Procedural/Resource to support
different retrieval patterns (concept lookup vs skill matching vs artifact
reference). Vault (MIRIX's Knowledge Vault) handles verbatim data that must
not be compressed.

**Amends ADR-0005:** The `episodic.db` table remains the storage for Tier 3.
New tiers (2b, 2c, 4) may use the same database with type-discriminated rows,
or separate files following the ADR-0005 precedent. Decision deferred to
implementation.

### D-2 — Context Preservation Strategy

Episodic memory (Tier 3) MUST preserve raw interaction segments, not just
summaries:

1. **Store raw, not just summaries.** The `agent_sessions` table already stores
   `summary` + `metadata` (JSON). Add a `raw_context` column (or separate
   segment table) for the full interaction trace.

2. **Segment-level activation.** On query, activate relevant segments rather
   than retrieving everything. Use the existing `semantic_timeline` and
   `recent_timeline` queries as the activation mechanism.

3. **Evidence extraction.** Before surfacing memory to the LLM, extract only
   the relevant evidence from each segment (not raw fragments). This can be
   done by a lightweight filter (rule-based or small model).

4. **Don't over-compress in consolidation.** The consolidation pipeline
   (issue 16/17) must preserve enough context for System 2 reasoning. Prefer
   segment-preserving consolidation over aggressive summarization.

**Rationale:** E-mem demonstrates that preprocessing destroys sequential
dependencies, hurting multi-hop and temporal reasoning. The port's
consolidation must balance compression with context preservation.

**Amends ADR-0006:** Consolidation must not destroy episodic context. Add a
`context_preservation` flag to consolidation config.

### D-3 — Cross-Tier Promotion Pipeline

Implement async promotion between tiers:

```
Session ends → Episodic segments stored (raw, uncompressed)
    ↓ (consolidation, background)
Facts extracted → Semantic/Procedural/Resource tiers
    ↓ (recurring patterns detected, background)
Profile insights updated → Cross-session generalization
    ↓ (verbatim critical data identified)
Vault entries created → Immutable, content-addressed
```

**Promotion rules:**
- **Episodic → Facts**: Extract facts, skills, and resource references from
  session segments. Dedup against existing facts.
- **Facts → Profile**: When a fact recurs across ≥3 sessions, promote to
  profile insight.
- **Any tier → Vault**: When data is identified as verbatim-critical (configs,
  exact code, API keys references), create an immutable vault entry.

**Implementation:** Background task (tokio) triggered after session close.
Not on the write path. Uses the existing consolidation scheduler.

**Rationale:** G-Memory's cross-trial learning enables "self-evolving"
agents. The port's consolidation (issue 16/17) should be extended to support
cross-tier promotion, not just within-tier consolidation.

### D-4 — Agent Isolation Model

Memory access is scoped by agent role:

| Agent Role | Profile | Facts (all) | Own Episodic | Other Episodic | Vault |
|------------|---------|-------------|--------------|----------------|-------|
| Main agent | R/W | R/W | R/W | R | R |
| Explorer | R | R | R/W | — | — |
| Reviewer | R | R | R (scoped) | R (scoped) | — |
| Background worker | — | R | — | — | — |

**Implementation:**
- Add `agent_id` and `role` to memory query parameters (API + storage).
- Enforce access control at the storage layer (not just API).
- Agent-scoped episodic: each agent writes only to its own session rows.
- Profile/Facts writes restricted to main agent; other agents propose
  (write to their episodic, promoted later).

**Rationale:** MIRIX's multi-agent architecture uses specialized managers
per type. G-Memory supports per-agent memory views. The port already has
`agent_id` in the scope triple (ADR-0005) — extend it to access control.

### D-5 — Active Retrieval

Before querying memory, generate a topic classification to route the query
to the appropriate tier(s):

1. **Topic generation:** Classify the query into one or more memory types
   (semantic, procedural, resource, episodic, profile).
2. **Tier routing:** Query only the relevant tiers, not all.
3. **Evidence extraction:** From retrieved segments, extract only the relevant
   evidence before surfacing to the LLM.

**Implementation:** Can be rule-based initially (keyword matching), upgraded
to LLM-based classification later. The `memory_filter` step in the retrieval
pipeline.

**Rationale:** MIRIX's Active Retrieval improves precision and reduces noise.
For the port, this reduces token cost (E-mem: 70% reduction) and improves
relevance.

### D-6 — Consistency Protocol

For concurrent writes to shared tiers (Profile, Facts):

1. **Write-ahead log (WAL):** All writes go through a WAL for crash recovery.
   (Already partially implemented via turso's WAL mode.)
2. **Optimistic concurrency:** Version field on shared tier rows. On conflict,
   re-read + retry. Max 3 retries, then fail.
3. **Agent-scoped writes:** Only the main agent writes to Profile/Facts.
   Other agents write to their own Episodic (no conflict).
4. **Promotion is async:** Cross-tier promotion happens in background, not
   blocking the write path. Promotion conflicts resolved by "latest wins"
   with audit log.

**Rationale:** No standard exists for multi-agent memory consistency (open
problem). This approach balances simplicity (optimistic) with correctness
(WAL + versioning). The agent-scoped write rule eliminates most conflicts.

### D-7 — Privacy Model

Extend the existing privacy infrastructure:

1. **`<private>` regions:** Already implemented in `mem-ai::privacy/tags.rs`.
   Extend to all tiers — private regions are stripped on export and not
   surfaced in retrieval.
2. **Vault encryption:** Knowledge Vault (Tier 4) entries encrypted at rest
   using the port's existing encryption (ADR-0004).
3. **Agent isolation:** Agent-scoped episodic memory not readable by other
   agents (D-4).
4. **Export/import:** Strip private regions on export (already in review
   findings for issue 26 F4). Add vault export as separate opt-in.

**Rationale:** MIRIX's Knowledge Vault handles verbatim sensitive data.
The port's existing privacy tags provide the foundation. Issue 26's review
(F4) already identified the export privacy gap.

## Consequences

### Positive

- **Finer-grained retrieval:** Splitting Facts into Semantic/Procedural/Resource
  enables type-specific retrieval patterns.
- **Context preservation:** Raw episodic segments improve multi-hop and
  temporal reasoning.
- **Cross-session learning:** Promotion pipeline enables self-evolving agents.
- **Agent safety:** Role-scoped access prevents unauthorized memory reads.
- **Token efficiency:** Active retrieval + evidence extraction reduce cost.

### Negative

- **Schema complexity:** 5 tiers vs 3 requires more storage and migration work.
- **Consolidation changes:** Must preserve context, not just compress.
- **Access control overhead:** Every query must check agent role.
- **Promotion pipeline:** Background task adds complexity to the scheduler.

### Neutral

- **Backward compatible:** Existing 3-tier data maps to new tiers (Facts →
  Semantic, Episodic → Episodic, Profile → Profile). No data migration needed
  for existing rows.
- **Incremental adoption:** Tiers 2b, 2c, 4 can be added incrementally.
  Start with context preservation (D-2) and agent isolation (D-4), which
  have the highest impact-to-effort ratio.

## Implementation Priority

| Priority | Decision | Issue | Effort |
|----------|----------|-------|--------|
| 1 | D-2 (Context preservation) | 21 (W5.3) | Medium |
| 2 | D-4 (Agent isolation) | 24 (W6.2) | Medium |
| 3 | D-7 (Privacy model) | 26 (W6.4) | Low (extends existing) |
| 4 | D-5 (Active retrieval) | 31 (W6.7) | Medium |
| 5 | D-3 (Cross-tier promotion) | 16/17 (consolidation) | High |
| 6 | D-1 (Tier refinement) | 31 (W6.7) | High (schema changes) |
| 7 | D-6 (Consistency protocol) | 24 (W6.2) | Medium |

## Follow-ups

- **Issue 21 (W5.3):** Session capture must store raw context segments, not
  just summaries. Add `raw_context` column to `agent_sessions` table.
- **Issue 24 (W6.2):** API handlers must accept `agent_id` and `role`
  parameters for scoped queries.
- **Issue 26 (W6.4):** Export/import must strip `<private>` regions and
  support vault entries. Add procedural tier to export.
- **Issue 29 (W6.5):** Explorer UI should show agent-scoped memory views.
- **Issue 31 (W6.7):** Core memory screens may need expansion for new tiers.
- **Consolidation (16/17):** Extend to support cross-tier promotion and
  context preservation.
- **ADR-0005 amendment:** Document the 5-tier model and storage strategy.
- **ADR-0006 amendment:** Document context-preserving consolidation.

## References

- G-Memory: Zhang et al. 2025, NeurIPS 2025 (arXiv:2506.07398)
- MIRIX: Wang et al. 2025 (arXiv:2507.07957)
- E-mem: Wang et al. 2026, ICML 2026 (arXiv:2601.21714)
- Memory Architectures Research Report: `output/memory-architectures-research-report.md`
- Multi-Agent Systems Research Report: `output/multi-agent-systems-research-report.md`
- ADR-0005: Episodic storage model
- ADR-0006: Episodic consolidation
- ADR-0007: Bridge strategy (mem-bridge / mem-web dependency edge)