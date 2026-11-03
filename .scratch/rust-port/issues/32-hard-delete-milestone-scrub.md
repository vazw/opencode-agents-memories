# 32 — W4.4 · Hard delete & milestone scrub (retention semantics)

Status: ready-for-human — 2026-11-03: AC1/AC2/AC3 landed; AC4 partial (the smoke doc half is missing);
AC5/AC6 and residual 3 deferred to the retention ruling (ADR-0010 D-1); residuals 1/2/4 settled, 5/6 open.

## What to build

Make deletion mean what it says. Under the thesis adopted on 2026-10-21, the episodic tier is **staging**, and the user's ruling that day was: *deleting a session removes the row (text + embedding) **and** scrubs the retained summary text from any milestone's `contributors` JSON, keeping only the attribution (who/when).* "I deleted it, it is gone" must be true, and staging rows must be able to age out.

Today it is not true: `Contributor.summary` retains the source session's summary **verbatim (D-27)** (`crates/mem-episodic/src/milestone.rs`), so a deleted session's text survives inside a consolidated milestone; and the only delete path is W4.2's `soft_delete_session` (tombstone + `MARK_CHILDREN_ORPHANED` in one atomic batch), which keeps the row's text and embedding on disk.

This wave delivers: the hard-delete path, the milestone scrub, and the age-based purge of staging rows that uses the same machinery.

**Open residuals this wave must settle or record (the user's ruling covered the row + the contributors' text; the rest is still open):**
1. **The milestone's own merged `summary` and its embedding — RESOLVED (user, 2026-10-21): KEPT.** The merged summary is a separate durable artifact and survives; only the contributors' retained summaries are scrubbed. No re-summarization, no re-embed, no loss of the consolidated recap.
2. **Tier-1 facts promoted from that session — RESOLVED (user, 2026-10-21): KEPT.** A promoted fact is durable knowledge the user never deleted, and it carries no session link today (the fact id is `sha256(project_id + "\x1f" + key)`), so there is no provenance to act on. The distinction "the session is gone; the knowledge it produced remains" is the product's stated semantic.
3. **The delete path must state that distinction** — the UI (and the delete response) must not imply that deleting a session removes the promoted facts or the consolidated milestone recap. Make it explicit where the user acts.
4. **Orphan semantics under a hard delete:** `agent_sessions.parent_session_id` carries `ON DELETE SET NULL`, so a hard delete severs the children's parent link while their `orphaned` flag (set by the same atomic batch) survives — consistent with D-12 ("the parent link was severed by a delete"). Confirm and prove it.
5. **The age threshold** for purging staging rows, and the interaction with the W4.3 consolidation pipeline (a row must not be purged before it is consolidated into a milestone, or its content is lost rather than promoted).
6. **The stale-vector interaction:** compaction strips a row's text to a bounded form (`stripped_source_summary`) but nothing re-embeds episodic rows, so an aged row's vector describes text it no longer contains — the semantic leg of the injection (W5.3) can retrieve such a row on a stale vector.

Wave detail: `.scratch/rust-port/roadmap.md` §W4.4.

## Acceptance criteria

- [ ] A hard delete removes the session row's text and embedding from the database — proven by a direct query against the store, not by an API assertion alone.
- [ ] After the delete, every milestone that lists the session as a contributor keeps the attribution (agent id, agent name, created_at) and carries an **empty** summary for it.
      **Ruling (owner, 2026-10-27):** this keeps the contributor **entry** with an empty `summary`. It
      **amends issue 43 decision 9**, which said "rewrite the contributor set … removing *that*
      contributor's entry" — removing the entry would destroy the attribution the 2026-10-21 ruling
      promised to keep. Text follows text; who/when is not text. The implementation therefore rewrites
      `contributors[i].summary = ""` in place and keeps the row.
