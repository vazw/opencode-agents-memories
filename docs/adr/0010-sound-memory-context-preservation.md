# 0010 — Sound Memory: Context Preservation as a Core Design Principle

- **Status:** Proposed (draft for review)
- **Date:** 2026-10-22
- **Wave:** Design (pre-W5.3, feeds issues 16, 17, 21, 24, 26, 31)
- **Supersedes:** nothing. **Amends:** ADR-0005 (episodic storage) and ADR-0006 (consolidation) — adds context preservation as a core invariant.
- **Source:** `wiki/synthesis/cross-domain-connections-books-agents.md` (Sound Memory thesis, derived from Bitcoin's sound money principles via When Money Dies, Broken Money, Layered Money, plus agent memory research: E-mem, SimpleMem, Goyal et al. 2026)

## Context

The rust port's memory system faces the same fundamental tension as monetary systems: **compression vs. preservation**. Just as fiat money printing debases purchasing power, over-compression of memory debases reasoning quality. The "Sound Memory" thesis, derived from Bitcoin's sound money principles, provides a coherent design philosophy:

> **Memory systems that "print" too much (over-compression, hallucination) destroy value just like fiat money printing. Context preservation is the "sound money" of memory.**

This thesis maps across four dimensions:

| Dimension | Fiat Money | Agent Memory |
|-----------|-----------|--------------|
| Supply control | Fixed supply (Bitcoin) vs. unlimited (fiat) | Raw context (fixed) vs. compressed (inflated) |
| Value preservation | Sound money preserves purchasing power | Raw context preserves reasoning quality |
| Debasement | Printing money debases value | Over-compression debases reasoning |
| Collapse | Hyperinflation (Weimar) | Hallucination, "lost in the middle" |

The port already has emerging support for this principle (ADR-0005's raw context, ADR-0009 D-2's context preservation), but it is not yet a **core invariant**. This ADR formalizes it.

## Decision

### D-1 — Raw Context Is a Core Invariant

**Every memory entry MUST store raw context as its primary representation.** Compression (embeddings, summaries) is derived and regenerable. Raw context is never destroyed.

**Schema change:**
```sql
-- Add raw_context to agent_sessions (non-nullable)
ALTER TABLE agent_sessions ADD COLUMN raw_context TEXT NOT NULL DEFAULT '';

-- Add content + derived to all memory entry types
-- (content is primary, derived is secondary)
```

**Rationale:** E-mem demonstrates that preprocessing destroys sequential dependencies ("destructive de-contextualization"). SimpleMem shows that lossless compression can reduce tokens by 30× while maintaining accuracy — but only if raw context is preserved as the source of truth. Goyal et al. 2026 shows that Knowledge Graph format survives model upgrades (95%) while embeddings don't (0%).

**Amends ADR-0005:** `raw_context` becomes a required column, not optional. The `summary` column becomes derived (regenerable from raw_context).

### D-2 — Compression Is Derived, Not Primary

All compressed representations (embeddings, summaries, knowledge graphs) are **derived data** with:
- `model_id`: which model generated them
- `stale`: whether they need regeneration (set to true on model upgrade)
- `generated_at`: when they were created

**On model upgrade:**
1. Keep all primary data (raw_context, content)
2. Mark derived data as stale
3. Background task regenerates derived data with new model
4. Serve queries from primary + stale derived (degraded but functional)
5. Full quality restored when regeneration completes

**Rationale:** Portability (Problem #8). Raw text is universally readable; embeddings are model-specific. Storing derived data as secondary allows graceful degradation and regeneration.

### D-3 — Compression Budget Policy (Tier-Specific)

Compression is applied differently per tier, based on information type:

| Tier | Compression Level | Strategy | Rationale |
|------|------------------|----------|-----------|
| **Profile (Tier 1)** | Summarize (L2) | Lossy compression | Stable, compressible, changes slowly |
| **Facts (Tier 2)** | Lossless (L1) | Atomic facts, dedup | Must preserve exact information |
| **Episodic (Tier 3)** | Segment-preserving (L4) | Raw segments, activate on demand | Must preserve sequential dependencies |
| **Vault (Tier 4)** | None (L0) | Verbatim, immutable | Exact bytes must be preserved |

**Rationale:** The "compression safety test" (Problem #7): sequential order matters for episodic, exact wording matters for vault, stability allows compression for profile.

### D-4 — Verification Against Raw Source

Every memory operation can be audited against raw context:
- **Retrieval:** Retrieved facts must be traceable to raw source
- **Consolidation:** Promoted facts must be verifiable against raw episodic
- **Conflict resolution:** Conflicting facts must be resolvable by reference to raw source
- **Audit log:** All operations record the raw source reference

**Implementation:** Add `source_ref` field to all derived data, pointing to the raw context entry it was derived from.

**Rationale:** "Don't trust, verify" (Bitcoin principle). Summaries and embeddings can be wrong; raw context is the ground truth.

### D-5 — Segment-Level Isolation (E-mem Pattern)

Episodic memory is stored as raw segments, not monolithic summaries:
- Each session is broken into segments (by topic, time, or tool use)
- Segments are stored raw (uncompressed)
- On query, relevant segments are activated (not all segments retrieved)
- Assistant agents (or rules) process segments locally, surfacing only evidence

**Rationale:** E-mem demonstrates that segment-level preservation avoids "destructive de-contextualization" while reducing token cost by 70% (only evidence surfaces to the master agent).

### D-6 — Scarcity of Compression

Compression is not free. It costs:
- **Compute:** Embedding generation, summarization
- **Information loss:** Even "lossless" compression loses some fidelity
- **Regeneration cost:** On model upgrade, all derived data must be regenerated

**Policy:** Compress only when necessary. Prefer raw storage. Use compression for:
- Profile (stable, compressible)
- Facts (lossless, for retrieval efficiency)
- NOT episodic (preserve raw)
- NOT vault (verbatim)

**Rationale:** "Scarcity of compression" mirrors Bitcoin's scarcity principle. Compression should be a deliberate, justified choice, not a default.

## Consequences

### Positive

- **No data loss on model upgrade:** Raw context survives; derived data regenerates
- **Better reasoning quality:** Raw context preserves sequential dependencies
- **Auditable memory:** All operations traceable to raw source
- **Clear decision framework:** "Does this preserve or destroy information?" answers most design questions
- **Graceful degradation:** Stale derived data still usable during regeneration

### Negative

- **Higher storage cost:** Raw context + derived data = more storage
- **Regeneration cost:** Re-embedding on model upgrade takes time and compute
- **Schema changes:** `raw_context` column, `derived` field, `source_ref` field

### Neutral

- **Backward compatible:** Existing data maps (summary → derived, new raw_context column)
- **Incremental adoption:** Can add raw_context to new entries first, backfill later
- **Aligns with existing ADRs:** ADR-0005 (episodic storage), ADR-0006 (consolidation), ADR-0009 (multi-agent memory)

## Implementation Priority

| Priority | Decision | Issue | Effort |
|----------|----------|-------|--------|
| 1 | D-1 (Raw context invariant) | 21 (W5.3) | Medium (schema + storage) |
| 2 | D-3 (Compression budget policy) | 16/17 (consolidation) | Low (config change) |
| 3 | D-2 (Derived data tracking) | 26 (W6.4) | Medium (schema + export) |
| 4 | D-4 (Verification against raw) | 24 (W6.2) | Medium (audit trail) |
| 5 | D-5 (Segment-level isolation) | 21 (W5.3) | High (segment storage) |
| 6 | D-6 (Scarcity of compression) | All | Low (policy documentation) |

## Follow-ups

- **Issue 21 (W5.3):** Session capture must store raw_context (non-nullable). Context injection must preserve raw segments.
- **Issue 16/17 (consolidation):** Add `context_preservation` flag. Never destroy raw context. Compression is tier-specific.
- **Issue 26 (W6.4):** Export/import must include raw context + derived staleness. Regenerate derived on import.
- **Issue 24 (W6.2):** API must support raw context retrieval. Add `source_ref` to derived data.
- **Issue 31 (W6.7):** UI should show raw context alongside compressed views. "Sound memory" dashboard.
- **ADR-0005 amendment:** `raw_context` becomes required column.
- **ADR-0006 amendment:** Consolidation must preserve context; compression is tier-specific.

## References

- E-mem: Wang et al. 2026, ICML 2026 (arXiv:2601.21714) — segment-preserving reconstruction
- SimpleMem: Liu et al. 2026 — semantic lossless compression, 30× token reduction
- Goyal et al. 2026 — "What Does Context Compression Cost?" (format-dependent survival)
- When Money Dies: Fergusson 1975 — Weimar hyperinflation (fiat debasement analogy)
- Broken Money: Alden 2023 — financial system failures
- Layered Money: Bhatia 2021 — layered money pyramid, Bitcoin as base money
- ADR-0005: Episodic storage model
- ADR-0006: Episodic consolidation
- ADR-0009: Multi-agent memory architecture
- Open Problem #7: Context preservation vs compression
- Open Problem #8: Memory portability
- Open Problem #21: Forgetting paradox