# 0019 — Erasure posture: what leaves, what is retained, what the sweep covers

- **Status:** Proposed (draft for owner approval)
- **Date:** 2026-10-29
- **Builds on:** `docs/adr/0012-vault-key-custody.md` (D-14: a purge is a logical deletion, and the vault's guarantee is key destruction, not a sweep); `docs/adr/0018-shard-facts-source-of-truth.md` (the shard owns its facts, and an authoritative count is a stored one); `docs/adr/0021-schema-version-discipline.md` (a file the sweep opens must be safe to open at its version); `docs/adr/0010-sound-memory-context-preservation.md` (raw context is destroyed only by an explicit, owner-initiated purge); `docs/adr/0007-bridge-strategy.md` (the bridge is the one place the episodic and profile stores are joined).
- **Supersedes:** the routed "separate status endpoint" default for the delete surface (`.scratch/rust-port/issues/43-database-base-hardening.md:111-114`), reversed in D-19-2.
- **Evidence:** `docs/evidence/opsec-w6-design-2026-10-25/REPORT.md` (the inventory at `:234-257`, the blockers at `:306-343`) and `FINDINGS-DETAIL.md` (`w6/erase/disclosure-unreachable` at `:13-68`, `w6/erase/view-clause-contradicts` at `:163-171`, `w6/milestone/rewrite-keeps-derived-text` at `:472-530`, `w6/milestone/rewrite-needs-sweep` at `:530-575`, `w6/purge/report-omits-vault` at `:636-690`); `NEEDS-VALIDATION.md:12-38` for `w6/erase/sweep-liveness`. Blockers 6–11 are closed here; blocker 12 moves to ADR-0025.
- **Wave:** issue 43 W6 — text only. Nothing in the hand-off is shipped except the two rows that say so.
- **Blueprint:** `bp-20261029-0034-adr0019-erasure-posture-r8` (sha256 `c92e3271f835`, `SYS-APPROVED 2026-10-29`).

## Context

The delete surface ships one boolean per route, and the two field names are crossed:
`DeletedMemoryDto` carries `deleted_prompt` for `DELETE /api/memories/{id}` (`dto.rs:294-296`,
`memories.rs:569`), and `DeletedPromptDto` carries `deleted_memory` for `DELETE /api/prompts/{id}`
(`dto.rs:301-303`, `prompts.rs:44`). Neither route says anything about erasure, and the prompt
delete's erasure report is currently discarded at the port boundary (`prompt.rs:387` →
`store_adapter.rs:658-663`). The purge report's residual vocabulary is a one-element constant,
`NOT_ERASED: [NotErasedTier; 1]` (`purge.rs:114-117`), whose only reason is "no per-session
provenance; Phase 4" (`:116`) — so a purge report cannot name the vault even when the purged session
marked content for it (`mem-vault/src/lib.rs:46`). `EpisodicPurgeReport` carries `milestones`, the
count of rows the purge deleted (`purge.rs:99`), and no `milestones_rewritten` exists anywhere.

The security gate on the W6 design is REJECTED with two high findings, and six of the twelve
blockers are the erasure text's half (`REPORT.md:306-343`). Blockers 6–11 are what this ADR closes;
blocker 12, the retirement sentence, concerns the open guard's permanence against pre-guard binaries
and moves with ADR-0025's blockers 1–5. The audit's finding is stronger than a documentation gap:
the delete DTOs are frozen at one boolean while the common outcome of a delete is `PendingErase`, so
the API satisfies "never an unqualified deleted" and the product does not
(`FINDINGS-DETAIL.md:13-68`).

The owner ruled five questions on 2026-10-29 (`.scratch/rust-port/issues/43-database-base-hardening.md:170-195`);
those rulings are the authority here, and this ADR may not soften or re-litigate them. (1) "Delete
surface = additive field" — `erasure: EraseState` on both DTOs, always present (`:172`) → D-19-2,
D-19-3 and D-19-11. (2) "No queue; a manual lever" — `--mode=sweep` (`:177`) → D-19-4 and D-19-5.
(3) "Milestone arithmetic is historical" (`:180`) → D-19-7 and D-19-8. (4) "Vault tier = per-request
tri-state" (`:183`) → D-19-9. (5) "The three-clause promise adopted", with two `surviving_derived`
counters (`:186`) → D-19-1, D-19-6 and D-19-10.

