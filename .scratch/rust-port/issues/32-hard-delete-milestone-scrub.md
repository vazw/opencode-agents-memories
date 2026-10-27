# 32 — W4.4 · Hard delete & milestone scrub (retention semantics)

Status: ready-for-agent — 2026-10-27 (re-verified at `8023dd8`: the tombstone delete, the verbatim
`Contributor.summary` and the whole-milestone purge are all real. Owner ruled that the scrub keeps the
contributor entry with an empty summary, amending issue 43 decision 9. Brief under `## Agent Brief`.)

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
