# 0033 — Episodic staging retention: dead rows are counted, never purged

- **Status:** Accepted (owner ruling, 2026-11-18)
- **Date:** 2026-11-18
- **Builds on:** `docs/adr/0010-sound-memory-context-preservation.md` (raw context is destroyed only by an explicit, owner-initiated purge); `docs/adr/0019-erasure-posture.md` (G3: erasure is never automatic — no timer, no sweep, no flag); `docs/adr/0007-bridge-strategy.md` (D-48: enqueue-before-ack, so `queued` rows are load-bearing).
- **Decides:** issue 32's residual 5 (the age threshold) and AC5/AC6.
- **Evidence:** `.scratch/rust-port/issues/32-hard-delete-milestone-scrub.md` (`## Comments`, 2026-11-18) and the blueprint's `[MEASUREMENTS]` section, `.tmp/blueprints/blueprint-20261117-2220-issue-32-staging-purge-r1.md:8-119`, measured 2026-11-18 on read-only copies of the live root.
- **Wave:** issue 32 — report-only. The count ships; no delete path does.

## Context

The episodic tier is staging. A captured session that never lands as a memory leaves a terminal row in
`capture_inbox` (`status = 'dead'`), and nothing ever removes it: `mark_done` purges the done row
(`DELETE_CAPTURE_INBOX_BY_ID`), the dead-letter state stays. On the 2026-11-18 snapshot of the live
root, 1,066 such dead rows had accumulated.

The owner's ruling (2026-11-18): a dead staging row may be deleted **only on proof that its own text
reached memory later** — never on triple identity (`project_id`, `session_id`, `agent_id`), never on
a `(project_id, session_id)` pair match, never on containment alone, never otherwise. If such proof is
unprovable, the rule degrades to purging nothing.

The measurement answered the ruling literally, and it landed on the fallback: **the proof is
unprovable on this schema.**

## The rule

1. A dead staging row may be deleted only on proof its text reached memory later.
2. No proof mechanism exists on this schema (§Grounds G1–G3), so **no dead row is deleted**. They are
   counted and reported.
3. A future delete path must ship its own proof predicate, its own clock and its own ADR. Triple
   identity and containment alone are pre-refused here so a later wave cannot quietly adopt them.

## Grounds

### G1 — A memory row stores the LLM summary, not the captured transcript

`memories.content` is written from `input.summary` (`crates/mem-bridge/src/capture.rs:309`); the web
tools and the promotion path write model-authored content or copy existing content. The raw-text
tables are empty on this root (`episodic_raw_context` 0 rows, `episodic_segments` 0 rows). So the
comparison a proof would need — dead transcript against landed row — compares a transcript with a
summary.

### G2 — Exact-text containment is measurable, and it is not proof

Measured on the 603 dead rows whose `(project_id, session_id)` pair matches a landed
`agent_sessions` row, against all 6,677 memory rows across the 36 shards (exhaustive, not a sample;
a planted 2,551-character dead transcript was found by both matchers, so a zero is a measurement and
not a blind spot):

- **28 of 603** dead rows are fully contained in some memory's content: 24 in a same-session memory,
  4 only in other sessions' memories (transcripts `done`, `your rec`, `ping pass`, `try again` —
  4, 8, 9 and 9 characters; capture-id suffixes `…ba1d8486`, `…12046848`, `…69f08518`, `…583b75d9`).
- Prefix ladder over the 603, restricted to transcripts at least as long as the prefix: 50 chars → 5
  rows, 80 → 3, 100 → 1, 118 → 1, **120 → 0, 200 → 0**.
- The observed verbatim ceiling is **118 characters**; 0 of the 603 lie above the 200-character
  floor.

Containment alone cannot distinguish "this memory was produced from this dead row" from "this memory
quotes the same short text" — the 4 cross-only rows are exactly that ambiguity, observed.

**The length gate is a policy, stated as one.** A 200-character floor sits above every observed proof,
so it is a defensible floor rather than a wall. Rows under 120 characters stay unpurged even when
verbatim-contained, because a short string matching by coincidence is not identity. (Short-prefix
counts — 126 / 35 / 15 at N = 5 / 10 / 20 under the length gate — are not used for any decision; a
5-character string matches inside long summaries by coincidence.)

### G3 — No digest or provenance link exists on either side

