# 47 — Profile · One ruling, two legs: does anything learn, and is the profile erasable?

Status: needs-triage — 2026-10-25. **The user-profile tier has no production learner and no erasure class,
and those are the same question asked twice — so they are decided here, together, before either leg's code
is planned.** This issue is an owner decision; it writes no code.

## Why these two cannot be separate issues

Concerns C2 and C3 of the design-concerns audit read as two findings and are one:

- **If nothing learns**, the erasure hole is **latent** for fresh installs (no profile row is created, so
  there is nothing derived to erase).
- **If the learner ships**, the hole is **live** the moment the first row is written: `profile_data` is
  model output derived from raw prompt text, and it is re-injected into future prompts (issue 44).
- **Either way**, the hole is already **live for migrated roots** — a storage root that carries a
  `user-profiles.db` from the TypeScript plugin has derived rows today, whether or not this binary ever
  writes one.

So the pair is decided in one place, and the enforcement is structural rather than editorial:

1. This issue records **both** answers in one ruling block (**47.1**). There is no tracker item that can
   carry one without the other, because they are the same field set.
2. **No code that creates a profile row may land before 47.4 is recorded.** Issue 48 (the implementation)
   is blocked by this issue; a blueprint for 48 that has no `47.x` to cite is out of order.
3. **The erasure class must be sufficient under *both* answers** (**47.5**) — so a later change of mind on
   the learner leg cannot silently invalidate it. That is the anti-drift clause: the class is not
   conditioned on the learner, it is conditioned on the derivation existing.

## Measured state — do not re-derive this

| fact | evidence |
|---|---|
| the learner type is `LearningRound`; **`LearningService` does not exist** in this tree | `crates/mem-profile/src/learning_service.rs:190`; `grep -rn LearningService crates src --include=*.rs` → **0** |
| it has **zero production constructors** — constructed only in 8 test files under `crates/mem-profile/tests/` | `LearningRound::new` at `learning_service.rs:210`; test-only sites include `tests/learning_round.rs:179`, `tests/profile_learning_idle.rs` family |
| `create_profile` has no production caller outside the unconstructed learner | `profile_store.rs:592`; only call sites `learning_service.rs:478`,`:677` (trait def `:72`, adapter impl `:666`) |
| **nothing writes the prompts the learner reads** — a fourth link in the same chain, found 2026-10-25 | `save_prompt` (`prompt.rs:174`, the `INSERT` at `:220`) has **zero production callers**; every call site is a test. So on a **fresh** root a wired learner would read an empty `user_prompts` table (a migrated root may already carry rows — see 48.9). |
| the production adapters and gateway **already exist** — this is wiring, not construction | `PromptLedgerAdapter` `learning_service.rs:591`, `ProfileWriterAdapter` `:625`, `ProviderAnalysisGateway` `provider_gateway.rs:122` |
| the idle debounce exists and is **pure**; nothing drives it | `crates/mem-profile/src/idle_scheduler.rs:1-16`, `IdleJob::Learn` `:23`; `on_session_idle`/`poll_due` have zero callers outside tests |
| `ProfileStore` **is** constructed, for the mem-web **read** surface only | `crates/mem-bridge/src/composition.rs:95`, comment `:43-45` |
| the erasure path does not touch the profile | `prompt.rs:286` (`delete_prompt`, bare `DELETE`), `:384` (`delete_old_prompts`, same); the web cascade `crates/mem-web/src/api/prompts.rs:79-111` reaches only the linked **memory** |
| the profile deleter exists and has **zero callers** | `profile_store.rs:547` |
| the derivation is **model-mediated, not verbatim** — raw prompt content (capped) plus the existing profile goes to the model, and its output becomes `profile_data` | `crates/mem-profile/src/analysis_prompt.rs:77-117` |
| there is no erasure ADR yet | `docs/adr/` stops at `0016`; ADR-0019 is issue 43's (`43:400`) |

