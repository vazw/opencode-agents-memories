# 64 — the profile tier has no live key: the shipped binary uses `"unknown"`, the injector asks for the git email, and nothing writes a profile row

Status: done — 2026-11-04 (ALREADY-FIXED — src/main.rs:252 calls web_identity::resolve_user_id; inject-side resolver 73a964f)
which approved issue 44's identity wiring as **necessary but not sufficient**. Brief at the end of this file.)

## PREMISE — measured, not inherited

Issue 44's D6 makes `resolve_project_scope` carry the git identity, so the **inject** side will look up
`get_active_profile(<git email>)`. Three measurements show that key does not exist on the **store** side, and
that nothing produces a profile row in production at all.

**1 — the shipped binary hard-codes the key.**

```
src/main.rs:203        user_id: "unknown".to_owned(),
```

That is the only assignment. `crates/mem-web/src/server.rs:43` documents the field as
*"the resolved user identity for the profile endpoints (upstream `getTags(process.cwd()).user.userEmail ||
"unknown"`)"* — but the value is a literal, so the documented derivation never happens and the `|| "unknown"`
fallback is never exercised. `crates/mem-web/src/api/profile.rs:447` repeats the same upstream quote.

**2 — nothing writes a profile row in production.**

`grep -rn 'create_profile\|upsert_profile' --include='*.rs' crates src` finds the store's own methods
(`crates/mem-profile/src/profile_store.rs:669`, `:851`) and exactly one caller outside them:
`crates/mem-bridge/src/composition.rs:497`, which sits inside the `#[cfg(test)]` module that opens at
`:367`. `LearningRound::new` is called **only** from `crates/mem-profile/tests/*` (seven files). The learner is
deliberately read-only this release (issue 47.1, Leg A = A2), so production has no producer.

**3 — the two sides disagree, and the tier is therefore unreadable in the shipped binary.**

With 44's D6 landed: inject asks `get_active_profile(<git email>)`; the store holds whatever was written under
`"unknown"` (nothing), or nothing at all. The tier can still render through an **explicit** `?userId=` request
(`crates/mem-web/src/api/profile.rs:448-452` — a non-empty `userId` wins) or through an import, but never on
the chat path.

## WHY IT MATTERS

Issue 44 ships a renderer, an escaping contract, a ranking pipeline, an identity gate, an emission and 31 tests.
Without this issue it renders on no production path. That is the same defect class as #62 (a provider gate with
test-only callers) and #63 (six config keys with zero readers) — and it is the reason #44's gate refused to let
D6 be described as "this decides whether the tier ships alive". D6 closes the **scope-side** join; this issue
owns the **store-side** key.

## WHAT A WAVE SHOULD PRODUCE

1. **Make the two sides agree, or record why they must not.** Either the composition root resolves the same
   identity the injector uses (the `user_email_override` → git email chain that D6 now builds), or the
   divergence is deliberate and carries a test pinning both values.
2. **Fix or falsify the doc.** `server.rs:43` and `profile.rs:447` both claim an upstream derivation that the
   code does not perform. Either perform it or correct the claim — a doc comment that describes a derivation
   nobody runs is how the next wave inherits a false premise.
3. **Decide the producer question explicitly.** If the tier is read-only this release and profiles arrive only
   by import or explicit API write, say so in the tracker and pin it with a test that proves a chat-path
   `assemble` with no import yields `NoProfile` (not a crash, not an empty block). If the learner was meant to
   be live, that is issue 47.1's ruling to revisit, not this wave's to assume.
4. **One observation, not two claims.** The oracle is: with a profile written under the resolved identity and
   no `?userId=`, a chat-path injection renders the block. Today it cannot, and after item 1 it must.

## ACCEPTANCE — what would close it

- [ ] The identity used to write and the identity used to read are demonstrably the same, or the divergence is
      recorded with a test pinning both.
- [ ] `server.rs:43` and `profile.rs:447` state what the code does.
- [ ] The producer question is answered in the tracker: read-only-by-design with a `NoProfile` test, or a
      named owner for the learner.
- [ ] One end-to-end observation: a stored profile under the resolved identity renders on the chat path with no
      explicit `?userId=`.
- [ ] Issue 44's tracker entry stops claiming the tier is live, and points here for the store-side key.

## Out of scope

- The learner itself (`LearningRound`) — issue 47.1 ruled the tier read-only this release.
- The renderer, escaping, ranking and emission — issue 44 owns those.
- The REST profile endpoints' success payloads.

## Agent Brief

**Category:** bug
**Summary:** the profile tier reads a key the shipped binary never writes, so it renders on no production path.

**Current behavior (verified at HEAD `9c46f8b`):**
`src/main.rs:203` sets `user_id: "unknown".to_owned()` — the only assignment — while `server.rs:43` documents
it as the upstream `getTags(...).userEmail || "unknown"` derivation. No production code writes a profile row:
`composition.rs:497` is inside the `#[cfg(test)]` module at `:367`, and `LearningRound::new` is test-only. So
`get_active_profile(<git email>)` — the key issue 44's D6 will pass — returns `None` in the shipped binary, and
the tier renders only through an explicit `?userId=` (`profile.rs:448-452`) or an import.

**Desired behavior:**
Write-side and read-side identity agree, or the divergence is deliberate and pinned. The doc comments describe
what the code does. The producer question is answered explicitly rather than assumed.

**Key interfaces:**
- `AppState.user_id` (`crates/mem-web/src/server.rs:43`) and `resolved_user_id`
  (`crates/mem-web/src/api/profile.rs:448-452`).
- `resolve_project_scope` → `TagResolver::tags` (issue 44's D6) — the read-side identity source.
- `ProfileStore::{create_profile, get_active_profile}` — the store-side key.
- `LearningRound` (`crates/mem-profile/src/learning_service.rs:190,209`) — test-only today.

**Acceptance criteria:**
- [ ] Write-side and read-side identities agree, or a test pins the divergence and the reason is recorded.
- [ ] Both doc comments state the real derivation.
- [ ] The producer question is answered: read-only with a `NoProfile` test, or a named owner.
- [ ] One chat-path observation with a stored profile and no explicit `userId`.
- [ ] Issue 44's tracker entry points here instead of claiming the tier is live.