## Grounds

### G1 — The artefact inventory splits into tiers, and each tier owes a different answer

Prompt text is written verbatim in three places (`user_prompts.content`, `episodic_raw_context` and
`episodic_segments`, and each milestone contributor's retained summary), near-verbatim in one
(`user_profiles` and its changelog snapshots), derived in two (`memories`, `facts.db`), invertible in
one (the milestone embedding), copied in two (the `.bak` families, export bundles), and encrypted in
one (the vault). A promise that does not name each of them is a claim about one tier dressed as a
claim about the store — the audit's inventory question (`REPORT.md:234-257`) is exactly that — and
D-19-10 makes the table normative so that a new writer cannot ship without its row.

### G2 — A delete is not a boolean

`DeletedPromptDto` and `DeletedMemoryDto` are each one `bool` (`dto.rs:294-303`), but a prompt delete
performs up to three steps with different completion times: the profile erase (synchronous, already
done today), the prompt row delete (synchronous, `prompt.rs:378`), and the physical sweep (not
scheduled). A single boolean cannot state which of the three happened, so it is wrong for every
outcome except the one in which all three are complete.

### G3 — Erasure is never automatic

`crates/mem-episodic/src/purge.rs:11-13` states the invariant that binds this ADR: "Erasure is a
SEPARATE operation and is never automatic: no timer, no sweep, no flag, no consolidation stage calls
this module (proved by the bridge's lexical source invariant)." The sweep this ADR describes must not
violate it, which is why D-19-5 forbids the sweep from selecting a row and D-19-4 refuses the queue
whose only worked-out drain would execute deletions at start-up.

### G4 — Logical deletion is not physical erasure here

`secure_delete` is **unimplemented**: the pragma set is accepted and the read returns zero rows
(`turso_capabilities.rs:1243-1259`), so an `Ok` from the set carries no information. `VACUUM` needs
the experimental feature and is refused in the shipping configuration
(`test_vacuum_is_refused_in_the_production_configuration`, `turso_capabilities.rs:1405`); with the
flag it rewrites the file and erases the canary
(`test_vacuum_with_the_experimental_flag_erases_the_canary`, `turso_capabilities.rs:1455`), but it is
refused again when multiprocess WAL is enabled
(`test_vacuum_with_the_flag_is_refused_when_multiprocess_wal_is_enabled`, `turso_capabilities.rs:1505`).
The guarded open enables multiprocess WAL in production (`db.rs:421`). A clean close never
checkpoints: the measured closed shape is a 4096-byte main file and a 107152-byte `-wal` that holds
the database (`turso_capabilities.rs:2062-2080`), so the at-rest `-wal` carries the text. And the
default database is plaintext — `database_encryption_enabled: false` (`schema.rs:612`).

### G5 — The vault is a different tier with a different guarantee

The vault is per scope at `{root}/vault/{scope_hash}/vault.db`, encrypted, and its erasure operation
is key-generation destruction (W8c), which is whole-database by construction (ADR-0012 D-14,
`docs/adr/0012-vault-key-custody.md:525-560`). A row delete there removes ciphertext; that is not the
same operation as a plaintext scrub, and the crate says so itself: without a named vault tier the
purge report "is **no longer** complete or true" (`mem-vault/src/lib.rs:46`).

### G6 — The tool writes full copies of its own databases

Six `.bak` families are produced: `schema_migrations.rs:114` (`.schema-v{n}-{ts}.bak`),
`legacy_migrator.rs:50` (`.legacy.bak`), `encryption_migrator.rs:328`
(`.pre-encrypt-{millis}.bak`), `migration_service.rs:648` (`.pre-reembed-{nonce}.bak`),
`shard_path_migration.rs:693` and `:781` (`.pre-path-migrate-{nonce}.bak`, two call sites, one
family), and `shard_manager.rs:1165` (`.{reason}-{pid}-{millis}.bak`). Each is a full copy including
the `-wal`, and `schema_migrations.rs:525` states the posture explicitly: "No code path deletes a
`.bak`." An erasure promise that does not reach them is false before the sweep starts.

## The promise — three clauses

### Clause 1 — what leaves

The **prompt row** leaves the **prompt history** in the delete request itself (`prompt.rs:378`), after the profile erasure (erase-before-delete, E2), so the prompt is gone from the list and from the reader. The **bytes** leave at the next successful sweep, which checkpoints and — where the configuration permits — vacuums each plaintext database a logical scrub wrote to. `PendingErase` names the second half, never the first.

The text does **not** leave the store. It remains in the transcript (A6), in profile-derived rows the
watermark rule did not reach (A2), and in memories (A7). "Removed from the agent's view" is retired as
a claim: the injected context *is* the memories (`FINDINGS-DETAIL.md:163-171`), so a reader who hears
"the agent no longer sees it" hears the opposite of what the memory tier guarantees.

### Clause 2 — what is retained

- **memories** (A7, class C) — derived knowledge under a separate promise, counted by `surviving_derived.memories`, which is `null` today: no populated provenance exists, and the delete is single-shard until issue 72 lands;
- the **host transcript** (A6) — retained, and only `purge-session` reaches it;
- **facts** (A8, class C) — derived; a prompt delete retains them, and only `purge-session`'s facts leg erases them;
- **milestones as historical** — on a `purge-session` the contributor entry leaves, but a prompt delete does not reach the milestone (`FINDINGS-DETAIL.md:163-171`); the merged `summary` (C-H) and its `embedding` (V-inv) stay, `source_count` and the window stay, and the W6 report adds `milestones_rewritten` (H6, planned);
- the **vault** (A10) — named, with its per-request tri-state (D-19-9);
- **profile rows and changelogs** (A2) — erased at delete time by the watermark rule (`erasure.rs:29-31`).

### Clause 3 — what the sweep covers

The sweep reaches exactly four targets: `user-prompts.db` (and its `-wal`), `episodic.db` for the milestone contributor rewrite only, `user-profiles.db` (the database the watermark scrub wrote to), and the `.bak` families under the exclusive root. It never reaches the vault, the host transcript, logs, exports, media, `facts.db`, or memories.

### The limits of the default configuration

`database_encryption_enabled = false` (`schema.rs:612`) means the residue until the sweep is
plaintext. Encryption would not change the promise: the key outlives the deletion, so a key holder
decrypts the pages whatever the file holds. Copy-on-write snapshots, SSD wear-levelling, external
backups and any copy outside the root are outside the promise by construction. The tool's own `.bak`
files are **inside** it (D-19-10).

## Decisions

**D-19-1 — The promise is three clauses, and no clause may be read as another's.**

Clause 1 says what leaves and when; clause 2 says what is retained and how much of it is countable;
clause 3 says what the sweep reaches and what it never reaches. The clauses are separate because the
audit found the earlier single sentence satisfying one clause while contradicting the next
(`FINDINGS-DETAIL.md:163-171`).

**Rejected:** one sentence ("on the next successful sweep, from every file this tool writes" — it
over-claims the six `.bak` families, the profile and the vault), and the "agent's view" clause (it
contradicts clause 2 by the promise's own next sentence, because the injected context is the
memories).

**D-19-2 — The delete answers with an additive `erasure: EraseState`, on both delete DTOs, always present.**

The field is additive: each DTO gains one object and keeps its existing boolean. It **Reverses the routed** 2026-10-25 default — frozen DTOs plus a separate status route (`43-database-base-hardening.md:111-114`) — because the common outcome of a delete is `PendingErase`, so `deleted: true` is wrong for the majority case, and a status route nobody is told to call is not a disclosure. The status read survives for state that outlives the call: **it is the detail read, not the disclosure** — the audit's remediation requires the follow-up detail, not a second place for the disclosure to hide (`FINDINGS-DETAIL.md:13-68`).

**Rejected:** frozen DTOs plus the status endpoint (satisfied in the API, violated in the product);
a mandatory same-interaction follow-up read (unforceable in the API).

**D-19-3 — `EraseState` vocabulary: labels, never paths.**

`Erased` is a unit variant; `PendingErase { tier, sweep_state, surviving_derived }` carries
`tier: prompts | memories | milestones`, `sweep_state: notYetSwept | sweepFailed | legacyPeerLive |
noSweepForTier`, and `surviving_derived { memories: Option<u64>, milestones_rewritten: u64 }`. The
surface rule is part of the decision: **`tier: milestones` is emitted only by `purge-session`'s report** —
neither delete DTO can rewrite a milestone. Three counts are never conflated: the live erased count is
`EpisodicPurgeReport.milestones` (`purge.rs:99`); the W6 report **adds** `milestones_rewritten` (H6,
**planned** — no such field exists today); and a delete's `surviving_derived.milestones_rewritten` is
**0 by construction**, because a prompt or memory delete rewrites no milestone, and the field's
documentation says so — a delete reporting a non-zero value is a bug. `memories: null` means **not
countable**, never zero. `Erased` means no step this tool performs remains for the named tier at
answer time; in W6 both HTTP delete paths answer `PendingErase` (prompts: the medium step has not
run; memories: `noSweepForTier`), and `Erased` is produced by the sweep's report after a completed
pass.

**Rejected:** a `memories: 0` (it reads "none survived" where the truth is "not countable"); a
request id in the residual (no queue means no request lifetime to name); absolute paths or any
content-derived value.

**D-19-4 — No queue; `PendingErase` may persist indefinitely; `--mode=sweep` is the operator lever (planned).**

A queue is refused. It would store prompt ids — itself an erasure artefact — and its only worked-out
drain executes at start-up, which makes erasure automatic and bypasses `--confirm`. Without a queue
the state is honest but unbounded: `PendingErase` may persist indefinitely, and the operator forces a
sweep with `--mode=sweep`, a mode of the same binary on the `purge-session` and `reembed-scope`
precedent (`src/main.rs:359`, `:376`). The binary has three modes today and `sweep` is not one
(`src/main.rs:351`, `:359`, `:376`).

**Rejected:** a durable bounded queue (it stores prompt ids — itself an erasure artefact — and its
only worked-out drain executes at start-up, making erasure automatic and bypassing `--confirm`); a
scheduled sweep (the same defect, one timer away).

**D-19-5 — The sweep compacts and reaps; it never selects a row.**

No row-selection statement exists in its path, so no sweep can be an erasure trigger: G3's invariant
(`purge.rs:11-13` — "no timer, no sweep, no flag") stays literally true, and a starved sweep can only
delay physical erasure. It deletes two things: `.bak` copies, and the dead bytes of a file it
rewrites.

**Rejected:** a sweep that finishes pending deletions (it needs D-19-4's refused queue, and it makes
the promise's liveness depend on a stage `purge.rs:11-13` forbids).

**D-19-6 — The sweep set is the plaintext databases a logical scrub wrote to, plus `.bak` under the exclusive root.**

`user-prompts.db`, `user-profiles.db`, and `episodic.db` (the rewrite). The vault is excluded
**because it is encrypted**: ADR-0012 D-14's ciphertext argument is what makes the exclusion sound,
and it does not transfer to a plaintext database.

**Rejected:** the vault in the sweep (it needs the key, can fail busy, D-14 refused it);
`user-profiles.db` excluded (its own promise would be false — a logical scrub whose database is not
swept is `w6/milestone/rewrite-needs-sweep`); "every database the request wrote to" unqualified (it
would sweep the vault).

**D-19-7 — Milestone arithmetic is historical.**

Rewriting a contributor out does not change `source_count` or `window_start`/`window_end`; the row
records what the milestone *was* consolidated from. The report **will** count `milestones_rewritten`
separately from the erased ones (H6, planned).

**Rejected:** recomputation — `Contributor` does carry `session_row_id` (`milestone.rs:31-32`), but
the purge hard-deletes the `agent_sessions` row it names, so the only survivor is the contributor
list the rewrite is shortening; recomputing from it would make the row describe the survivors and
silently rewrite history, and regenerating the merged summary needs the summarizer (an LLM call a
sweep must not make). Decrement is W3's deleted delta arithmetic (ADR-0018 D-18-12).

**D-19-8 — The merged `summary` is class C-H; its `embedding` is class V-inv; both are retained and both are named.**

The contributor entry (V) leaves; the distillate stays and is reported, so a rewritten milestone is
never reported as an erase. The embedding rides with the summary but is not folded into C-H: inversion
recovers substantial source text (`FINDINGS-DETAIL.md:495`), so the report names it as the residual it
is.

**Rejected:** regenerate (needs the summarizer); drop (destroys co-contributors' knowledge,
contradicting "knowledge is independent"); silence (the audit's `rewrite-keeps-derived-text`).

**D-19-9 — The vault tier is a per-request tri-state in `NOT_ERASED`; `Unknown` is scoped to it.**

`NOT_ERASED` grows to two tiers, `memories-shard` and `vault`, each carrying `erased: Yes | No | Unknown`. `memories-shard` is **`No` today**; the existing reason string names **`Phase 4` as the condition** under which that could change — a declaration, not a measurement. `vault` is **`Unknown`** until W8b ships the purge leg, and `Unknown` **on any failed open, never `No`**; `Yes` and `No` become reachable only when the leg can establish them, and what `Yes` is worth changes at W8c (key-generation destruction). No other tier ever answers `Unknown`.

**Rejected:** a boolean present/absent (a failed open reads as "nothing there" — the fail-open
direction the audit filed twice); leaving the vault out (`mem-vault/src/lib.rs:46` says the report is
"no longer complete or true" without it); a tri-state on every tier (it would print `Unknown` where
the answer is a known `No`).

**D-19-10 — The inventory table is normative: an artefact class not in the table is not in the promise.**

Any new writer adds its row before it ships. `.bak` reaping happens under the exclusive root only.

**Rejected:** a prose sentence (the audit's remediation: "as a table, not a sentence").

**D-19-11 — The wording obligation: three surfaces, and the facts each must state.**

(1) The delete response. (2) The prompt-delete copy — "removed from the **prompt history** now; N
memories learned from it were kept and can be deleted separately" (with `N` only when `memories` is a
number, and the unconditional sentence when it is `null`), plus "the session transcript still contains
it until you purge the session". (3) This ADR's own statement of the default configuration, with the
limits in the same section.

**Rejected:** API-only disclosure; a count rendered `0` when unknown.

## The inventory

Vocabulary:

| label | meaning |
|---|---|
| V | verbatim, or near-verbatim, user text |
| V-W | verbatim by watermark: a derived row carrying near-verbatim text, erased when the prompt's watermark test passes |
| V-R | verbatim retained: text the promise keeps on purpose |
| V-inv | invertible residual: a vector from which substantial source text can be recovered |
| C | derived knowledge that does not invert to its source |
| C-H | historical distillate: a paraphrase of a contributor set that no longer exists in full |
| copy | a byte copy of another artefact (inherits its source's class) or of a user-supplied file |
| none | no user text of any kind |

Artefacts:

| # | artefact (path pattern) | class | holds prompt text? | what this request does | in the sweep? |
|---|---|---|---|---|---|
| A1 | `{root}/user-prompts.db` + `-wal` | V | yes, verbatim (`sql.rs:44-58`) | row deleted now, bytes at sweep | yes |
| A2 | `{root}/user-profiles.db` + `-wal`, incl. `user_profile_changelogs.profile_data_snapshot` | V-W | near-verbatim, **derived** (`REPORT.md:243`; `sql.rs:166`) | rows + changelogs erased at delete by the watermark test (`erasure.rs:29-31`) | yes |
| A3 | `episodic.db` `episodic_milestones.contributors[].summary` | V | yes, verbatim (D-27) | contributor entry rewritten by `purge-session` | yes |
| A4 | `episodic.db` `episodic_milestones.summary` | C-H | paraphrase of the contributor set | retained, named (D-19-8) | yes (compaction) |
| A5 | `episodic.db` `episodic_milestones.embedding` | V-inv | invertible to the summary's text | retained, named, reported (D-19-8) | yes (compaction) |
| A6 | `episodic.db` `episodic_raw_context`, `episodic_segments`, `agent_sessions.summary` | V-R | yes, verbatim (`FINDINGS-DETAIL.md:163-171`) | retained; erased only by `purge-session` | no |
| A7 | `{root}/memories/*.db` + `-wal` | C | derived | rows deleted by id; knowledge outlives | no (declared) |
| A8 | `{root}/facts.db` | C | derived | erased by `purge-session`'s facts leg | no |
| A9 | `{root}/metadata.db` | none | no (counts, registry) | n/a | n/a |
| A10 | `{root}/vault/{scope_hash}/vault.db` | V | yes, encrypted | excluded; tri-state (D-19-9) | no (D-14) |
| A11 | `*.bak` — `.schema-v*`, `.legacy.bak`, `.pre-reembed*`, `.pre-encrypt*`, `.pre-path-migrate*`, `.{reason}-{pid}-{millis}.bak` | copy | yes, full copy incl. `-wal` | **reaped** | yes |
| A12 | `~/.opencode-mem/opencode-mem.log` + `.old` | none | no in the shipped configuration: the payload rule forbids raw context, so a violating call site would be the bug, not this row | excluded | no |
| A13 | portability exports, media, CoW/SSD/external backups | copy | yes (an export bundle is a copy; media is the user's file; snapshots are the filesystem's) | excluded | no |
| A14 | `{root}/**/*-tshm` | none | no (header counters + a page→frame index of two integers) | retained permanently: **the byte-0 shared lock is the legacy-liveness oracle**; presence, size and header are never signals (`delete_guard.rs:33-35`, `sqlite_files.rs:164-166`) | n/a |

## Blocker closures

| blocker | closed by | what changed |
|---|---|---|
| B6 | D-19-4 | The queue is refused; `PendingErase` may persist and the operator has a lever. |
| B7 | D-19-1 | The promise is three clauses, and the "agent's view" sentence is retired. |
| B8 | D-19-10 + the inventory | The inventory is a normative table naming `.bak`, the profile class, the vault, logs, exports and media; the canary extension is H8. |
| B9 | D-19-2 + D-19-3 + D-19-11 | `PendingErase` reaches the delete response, the residual is labels rather than paths, and the copy states what was kept. |
| B10 | D-19-5, D-19-6, D-19-7, D-19-8 | The rewrite, its sweep set, the historical arithmetic and the retained distillate are each decided. |
| B11 | D-19-9 | `NOT_ERASED` grows the vault tier with a per-request tri-state. |
| B12 | out — ADR-0025 | The guard's permanence against pre-guard binaries moves with ADR-0025's blockers 1–5; ADR-0025 does not exist yet, and this ADR does not create it. |

## W6 hand-off

| item | what | status |
|---|---|---|
| H1 | the `erasure` field on both DTOs **and** the port widening (`PromptPort::delete_prompt` returns `()`, `ports.rs:485`; `store_adapter.rs:658-663` discards the report) | planned |
| H2 | the `DELETE /api/memories/{id}` mapping | planned |
| H3 | the web-ui rendering (the disclosure must bite; `web-ui/` is a separate workspace) | planned |
| H4 | `--mode=sweep` in the dispatch | planned |
| H5 | the sweep steps (checkpoint, vacuum where permitted, `.bak` reap under the exclusive root) | planned |
| H6 | the contributor rewrite and `EpisodicPurgeReport.milestones_rewritten` (no rewrite in `purge.rs` today; no such field anywhere) | planned |
| H7 | the `NOT_ERASED` second tier and the per-run tri-state (`NotErasedTier` has `tier` and `reason` only) | planned |
| H8 | the canary extension (a deleted prompt's marker absent from every file under the root, **including `*.bak`** and the profile snapshots) | planned |
| H9 | the status read (`GET /api/prompts/{id}/erasure` does not exist today; it is built with W6) | planned |
| H10 | the open guard (`db.rs:386-430`, preflight `395`, postflight `430`; issue 45, `8b635a8`) | shipped |
| H11 | the watermark eraser (`prompt.rs:350-387`, `erasure.rs:29-31`; issue 48) | shipped |

## Consequences

- **The operator pays for the lever.** One more CLI mode (`--mode=sweep`, planned) and the obligation to run it; between runs the bytes wait.
- **The agent changes nothing.** It never saw the prompt timeline, and the memories it queries are the retained class C.
- **The API pays one additive field and one signature.** Both DTOs gain `erasure`, and `PromptPort::delete_prompt` must stop returning `()` (`ports.rs:485`, `store_adapter.rs:662`).
- **The report pays two counts and a tier.** `milestones` and `milestones_rewritten` are separate numbers, and the vault tier answers `Unknown` for a while.
- **The support story pays candour.** An indefinitely-pending state is documented and counted, not implied by silence.

## Residuals

- **R-19-1** — `PendingErase` may persist indefinitely: there is no queue and no schedule, by D-19-4.
- **R-19-2** — The sweep's physical step needs both the engine-wide multiprocess-WAL drop and `experimental_vacuum(true)` on the erasure handle; the flag is not what refuses `VACUUM`, the feature is. Until both, a sweep truncates the `-wal` but cannot rewrite the main file's pages.
- **R-19-3** — `surviving_derived.memories` is `null`: `link_memory_to_prompt` (`prompt.rs:453`) has no caller, and `delete_memory_by_id` is single-shard until issue 72 lands.
- **R-19-4** — The milestone distillate (A4) and its embedding (A5) survive by decision (D-19-8).
- **R-19-5** — The vault's residue is ciphertext until W8c; a key holder recovers it (ADR-0012 D-14).
- **R-19-6** — CoW snapshots, SSD wear-levelling and external backups are outside the promise by construction.
- **R-19-7** — `.bak` reaping reaches only the exclusive root.
- **R-19-8** — The host transcript (A6) survives a prompt delete by design.
- **R-19-9** — `-tshm` is retained permanently; the byte-0 lock is the oracle, the file is not a signal.
- **R-19-10** — The sweep's liveness is unmeasured (`w6/erase/sweep-liveness`); this ADR adopts the audit's second closure ("opportunistic and unbounded") rather than inventing a bound.

## Security and edge cases

- **Never promise physical erasure.** The default is plaintext (`schema.rs:612`) and `secure_delete` is unimplemented (`turso_capabilities.rs:1243-1259`); any sentence that reads as "the bytes are gone" is the defect the audit was written about.
- **Do not lift ADR-0012 D-14 onto a plaintext database.** D-14 works because the vault is ciphertext; `user-prompts.db` is not, which is why clause 1 says the bytes leave only at a sweep and D-19-6 excludes the vault for the opposite reason.
- **Do not let `NOT_ERASED` fail open.** A failed vault open answers `Unknown`, never absence; a boolean present/absent is the fail-open direction the audit filed twice (blockers 2 and 11). Equally, do not over-correct: `memories-shard` is a known no, and printing `Unknown` there would be a new lie.
- **`-tshm` is a file, not a signal.** The byte-0 shared lock is the legacy-liveness oracle; presence, size, header and magic are never signals (`delete_guard.rs:33-35`, `sqlite_files.rs:164-166`). A guard that reads them refuses every open forever after a crash.
- **Crossed DTO names.** `DeletedMemoryDto.deleted_prompt` and `DeletedPromptDto.deleted_memory` are both real; cite both and say which route owns which, or a reader will "fix" the wrong struct.
- **The stale-line trap.** The audit's line numbers have drifted with W2 and W3: `schema_migrations.rs:421` is now `:525`, and the design's `schema.rs:602` is now `:612`. Re-derive, never inherit.
- **`milestones_rewritten` must not be added to `milestones`.** Folding it in silently converts a rewrite into an erasure, which is the second half of the audit's `rewrite-needs-sweep`.
- **`.bak` reaping scope.** Under the exclusive root only; never follow a symlink out; never touch `-tshm` (A14, permanently retained).
- **No upstream wording.** Decision 3 (never shared with upstream) forbids a compatibility sentence here.
- **The sweep is ADR-0025's dependent, not its rival.** The exclusive root is enforced by the open guard shipped at issue 45 (`db.rs:386-430`); a sweep against a live flag-on holder is unmeasured (`w6/guard/vacuum-under-legacy-holder`) and must not be attempted. This ADR states the dependency; it does not re-specify the guard.