**Attribution, corrected.** Issue 33 line 41 defers *"Upstream's `session.idle` auto-capture and profile
learning — W5.3's lane"* (`33-compaction-hook.md:41`), and issue 44 repeats it as *"issue 13/14"*
(`44-user-profile-injection-renderer.md:114`). **Issue 21 never contained profile learning** (its scope is
US 1.1/1.2/1.3 capture and injection), issue 13 is `done` and deliberately shipped the service with no
timer, and issue 14 is confidence decay. The defect is a **deferral chain pointing at a lane that never
existed** — not an epic marked complete. Do not write it the other way.

## What to build

This issue builds a **decision**, not code: one ruling block with two legs, recorded where a blueprint for
issue 48 can cite it. The measured state below is the input; the options are the choice.

**Leg A — the learner.**

- **(A1) Ship it this release.** `LearningRound` is constructed at a named composition site, driven by a
  named trigger, behind the identity gate 44.12 already defines. Consequence: the tier is live, and the
  erasure class becomes load-bearing immediately.
- **(A2) Read-only this release.** No learner; the renderer (issue 44) still ships and renders whatever row
  exists — including a migrated one. Consequence: the erasure hole stays live **only** for migrated roots,
  which is not nothing and must be said out loud.

**Leg B — the erasure class.** The three candidates ADR-0019 must choose between:

- **(B1) Regenerate at sweep** — derived text is refreshed from what remains, so erased prompts stop
  contributing. Cost: needs a sweep that touches `user-profiles.db`, and a rule for the profile's own
  version/changelog rows.
- **(B2) Invalidate on prompt delete** — a prompt delete marks the profile stale (or deletes it) rather
  than editing it. Cheap, honest, loses the profile more often than necessary.
- **(B3) Explicit exclusion, by name, with a stated rationale** — the promise says derived profiles
  outlive their sources, and the user is told. Acceptable only with the rationale written down, because
  the whole W6 line exists to keep the promise honest (`43:55`, decision 6) and the profile is the most
  direct derivative of prompt text.

The audit's framing of why B3 cannot be waved through: the promise in decision 6 is **artefact-split** —
prompt text versus memories — and the profile is a **third artefact** that appears in neither leg
(`REPORT.md` §3.4).

## RULING 47.1 — recorded 2026-10-25 (owner)

**Leg A = A2 (read-only this release). Leg B = B2′ (watermark invalidation).** Both are recorded here
together, as the header requires; neither may be revised without the other.

### Leg A — A2, read-only

**No prompt-derived learning this release.** The renderer (issue 44) still ships and renders whatever row
exists, including one carried in from a migrated root.

**Precisely — and correcting a sentence written here earlier.** A2 forbids any model call that turns
**prompt text** into profile content. It does **not** freeze the profile row; the first draft of this ruling
said "nothing creates or updates a profile row", which is **false**. Three production endpoints already
write the row without learning from prompts, and they stay:

| endpoint | writes how | why it is not learning |
|---|---|---|
| `POST /api/user-profile/refresh` | `decay()` then `update_profile(…, "Applied confidence decay")` (`crates/mem-web/src/api/profile.rs:171-201`) | deterministic ageing of existing content |
| `POST /api/user-profile/ai-cleanup/apply` | `apply_cleanup` | a model call over existing profile content, never over prompts |
| `PATCH /api/user-profile/item` | `update_profile_item` | user-driven edit |

None can introduce content derived from a prompt, so none widens the erasure hole, and disabling them
would remove working features for no privacy gain. The definition is stated this precisely so nobody
later smuggles prompt-derived learning in as "maintenance": **the test is what the write's input is, not
whether a write happens.**

**One honesty item this exposes:** `crates/mem-web/src/api/store_adapter.rs:496` calls
`count_unanalyzed_for_user_learning`, so the UI advertises a learning backlog that, under A2, nothing will
ever consume. It must be removed or relabelled in issue 48 — a readout that promises work the system will
not do is the same class of defect as a config field nothing reads.

*Rationale.* A1 is not one feature but four unwired links plus a dependency: `save_prompt`
(`prompt.rs:174`) has zero production callers, `LearningRound::new` (`learning_service.rs:210`) has zero
production constructors, `idle_scheduler`'s `IdleJob::Learn` has zero drivers, and issue 52's
injected-block filter has no production reader — so a wired learner either reads an empty `user_prompts`
table (fresh root, learns nothing) or learns from other plugins' injected boilerplate
(`crates/mem-ai/src/privacy/injection.rs:3-7`). Shipping that during a storage-hardening phase would
create a **live** privacy surface — model output derived from raw prompt text, re-injected into future
prompts — at the same time as the work meant to make the storage base trustworthy. A2 declines to create
new derivation while Leg B governs the derivation that already exists.

