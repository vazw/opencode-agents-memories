# 143 — the status read 404s for a deleted prompt, so post-delete state cannot be reported

Status: done — 2026-11-18 — 410 + a minimal tombstone: a deleted prompt id answers `410 Gone` with `state: "deleted"`; an id that never existed keeps `404`. The tombstone (`prompt_tombstones`, `user-prompts` v4) stores id and deletion time only, written in the delete's own transaction. Prompts tier only.

*(Parent: issue 43.)*

## What to build

`GET` on a prompt's status answers `404` once the prompt is deleted, because the blueprint's Step 18(f) rule is
**unknown id ⇒ 404**. That rule is correct for an id that never existed, and it makes a deleted id indistinguishable
from a fabricated one.

The consequence: the API cannot answer "is this prompt erased?" after the fact. A client that deleted a prompt and
then wants to confirm the erasure completed has no read that says so — it gets `404` and must interpret that as
either success or a wrong id. `legacyPeerLive` and the erasure vocabulary in ADR-0019 describe post-delete states the
server never reports.

## The decision this issue must make

This is a **contract decision, not a bug fix**, and issue 43 recorded it as such. Pick one and defend it:

1. **Tombstone** — a deleted id keeps a minimal record (id, tier, deletion time, nothing that is text) so the status
   read can answer `erased`. Cost: a durable row that survives deletion, which interacts with the erasure posture in
   ADR-0019. This is the only direction that makes post-delete state observable.
2. **Tier-scoped read** — a separate endpoint takes a tier and reports aggregate erasure state without per-id
   history. Cheaper, weaker: it cannot answer about one id.
3. **Keep 404 and write it down** — record that post-delete per-id state is deliberately unobservable, in ADR-0019,
   and stop the surrounding vocabulary from implying otherwise. Cheapest and legitimate if the product does not need
   the answer.

## Acceptance criteria

- [x] One direction chosen, with the trade-off against `docs/adr/0019-erasure-posture.md` recorded in `## Comments`.
- [x] A test pins the chosen behaviour: for a tombstone, a deleted id answers a distinguishable state rather than
      `404`; for 404-kept, a test asserts a deleted id and a never-existed id are **deliberately** indistinguishable
      and the ADR says so.
- [x] `404` for never-existed ids is preserved in every direction — this issue must not turn a wrong id into a `200`.
- [x] If a tombstone lands, it stores no prompt text, no summary and no embedding; the test asserts the column set,
      not just the answer.
- [x] ADR-0019 and the API docs agree with the implementation in the same commit.

## Blocked by

Nothing. Related: issue 142 (the same ADR's vocabulary promises states the server cannot report).

## Comments

- **2026-11-18 — direction chosen: tombstone (410 + a minimal record), prompts tier only.** Blueprint `blueprint-20261118-0718-issue-143-deleted-prompt-410-r4` (sha256 `47272cfdd84f`, SYS-APPROVED 2026-11-18T07:47Z). The record is `prompt_tombstones(id, deleted_at_unix_ms)` in `user-prompts.db` v4, written in the same `BatchMode::Write` transaction as the hard `DELETE`. The status read answers `410 Gone` / `state: "deleted"` from it, and `404` stays for an id that never existed. The DELETE answer is unchanged. ADR-0019 D-19-12 records the trade-off.
- **Rejected, with the reason:**
  - `204 No Content`: reads as success on a wrong id.
  - A soft-delete column on `user_prompts`: every read path (`get_captured_prompts`, `search_prompts`, `get_prompts_by_ids`, `get_prompt_by_id`, the retention sweep, the UI roster) must learn the filter, and a missed one returns a gutted prompt.
  - Keeping `404`: post-delete state stays unobservable, which is this issue's own residual.
  - A `PromptDeletion::AlreadyDeleted` variant: changes the DELETE answer for no reader.
  - A `tier` column: one table is one tier, so the column would be a constant.
- **Consequence:** `user_version` 3 → 4 is one-way. A v3-era build refuses a v4 file with `SchemaNewerThanBuild` (ADR-0021; issue 43 decisions 2–3). No upgrade is performed by this wave; the live root is untouched.
- **Limit:** the tombstone is for the prompts tier only. Memories and episodic keep `404`.

### Acceptance

- [x] One direction chosen, with the trade-off recorded above and in ADR-0019 D-19-12. Pinned by the ADR diff.
- [x] Pinned by `test_a_deleted_prompt_answers_gone_and_an_unknown_id_still_answers_404` (`mem-web/tests/api_erasure_disclosure.rs`).
- [x] `404` for never-existed ids preserved: the 404 half of the same test, plus `test_a_failed_tombstone_read_answers_the_d9_500`.
- [x] No text, summary or embedding: `test_the_tombstone_column_set_is_id_and_deletion_time_only` (`mem-store/tests/prompt_tombstone_schema.rs`, a `table_info` equality) and `test_the_gone_answer_carries_no_prompt_text`.
- [x] ADR-0019 and the API docs agree in the same change: D-19-3 amended, D-19-12 added, the `ErasureStateDto.state` doc updated.
