# 48 — Profile · The learner leg and the erasure leg land together

Status: **ready-for-agent** — 2026-10-25, scoped by 47.1 (`Leg A = A2`, `Leg B = B2′`). **This issue is
now the erasure leg only**: the learner leg is N/A, and the coupling rule resolves to *the erasure leg may
not be partially landed*. Blocked by issue 47 — now **ruled**, so that blocker is discharged. See
§ *SCOPE RESTATED* for the five decisions that fix the seam, the ordering, the canary and the surface.

## What to build

Two legs, one wave, because issue 47 established that they are one question:

1. **The learner leg** (only if 47 ruled `A1 = ship`): the engine constructs `LearningRound` and drives it
   from the idle event that **already reaches Rust**.
2. **The erasure leg** (under **both** rulings): the profile's class from 47.4 is enforced at the seams it
   names, including for rows that already exist on disk.

The trigger seam is not new work and must not be invented: the host's `session.idle` is already forwarded
(`plugin/src/plugin.js:598-622`, `fireIdleCapture`) into the engine's `capture` request
(`crates/mem-bridge/src/engine.rs:186-206` method table). The learner goes **on that existing idle path**,
inside the engine process, behind `LearningLock` — not on a new hook, not in the JS adapter.

Everything below is wiring, not construction: `LearningRound::new` (`learning_service.rs:210`) needs a
`PromptLedger`, a `ProfileWriter`, an `AnalysisGateway`, a `LearningLock`, a readiness closure, a threshold
and a `user_id` — and production implementations of all of them already exist
(`PromptLedgerAdapter` `:591`, `ProfileWriterAdapter` `:625`, `ProviderAnalysisGateway`
`provider_gateway.rs:122`).

## SCOPE RESTATED 2026-10-25 — 47.1 ruled A2: this issue is the **erasure leg**

Issue 47 ruled `Leg A = A2` and `Leg B = B2′`. The learner leg below is therefore **skipped**, and the
coupling rule of this issue resolves to: the erasure leg ships, and **48.11 now reads "the erasure leg may
not be partially landed"**. Five decisions were taken in a grilling round, each after the code was measured.

**E1 — the invalidation seam is inside `delete_prompt`, with an injected eraser.** Upstream keeps the two
tiers independent (`user-prompt-manager.ts:155` is a bare `DELETE`; `grep profile src/services/user-prompt*`
is empty), so B2′ is a **deliberate divergence** — justified because we made a promise upstream never made.
A caller-level hook was rejected on evidence: upstream alone has **three** callers of `deletePrompt`
(`api-handlers.ts:459`, `:980`, `auto-capture.ts:160`), and our next one is issue 39's pruning path.
`crates/mem-bridge/src/composition.rs` builds both stores five lines apart (`prompts` `:90`, `profile`
`:95`), and `ProfileWriter` (`learning_service.rs:625`) is the precedent for the injected trait.
`delete_prompt` must read `created_at` before deleting — it is a bare `DELETE` today — because the
watermark comparison needs it (`user_prompts.created_at`, `mem-store/src/sql.rs:50`).

**E2 — erase the profile first, the prompt second.** The two tables live in different databases and no
transaction spans them. Profile-first fails toward privacy (a lost profile is recoverable in principle; a
surviving derivative is a silent promise break with no event left to re-trigger it, since the prompt is
gone). A partial failure must return a **typed outcome** so the API reports it rather than swallowing it.

**E3 — the canary asserts logical erasure, and *measures* byte residue.** `turso_core-0.8.3` has **no
`secure_delete`**, so a plain `DELETE` does not make bytes unrecoverable; a canary written as "grep finds
nothing" would fail by construction after a correct erase. 48.10 therefore asserts: no query returns the
profile, the changelog rows are gone, a fresh open sees neither — and separately records what residue the
file and `-wal` still hold, as ADR-0019's residual. It is a measurement with a stated outcome, not a
threshold. VACUUM was rejected: turso promises only that unused pages are removed (47.4b) and it writes a
full copy to `{temp_dir}/etilqs_{db}`, the residual already named in 47.4.