### Leg B — B2′, watermark invalidation

**On prompt deletion, delete the profile iff the deleted prompt could have contributed**, i.e. its
timestamp is at or before the profile's `last_analyzed_at` (`user_profiles`, `sql.rs:143-155`). The erase
is **explicit over both tables**, in this order, and never left to the declared FK cascade:

1. `DELETE FROM user_profile_changelogs WHERE profile_id = ?` — the changelog carries
   `profile_data_snapshot TEXT NOT NULL` (`sql.rs:160-170`), i.e. full copies of the profile text, so a
   profile delete that leaves them behind has erased nothing;
2. `DELETE FROM user_profiles WHERE id = ?`.

The second statement only, as `delete_profile` does today (`profile_store.rs:547-560`), is **not
sufficient**. Grounds: the `ON DELETE CASCADE` is declared but **no test exercises it**, and
`user_profiles_migrations` applies `CREATE TABLE IF NOT EXISTS`, which cannot add an FK to a table a
migrated root already has — so "the schema says so" holds for databases this build created and not for
the migrated population this issue exists to cover.

**Unset or zero watermark ⇒ treat as "could have contributed"** and delete. A migrated row with an
unusable `last_analyzed_at` must fail toward erasure, not toward retention.

*Why B2′ and not the other two:*

- **B1 (regenerate at sweep) is disqualified twice.** Under A2 there is no analysis pipeline to
  regenerate *with*; and even under A1 it would not work as described, because
  `build_user_prompt(prompts, existing, …)` (`analysis_prompt.rs:77-117`) is **incremental** — when a
  profile exists it feeds `build_category_summary(existing)` back to the model, so an erased prompt's
  contribution survives as the seed of the next round. A true regeneration would have to discard the row
  and re-derive from scratch, at full re-analysis cost and at the price of the version/changelog history.
- **B3 (explicit exclusion) is rejected** because the profile is the most direct derivative of prompt
  text; carving it out of decision 6's artefact-split promise would make the promise narrower than the
  user was told, which is the failure mode the whole W6 line exists to prevent.

### 47.3 — what happens to a profile row that already exists (A2)

**It is rendered, never updated.** Consistent with the promise because the row is data the user's
previous install created and A2 is *read-only*, not *amnesia* — and because Leg B still governs it: the
first source deletion past its watermark erases it, so a rendered profile is never exempt from erasure,
only from refresh. On a **fresh** root no row is ever created and the renderer renders nothing; that is
the accepted, stated cost of A2, not an oversight.

### Pruning (retention) — deferred to issue 39, with a binding condition

Deleting prompts through **explicit user deletion** invalidates per B2′. **Retention pruning is
deferred** to issue 39, because `delete_old_prompts` (`prompt.rs:384`) has **zero callers** today and 39
owns deletion semantics. The deferral is structural, not editorial: **issue 39 must cite 47.1/B2′ and
decide whether the pruning path joins the invalidation.** Until it does, material from a
retention-pruned prompt may persist in the profile — recorded as a named residual in 47.4's table rather
than left as silence.

### 47.5 — why this class is sufficient under both A1 and A2

The class is conditioned on **derivation**, not on the learner: the trigger is "a source was deleted",
the action is "remove the derivative". Under **A1** the learner would rebuild the profile from the
surviving prompts; under **A2** it stays deleted. Neither leg makes the rule insufficient, and a later
change of mind on Leg A cannot invalidate the class — which is the anti-drift clause of this issue,
satisfied by construction rather than by a promise to revisit.

### 47.4 — ADR-0019 input

Class: **B2′**, with the derivation path (`analysis_prompt.rs:77-117`, model-mediated and incremental)
and the re-injection path (issue 44) named explicitly. Residual classes to be itemised in ADR-0019, not
summarised in a sentence:

