# Findings detail — W6 erasure design

Derived from `findings.json` (the authoritative record). Sorted by fingerprint. Severity is
present only for `confirmed` records; `needs_validation` carries no severity by contract.

**Reminder:** `confirmed` in this fleet means a static property of source — a complete trace
with no visible preventing layer — never a runtime-demonstrated result. No target code was
run.


---

## `w6/erase/disclosure-unreachable` — PendingErase has no reachable surface at the moment of deletion: frozen single-boolean DTOs, a separate status endpoint, and a 501 cleanup stub

**Verdict:** `confirmed`
 · **Overall:** **medium** (likelihood high, impact medium)
 · **Confidence:** high

Decision 6 forbids an unqualified 'deleted', yet the routed delete-surface shape keeps DeletedPromptDto and DeletedMemoryDto at one boolean each, moves erasure state behind a new GET /api/prompts/{id}/erasure endpoint, and leaves 'erase now' to a cleanup endpoint that is a documented 501 stub. A user who deletes a prompt receives {"deletedPrompt": true} and nothing else. The PendingErase contract therefore exists only for a caller that (a) already knows the prompt id, (b) knows the status route exists, and (c) chooses to poll it after a delete. Nothing in the design requires any surface the user actually sees to perform that read or to render the residual, so the design's own rule ('never an unqualified deleted') is satisfied in the API and violated in the product.

**Root cause.** The design separates erasure state from the call that creates it, and then relies on the caller to join them, while freezing the only response the caller is guaranteed to see.

**Intended behaviour.** Every successful delete must either answer with the erasure outcome (PendingErase plus residual and surviving-derived count) or be accompanied by a mandatory, same-interaction read that the UI is required to render. A status endpoint the user is never told to call does not satisfy decision 6.

**Trace.**

- *entrypoint* — `.scratch/rust-port/issues/43-database-base-hardening.md:100` — The owner-routed surface shape forbids extending the delete response and places erasure state on GET /api/prompts/{id}/erasure.
- *propagation* — `crates/mem-web/src/api/dto.rs:301` — DeletedPromptDto carries exactly one field, deleted_memory: bool, so the delete call cannot carry a residual, a request id or a surviving-memory count.
- *propagation* — `crates/mem-web/src/api/prompts.rs:44` — The handler answers Ok(Some(deleted_memory)) with that DTO and no erasure state; there is no field, header or follow-on call in the response path.
- *sink* — `crates/mem-web/src/api/maintenance.rs:234` — POST /api/cleanup is the documented 501 stub (CLEANUP_UNAVAILABLE, D-15), so 'erase now' has no implementation to attach to and the design must name a different lever.

**Evidence.**

- `.tmp/drafts/20261025-w6-design-decisions-consult.md:33` — The promise: 'never an unqualified deleted'.
- `crates/mem-web/src/api/dto.rs:294` — DeletedMemoryDto is likewise a single boolean (deleted_prompt).
- `crates/mem-web/src/api/memories.rs:569` — DELETE /api/memories/{id} answers with that single-boolean DTO.
- `crates/mem-web/src/api/maintenance.rs:55` — CLEANUP_UNAVAILABLE: 'cleanup is not available in this build'.
- `.tmp/drafts/20261025-w6-consult-response.md:352` — The consultant in fact requires the opposite: 'the delete call's return ... Default return is PendingErase { NotYetSwept }'. The routed default and the design's only worked-out surface disagree, and the disagreement was not resolved.

**Conditions.**

- *user_interaction* — A user deletes a prompt or a memory from the web UI and reads the returned envelope or the toast built from it.
- *system_configuration* — Default configuration: sweeps are opportunistic, so the common outcome of a delete is PendingErase, not Erased.

**Attacker perspective.** Not an attacker: the data subject, who is told 'deleted' and has no reachable statement that a physical erase is pending.

**Minimal input / fixture.**

- DELETE /api/prompts/{id} with a valid local token (the only prompt-delete entry point)

**Owner verification steps.**

1. Read crates/mem-web/src/api/prompts.rs:33-48 and crates/mem-web/src/api/dto.rs:298-303: the response is DeletedPromptDto{deleted_memory} and nothing else.
2. Read the routed default at .scratch/rust-port/issues/43-database-base-hardening.md:100: the delete DTO stays frozen and erasure state is a separate endpoint.
3. Read crates/mem-web/src/api/maintenance.rs:229-240: the only cleanup endpoint is a 501 stub.
4. Regression test to add: a delete whose request cannot be swept in the same tick must not be expressible as a bare success on any surface a user sees; assert the UI adapter's post-delete read of the erasure endpoint, or assert the response carries the outcome.

**Observed result (source-level).** Reading crates/mem-web/src/api/prompts.rs:44 against crates/mem-web/src/api/dto.rs:301 establishes that the prompt-delete response is one boolean; reading the routed default at .scratch/rust-port/issues/43-database-base-hardening.md:100 establishes that this is deliberate; reading crates/mem-web/src/api/maintenance.rs:234 establishes that the consultant's 'erase now' surface is a 501 stub. The design therefore contains no surface that both exists and is required to disclose PendingErase at delete time.

**Severity rationale.** Likelihood high: Every prompt and memory delete traverses these handlers, and the default scheduling makes PendingErase the normal outcome. Impact medium: The data subject is given a false finality signal for text that is still readable on disk and still present in derived artefacts; the effect is on the user's decision to stop looking and to keep or destroy the machine, which is exactly the harm decision 6 exists to prevent.

**Remediation.** Keep the DTO frozen only if the UI is contractually required to perform the post-delete erasure read in the same interaction and the ADR says so; otherwise add one additive field (erasure: {state, request_id, residual_count, kept_derived_count}) and treat the status endpoint as the follow-up detail, not the disclosure. In both cases add a regression test that a delete which cannot complete a sweep is never rendered as 'deleted'.

- `crates/mem-web/src/api/dto.rs` — `pub struct DeletedPromptDto { pub deleted_memory: bool, pub erasure: ErasureStateDto }`
- `crates/mem-web/src/api/prompts.rs` — `match delete_prompt_by_id(&state, &id, cascade, ENDPOINT).await? { Some(outcome) => envelope::ok(&DeletedPromptDto { deleted_memory: outcome.deleted_memory, erasure: outcome.erasure }), None => envelope::business_error("Prompt not found") }`

**Confidence.** All four source locations are read directly and the contradiction between the routed default and the consultant's §2.5 is explicit in the documents.


---

## `w6/erase/promise-overclaims-inventory` — The promise ranges over 'every file this tool writes' but the tool writes six families of never-reaped full-database backups, plus profile snapshots the sweep never visits

**Verdict:** `confirmed`
 · **Overall:** **medium** (likelihood medium, impact high)
 · **Confidence:** high

Decision 6's promise is scoped to files: 'on the next successful sweep, from every file this tool writes'. The inventory that scope actually covers is much larger than the database set the sweep can vacuum. Six distinct backup families write full copies of a database together with its -wal sidecar and no code path deletes them: .schema-v{N}-{millis}.bak, .legacy.bak, .pre-reembed-{nonce}.bak, .pre-encrypt-{millis}.bak, .pre-path-migrate-{nonce}.bak, and the shard quarantine copy {reason}-{pid}-{millis}.bak. A .bak of user-prompts.db or episodic.db is a complete plaintext copy of the text the promise says is erased, taken before a migration and retained indefinitely; the W5 prompts-v2 migration is precisely a destructive migration that will take one for every existing root. Separately, user-profile.db retains prompt-derived descriptions in the live row and in five versioned profile_data_snapshot blobs, and the design does not classify the profile at all. The design's own limits paragraph names filesystems, snapshots and external backups - none of which is the tool's own .bak.

**Root cause.** The promise was written against the database files the sweep can vacuum, while the tool's write set includes full-database copies and derived-text tables that no sweep enumerates.

**Intended behaviour.** The promise must either enumerate the artefact classes it covers, with the .bak families either reaped by the sweep or named as exclusions, or be narrowed to 'every database file this tool writes under the storage root'. Either way the profile and its changelog snapshots must be assigned a class.

**Trace.**

- *entrypoint* — `.tmp/drafts/20261025-w6-design-decisions-consult.md:33` — 'from every file this tool writes' - an unbounded file-scope claim.
- *propagation* — `crates/mem-store/src/sqlite_files.rs:48` — copy_sqlite_database copies the main file and every sidecar (-wal, -shm, -tshm), so each .bak is a self-contained copy of the database including un-checkpointed commits.
- *sink* — `crates/mem-store/src/schema_migrations.rs:73` — The backup path is {db}.schema-v{from_version}-{millis}.bak; the same file states at line 421 that no code path deletes a .bak.

**Evidence.**

- `crates/mem-store/src/schema_migrations.rs:421` — 'No code path deletes a .bak.'
- `crates/mem-profile/src/profile_store.rs:794` — Every profile update inserts a changelog row carrying the full profile snapshot.
- `crates/mem-store/src/sql.rs:166` — profile_data_snapshot TEXT NOT NULL - verbatim profile text retained per version.
- `crates/mem-vault/src/lib.rs:23` — The vault database lives at {storage_root}/vault/{scope_hash}/vault.db and holds session content; it is not in any sweep set.
- `.tmp/drafts/20261025-w6-consult-response.md:372` — The consultant's limits paragraph covers CoW/snapshots/wear-levelling and 'backups taken by anything else' - it does not cover the tool's own .bak files, which are the larger set.