No payload key among the 1,066 dead payloads is `sha256`, `digest`, `content_hash` or
`payload_hash`; `capture_inbox` and `memories` share no hash column and no column ties a dead row to a
memory row. `sha256` appears in `mem-core` and in project-id derivation
(`crates/mem-episodic/src/promotion.rs:85`), not on the capture or memory write path. Containment is a
string relation; provenance is what is missing, and provenance is what the ruling asks for.

### G4 — The population, reconciled

- **603 rows** whose `(project_id, session_id)` pair matches a landed `agent_sessions` row — the set
  G2 measured.
- **463 rows** whose pair has no `agent_sessions` row — the never-landed set, spanning **341 distinct
  `(project_id, session_id)` pairs** and **339 distinct `session_id` values** (two sessions span two
  projects). These have no comparison target under any rule; they survive and are reported by count.

603 + 463 = 1,066. "339" always means distinct `session_id`; "341" means distinct pairs. The earlier
"339 never-landed" figure was a distinct-`session_id` count taken without the project key and is
retired.

**Triple identity is not the rule either.** Joining on all three columns matches **434** rows — a
strict subset of the 603 — and the 169 rows it drops are not evidence of anything except that the
agent column is noisy. Using it as a delete predicate would have destroyed rows for no measured
reason.

## Consequences

- `CaptureInbox::count_dead` counts `status = 'dead'`; `CleanupReport.inbox_dead_retained` carries it
  on `POST /api/cleanup` and in the `cleanup.tick` log. `None` (wire `null`) means the episodic store is
  absent; a count **error** is a run failure (`CleanupRunError::Failed`), never `null` — folding it
  would report "no store" for a store that is merely ill.
- `GET /api/cleanup/status` is unchanged (issue 141 shipped against that shape).
- No config key. `captureInboxRetentionDays` was proposed and is dropped: with no delete path there is
  no clock to configure.
- The count is a number; no payload text reaches a log, report or error string.
- `queued` rows are untouched by the count (D-48).

## Rejected alternatives

1. **Purge after a retention window** (the `captureInboxRetentionDays` idea). Rejected: it deletes the
   only copy of the 463 never-landed rows, whose text reached no memory and never will — the one
   population the ruling exists to protect.
2. **Purge on triple identity.** Rejected by the owner ruling and by G4: 434 rows join, 169 of the 603
   do not, and a join is not a proof of arrival.
3. **Purge on `(project_id, session_id)` pair match.** Rejected: 603 rows join, and G2 shows what a
   pair match is worth — 28 contained transcripts, 24 same-session, 4 cross-only.
4. **Purge on containment at any length.** Rejected: the observed ceiling is 118 characters, and the
   four cross-only rows are verbatim-contained in another session's memory. A 200-character floor is
   the stated policy above which no row currently sits.
5. **Milestone-signal purge** (delete once the session's milestone exists). Rejected: unobservable by
   construction — the signal is a property of a different table and does not identify the row.
6. **A retention clock: capture-age vs refusal-age.** Moot: no delete exists to age. Recorded here so
   the question is answered by whoever proposes one — a refusal-age clock would keep a row that was
   refused for months; a capture-age clock would retire it while it is still the only copy.
7. **Widening `GET /api/cleanup/status`.** Rejected: issue 141 shipped against that shape at
   `1fc1f8a`; the count rides `POST /api/cleanup` only.

## Numbering collision (D-8 / D-9)

D-8 and D-9 exist in other decision spaces: ADR-0007 (Host allowlist), ADR-0011 (integration notes)
and ADR-0012 (rekey). `crates/mem-store/src/episodic_sql.rs:47` cites a D-9 for the v2 additive
columns with no episodic definition in the tree, and the episodic D-8 this issue names is a dangling
reference, not a slot any ADR owns. This ADR therefore carries **no D-number**: the rule above is
identified by name, so no reader has to resolve a numeral that means four different things here.

## Relation to `purge.rs` and ADR-0019 G3

`crates/mem-episodic/src/purge.rs:11-13` states the binding invariant: erasure is a separate operation
and is never automatic — no timer, no sweep, no flag. This ADR keeps it: the shipped change adds a
read (`CaptureInbox::count_dead`) and no write. A future delete path for dead staging rows re-opens
G3 of ADR-0019 explicitly, in its own ADR, and must show the invariant still holds.