| residual class | mechanism | note |
|---|---|---|
| `.bak` families | carried from opsec §9 item 8 | unchanged by this ruling |
| the vault | carried from opsec §9 item 8 | `mem-vault` always encrypted; see issue 53 |
| logs / exports / media | carried from opsec §9 item 8 | unchanged by this ruling |
| **the profile (this ruling)** | `user_profiles` **+** `user_profile_changelogs`, explicit two-table delete | **retention-pruned prompts are a named residual until 39 decides**; unset watermark ⇒ conservative delete |
| VACUUM's temporary rebuild (`{temp_dir}/etilqs_{db}`) | turso `VACUUM` writes a full copy outside the storage root | **conditional** — reachable only if W6 chooses the flag-off option; sourced from the archived ingest quoting `core/vdbe/vacuum.rs:300-306`, not verified from source by us |

**47.4b stands unchanged:** the claim rests on our canary measurement in 48.10, not on a vendor promise —
turso promises only that unused pages are removed, and SQLite's stronger "cleans all traces" is not
turso's sentence.

### 47.7 — the follow-up

**Issue 48 carries the erasure leg only** (A2). It may not be dropped: migrated roots already hold
derived rows, so the erasure hole is live there whether or not this binary writes anything. 48 must cite
this ruling, and its blueprint may not introduce a profile writer — a blueprint for 48 that creates a
profile row has silently reversed Leg A.

## Acceptance criteria

- [x] **47.1** **One ruling block** in this issue records both legs: `Leg A = A1 | A2` and
      `Leg B = B1 | B2 | B3`, each with a one-paragraph rationale. Neither may be recorded without the
      other; an issue revision that fills one field and leaves the other empty fails review.
- [x] **47.2** *(N/A — Leg A = A2; retained so the A1 branch is not lost)* **If A1:** the ruling names (a) the exact composition site that constructs `LearningRound`,
      (b) what drives it — `idle_scheduler`'s debounce plus who calls `on_session_idle`, or a different
      trigger with a reason, (c) the provider/model config source, (d) that no identity ⇒ neither learn
      nor inject (44.12's symmetric refusal), and (e) **whether prompt recording is part of A1 or a separate
      step** — `save_prompt` (`prompt.rs:174`) has no production caller, so on a **fresh** root a learner with
      no prompts learns nothing (a migrated root may already carry rows; 48.9 seeds one), and A1 that omits
      this link is a chain of five unconstructed pieces pretending to be one feature.
- [x] **47.3** **If A2:** the ruling states what happens to a profile row that already exists — rendered,
      ignored, or erased — and why that is or is not consistent with the promise. Silence is not an
      option: migrated roots are exactly the population the audit left live.
- [x] **47.4** **The erasure class is recorded as ADR-0019 input**, in the shape ADR-0019 needs: the class
      (B1/B2/B3), the reason, the derivation path (`analysis_prompt.rs:77-117`) and the re-injection path
      (issue 44) named explicitly, and the residual **itemised as a table of named classes**, not as a
      sentence. This satisfies the opsec report's ADR-0019 blocker item 8, *"the profile assigned a class"*
      (`.tmp/scratch/opsec-opencode-agents-memory-run-1/REPORT.md` §9). The table carries the three classes
      that item already names — the `.bak` families, the vault, and logs/exports/media — plus this issue's
      profile class, plus **one this set found**:
      **VACUUM's temporary rebuild.** In-place `VACUUM` writes a full copy to
      `{temp_dir}/etilqs_{source_db_name}` (source: the archived ingest quoting `core/vdbe/vacuum.rs:300-306`
      — **not** verified from source by us), i.e. every live row of a plaintext `user-prompts.db`, **outside
      the storage root**, for the duration. A canary that walks the storage root cannot see it, so W6's
      VACUUM option must either name it as a residual or delete it after the rebuild. It is **conditional**:
      today's configuration refuses `VACUUM` at translation (issue 43's W0), so this class is reachable only
      if W6 chooses the flag-off option.