- [ ] The children of a deleted session stay visible in the timeline, flagged `orphaned: true`, with the parent link severed per the FK's `ON DELETE SET NULL` — the W4.2 orphan guarantee is preserved, not weakened.
- [ ] The residuals above are each settled or explicitly recorded as open (with the user's ruling quoted), and any accepted residue is documented in the smoke doc and the issue's `## Comments`.
- [ ] An age-based purge of staging rows exists, is bounded, and never purges a row that has not been consolidated into a milestone (no silent content loss) — with the threshold in the store config, not hard-coded.
- [ ] The D-8 decision is closed in the tracker and in the ADR trail: the canonical triage/retention rule for episodic rows is written down, and `docs/agents/` and the smoke doc agree with it.

## Blocked by

- 15 · W4.1 (the episodic schema) — closed
- 16 · W4.2 (timeline, delete semantics, orphan rules) — closed
- 17 · W4.3 (consolidation → milestones, which is what the scrub writes back) — closed

## Out of scope

- The tier-1 → tier-2 retention/cleanup policy (W3.3 owns the confidence/decay cleanup).
- Any change to the W6.2 REST contract (the delete endpoint's 404 semantics are unchanged).
- The Agents contribution view (issue 30 / W6.6) and the explorer screens (issue 31 / W6.7) — they consume the read paths this wave leaves intact.

## Comments

- **2026-10-27 — triaged: partial and superseded in part.** The user-facing delete
  (`DELETE /api/projects/{id}/sessions/{id}`) still calls `soft_delete_session`, so text and embedding survive.
  The CLI `purge_session` does hard-delete, but its milestone behaviour is the **opposite** of this issue's
  acceptance: it deletes the whole milestone row and thereby destroys a co-contributor's retained summary.
  Issue 43 decision 9 now mandates rewriting the contributor set instead. Re-scope this issue against that
  decision before planning; the two cannot both be implemented.

- **2026-11-03 — wave 1 (deletion + attribution) landed AC1/AC2/AC3; the rest is named, not claimed.**
  - **AC1 landed.** `crates/mem-episodic/tests/hard_delete.rs` proves the erasure with a direct
    `SELECT id, summary, length(embedding) FROM agent_sessions WHERE id = ?` returning 0 rows — not an API
    assertion — plus `raw_context_for_session` → `None`, `segments_for_session` → empty and no
    `capture_inbox` row. The path is `EpisodicStore::hard_delete_session`
    (`crates/mem-episodic/src/purge.rs`, the allowlisted module; `store.rs` is inside the lexical barrier
    `crates/mem-bridge/tests/purge_cli.rs` guards, which is why it lives there).
  - **AC2 landed.** The milestone row survives: the contributor entry is kept with `summary == ""` and its
    attribution (`agent_id`, `agent_name`, `created_at`) intact; a co-contributor's summary is untouched.
    The recap columns (`summary`, `embedding`, `source_count`, `window_start`, `window_end`) are asserted
    byte-identical across the purge (owner ruling 2026-10-21: *no re-summarization, no re-embed*). The
    batch statement is `SCRUB_MILESTONE_CONTRIBUTORS`, and `EpisodicPurgeReport.milestones` is renamed
    `milestones_scrubbed` so the report cannot be read as a delete.
  - **AC3 landed.** `test_hard_delete_orphans_children_and_the_fk_nulls_the_link`: the child row survives
    with `orphaned == 1` and `parent_session_id IS NULL` (the FK's `ON DELETE SET NULL`), and the REST
    delete's `parentSessionId` is now `null`.
  - **AC4 partial.** The `## Comments` half is this entry. The **smoke-doc half is missing**: no smoke doc
    exists in the tree for this wave and none was created here. Recorded as the gap.
  - **AC5, AC6 and residual 3 deferred** together: all three are blocked on the retention ruling and
    ADR-0010 D-1 (*"no timer, no sweep, no flag"*), which `crates/mem-bridge/tests/purge_cli.rs`'s lexical
    barrier enforces. Residual 3 (the UI/delete-response copy) cannot state a distinction that has no rule
    yet.
  - **Residual 1 settled** (owner, 2026-10-21): the merged recap and its embedding are kept, and are now
    named in the CLI report's `not_erased()` as tier `milestones-recap` beside the legacy
    `memories-shard` — a survivor the report does not name is the same lie one level up.
  - **Residual 2 settled** (owner, 2026-10-21): facts are **kept**. The ruling's premise is corrected, not
    the ruling: this issue says a promoted fact "carries no session link today", but `fact_provenance`
    exists (`crates/mem-facts/src/schema.rs` writes one row per contributor) and the triple arm still
    erases session-sourced facts. The semantic that stands is *the session is gone; the knowledge it
    produced remains* — so the purge passes an empty milestone-id list to `purge_sources`, and
    `test_purge_keeps_a_fact_sourced_only_by_a_surviving_milestone` is its discriminator.
  - **Residual 4 settled** — proven by `test_hard_delete_orphans_children_and_the_fk_nulls_the_link` and the
    flipped REST assertion (`test_sessions_delete_is_orphan_safe`).
  - **Residuals 5 and 6 remain OPEN** (age threshold / consolidation interaction; stale vectors after
    compaction) — untouched by this wave.

## Agent Brief

> *This was generated by AI during triage.*

**Category:** bug
**Summary:** make deletion mean deletion — hard delete, contributor scrub, age-based staging purge.

**Current behaviour (verified at `8023dd8`):** the user-facing delete calls `soft_delete_session`
(`crates/mem-web/src/api/store_adapter.rs:781` → `crates/mem-episodic/src/store.rs:767`), so text and
embedding stay on disk, and `hard_delete_session` does not exist. `Contributor.summary` retains the source
summary verbatim (`crates/mem-episodic/src/milestone.rs:44`–`:46`, D-27). The CLI purge issues
`DELETE_MILESTONE` for every milestone containing the session (`crates/mem-episodic/src/purge.rs:206`–`:211`),
destroying co-contributors' retained text. The FK is `ON DELETE SET NULL`
(`crates/mem-store/src/episodic_sql.rs:31`).

**Desired behaviour:** a hard-delete path that removes the row's text and embedding (proven by a direct
store query, not an API assertion); a scrub that rewrites the matching contributor's `summary` to empty
**in place**, keeping the entry and its attribution (owner ruling 2026-10-27, amending 43 decision 9);
children orphaned per the FK; and an age-based purge whose threshold lives in store config and which never
purges an unconsolidated row.

**Key interfaces:** `EpisodicStore::soft_delete_session`, the new hard-delete path, `purge_session`'s
milestone branch, `Contributor`, `EpisodicPurgeReport`, the store's retention config.

**Acceptance criteria:** the issue's six ACs, with AC 2 as amended by the ruling above.

**Out of scope:** the REST contract's 404 semantics; tier-1 facts promoted from the session (kept); the
consolidated milestone's own summary (kept).
