# 32 — W4.4 · Hard delete & milestone scrub (retention semantics)

Status: needs-triage — 2026-10-27 (user-path delete is still a tombstone and the CLI purge destroys whole milestones — issue 43 decision 9 overrides this issue's scrub semantics, so it must be re-scoped)

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