**Conditions.**

- *system_configuration* — At least one destructive migration has run against the root (the scheduled prompts-v2 / shard-v3 waves do exactly this), or the shard quarantine path has fired.
- *data_state* — The backup is taken before the erased row's deletion, so it contains the pre-erasure text by construction.

**Attacker perspective.** Anyone who obtains the disk or a backup of it (stolen laptop, sync target, support bundle), after the user has been told the text was erased.

**Minimal input / fixture.**

- cat ~/.opencode-mem/data/user-prompts.db.schema-v1-<millis>.bak | grep -a '<prompt text>'

**Owner verification steps.**

1. Read crates/mem-store/src/shard_manager.rs:1033-1065, crates/mem-store/src/shard_path_migration.rs:660 and :748, crates/mem-store/src/legacy_migrator.rs:50, crates/mem-store/src/encryption_migrator.rs:315, crates/mem-store/src/migration_service.rs:416: six .bak producers.
2. Read crates/mem-store/src/schema_migrations.rs:57-86 and :421: the copy is taken before the destructive migration and nothing removes it.
3. Read crates/mem-store/src/sqlite_files.rs:45-56: the -wal sidecar is copied too, so the backup can contain un-checkpointed rows.
4. Owner-run check (read-only): list *.bak under the storage root and grep one for a known deleted marker string.

**Observed result (source-level).** Reading crates/mem-store/src/schema_migrations.rs:57-86 and :421 establishes that a full database copy named {db}.schema-v{N}-{millis}.bak is created before a destructive migration and is never removed; reading crates/mem-store/src/sqlite_files.rs:45-56 establishes that the copy includes the -wal; reading crates/mem-store/src/shard_manager.rs:1033-1065, shard_path_migration.rs:660, legacy_migrator.rs:50, encryption_migrator.rs:315 and migration_service.rs:416 establishes five further producers of the same shape. None of these files is a sweep target in the design.

**Severity rationale.** Likelihood medium: A backup exists only after a destructive migration or a quarantine event, but the scheduled W5 prompts-v2 and W2 shard-v3 migrations run them for every existing root. Impact high: A .bak of user-prompts.db is a complete, plaintext, indefinitely retained copy of the most sensitive table in the system; disclosure to the named adversaries (stolen laptop, sync target, support bundle) fully defeats the erasure promise.

**Remediation.** Enumerate the write set in ADR-0019 as a table, not a sentence: each row names a path pattern, whether it can hold prompt text, and whether the sweep covers it or it is an explicit exclusion. Add a sweep step that reaps .bak families older than the request being swept (or deletes them on the next successful sweep), a configuration knob for retention, and a regression test that inserts a canary, deletes it, runs the sweep, then greps every file under the root including *.bak.

- `crates/mem-store/src/schema_migrations.rs` — `// after a successful migration, unlink the backup, or register it in the sweep's .bak reaper:
let _ = std::fs::remove_file(&backup_path);`

**Confidence.** Every cited producer and the absence of any reaping path are read directly from source; only the per-root presence of a .bak is environmental.


---

## `w6/erase/sweep-liveness` — Whether a 'successful sweep' is reachable at all on a busy engine is unmeasured, and the sweep's close/reopen window has no stated quiesce rule

**Verdict:** `needs_validation`


The promise is conditioned on 'the next successful sweep', so its liveness is part of its meaning. Two source facts bear on it. First, VACUUM refuses rather than waits when another connection holds a write transaction (pinned Busy refusal), so a sweep can only succeed in a quiescent window. Second, the only reachable erasure shape is close the main handle, open with experimental_vacuum, VACUUM, reopen (a second same-path open inherits the first handle's option set, so there is no second vacuum-enabled handle). During that window the engine holds no handle on the file and no rule in the design prevents another task from reopening it, which would make the VACUUM busy and the request permanently PendingErase. Whether the engine's own concurrency (db_lane serialisation, capture drain, consolidation) actually leaves a window long enough for a sweep, and how often the sweep is starved, is a runtime property that cannot be settled from source and was not measured.

**Claimed root cause.** The design specifies what a sweep does but not the quiesce discipline that makes a sweep schedulable, so the promise's liveness term is unbounded and unstated.

**Trace.**

- *entrypoint* — `.tmp/drafts/20261025-w6-design-decisions-consult.md:34` — 'on the next successful sweep' - the promise has no bound on when that is.
- *propagation* — `crates/mem-store/tests/turso_capabilities.rs:4499` — The pinned probe requires VACUUM to return Busy rather than block, so every concurrent write transaction converts a scheduled sweep into a failed one.
- *sink* — `crates/mem-store/tests/turso_capabilities.rs:3470` — The close-then-reopen probe shows the sweep needs the primary handle closed, which is exactly the window in which another task can reopen the file without the vacuum option.

**Evidence.**

- `crates/mem-store/tests/turso_capabilities.rs:4546` — The Busy variant is the pinned refusal shape for VACUUM under a live write transaction.
- `crates/mem-store/tests/turso_capabilities.rs:3423` — A second same-path open inherits the first Database's options, so 'sweep beside the engine' is not available.
- `crates/mem-bridge/src/db_lane.rs:5` — The db queue is the existing serialisation point the sweep would have to join to get a quiescent window.
- `.tmp/drafts/20261025-w6-consult-response.md:352` — PendingErase is described as the default outcome because sweeps are opportunistic; the documents never bound how long opportunistic can last.

**Blockers.**

- The sweep's starvation rate depends on engine concurrency (capture drain, consolidation, web reads) and cannot be derived from source.
- No measurement exists of the close/reopen window under a concurrent writer: whether the VACUUM always loses, or usually wins, is unknown.
- The design does not state whether the sweep participates in the db_lane at all, so the intended quiesce rule cannot be inferred.

**Validation plan (local).** Owner-run, in a throwaway root, using the existing prose-free harness shape: start the engine, drive a steady stream of captures (or a loop of web reads plus one write every 50 ms), request two prompt deletes, and record the wall-clock time until each request reports Erased, plus the count of SweepFailed/Busy residuals. Repeat with the engine idle as the control. Also instrument whether the reopen during the window takes the vacuum option (the probe at turso_capabilities.rs:3256 already shows how to detect an inherited option set). This is a read-only observation of the tool's own behaviour on a disposable root, not a probe of user data.


---

## `w6/erase/view-clause-contradicts` — 'Removed from the agent's view immediately' contradicts the next clause: prompt text is retained verbatim in episodic raw context and the injected context is memories

**Verdict:** `confirmed`
 · **Overall:** **medium** (likelihood high, impact medium)
 · **Confidence:** high

Decision 6's first clause says deleting prompt text 'removes it from the agent's view immediately'. The same promise then says memories are a separate promise that may outlive the source, and the injected context is built from memories. So either 'the agent's view' means only the prompt timeline (in which case the clause says nothing about the agent and the user will read it as forgetting), or it means the model context (in which case the clause is false by the promise's own second half). Independent of that ambiguity, the same text is retained verbatim in episodic_raw_context and episodic_segments as whole-session rows keyed by (project_id, session_id, agent_id), and a prompt delete issues one DELETE against user_prompts. A per-prompt delete therefore cannot remove the text from the episodic store at all, and that store is what recall and any future episodic injection read. delete_old_prompts has the same shape. The design never defines the view, so the sentence can be read as a stronger claim than any mechanism supports.

**Root cause.** The promise is phrased in terms of the agent's view without defining the view, and the storage model keeps the same text in a second verbatim tier that a per-prompt delete cannot address.

**Intended behaviour.** The ADR must define 'the agent's view' explicitly (prompt timeline vs model context), state that injected memories and episodic retention are not affected by a prompt delete, and make the delete response name the surviving derived count so the user is not left to infer forgetting.

**Trace.**

- *entrypoint* — `.tmp/drafts/20261025-w6-design-decisions-consult.md:33` — 'removes it from the agent's view immediately and, on the next successful sweep, from every file this tool writes' followed immediately by 'Memories are a separate promise'.
- *propagation* — `crates/mem-profile/src/prompt.rs:289` — delete_prompt issues DELETE FROM user_prompts WHERE id = ? - one row in one database, no episodic leg, no sweep trigger.
- *sink* — `crates/mem-store/src/episodic_sql.rs:111` — episodic_raw_context stores raw_context TEXT NOT NULL per (project_id, session_id, agent_id); episodic_segments stores raw_content with byte offsets, so the transcript containing the prompt is retained until a whole-session purge.

**Evidence.**

- `crates/mem-bridge/src/inject.rs:405` — The injected block is built from list_memories_by_container_tag - i.e. from memories, which the same promise exempts.
- `crates/mem-store/src/episodic_sql.rs:124` — episodic_segments.raw_content is the transcript in contiguous segments; a per-prompt delete does not touch it.
- `crates/mem-web/src/api/prompts.rs:99` — The delete handler's only cascade is to the linked memory; there is no episodic leg.
- `.scratch/rust-port/issues/43-database-base-hardening.md:89` — Decision 9 classifies Contributor.summary as class V verbatim source text, confirming that the design already knows session text is retained separately from the prompt row.

