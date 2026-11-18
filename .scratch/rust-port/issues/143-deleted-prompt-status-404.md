# 143 — the status read 404s for a deleted prompt, so post-delete state cannot be reported

Status: ready-for-agent — filed 2026-11-18 from issue 43 residual 7 (`docs/evidence/issue-43-w6/REPORT.md:150`).

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

- [ ] One direction chosen, with the trade-off against `docs/adr/0019-erasure-posture.md` recorded in `## Comments`.
- [ ] A test pins the chosen behaviour: for a tombstone, a deleted id answers a distinguishable state rather than
      `404`; for 404-kept, a test asserts a deleted id and a never-existed id are **deliberately** indistinguishable
      and the ADR says so.
- [ ] `404` for never-existed ids is preserved in every direction — this issue must not turn a wrong id into a `200`.
- [ ] If a tombstone lands, it stores no prompt text, no summary and no embedding; the test asserts the column set,
      not just the answer.
- [ ] ADR-0019 and the API docs agree with the implementation in the same commit.

## Blocked by

Nothing. Related: issue 142 (the same ADR's vocabulary promises states the server cannot report).
