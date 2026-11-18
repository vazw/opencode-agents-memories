# 32 — W4.4 · Hard delete & milestone scrub (retention semantics)

Status: done — 2026-11-18. AC1/AC2/AC3 landed; AC4 complete (smoke doc at `docs/evidence/issue-32-hard-delete/README.md`); AC5 rewritten to what an owner ruling and a measurement support — dead staging rows are counted and reported, the deletion rule is written down in ADR-0033, and **no purge ships** because no digest or provenance link exists on this schema; AC6 done with ADR-0033. Issue 41 is closed (`abf2bce`) and did not satisfy AC5 (its sweep never opens `episodic.db`), so the deferral is retired rather than renewed. Residuals 1/2/3/4/5 settled, 6 open and recorded as a follow-up rather than a blocker.

## What to build

Make deletion mean what it says. Under the thesis adopted on 2026-10-21, the episodic tier is **staging**, and the user's ruling that day was: *deleting a session removes the row (text + embedding) **and** scrubs the retained summary text from any milestone's `contributors` JSON, keeping only the attribution (who/when).* "I deleted it, it is gone" must be true, and staging rows must be able to age out.

Today it is not true: `Contributor.summary` retains the source session's summary **verbatim (D-27)** (`crates/mem-episodic/src/milestone.rs`), so a deleted session's text survives inside a consolidated milestone; and the only delete path is W4.2's `soft_delete_session` (tombstone + `MARK_CHILDREN_ORPHANED` in one atomic batch), which keeps the row's text and embedding on disk.

This wave delivers: the hard-delete path and the milestone scrub. The third item — the age-based purge of staging rows — was **measured unprovable and does not ship** (2026-11-18, AC5 below; ADR-0033): dead staging rows are counted and reported instead.

**Open residuals this wave must settle or record (the user's ruling covered the row + the contributors' text; the rest is still open):**
1. **The milestone's own merged `summary` and its embedding — RESOLVED (user, 2026-10-21): KEPT.** The merged summary is a separate durable artifact and survives; only the contributors' retained summaries are scrubbed. No re-summarization, no re-embed, no loss of the consolidated recap.
2. **Tier-1 facts promoted from that session — RESOLVED (user, 2026-10-21): KEPT.** A promoted fact is durable knowledge the user never deleted, and it carries no session link today (the fact id is `sha256(project_id + "\x1f" + key)`), so there is no provenance to act on. The distinction "the session is gone; the knowledge it produced remains" is the product's stated semantic.
3. **The delete path must state that distinction** — the UI (and the delete response) must not imply that deleting a session removes the promoted facts or the consolidated milestone recap. Make it explicit where the user acts.
4. **Orphan semantics under a hard delete:** `agent_sessions.parent_session_id` carries `ON DELETE SET NULL`, so a hard delete severs the children's parent link while their `orphaned` flag (set by the same atomic batch) survives — consistent with D-12 ("the parent link was severed by a delete"). Confirm and prove it.
5. **The age threshold** for purging staging rows, and the interaction with the W4.3 consolidation pipeline (a row must not be purged before it is consolidated into a milestone, or its content is lost rather than promoted). — **SETTLED 2026-11-18: no age threshold ships, and none can.** The owner ruled a dead row dies only on proof its text reached memory; the measurement proved the proof unprovable on this schema, so nothing is purged and the consolidation interaction is moot (ADR-0033; residual 5 of the smoke doc).
6. **The stale-vector interaction:** compaction strips a row's text to a bounded form (`stripped_source_summary`) but nothing re-embeds episodic rows, so an aged row's vector describes text it no longer contains — the semantic leg of the injection (W5.3) can retrieve such a row on a stale vector.

Wave detail: `.scratch/rust-port/roadmap.md` §W4.4.

## Acceptance criteria

- [x] A hard delete removes the session row's text and embedding from the database — proven by a direct query against the store, not by an API assertion alone.
- [x] After the delete, every milestone that lists the session as a contributor keeps the attribution (agent id, agent name, created_at) and carries an **empty** summary for it.
      **Ruling (owner, 2026-10-27):** this keeps the contributor **entry** with an empty `summary`. It
      **amends issue 43 decision 9**, which said "rewrite the contributor set … removing *that*
      contributor's entry" — removing the entry would destroy the attribution the 2026-10-21 ruling
      promised to keep. Text follows text; who/when is not text. The implementation therefore rewrites
      `contributors[i].summary = ""` in place and keeps the row.