**Conditions.**

- *data_state* — The prompt was captured into episodic raw context (soundMemory enabled, the default capture path) or a memory was derived from it.
- *user_interaction* — A user deletes one prompt expecting the model to stop seeing it.

**Attacker perspective.** Not an attacker: the data subject, whose mental model of 'delete my prompt' is 'the model no longer has it'.

**Minimal input / fixture.**

- DELETE /api/prompts/{id} after a normal capture of the same message

**Owner verification steps.**

1. Read crates/mem-profile/src/prompt.rs:285-293: delete_prompt is one DELETE on user_prompts.
2. Read crates/mem-store/src/episodic_sql.rs:111-134: the same transcript is retained in episodic_raw_context and episodic_segments.
3. Read crates/mem-bridge/src/inject.rs:405: injected context comes from memories.
4. Regression test to add: capture a canary prompt, delete the prompt row, then assert what the inject path and the episodic reads return; the test should pin whichever semantics the ADR chooses, and fail if the ADR's wording and the behaviour diverge.

**Observed result (source-level).** Reading crates/mem-profile/src/prompt.rs:289 establishes that a prompt delete removes exactly one row from user-prompts.db. Reading crates/mem-store/src/episodic_sql.rs:111-134 establishes that the same transcript is retained in episodic_raw_context and episodic_segments keyed by session, which a per-prompt delete does not address. Reading crates/mem-bridge/src/inject.rs:405 establishes that the injected context is composed from memories, which decision 6 explicitly exempts. The first clause therefore has no reading under which it is both true and about the agent.

**Severity rationale.** Likelihood high: Every prompt delete has this shape and every captured prompt is retained in the episodic tier. Impact medium: The user is led to believe the model no longer has text that remains fully readable to it; the harm is a wrong privacy decision, not a remote compromise.