**E4 — the promise must be exercisable: `DELETE /api/user-profile`.** No delete route exists today, and
`save_prompt` has no production caller, so prompts exist only on migrated roots — a root with a profile and
**zero prompts** has no event B2′ can fire on. The route reuses the same explicit two-table delete, and is
also what gives `delete_profile` its first caller (it has **zero upstream too**,
`user-profile-manager.ts:621`).

**E5 — A2 is precisely "no prompt-derived learning", not "no writes".** The profile row is written today by
`refresh` (decay), `ai-cleanup/apply` and `PATCH /item`. Those stay; see 47.1's corrected definition.

**AC mapping after the re-scope:** 48.1–48.5 and 48.12 → **N/A** (no learner; issue 52 is not blocking).
48.6 → E1/E2. 48.7 → E4 **and** the B2′ hook. 48.8 → enforced paths are `delete_prompt` and
`DELETE /api/user-profile`; the exempted path is `delete_old_prompts`, deferred to issue 39 with the reason
recorded there, and the upstream capture-skip deletion (`auto-capture.ts:160`) has no port counterpart
because capture does not write prompts. 48.9 → unchanged, and it is the reason A2 is not a no-op.
48.10 → E3. 48.11 → erasure leg only. 48.13/48.14 are new.

## Acceptance criteria

**The learner leg — SKIPPED: 47.1 ruled `A2` (2026-10-25). ACs 48.1–48.5 are N/A; 48.12 is N/A too.**

- [ ] **48.1** `LearningRound` is constructed at the composition site 47.2 names, with the production
      adapters above — no test doubles in a production path, no second `ProfileStore` instance (issue 44's
      renderer and this learner must read the same row; `composition.rs:95` already builds the store) — **and
      the ledger it reads is not empty**: `save_prompt` (`crates/mem-profile/src/prompt.rs:174`, insert
      `:220`) has zero production callers, so on a **fresh** root a wired learner reads an empty
      `user_prompts`; a migrated root may already carry rows, which is why 48.9 seeds one. Wiring prompt
      recording lands here or in the issue 47.2(e) names. A learner constructed over a table nothing writes
      is the failure this whole set exists to stop.
- [ ] **48.2** It is driven from the idle path: a `session.idle` that reaches the engine results in at most
      one learning round per debounce window, and the debounce is `idle_scheduler`'s (`DEBOUNCE_MS`,
      `idle_scheduler.rs:15`), not a new timer. A test proves two idle events inside one window produce
      **one** round — over prompts that arrived through the **production** record path, not rows seeded
      directly into `user_prompts` (a seeded test passes while the real ledger is empty).
- [ ] **48.3** The identity gate is 44.12's, verbatim: no user identity ⇒ **neither** learn nor inject, and
      the outcome is typed, not a silent skip. This is the deliberate divergence from upstream's asymmetry
      (learn under `|| "unknown"`, inject under `|| null`).
- [ ] **48.4** A learning round cannot run concurrently with itself or with another process:
      `LearningLock` is held for the round, and a second round refuses or queues with a typed outcome
      (`learning_lock.rs`). A test drives two rounds concurrently and asserts one profile write.
- [ ] **48.5** The round's failure modes do not take the engine down: storage errors, gateway errors and
      corrupt profile rows each produce a typed outcome and a log line, never a panic (ADR-0013). A test
      per arm.

**The erasure leg (required under both rulings)**

- [ ] **48.6** The class from 47.4 is implemented at the seam(s) it names, and nowhere else. If the class
      requires the sweep, this issue names the sweep issue and is **blocked by** it rather than
      implementing a second sweep here.
- [ ] **48.7** `delete_profile` (`profile_store.rs:547`) gains its first caller — the erasure surface
      47.4 chose — or the issue records the decision *not* to wire it and what replaces it. A public
      method whose only purpose is erasure, with zero callers, is a claim the tool cannot honour.