- [x] The children of a deleted session stay visible in the timeline, flagged `orphaned: true`, with the parent link severed per the FK's `ON DELETE SET NULL` — the W4.2 orphan guarantee is preserved, not weakened.
- [x] The residuals above are each settled or explicitly recorded as open (with the user's ruling quoted), and any accepted residue is documented in the smoke doc and the issue's `## Comments`.
- [x] **AC5 (rewritten 2026-11-18 — the age-based purge was measured unprovable and does not ship).** Dead-lettered staging rows are counted and reported, and the deletion rule
      is written down (ADR-0033): a dead row may be deleted only on proof its text reached memory, never on
      triple identity, never otherwise. No digest or provenance link exists on this schema. On the 2026-11-18
      snapshot, exact-text containment fully matches 28 of the 603 triple-matched rows (24 same-session, 4
      cross-session-only), and 0 of the 603 lie above the 200-character floor (observed verbatim ceiling 118).
      The never-landed population is 463 rows across 341 `(project_id, session_id)` pairs (339 distinct
      `session_id`s), and is reported by count.
- [x] The D-8 decision is closed in the tracker and in the ADR trail: the canonical triage/retention rule for episodic rows is written down in ADR-0033, and `docs/agents/testing.md` and the smoke doc agree with it.

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

- **2026-11-18 — AC5 rewritten: measurement, not deliverable. Report-only count ships; no purge does.**
  - **Owner ruling (2026-11-18):** a dead staging row may be deleted only on proof that its own text
    reached memory later — never on triple identity, never otherwise; if such proof is unprovable, the
    rule degrades to purging nothing. The rule is ADR-0033.
  - **The measurement then proved it unprovable on this schema.** `memories.content` is the LLM summary
    (`crates/mem-bridge/src/capture.rs:309` sets `content: input.summary.clone()`), the raw-text tables
    are empty on this root (`episodic_raw_context` 0 rows, `episodic_segments` 0 rows), and no payload
    key among the 1,066 dead payloads is `sha256`/`digest`/`content_hash`/`payload_hash`; `capture_inbox`
    and `memories` share no hash column and no column ties a dead row to a memory row.
  - **Population reconciliation (1,066 dead rows):** **603** rows whose `(project_id, session_id)` pair
    matches a landed `agent_sessions` row; **463** rows whose pair has none (never-landed), spanning
    **341 distinct pairs** and **339 distinct `session_id`s** — two sessions span two projects. A
    triple (`project`, `session`, `agent`) join matches 434, a strict subset, so triple identity is not
    the rule either.
  - **Finding 1 — what a memory row stores:** the provider's structured summary, not the captured
    transcript. Comparison a proof needs is transcript-vs-summary.
  - **Finding 2 — exact-text containment (measured, positive control passed):** 28 of the 603 dead rows
    are fully contained in some memory's content — 24 same-session, 4 only in other sessions' memories
    (transcripts `done`, `your rec`, `ping pass`, `try again`; capture-id suffixes `…ba1d8486`,
    `…12046848`, `…69f08518`, `…583b75d9`). Prefix ladder (length-gated): 50 → 5, 80 → 3, 100 → 1,
    118 → 1, **120 → 0, 200 → 0**. Observed verbatim ceiling 118 characters. The 200-character floor is
    a **stated policy**, not a wall: sub-120 rows stay unpurged even when contained, because a short
    string matching by coincidence is not identity. Short-prefix counts (126/35/15 at N=5/10/20) are
    not used for any decision.
  - **Finding 3 — no digest or provenance link:** containment is a string relation; provenance is what
    is missing, and the 4 cross-only rows are the ambiguity observed, not a hypothetical.
  - **Finding 4 — the outcome:** 0 candidates lie above the stated floor on the 2026-11-18 snapshot, so
    the ruling's own fallback fires: nothing is purged; dead rows are counted and reported.
  - **Finding 5 — the never-landed population:** 463 rows have no comparison target under any rule;
    they survive and are reported by count.
  - **What shipped:** `CaptureInbox::count_dead` (`crates/mem-episodic/src/inbox.rs`, SQL const
    `COUNT_CAPTURE_INBOX_DEAD` in `crates/mem-store/src/episodic_sql.rs`),
    `CleanupReport.inbox_dead_retained: Option<u64>` on `POST /api/cleanup` (wire `inboxDeadRetained`;
    `null` = store absent, a count error = run failure, never `null`), and the `cleanup.tick` log field.
    `GET /api/cleanup/status` is unchanged (issue 141). No config key: `captureInboxRetentionDays` is
    dropped, because with no delete path there is no clock.
  - **What explicitly does not ship:** any `DELETE`, any retention window, any purge predicate. The
    enforcement mutant `triple_match_delete_added` is red by two assertions of
    `test_report_never_deletes_dead_rows` (`crates/mem-web/tests/cleanup_retention.rs`), and it stays in
    the plan as the mutant a future proof predicate must reject.
  - **AC5 is rewritten, not ticked:** the checkbox records that the original purge AC was measured
    unprovable; the replacement text above is what shipped.
  - **Artifacts:** `docs/adr/0033-episodic-staging-retention.md` (the rule, the grounds, the rejected
    alternatives, the D-8/D-9 collision note);
    `docs/evidence/issue-32-hard-delete/README.md` (residual 5 rewritten to Finding 4, residual 6 open);
    `docs/agents/testing.md:270` (`6-field CleanupReport`, `cleanup_retention.rs` 12 → 17).

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

> **Superseded 2026-11-18 — the age-based purge in this brief does not ship.** The owner ruled that a dead
> staging row may be deleted only on proof its text reached memory later, and the measurement proved no such
> proof exists on this schema. Read AC5 and ADR-0033 instead of this paragraph.

**Acceptance criteria:** the issue's six ACs, with AC 2 as amended by the ruling above.

**Out of scope:** the REST contract's 404 semantics; tier-1 facts promoted from the session (kept); the
consolidated milestone's own summary (kept).

**Closed 2026-11-18 at commit `fbdc74e`.** Blueprint `blueprint-20261117-2220-issue-32-staging-purge-r1`
(`sha256=7c0da640db84`), SYS-APPROVED 2026-11-18T12:58Z after six gate rounds. Reviewer verdict `APPROVED`,
reached by its own runs rather than by reading the handoff: 7 suites green (`capture_inbox` 10,
`cleanup_retention` **17**, `cleanup_tick` 8, `cleanup_status` 6, `purge_cli` 1 of 31 filtered,
`api_endpoint_contract` 20), `fmt` and both clippy gates exit 0, **six evidence rows re-executed with values
and exit codes matching**, and three scratch-copy mutants all killed and reverted with `diff -r crates` empty
and 17/17 green on a fresh build (triple-identity DELETE; count error folded into `None`; absent store reported
as `Some(0)`).

What ships: `CaptureInbox::count_dead` (`pub async fn`, mirroring `queued_count`), `CleanupReport.inbox_dead_retained`
as `Option<u64>` — wire `null` when the episodic store is absent, so a missing store cannot read as a true zero —
serialised as `inboxDeadRetained` on the **`POST /api/cleanup`** answer only (`GET /api/cleanup/status` is
unchanged; issue 141 shipped against that shape at `1fc1f8a`), and logged by `cleanup.tick`. A count error maps
to `CleanupRunError::Failed` and never to `null`. `with_episodic` is applied inside
`production_deduplication_port_with_cleanup` before the `Arc<dyn MaintenancePort>` coercion; the two call sites
are `main.rs:181` (`Some(deps.episodic.clone())`) and `cleanup_tick.rs:105` (`None`). **No `DELETE`, no retention
config key, no `captureInboxRetentionDays`** anywhere executable — the reviewer checked.

Enforcement is `test_report_never_deletes_dead_rows`, killed by two assertions that say different things: row
survival (`left: 1` vs `right: 2`) and the reported count (`left: Some(1)` vs `right: Some(2)`). The mutant is
`triple_match_delete_added` — exactly the triple-identity rule the owner rejected.

Two reviewer judgments recorded rather than fixed: a mutant row citing `Compiled 2 packages` (custom cargo
progress) **satisfies** the compile-line rule, since the row's distinguishing output is only producible by the
mutated predicate — with the caveat that this form must not be used where the RED is not mutant-identifying;
and the AI-generated triage brief above still promised an age-based purge, now marked superseded in place rather
than left as a second, stale voice.

Residual 6 (stale vectors after compaction) remains open and is a follow-up, not a blocker: this issue closed on
the six acceptance criteria, and the residual is recorded here so the next reader does not treat the table as
fully swept.