**Remediation.** Rewrite the clause to name its surface: 'removed from the prompt history immediately; not removed from memories or from the session transcript until you purge the session'. Define the classes in the ADR table and make the delete response carry the count of surviving derived memories (and, once decision 9 lands, of milestones that retain the session's text).

- `crates/mem-web/src/api/dto.rs` — `pub struct DeletedPromptDto { pub deleted_memory: bool, pub kept_derived_memories: u32, pub erasure: ErasureStateDto }`

**Confidence.** The storage model and the delete handler are read directly; the ambiguity is in the design sentence itself, which is quoted.


---

## `w6/guard/oracle-cannot-detect` — Claim that the -tshm byte-0 lock cannot tell a live flag-on holder is present

**Verdict:** `rejected`


Refuted. The claim would make the whole guard vacuous. turso 0.8.3 takes the byte-0 lock in shared mode at open (PROCESS_LIFETIME_LOCK_OFFSET in shared_wal_coordination.rs, taken in both detect_open_mode branches) and holds it until the mapping drops, which is the Database's lifetime. A probe that requests a write lock on that byte therefore conflicts for as long as the holder lives, and that is exactly what turso's own reject_live_multiprocess_wal_for_legacy_open does (it opens the coordination file and asks open_mode()). The pinned mixed-mode measurement does not contradict this: turso's check is unreachable on the SDK open path (resolve_default_storage is skipped when storage is supplied), so nothing probed the byte in that cell. The oracle exists; the design's job is to reproduce it faithfully, which is a different finding (w6/guard/oracle-failopen).

**Claimed root cause.** The mixed-mode open succeeds, therefore the shared-memory lock cannot distinguish a live flag-on holder.

**Trace.**

- *entrypoint* — `crates/mem-store/tests/turso_capabilities.rs:4242` — The flag-off opener is accepted against a live flag-on holder - the observation the claim rests on.
- *sink* — `crates/mem-store/tests/turso_capabilities.rs:4415` — The cell asserts the flag-on opener created its -tshm next to the flag-off holder, i.e. the holder really was in the coordination path.

**Evidence.**

- `.scratch/rust-port/issues/43-database-base-hardening.md:96` — The routed default already states the correct mechanism: a live multiprocess holder holds a shared lock on -tshm byte 0.
- `crates/mem-store/tests/turso_capabilities.rs:4229` — The cell's own comment attributes the acceptance to the holder having opened with NoLock, not to a missing lock on -tshm.

**Why rejected.** Refuted by the pinned dependency source: the flag-on holder takes a shared 1-byte fcntl lock at offset 0 of the -tshm and holds it for the mapping's lifetime, and a write-lock probe on that byte conflicts; turso's own reject_live_multiprocess_wal_for_legacy_open is exactly this probe. The mixed-mode cell succeeds because that check is unreachable on the SDK open path, not because the byte is unlocked.


---

## `w6/guard/oracle-failopen` — The guard's -tshm oracle is specified without a primitive, a positive control or a post-open re-probe, and fails open in seven enumerated ways to the pinned silent-row-loss outcome

**Verdict:** `confirmed`
 · **Overall:** **high** (likelihood medium, impact high)
 · **Confidence:** high

The routed design makes the guard's correctness rest on one probe: the shared lock turso takes on byte 0 of {db}-tshm. The design does not say which lock primitive implements the probe, on which platforms, what a failed probe-open means, or that a positive control is required. The mechanism the probe must reproduce is narrow and undocumented: a 1-byte POSIX record lock at offset 0, taken in shared mode, held for the mapping's lifetime, using F_OFD_SETLK on Linux and F_SETLK elsewhere (chosen at compile time), with l_pid unreadable under OFD. Seven ways the probe can report free while a legacy holder is live, each with its consequence: (1) a flock(2)-based probe, the natural Rust choice, does not conflict with fcntl record locks on Linux, so it always reports free and every guarded open proceeds; (2) the holder is inside its open window - the database file is open, the -tshm does not exist yet or the byte is not yet locked - so the probe sees free (a genuine TOCTOU, closable only by probing again after the open, which is what turso itself does in resolve_default_storage); (3) the -tshm was unlinked while the holder lives (its own -tshm hygiene, a restore from a backup taken without sidecars, an interrupted rename, or a user following the consultant's tripwire advice), leaving the holder on an orphaned inode; (4) the holder opened read-only with no -tshm present, which takes no coordination lock at all and no database-file lock either; (5) the probe open fails with EACCES/EIO and the implementation maps 'cannot read' to 'free' - the same fail-open shape read_live_lock already has for the identity locks; (6) a non-Linux platform where the probe used the wrong fcntl flavour or a different lock domain; (7) Windows, where no legacy holder exists, so fail-open is harmless. Cases 1-5 each let a guarded open proceed against a live legacy writer, which is the measured cell-3 outcome: the opener's committed row is silently gone on the other side's next commit with integrity_check still reporting ok. A guard that fails open is worse than none, because it converts a deploy-rule hazard into a claim of protection. Cases 4 and 6 differ in consequence (reader-side inconsistency / no legacy holders), but they still must be enumerated so the ADR states the residual rather than implying uniform coverage.

**Root cause.** The design adopts an oracle by description ('the shared lock on byte 0') instead of by primitive, platform and failure semantics, and requires no test that can fail when the probe stops conflicting with the holder's lock.

**Intended behaviour.** ADR-0025 must name the exact primitive per platform (F_OFD_SETLK on Linux, F_SETLK elsewhere, 1 byte at offset 0, opened without taking any lock), define every failure of the probe open as 'unknown -> refuse' rather than 'free', probe both before and after the database open, and carry a pinned positive control: a child process holding a real flag-on handle must make the guard refuse. It must also state that an unlinked or restored-without-sidecars -tshm defeats the oracle, and that the oracle can never be replaced by file presence, file size or header magic, because stale debris of those shapes is measured inert and treating it as live would brick the root.

**Trace.**

- *entrypoint* — `.scratch/rust-port/issues/43-database-base-hardening.md:95` — The oracle is fixed as 'a live multiprocess holder holds a shared lock on -tshm byte 0, which is the only reliable legacy-liveness oracle' - a description, not an implementation contract.
- *propagation* — `.tmp/drafts/20261025-w6-consult-response.md:104` — legacy_peer_check uses 'the -tshm tripwire' by presence/absence (delete-it logic) rather than by lock state, and the routed default then removed that deletion while keeping the same wording - so the surviving text still does not say which primitive the oracle is.
- *sink* — `crates/mem-store/tests/turso_capabilities.rs:4285` — With the guard bypassed, the opener's committed row is absent from the quiet file while integrity_check reports ok - silent loss of committed user data.

**Evidence.**

- `crates/mem-store/tests/turso_capabilities.rs:4286` — The pinned final_ids assertion: the opener's row 100 is gone after both handles close.
- `crates/mem-store/tests/turso_capabilities.rs:4292` — The pinned integrity verdict: ok - the loss is silent.
- `.tmp/drafts/20261025-w6-consult-response.md:109` — 'if <db>-tshm exists: ... delete it' - presence-based logic the routed default replaced with retention but did not replace with a primitive.
- `crates/mem-store/src/operation_lock.rs:56` — read_live_lock already treats an unreadable lock file as no lock; a guard built in the same style inherits the fail-open.
- `crates/mem-store/src/platform.rs:47` — The shipped liveness probe is the in-repo precedent for 'the comment promises fail-closed, the code fails open'.

**Conditions.**

- *timing_dependency* — The holder's open window, a -tshm unlink, or a second open between probe and open.
- *third_party_dependency* — The probe must match turso's compile-time lock backend; a dependency change of backend silently invalidates it.
- *environmental_dependency* — A local holder on a filesystem where turso permits shared-WAL coordination and a non-Linux target where the fcntl flavour differs.

**Attacker perspective.** A local process or user that holds, creates or removes a -tshm path (or simply the ordinary pre-W6 build still running), against a guard that reports free.

**Minimal input / fixture.**

- A pre-W6 flag-on engine process holding the database open while the new build starts
- rm ~/.opencode-mem/data/<db>-tshm while that holder is live, then start the new build

**Owner verification steps.**

1. Read shared_wal_coordination.rs in the vendored turso 0.8.3 source (cargo registry): PROCESS_LIFETIME_LOCK_OFFSET, detect_open_mode, unix_shared_wal_lock_byte and Drop establish the shared 1-byte lock and its lifetime.
2. Read database.rs resolve_default_storage and the rsapi open path: the two reject probes are only reachable when OpenOptions::storage is unset, which the SDK path never leaves unset.
3. Write the guard's probe as a positive control first: a child process opens the database with experimental_multiprocess_wal(true), holds it, and the guard must report legacy-live. Then invert the child to a flag-off holder and require the guard to report free - a probe that cannot pass both arms is not an oracle.
4. Add the unlink arm as a documented residual: hold the flag-on handle, unlink the -tshm, and record that the guard reports free. This must fail the test if the ADR claims the unlink case is covered.

**Observed result (source-level).** Reading the vendored turso 0.8.3 sources establishes that the holder's lock is a 1-byte POSIX record lock at offset 0 taken shared for the mapping lifetime, with the primitive selected at compile time (F_OFD_SETLK on Linux, F_SETLK elsewhere); that a read-only open takes neither that lock nor a database-file lock; and that turso's own cross-mode rejection is skipped on the SDK open path the tool uses. Reading .scratch/rust-port/issues/43-database-base-hardening.md:95-99 against .tmp/drafts/20261025-w6-consult-response.md:100-114 establishes that the design fixes the oracle by description only - no primitive, no platform matrix, no 'unknown means refuse' rule and no positive control - so each of the enumerated free readings reaches crates/mem-store/tests/turso_capabilities.rs:4285's silent-loss outcome.

**Severity rationale.** Likelihood medium: Depends on the implementation the ADR does not yet constrain; at least three of the seven free readings (flock, probe-open error, unlink) are what a straightforward implementation produces. Impact high: The consequence is the pinned mixed-mode outcome: committed user rows silently absent, integrity_check ok, no error on either side - the exact hazard the guard was adopted to prevent.

**Remediation.** Specify the oracle as code-level contract in ADR-0025, then implement it in one seam with a positive control before any other W6 work. Prefer not to reimplement: file the missing wiring upstream (turso's rejection exists but is unreachable from the SDK builder) and in the meantime keep the probe behind a single function whose contract is 'returns Held only when a conflicting lock was observed; any other outcome, including an I/O error, is Unknown and the caller refuses'.

- `crates/mem-store/src/legacy_peer.rs` — `pub enum Oracle { Free, Held, Unknown } // Unknown -> callers refuse; never map an open()/fcntl() error to Free`

**Confidence.** The mechanism is read from the pinned dependency source; the design's omission is read from the decision text; the consequence is a pinned test in the repository.


---

## `w6/guard/retirement-nonsequitur` — The guard's retirement condition cannot be met by the binaries it defends against

**Verdict:** `confirmed`
 · **Overall:** **low** (likelihood medium, impact high)
 · **Confidence:** high

The routed default retires the guard once SchemaNewerThanBuild (43.8 / ADR-0021) ships 'in every fielded build' and W6 bumps user_version, after which the guard becomes defence-in-depth. The guard exists because a pre-W6, flag-on build may be live against the same root. A pre-W6 binary does not contain the version check and cannot be given it; bumping user_version does not add one to it. So the retirement condition is satisfied by builds that are not the hazard and is unsatisfiable for the builds that are. Worse, the version bump makes the legacy build's behaviour less predictable, not more: it opens a newer file with no check at all. The only condition that actually retires the guard is a support-policy one: no fielded binary older than the guard can still be launched against the root (a minimum-supported-version floor plus a release note), and 43.8's check then protects the rollback direction - a different hazard.

**Root cause.** The retirement criterion names a control that lives in new builds while the threat is old builds, conflating the rollback hazard (newer file, older build) with the mixed-version hazard (old holder, new opener).

**Intended behaviour.** ADR-0025 must state that the guard is permanent for as long as any pre-guard binary is supported, and that SchemaNewerThanBuild protects only the newer-file/older-build direction. If a retirement date is wanted, express it as a minimum-supported-version policy with a release-note obligation, not as a schema fact.

**Trace.**

- *entrypoint* — `.scratch/rust-port/issues/43-database-base-hardening.md:104` — 'Guard retirement: permanent until ADR-0021/43.8 (SchemaNewerThanBuild) ships in every fielded build and W6 bumps user_version'.
- *propagation* — `.scratch/rust-port/issues/43-database-base-hardening.md:44` — Decision 2: a root written by a newer build refuses to open in the new build. The check is a feature of the new build.
- *sink* — `crates/mem-store/src/engine_lock.rs:3` — The engine lock is the only mixed-version guard today, and it works because both builds share it - which is exactly what a pre-W6 build cannot do for a new check it does not contain.

**Evidence.**

- `crates/mem-store/src/schema_migrations.rs:124` — The version mechanism is a per-database user_version + ordered migrations; a pre-W6 build reads the number it knew about and has no refusal branch.
- `.tmp/drafts/20261025-w6-consult-response.md:94` — The consultant reaches the opposite conclusion from the same premise: 'the window recurs ... Keep the guard permanently' - because rollback reopens it.

**Conditions.**

- *third_party_dependency* — A pre-W6 binary remains launchable (an old install, a pinned wrapper, a rollback).

**Attacker perspective.** A user or process running any pre-W6 build against the root after the guard was retired as 'defence-in-depth only'.

**Minimal input / fixture.**

- A pre-W6 binary launched against a W6 root after the guard is removed

**Owner verification steps.**

1. Read .scratch/rust-port/issues/43-database-base-hardening.md:105 (retirement) and :44 (what 43.8 does).
2. Read crates/mem-store/src/schema_migrations.rs:100-160: the check is executed by the build that contains it.
3. Fix the ADR text: replace the schema-based condition with a minimum-supported-version policy and state that the guard is permanent while any older binary is supported.

**Observed result (source-level).** Reading .scratch/rust-port/issues/43-database-base-hardening.md:105 against :44 establishes that the retirement condition depends on a check that only new builds execute, while the hazard the guard addresses is an old build holding the database; the two do not intersect.

**Severity rationale.** Likelihood medium: Retirement is a future act; if it happens on the stated condition the hazard returns, but the trigger requires someone to follow the written rule. Impact high: Re-running the mixed-mode window without a guard is the pinned silent-loss scenario.

**Remediation.** Restate the condition in ADR-0025 as a support-policy floor, and make the release note the enforcement point. Keep the guard as defence-in-depth only after that floor is announced and the previous version is out of support.

- `.scratch/rust-port/issues/43-database-base-hardening.md` — `Guard is permanent while any pre-guard binary is supported; 43.8 covers the newer-file/older-build direction only.`

**Confidence.** Both sentences are quoted from the decision record and their interaction is a matter of what a binary contains.


---

## `w6/guard/tshm-probe-portability` — Whether a mem-store probe reproduces turso's per-platform lock flavour - and how it behaves in the selectable-WAL future - is unmeasured

**Verdict:** `needs_validation`


Implementing the oracle outside turso means re-deriving the dependency's compile-time lock backend. On Linux the holder uses F_OFD_SETLK, which is per open-file-description, so a probe from a fresh descriptor conflicts even inside the same process; on other Unix the holder uses F_SETLK, which is per process, so the conflict semantics differ and a same-process probe does not conflict with itself. l_pid is not populated for OFD locks, so the guard cannot learn who the holder is and cannot implement an 'it is me' exception by identity. That matters for the recorded owner intent (selectable WAL behaviour): once a build can select multiprocess mode, the engine itself holds byte 0 on its own databases, and a naive guard would refuse to open its own database. The repository contains no probe of any of this; the pinned tests exercise turso's own opens, not a tool-side byte-range probe.

**Claimed root cause.** The oracle is reimplemented from a private dependency internal whose locking backend is chosen by cfg(target_os) and whose holder identity is not readable, so portability and self-conflict behaviour are assumptions rather than measurements.

**Trace.**

- *entrypoint* — `.scratch/rust-port/issues/43-database-base-hardening.md:96` — The routed default asserts the byte-0 shared lock as the legacy-liveness oracle without naming a primitive or platform.
- *propagation* — `crates/mem-store/tests/turso_capabilities.rs:4416` — The existing cells prove turso's own opens behave as measured; none probes byte 0 from outside turso.
- *sink* — `crates/mem-store/src/platform.rs:46` — The repo already carries a platform-divergent liveness primitive whose divergence was wrong, which is the precedent for requiring a measured probe rather than an inferred one.

**Evidence.**

- `.scratch/rust-port/issues/43-database-base-hardening.md:69` — The marker must record {pid, start_time, mode} and the guard must read the truth rather than infer it - the same discipline has to apply to the lock flavour.
- `crates/mem-store/tests/turso_capabilities.rs:4372` — The mixed-direction cell shows a flag-on opener bypassing the database-file lock, so the guard cannot be replaced by the .db lock either.

**Blockers.**

- No measurement of a tool-side F_OFD_SETLK / F_SETLK probe against a live flag-on holder exists in the repository.
- Whether the guard can distinguish 'held by this process' from 'held by a peer' depends on lock-flavour semantics that are not exposed (l_pid is -1 for OFD locks).
- The selectable-WAL mode is unscheduled, so the self-conflict case has no pinned shape yet.

**Validation plan (local).** In a throwaway copy of the probe harness, add two child processes: one holds a flag-on handle on a temp database, the other attempts (a) an F_OFD_SETLK write lock on byte 0 of the -tshm, (b) an F_SETLK write lock, and (c) a flock(LOCK_EX|LOCK_NB). Each arm records whether it conflicts. Run the same three arms on macOS if available. The expected matrix (Linux: OFD and F_SETLK conflict, flock does not; macOS: F_SETLK conflicts, OFD is unavailable) is the contract the ADR must pin; the test fails if the matrix changes with a turso or libc update. Then add the self-conflict arm: one process holds a flag-on handle and probes its own file through a fresh descriptor.


---

## `w6/guard/vacuum-under-legacy-holder` — What VACUUM does to a database held live by a flag-on build is unmeasured, so the guard's impact ceiling is unknown

**Verdict:** `needs_validation`


The guard's justification is that a guarded open against a live legacy holder loses the opener's committed rows. That is measured for an ordinary mixed-mode open. The sweep is worse than an ordinary open: it rewrites the whole file. No measurement exists of VACUUM running while a flag-on holder is live. The plausible outcomes span a range the ADR must not guess at: the legacy holder silently reads a torn image, the legacy holder's subsequent commit rewrites the file from its own stale WAL index and destroys the vacuumed content, the VACUUM itself refuses Busy, or the file is left integrity-clean but with one side's rows gone. The difference decides whether the fail-open cases in w6/guard/oracle-failopen are graded 'data loss' or 'corruption of a live peer', and therefore how much ceremony the guard needs.

**Claimed root cause.** The design inherits the ordinary mixed-open measurement as its worst case, but the sweep is a file rewrite and no probe covers a rewrite next to a live legacy holder.

**Trace.**

- *entrypoint* — `crates/mem-store/tests/turso_capabilities.rs:3470` — The sweep is VACUUM on a vacuum-enabled handle with the primary closed - a full file rewrite.
- *propagation* — `crates/mem-store/tests/turso_capabilities.rs:4499` — VACUUM under a same-database writer is measured to refuse Busy; the mixed cross-mode case is not measured.
- *sink* — `crates/mem-store/tests/turso_capabilities.rs:4242` — The pinned mixed-mode cell is an ordinary open plus writes, not a rewrite.

**Evidence.**

- `.tmp/drafts/20261025-w6-consult-response.md:142` — 'A VACUUM concurrent with an unaware writer is the worst outcome in this design' - stated as a judgement, with no measurement behind it.
- `crates/mem-store/tests/turso_capabilities.rs:3510` — The close-reopen erasure probe is hermetic: no second process exists during the sweep, so it cannot answer this question.

**Blockers.**

- No probe runs VACUUM while a flag-on child holds the same database.
- The outcome shape (peer corruption, row loss, Busy refusal, or clean rewrite) is a runtime property of the engine's WAL bookkeeping split.

**Validation plan (local).** Extend the existing child-process probe harness in a throwaway copy: parent creates a temp database, a child opens it with experimental_multiprocess_wal(true) and holds it with a committed seed row and an open write transaction; the parent (which must not hold a handle) opens the same file with experimental_vacuum(true), with the multiprocess flag off - this is the fail-open path the guard is supposed to make unreachable - and attempts VACUUM under a bounded timer. Record: VACUUM result, the child's read of its own rows afterwards, the child's ability to commit, the final row set through a fresh handle, and integrity_check on both sides. Non-vacuity control: the same sequence with the child flag-off must refuse the parent's open, so the arm is attributable to the mode difference.


---

## `w6/liveness/platform-probe-fails-open` — SystemProcessProbe contradicts its own contract off Linux, so D-39 steals a live engine's lock - and the guard is designed to reuse the same liveness logic

**Verdict:** `confirmed`
 · **Overall:** **high** (likelihood high, impact high)
 · **Confidence:** high

The production liveness probe is documented as deliberately conservative: 'answers true so a possibly-live foreign lock can NEVER be stolen'. The implementation is Path::new("/proc").join(pid).exists(), so on every non-Linux host it answers false for every pid. read_live_lock therefore treats a live holder as dead, unlinks its .engine.lock and returns None; acquire_engine_lock retries the exclusive create and succeeds. Two engines then own the root, the first engine's release() declines to remove a lock that is not its own, and no status file is written by the loser because it believes it won. On macOS - where the multiprocess WAL flag is on (for_config enables it for every non-Windows platform) - starting the new build while a pre-W6 engine is live makes the new engine take the lock, open flag-off against a NoLock holder, and reach the measured mixed-mode row loss on the first database they share. The same probe is what the guard's design says to reuse for 'is the owner live', so the guard inherits the inversion. On Linux the opposite defect exists: liveness is PID-only, so a recycled pid makes a stale lock read as live and the engine exits 75 until a human removes the file; a zombie holder does the same. The routed default describes the non-Linux failure as 'reports alive unconditionally ... which makes a crashed holder brick every open', which is the intended contract, not the shipped behaviour, and gets the direction of the Linux defect wrong as well.

**Root cause.** A platform-conditional liveness probe was implemented as a single Linux-only expression under a document that promises the opposite behaviour off Linux, and no test asserts the non-Linux branch.

**Intended behaviour.** One liveness primitive per platform, matching the documented contract: on Unix use kill(pid, 0) and treat ESRCH as dead and EPERM as alive; add a process start-time token to LockState so pid reuse cannot read as live; and assert both directions in a test (a live child is alive, a zombie or an exited child is dead) on every platform the tool ships.

**Trace.**

- *entrypoint* — `crates/mem-store/src/platform.rs:51` — is_alive returns /proc/<pid> existence, which is false for every pid on a host without /proc.
- *propagation* — `crates/mem-store/src/operation_lock.rs:73` — read_live_lock unlinks the lock file and returns None when the pid reads dead, so a live holder's lock is destroyed.
- *sink* — `crates/mem-store/src/engine_lock.rs:157` — try_create succeeds on the retry, so the second engine returns Acquired instead of Lost{holder_pid} and never writes the degradation status.

**Evidence.**

- `crates/mem-store/src/platform.rs:38` — The documented contract: 'answers true so a possibly-live foreign lock can NEVER be stolen'.
- `crates/mem-store/src/db.rs:260` — for_config enables MultiprocessWal for every non-Windows platform, so macOS roots are flag-on roots.
- `crates/mem-store/src/operation_lock.rs:35` — LockState carries pid, timestamp and operation only - no process start time, so pid reuse is indistinguishable from a live holder on Linux.
- `crates/mem-store/src/engine_lock.rs:166` — A still-live holder is the only thing that turns AlreadyExists into Lost; when liveness answers false the branch is unreachable.

**Conditions.**

- *environmental_dependency* — A non-Linux host without /proc (macOS), which the tool supports and where the WAL flag is enabled.
- *timing_dependency* — A second engine start while the first holds the lock.

**Attacker perspective.** A local process or user that starts a second engine (or an agent that launches one) against a root another engine owns.

**Minimal input / fixture.**

- opencode-agents-memory --mode engine (a second instance) against a root whose engine is live, on macOS

**Owner verification steps.**

1. Read crates/mem-store/src/platform.rs:36-53: the doc contract and the single Linux-only expression.
2. Read crates/mem-store/src/operation_lock.rs:56-79: the dead-pid branch unlinks the file.
3. Read crates/mem-store/src/engine_lock.rs:150-179: the retry converts the unlink into Acquired.
4. Regression test to add: with a child process holding the engine lock, acquire_engine_lock must return Lost on every supported platform; and with the child killed, it must acquire after exactly one retry. Both arms must run on the CI platforms the tool ships, not only Linux.

**Observed result (source-level).** Reading crates/mem-store/src/platform.rs:47-52 establishes that is_alive is /proc/<pid> existence and therefore false for every pid on a host without /proc, while the same file's comment at :38-43 states the probe answers true so a live foreign lock is never stolen. Reading crates/mem-store/src/operation_lock.rs:70-73 establishes that a dead-pid verdict unlinks the lock file, and crates/mem-store/src/engine_lock.rs:156-171 establishes that the acquire then succeeds on its retry. Reading crates/mem-store/src/db.rs:262-269 establishes that the multiprocess flag - and therefore the mixed-version hazard - applies to macOS.

**Severity rationale.** Likelihood high: On macOS every liveness answer is false, so any second engine start steals a live lock; no race or unusual configuration is needed. Impact high: D-39's single-owner guarantee is defeated and, in the mixed-version case the guard exists for, the new engine reaches the pinned silent loss of committed rows.

**Remediation.** Replace the probe with a per-platform liveness primitive that matches its own contract (kill(pid,0) on Unix: ESRCH dead, EPERM alive; /proc on Linux is acceptable only as an additional signal), add a start-time token to LockState so pid reuse cannot read as live, and pin both directions of the answer on every shipping platform. Do this before the guard reuses the same logic, and correct the routed default's description of the failure mode in issue 43.

- `crates/mem-store/src/platform.rs` — `fn is_alive(&self, pid: u32) -> bool { if pid == 0 { return false; } match unsafe { libc::kill(pid as libc::pid_t, 0) } { 0 => true, _ => std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) } }`
- `crates/mem-store/src/operation_lock.rs` — `pub struct LockState { pub pid: u32, pub start_time: Option<String>, pub timestamp: String, #[serde(default)] pub operation: Option<String> }`

**Confidence.** The implementation, its contradicting comment, both call sites and the platform gate are read directly; no runtime fact is needed for the inversion.


---

## `w6/milestone/rewrite-keeps-derived-text` — Rewriting only the contributor set leaves the removed session's text in the milestone summary, its embedding, and the window/source_count columns

**Verdict:** `confirmed`
 · **Overall:** **medium** (likelihood high, impact medium)
 · **Confidence:** high

Decision 9 replaces the milestone DELETE with a rewrite of the contributor set, correctly identifying Contributor.summary as class V verbatim text. But episodic_milestones also carries summary TEXT NOT NULL (the merged summary over those same contributors), embedding F32_BLOB NOT NULL (the vector of that summary), source_count, window_start and window_end. Rewriting contributors alone leaves the removed session's content inside the merged summary - a paraphrase that the design's own class-C definition says leaves with its source - and inside an embedding that inverts toward the source text, which the consultant's rule (§2.1) explicitly places in class V rather than C. source_count, window_start and window_end also describe the old contributor set, so a later reader is told the milestone summarises a session it no longer names. Re-deriving the merged summary at sweep time is not available: it needs the summarizer, which is an LLM call the sweep must not make. So the ADR must choose and state one of: regenerate the summary and embedding (out of scope for a sweep), drop the summary and embedding (destroys the other contributors' knowledge, contradicts 'knowledge is independent'), or declare the merged summary an explicitly disclosed residual of the erasure report. The third is defensible; silence is not, because the rewrite as specified reads as complete.

**Root cause.** Decision 9 treats the contributor array as the only carrier of the session's text, while the milestone row also stores a merged summary and its embedding derived from exactly that contributor.

**Intended behaviour.** The rewrite must name every column derived from the removed contributor, decide each one explicitly, recompute source_count and the window bounds, and report the surviving derived summary as a residual when it is kept.

**Trace.**

- *entrypoint* — `.scratch/rust-port/issues/43-database-base-hardening.md:89` — 'The purge must rewrite the contributor set instead of deleting the row' - the rewrite is specified over the contributor set only.
- *propagation* — `crates/mem-store/src/episodic_sql.rs:65` — The DDL carries summary, contributors, source_count, window_start, window_end and embedding, all NOT NULL, for one logical milestone.
- *sink* — `crates/mem-episodic/src/milestone.rs:63` — Milestone.summary is the merged summary over the contributors, and Milestone.embedding is its vector; both survive a contributors-only rewrite.

**Evidence.**

- `crates/mem-episodic/src/milestone.rs:46` — Contributor.summary is the retained verbatim source text (D-27) - the field decision 9 removes.
- `crates/mem-episodic/src/milestone.rs:76` — embedding is stored, NOT NULL, and 'populated at write time; never serialized and never read back' - so nothing regenerates it today.
- `crates/mem-store/src/episodic_sql.rs:73` — source_count is a stored column over the contributor set, not derived on read.
- `.tmp/drafts/20261025-w6-consult-response.md:246` — The consultant's rule puts embeddings in class V because inversion recovers substantial source text - the rule that makes the surviving embedding a leak.

**Conditions.**

- *data_state* — A milestone with two or more contributors, one of which is being purged.

**Attacker perspective.** Anyone who obtains episodic.db after a session purge (stolen laptop, backup, support bundle).

**Minimal input / fixture.**

- SELECT summary, source_count, window_start FROM episodic_milestones WHERE id = '<rewritten milestone>' after purging one contributor

**Owner verification steps.**

1. Read crates/mem-store/src/episodic_sql.rs:65-83 and crates/mem-episodic/src/milestone.rs:36-90: four stored artefacts derive from the same contributor set.
2. Read .scratch/rust-port/issues/43-database-base-hardening.md:89: only the contributor set is rewritten.
3. Regression test to add: build a milestone from two contributor summaries, purge one, then assert the surviving row against whichever policy the ADR picks - and fail if summary/embedding still contain the removed contributor's distinctive marker string while the report says the milestone was rewritten.

**Observed result (source-level).** Reading crates/mem-store/src/episodic_sql.rs:65-83 establishes that summary, embedding, source_count, window_start and window_end are stored columns of the same row as contributors; reading crates/mem-episodic/src/milestone.rs:63-76 establishes that summary and embedding are derived from the contributor set and are never regenerated; reading .scratch/rust-port/issues/43-database-base-hardening.md:89 establishes that decision 9 rewrites only the contributor set.

**Severity rationale.** Likelihood high: Every multi-contributor milestone purge takes this path; multi-contributor milestones are the normal consolidated shape. Impact medium: The purged session's content survives as a paraphrase and a vector in the row the purge claims to have rewritten, so the report is wrong about the most likely carrier of the text.

**Remediation.** In ADR-0019's class table, assign the merged summary and its embedding explicitly. Recommended: keep the merged summary as class K with a named residual in the erasure report (it is a distillation, and dropping it destroys the other contributors' knowledge), recompute source_count and the window bounds on every rewrite, and state that the embedding of a pre-rewrite summary is not regenerated - so it must be either cleared or reported. Never leave the row silently describing a contributor set it no longer has.

- `crates/mem-episodic/src/purge.rs` — `// rewrite: UPDATE episodic_milestones SET contributors = ?, source_count = ?, window_start = ?, window_end = ? WHERE id = ? -- and report kept_derived_summary: true`

**Confidence.** The stored columns and their derivation are read directly; only the ADR's chosen policy is open.


---

## `w6/milestone/rewrite-needs-sweep` — An in-place contributor rewrite is physically no safer than DELETE: the old row image stays in the page and WAL unless episodic.db joins the request's sweep set

**Verdict:** `confirmed`
 · **Overall:** **medium** (likelihood high, impact medium)
 · **Confidence:** high

Decision 9's intent is that the removed contributor's verbatim text must not survive. An UPDATE writes a new row image; the old one remains in the page freelist and in the -wal exactly as a deleted row would, until a checkpoint(TRUNCATE) plus VACUUM rewrites the file. So the rewrite moves the text from 'logically present' to 'physically present until the sweep', which is the same state DELETE leaves it in - unless episodic.db is a member of the erase request's sweep set. Nothing in the recorded decisions puts it there: decision 9 says what the purge must write, not what the sweep must vacuum, and the design's sweep-set language (the consultant's §2.4) is about shards holding scrubbed prompt text, not about episodic.db holding scrubbed session text. The same gap means the report vocabulary must change: EpisodicPurgeReport.milestones currently sums DELETE changes, so a rewritten milestone must not be counted as erased - it needs its own count and its own residual.

**Root cause.** The design conflates a logical scrub with a physical erase, and specifies the former without adding the file that now holds the scrubbed copy to the sweep set.

**Intended behaviour.** Every logical scrub must record the database it wrote to as a member of the request's sweep set, and the report must distinguish rows deleted from rows rewritten. A request whose sweep set includes episodic.db is Erased only after that database has been vacuumed and checkpointed.

**Trace.**

- *entrypoint* — `.scratch/rust-port/issues/43-database-base-hardening.md:89` — The rewrite is specified as a logical operation on the milestone row; no physical step is attached to it.
- *propagation* — `crates/mem-episodic/src/purge.rs:206` — The purge pushes DELETE_MILESTONE per pre-scanned id into one write batch and counts the rows it removed.
- *sink* — `crates/mem-store/src/episodic_sql.rs:416` — DELETE FROM episodic_milestones WHERE id = ? - the statement decision 9 replaces, in the one database the sweep set does not name.

**Evidence.**

- `.tmp/drafts/20261025-w6-design-decisions-consult.md:19` — The pinned measurement that a DELETE leaves the row image readable in the main file and the WAL until checkpoint and vacuum.
- `crates/mem-store/tests/turso_capabilities.rs:1127` — The canary survives in the main file after checkpoint without a vacuum - the physical fact the rewrite inherits.
- `.tmp/drafts/20261025-w6-consult-response.md:304` — 'an erase request's sweep set is every database the logical delete or scrub wrote to' - the rule that makes episodic.db a member, which the recorded decisions do not adopt.
- `crates/mem-episodic/src/purge.rs:224` — The milestone count is summed from DELETE changes, so a rewrite would silently be reported as an erasure.

**Conditions.**

- *data_state* — The request's logical scrub touched episodic.db.

**Attacker perspective.** Anyone who obtains episodic.db before its next vacuum (stolen laptop, backup, support bundle).

**Minimal input / fixture.**

- grep -a '<removed contributor marker>' ~/.opencode-mem/data/episodic.db ~/.opencode-mem/data/episodic.db-wal after a session purge

**Owner verification steps.**

1. Read crates/mem-episodic/src/purge.rs:206-225: one write batch, DELETE per milestone, count from changes.
2. Read .scratch/rust-port/issues/43-database-base-hardening.md:89 and .tmp/drafts/20261025-w6-consult-response.md:304-306: the rewrite is logical; only the consultant's rule would add the file to the sweep set.
3. Regression test to add: seed a milestone with a canary contributor summary, run the purge, then grep episodic.db and episodic.db-wal for the canary before the sweep and require absence after it; the pre-sweep half must be reported as PendingErase naming episodic.db.

**Observed result (source-level).** Reading crates/mem-episodic/src/purge.rs:206-225 establishes that the milestone leg is one write batch of DELETE statements whose row images the pinned measurements leave readable until checkpoint and vacuum; reading .scratch/rust-port/issues/43-database-base-hardening.md:89 against .tmp/drafts/20261025-w6-consult-response.md:304 establishes that the rewrite is specified without attaching episodic.db to the request's sweep set.

**Severity rationale.** Likelihood high: Every decision-9 rewrite lands in episodic.db, which no sweep set names. Impact medium: The verbatim text remains greppable in the database the design just rewrote, until an unrelated sweep happens - and the report says erased.

**Remediation.** Adopt the sweep-set rule as a decision, not a suggestion: record every database a logical delete or scrub wrote to, on the request, at delete time. Add episodic.db to it for decision 9's rewrite, add a milestones_rewritten count distinct from milestones_erased, and make the request Erased only after every recorded database has been checkpointed and vacuumed.

- `crates/mem-episodic/src/purge.rs` — `report.milestones_rewritten += changes_at(offset + 4 + index); // and the caller records DbLabel::Episodic in the request's sweep set`

**Confidence.** The write path, the reporting arithmetic and the sweep-set omission are all read directly.


---

## `w6/purge/queue-automates-erasure` — The enqueue design has no specified authorisation or human gate, and its only worked-out version has the engine execute queued deletions automatically at start-up

**Verdict:** `confirmed`
 · **Overall:** **medium** (likelihood medium, impact medium)
 · **Confidence:** high

Decision 8 converts purge-session from a second opener into an enqueue. That is the right call for the lock problem, but the decision record specifies neither who may enqueue, how an entry is authorised, when the engine executes it, nor what bounds it. The only concrete version on the table is the consultant's: the refused purge writes a spool entry, 'the engine drains the spool at start-up, after its own clean check'. That shape makes erasure automatic - the module's own invariant says erasure 'is a SEPARATE operation and is never automatic: no timer, no sweep, no flag, no consolidation stage calls this module', and a start-up drain of a directory is exactly such a stage - and it removes the only human-intent gate the tool has today, the --confirm string typed into purge-session, because the entry is written by whichever process was refused. In this product the realistic writer of such an entry is the agent itself: it runs as the user, it can run shell commands, and prompt-injected content can tell it to. The earlier review's framing ('unauthenticated cross-process write path, a deletion oracle') over-states the boundary (see w6/queue/deletion-oracle-crossuser) but under-states the real defect: the design turns an explicit, confirmed, owner-initiated deletion into a durable request that a lower-trust path can create and the engine will honour without a human in the loop.

**Root cause.** The decision adopts enqueueing as a transport change but leaves the authorisation model, the execution trigger and the bounds unspecified, while the only concrete mechanism fills that gap with an automatic start-up drain.

**Intended behaviour.** ADR-0019 must state: the enqueue is a request, not an instruction; the engine never deletes on drain without an owner-present confirmation bound to that request; the entry contains ids only, is validated against a live row, is bounded in count and size, and is refused (not silently dropped) when the bounds are exceeded. If those constraints are not worth their complexity, drop the queue and make the CLI refuse with a typed PendingErase that tells the owner to stop the engine and retry.

**Trace.**

- *entrypoint* — `.scratch/rust-port/issues/43-database-base-hardening.md:88` — purge-session enqueues and the engine performs the purge - no authorisation, trigger or bound is stated.
- *propagation* — `.tmp/drafts/20261025-w6-consult-response.md:163` — 'The engine drains the spool at start-up, after its own clean check' - execution on a timer the user did not initiate.
- *sink* — `crates/mem-episodic/src/purge.rs:11` — 'Erasure is a SEPARATE operation and is never automatic: no timer, no sweep, no flag, no consolidation stage calls this module'.

**Evidence.**

- `crates/mem-bridge/src/purge.rs:431` — refuse_missing_confirmation: the existing human-intent gate the enqueue bypasses if the entry, not the invoker, is authenticated.
- `crates/mem-bridge/src/purge.rs:79` — The confirmation is the literal project:session:agent triple - reproducible by anything that knows the ids, so it is only a human gate when it is typed at a TTY by the owner.
- `.tmp/drafts/20261025-w6-consult-response.md:159` — 'The refused purge writes a spool entry (ids only, never text) outside the databases' - the write path the queue adds.
- `crates/mem-bridge/src/capture.rs:639` — The repo already has the right precedent: a durable, idempotent, engine-owned inbox with per-entry terminal states; a purge queue should reuse that shape rather than invent a directory.

**Conditions.**

- *authentication_level* — Any same-uid process, including the agent driven by untrusted model output.
- *user_interaction* — The user does not confirm the drained request; the confirmation happened at enqueue time, in a process the user may not have run.

**Attacker perspective.** A same-uid process that can create one queue entry - realistically the AI agent, after ingesting untrusted content.

**Minimal input / fixture.**

- write one entry naming a project/session/agent triple into the engine's queue location

**Owner verification steps.**

1. Read .scratch/rust-port/issues/43-database-base-hardening.md:88 and .tmp/drafts/20261025-w6-consult-response.md:159-164: enqueue plus start-up drain, with no authorisation step.
2. Read crates/mem-episodic/src/purge.rs:9-13: the module's own no-automation invariant.
3. Read crates/mem-bridge/src/purge.rs:67-82 and :429-439: the existing confirmation gate and what it actually proves.
4. Design test to add: an entry created by a process that did not present an owner confirmation must not cause a deletion on drain; it must surface as a pending request that an owner action resolves.

**Observed result (source-level).** Reading .scratch/rust-port/issues/43-database-base-hardening.md:88 establishes that decision 8 specifies enqueue-and-engine-performs with no authorisation, trigger or bound; reading .tmp/drafts/20261025-w6-consult-response.md:159-164 establishes the only worked-out trigger as an automatic start-up drain; reading crates/mem-episodic/src/purge.rs:9-13 establishes that the module forbids exactly that shape.

**Severity rationale.** Likelihood medium: Requires a same-uid writer to create an entry; that writer exists by design in this product (the agent) and is reachable by untrusted content. Impact medium: Irreversible deletion of one session's episodic rows and its milestones' contributor text, with no human in the loop; scope is one session, not the root.

**Remediation.** Preferred: no persistent queue. The CLI refuses with a typed outcome (exit 75, PendingErase{EngineRunning}) that names the next action, and the owner re-runs purge-session once the engine is stopped - zero new state, zero new write path, and the existing confirmation gate stays where it is. If a queue is kept: put it inside the engine-owned store (reuse the capture inbox pattern), ids-only, O_EXCL single-writer, 0600/0700, bounded count and size with a loud refusal instead of silent eviction, every entry revalidated against a live row at drain, and the drain must only mark requests pending - execution requires an owner action (a TTY-confirmed CLI or an authenticated UI action), so erasure stays owner-initiated.

- `crates/mem-bridge/src/purge.rs` — `Err(PurgeError::EngineRunning) => { structured_stderr("purge.pending_erase", "warn", "purge", json!({"reason":"engine_running","action":"stop the engine and re-run purge-session"})); 75 }`

**Confidence.** The decision text, the consultant's only concrete drain, the module invariant and the existing confirmation gate are all read directly.


---

## `w6/purge/report-omits-vault` — The purge report's not_erased names one tier while the vault holds session content - and the vault crate says so

**Verdict:** `confirmed`
 · **Overall:** **medium** (likelihood high, impact medium)
 · **Confidence:** high

Decision 6 requires that an erasure report 'names the residual'. The shipped report carries a single NotErasedTier entry, 'memories-shard'. The vault crate's module documentation states the opposite of completeness in its own words: the vault now holds content for any session that marked it, purge.completed's not_erased 'is no longer complete or true without naming the vault', and 'a purge run today erases no vault rows and does not say so'. This is not a W6 regression; it is a residual the W6 promise inherits and must fix, because ADR-0019 will be the document users and support read when a purge claims completion. It also cannot be solved by the W6 sweep: the vault is encrypted and its guarantee is key destruction at whole-database scope (ADR-0012 D-14, W8c), so the correct treatment is an explicit, named exclusion plus a per-request statement of whether the request touched the vault at all.

**Root cause.** The residual vocabulary predates the vault tier and was never extended when the vault began storing session content.

**Intended behaviour.** NOT_ERASED must enumerate every tier the request did not erase - at minimum memories-shard and vault - and the vault entry must state why (encrypted, key destruction is whole-database, W8c) and whether the request's session has vault rows.

**Trace.**

- *entrypoint* — `crates/mem-bridge/src/purge.rs:114` — NOT_ERASED is a one-element array: memories-shard, 'no per-session provenance; Phase 4'.
- *propagation* — `crates/mem-bridge/src/purge.rs:397` — print_completed serializes not_erased from that constant, so purge.completed cannot mention the vault.
- *sink* — `crates/mem-vault/src/lib.rs:45` — 'purge.completed's not_erased is no longer complete or true without naming the vault ... a purge run today erases no vault rows and does not say so.'

**Evidence.**

- `crates/mem-vault/src/lib.rs:23` — The vault database is per scope at {storage_root}/vault/{scope_hash}/vault.db and holds session content.
- `crates/mem-bridge/src/purge.rs:136` — The report's own doc comment: 'so a success is never a complete-erasure claim' - true only for the one tier it names.
- `.tmp/drafts/20261025-w6-design-decisions-consult.md:159` — ADR-0012 D-14 refuses VACUUM and secure_delete in the vault path because the vault's guarantee is key destruction - the reason the vault is an exclusion rather than a sweep target.

**Conditions.**

- *data_state* — The purged session marked content for the vault (W8a is landed).

**Attacker perspective.** Anyone who obtains the root plus the key file after being told the session was purged - the vault rows decrypt.

**Minimal input / fixture.**

- purge-session --project P --session S --agent A --confirm P:S:A  (then read the emitted not_erased)

**Owner verification steps.**

1. Read crates/mem-bridge/src/purge.rs:113-117: one tier.
2. Read crates/mem-vault/src/lib.rs:44-50: the crate states the gap in its own module doc.
3. Regression test to add: a purge of a session with vault rows must emit a not_erased entry naming the vault, and must not emit 'erased' as a complete statement.

**Observed result (source-level).** Reading crates/mem-bridge/src/purge.rs:114 establishes that NOT_ERASED contains exactly one tier; reading crates/mem-vault/src/lib.rs:44-50 establishes that the vault holds session content and that the crate itself documents the report as incomplete and untrue without naming it.

**Severity rationale.** Likelihood high: Every purge of a vault-marked session has this shape. Impact medium: An erasure report that a user reasonably reads as complete while a whole encrypted tier of the same session survives.

**Remediation.** Extend NOT_ERASED to the full tier list in the same commit that writes ADR-0019, and make each entry carry whether it applies to this request. Keep the vault's exclusion tied to ADR-0012 D-14 and mark W8c as the point where the promise changes.

- `crates/mem-bridge/src/purge.rs` — `pub const NOT_ERASED: [NotErasedTier; 2] = [ NotErasedTier { tier: "memories-shard", reason: "no per-session provenance; Phase 4" }, NotErasedTier { tier: "vault", reason: "encrypted; erasure is key destruction at whole-database scope (ADR-0012 D-14, W8c)" } ];`

**Confidence.** Both the constant and the self-documenting gap are read directly.


---

## `w6/queue/deletion-oracle-crossuser` — Claim that a spool entry is an 'unauthenticated cross-process write path' letting 'anything that can write one' delete an arbitrary id across a user boundary

**Verdict:** `rejected`


Refuted as stated. Two independent reasons. First, decision 8 already replaced the consultant's free-form spool with an engine-owned queue that revalidates each request against a live row, so the claim's premise (the spool is the adopted design) is not the design under audit. Second, the boundary it claims does not exist on this product's normal filesystem layout: the storage root is created with create_dir_all and no explicit mode, so it takes the process umask, which on a normal account yields a directory another local user cannot write to; the tool never creates a group- or world-writable root, and the web token and key directory are 0600/0700. Another local user can therefore read prompt text (the databases are 0644 under a 022 umask) but cannot write a queue entry. The real defect in the same area is different and is filed as w6/purge/queue-automates-erasure: the entry is writable by any same-uid process, and the drain executes without an owner confirmation, which makes the agent - not another local user - the relevant writer.

**Claimed root cause.** A file-based request queue is writable by any process that can reach the storage root, and the engine deletes whatever the entry names.

**Trace.**

- *entrypoint* — `.tmp/drafts/20261025-w6-consult-response.md:159` — 'The refused purge writes a spool entry (ids only, never text) outside the databases'.
- *sink* — `.scratch/rust-port/issues/43-database-base-hardening.md:88` — 'The queue is engine-owned and request-revalidated against a live row (not the consultant's free-form spool, which was an unauthenticated deletion oracle)'.

**Evidence.**

- `crates/mem-store/src/engine_lock.rs:204` — create_dir_all with no explicit mode - the root inherits the umask, so it is not group/other writable on a default account.
- `crates/mem-store/src/connection.rs:206` — Same for database parent directories.
- `crates/mem-web/src/api_auth.rs:213` — The one secret the tool creates is 0600, showing the codebase does set modes where it cares.

**Why rejected.** Refuted as stated: decision 8 already replaced the free-form spool with an engine-owned, row-revalidated queue, and the storage root is created with the default umask so another local user cannot write an entry. The real defect in the area is the missing owner confirmation at drain, filed separately.


---

## `w6/tshm/carries-content` — Claim that retaining -tshm permanently leaves prompt text on disk

**Verdict:** `rejected`


Refuted. The -tshm file is a coordination region, not a WAL copy: a fixed header (8-byte magic, reader-slot count, max_frame, nbackfills, transaction_count, visibility_generation, checkpoint sequence and epoch, page_size), owner records, reader slots, and an append-only page-to-frame index whose entries are two integers. No database page bytes and no row text are mapped there. It does contain metadata about the database (page size, frame and transaction counts), which is not content. The routed default's retention of -tshm is therefore not a content residual; it is a permanent ~86 KiB per database of coordination debris, whose cost is disk and the sensor's usefulness, and the ADR should say exactly that rather than leaving 'probably contentless' in the consultant's terms.

**Claimed root cause.** The sidecar is a shared-memory image of the WAL, so it retains page content.

**Trace.**

- *entrypoint* — `.scratch/rust-port/issues/43-database-base-hardening.md:95` — The routed default keeps the sidecar as the sensor.
- *sink* — `crates/mem-store/tests/turso_capabilities.rs:2499` — A flag-off open and close leaves the stale -tshm byte-identical and unread - consistent with it being an index, not a cache of pages.

**Evidence.**

- `crates/mem-store/src/sqlite_files.rs:6` — '-tshm is Turso multiprocess-WAL shared memory' - shared coordination state, carried with copies for consistency, not for content.

**Why rejected.** Refuted by the dependency's file format: the -tshm holds a fixed header of counters, owner records and a page-to-frame index of two integers per entry; no database page bytes or row text are mapped into it.


---

## `w6/tshm/presence-is-liveness` — Claim that -tshm presence can serve as the legacy-liveness signal

**Verdict:** `rejected`


Refuted by the pinned measurements the design already rests on: -tshm is permanent debris that turso never unlinks, so it is present after every flag-on era including a clean exit, and its stale, zero-length and garbage forms are never read or rewritten. Presence therefore cannot distinguish a live holder from a months-old one, and the routed default already says so. The residual point the measurement does not settle is the inverse direction: a guard must also never treat an unparsable or zero-length -tshm as a live holder, because that would refuse every open after a crash and brick the root; the lock probe, not the file's existence or header, has to be the authority.

**Claimed root cause.** -tshm exists only while a flag-on process holds the database.

**Trace.**

- *entrypoint* — `.tmp/drafts/20261025-w6-consult-response.md:109` — 'if <db>-tshm exists: ... delete it' - presence used as a signal.
- *sink* — `crates/mem-store/tests/turso_capabilities.rs:2512` — A flag-off open and close of an upgraded database leaves the same file set: it neither reaps nor recreates -tshm, so presence persists across eras.

**Evidence.**

- `.scratch/rust-port/issues/43-database-base-hardening.md:95` — The routed default states it directly: -tshm is not debris, it is the sensor - because presence alone is not a liveness signal.

**Why rejected.** Refuted by the pinned measurement that -tshm is permanent debris never unlinked by turso and never read or rewritten when stale; presence persists across eras. The design already records this.