- [x] **47.4b** **The claim states what it rests on: our canary measurement, not a vendor guarantee.**
      turso promises only that *"Unused pages from deleted rows and dropped objects are removed, shrinking
      the file"* (`vacuum.mdx:42`, doc-sourced); a search for erase/forensic/residue across its docs finds
      nothing but a `VACUUM INTO` encryption warning. SQLite's stronger *"cleans all traces of deleted
      content"* is **not** turso's sentence and must not be borrowed. No issue in this set may lean on an
      upstream promise, because there is not one — the erasure evidence is the canary in 48.10.
- [x] **47.5** **One paragraph states why the class is sufficient under both A1 and A2.** If the paragraph
      cannot be written, the class is wrong — that is the test.
- [x] **47.6** The deferral chain is corrected in the tracker: `33-compaction-hook.md:41` and
      `44-user-profile-injection-renderer.md:114` are annotated to point at **47/48**, not at "W5.3's lane"
      and not at "issue 13/14". `44-user-profile-injection-renderer.md:120` is corrected too — it still
      names the phantom `LearningService`, which this issue's own measured table shows has **0** matches
      tree-wide; the sentence must name `LearningRound`. A future reader must not be able to follow any of
      the three pointers into a closed door or a symbol that does not exist.
- [x] **47.7** **The follow-up is named.** If A1, this issue records that **48** carries both legs (learner
      wiring *and* the erasure mechanism) and that 48 may not merge one without the other. If A2, this
      issue records that 48 carries the erasure leg only, and why that leg cannot be dropped.
- [x] **47.8** No code changes in this issue: no `crates/` or `src/` diff, no schema change, no new ADR
      file (ADR-0019 is written by issue 43's line once its gate opens — `43:159`).

## Blocked by / Blocks

- **Blocked by:** nothing. This is the cheapest item in the set and it unblocks the most.
- **Blocks:** issue **48** (all of it). Also feeds **ADR-0019** (issue 43's, gated separately) via 47.4,
  and is a precondition for treating issue 44's tier as anything other than a renderer for an always-empty
  row.
- **Cross-reference:** if A1 ships a learner, **issue 52** must land first or alongside — the prompts that
  would be recorded are captured from a transcript that carries other plugins' injected blocks, and a
  learner over unfiltered prompts produces a profile describing orchestration boilerplate
  (`crates/mem-ai/src/privacy/injection.rs:3-7` is that consequence in the filter's own words). This issue
  does not re-scope 52; it records the edge.
- **Cross-reference, not scope:** the renderer, the identity gate and the escaping belong to **issue 44**
  (`44.1`–`44.13`). This issue does not touch them, and 47.2(d) reuses 44.12's ruling rather than
  re-deciding it.

## Comments

- Filed 2026-10-25 from the opsec design-concerns audit (concerns C2 and C3, both `SOUND`;
  C2's evidence corrected), merging the audit's §6 recommendations 2 and 6 into one decision.
- **Corrections carried in, per the brief's AUDIT RESULT block**
  (`20261025-design-concerns-audit-brief.md:35-36`): the type is **`LearningRound`**
  (`learning_service.rs:190`), not `LearningService`; it has **zero production constructors anywhere**, not
  "zero outside `mem-profile`"; and the misattribution is in issues **33/44**, not issue 21. The phrase
  "an epic was marked done but its feature is a table nothing writes" is **not** used here, because it is
  not what the tracker says.
- **Why a decision issue and not an implementation issue.** The audit's own recommendation 2 offers
  "construct `LearningRound` … **or** state that profiles are read-only this release" — those are not two
  scopes, they are two answers to one question, and picking one by filing code would be deciding it
  implicitly. The coupling rule in the header is what keeps that from happening: 48 is blocked here, and
  the class must survive both answers.
- **Deviations from the audit's grouping** (recorded in `.tmp/drafts/20261025-concerns-issue-map.md`):
  C2 and C3 become **two issues, not one** — this one decides, **48** implements — because a decision and
  a wave have different review gates. The coupling is structural (48 is blocked by 47, and 48 carries both
  legs), not editorial.
- Derivation is model-mediated and abstracted (categories/descriptions), **not a verbatim copy** — but it
  can contain specifics and it is re-injected, which is why it is not a neutral summary
  (`REPORT.md` §3.4).