- [ ] **48.8** The prompt path is reconciled with the class: whichever of `delete_prompt` (`prompt.rs:286`)
      / `delete_old_prompts` (`:384`) / the web cascade (`crates/mem-web/src/api/prompts.rs:79-111`)
      the class touches is named, and the paths it deliberately does **not** touch are named with the
      reason. Both lists are tested: one test per enforced path, one assertion per exempted one.
- [ ] **48.9** **Migrated roots are covered.** A storage root seeded with an existing `user-profiles.db`
      row (the population the audit left live) is exercised: the class's mechanism runs against it and the
      observable outcome matches 47.4. This test exists under `A2` too — it is the reason `A2` is not a
      no-op.
- [ ] **48.10** The canary extends to the profile database: a unique string in `profile_data`, run the
      erasure mechanism, grep the database and `-wal` bytes for it. Its doc comment says what it is — **our
      measurement, not a vendor guarantee** (47.4b): turso promises only that unused pages are removed, so
      this test *is* the erasure evidence and must state its own limits. (The `.bak`/profile-snapshot arm is
      the opsec report §9 item 8 scope that ADR-0019 owns; if it lands here, say so. VACUUM's temp rebuild
      under `{temp_dir}` is **outside** what this canary walks — it is a named residual in 47.4, not a test
      target.)
- [ ] **48.11** **No partial landing.** The blueprint and the review checklist cite every AC above; a wave
      that wires the learner (48.1–48.5) and defers 48.6–48.10 is rejected, because that is exactly the
      state this issue exists to prevent.
- [ ] **48.12** **The prompts are filtered before they are learned.** This issue does not implement
      filtering: **issue 52** owns it, and when 47 rules `A1` this issue is blocked by 52 (see below). The
      criterion here is only that the wiring order is right — a profile learned from another plugin's
      orchestration boilerplate is a worse artefact than no profile, because it is *re-injected*.

- [ ] **48.13** **The pending-learning readout stops advertising work nothing does.** Under A2 the UI's
      unanalyzed-prompt count (`crates/mem-web/src/api/store_adapter.rs:496`,
      `count_unanalyzed_for_user_learning`) advertises a learning backlog that will never be consumed.
      Remove it or relabel it as "not analysed — learning disabled", with a test. A readout that promises
      work the system will not do is the same defect class as a config field nothing reads.
- [ ] **48.14** **`DELETE /api/user-profile` exists and erases both tables.** The handler reuses the same
      explicit two-table delete as B2′ (changelogs first, then the profile row), is the first caller of
      `delete_profile` in this tree or upstream, and is tested against a root that has **no prompts** — the
      population for which B2′ can never fire.

## Blocked by / Blocks

- **Blocked by:** issue **47** (the ruling, in full). Under `A1`, also blocked by **issue 52** — the
  injected-prompt filter is implemented but unreachable (`crates/mem-ai/src/privacy/injection.rs`), so the
  prompts this learner reads would carry other plugins' blocks — and by whatever 47.2(b) names as the
  trigger owner, if that is a separate unimplemented seam.
- **Blocks:** nothing formally. In practice it gates treating the profile tier as "delivered": issue 44 can
  render a row, but no row exists until 48.1, and no erased row exists until 48.6.

## Comments

- Filed 2026-10-25 from the opsec design-concerns audit (concerns C2 and C3, implementation half), paired
  with issue 47's decision half.
- **Why one issue for two legs.** The audit's recommendation 2 offers "wire the learner **or** declare
  profiles read-only"; the audit's own scope caveat (§3.4) then says the erasure exposure is *"real for
  storage roots that already contain a `user-profiles.db`"* and that *"fix C2 and C3 becomes live
  immediately"*. So the erasure leg is not conditional and cannot be deferred to "after the learner": the
  migrated-root population has the hole **today**. Carrying both legs here is what makes 48.11 enforceable.
- **Not in this issue:** the renderer/escaping/identity-threading (issue 44); ADR-0019's file (issue 43's
  line, gated by its own §9 blockers); the sweep's retention rules (issue 39, paused).
- Provenance for every cited line is in issue 47's measured-state table; this issue deliberately does not
  restate the measurements, so a correction has one home.
