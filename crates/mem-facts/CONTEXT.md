# mem-facts — domain context

**Purpose:** the durable five-tier facts DB (`facts.db`): atomic facts with a
project scope and a tier discriminator, per-project/per-tier key dedup,
per-session provenance, and the explicit owner-initiated erasure path.

## Ubiquitous language

- **Fact** — one atomic statement in one of the three fact tiers
  (`semantic`, `procedural`, `resource`). Its id is deterministic:
  `"fact-"` + 32 hex of `sha256(project_id | tier | normalized key)`.
- **Key** — `mem_core::dedup::normalize_key(content)` unless the producer
  supplies one. The unique constraint is
  `(project_id, tier, key_normalized)`: dedup is per project and per tier.
- **Source ref** — the milestone id whose consolidation produced the fact.
  It is authoritative and the reason a fact can be purged by milestone, and
  it is the milestone projection's only join key (`LIST_FACTS_BY_MILESTONE`).
  Provenance is not a substitute: a `fact_provenance` row is per contributing
  session and is written `INSERT … ON CONFLICT DO NOTHING`, so it names the
  FIRST promoting milestone, while `facts.source_ref` names the run that
  produced the current content.
- **Provenance** — `fact_provenance(fact_id, project_id, session_id,
  agent_id, milestone_id)`: one row per contributing session. A fact that
  came from two sessions has two rows, and purging either session deletes
  the fact (and all of its provenance rows) on purpose — erasure wins over
  durability. The shared milestone is deleted with it, so the surviving
  contributor's summary is lost permanently; recovery is an explicit
  re-capture of that session.
- **Soft delete** — `soft_delete(id)` hides a fact from live reads while
  keeping the row. It is the opposite of erasure.
- **Purge** — `purge_sources(project, session, agent, milestone_ids)`, the
  ONE hard delete. It is explicit, owner-initiated, project-scoped, counts
  distinct rows measured from the union of its predicates **before** any
  mutation, and sweeps provenance only inside the project it is erasing.

## Invariants

- One `facts.db` per storage root; rows are `project_id`-scoped.
- `mem_facts::FactsStore` is **not** `mem_portability::facts::FactStore`
  (the tier-1 memory seam). The names differ by design; do not conflate them.
- The id digest is over the normalized KEY, never over content (Q2b's
  keyed-HMAC rule is vault-only, but a raw content digest would still be a
  confirmation oracle).
- A purge writes **no tombstone**: after it, the same triple/key is a normal
  write. `soft_delete` is the only tombstone-shaped state, and it exists to
  make reads fail, not to erase.
- No fact is ever hard-deleted by consolidation or any lifecycle stage.

## Boundaries

- May depend on `mem-core` and `mem-store` only (ADR-0001).
- `mem-vault` and its keyed HMAC content ids land in W7.2; this crate ships
  no vault code.
- Fact embeddings, `model_id`/`stale` population and retrieval are Phase 2
  or later (no producer this wave).
