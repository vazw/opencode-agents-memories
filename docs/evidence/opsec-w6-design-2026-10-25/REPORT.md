# opsec audit — the W6 erasure design

**Target:** `/home/vaz/Code/Personal/Rust/opencode-agents-memory`
**Source ref:** `b42905c` (parent `7390de1`; the probe file named as dirty in the spawn contract was
committed as `b42905c` during reconnaissance, same content, tree clean) · **Profile:** standard
(scoped) · **Run:** `opsec-opencode-agents-memory-run-1` · **Date:** 2026-10-25

**Subject:** the design, not the code. The implementation does not exist; the material audited is two
owner decisions (6, 7), two amendments (8, 9), four routed defaults, a consultant response, and the
pinned measurements in `crates/mem-store/tests/turso_capabilities.rs`. Findings that are *design*
flaws are labelled as such in `findings.json`; no finding is an implementation defect.

---

## 0. Disclosure of the `confirmed` definition

In this fleet there is **no sandbox**, so target-controlled execution is not permitted and no probe
was run. `confirmed` here is a **static property of source** — a complete trace, no visible
preventing layer, a named principal/resource and a concrete result — never a runtime-demonstrated
result. For a design audit, "source" is the decision text plus the code and pinned tests it binds;
dependency facts are read from the vendored `turso 0.8.3` crate sources. Where runtime behaviour is
decisive the record is `needs_validation`, even when the source looks conclusive. Three records are
in that state.

The pinned measurements in the brief were treated as given and were not re-litigated.

---

## 1. Verdict summary

| verdict | count | fingerprints |
|---|---|---|
| confirmed | 10 | see table below |
| needs_validation | 3 | `w6/erase/sweep-liveness`, `w6/guard/tshm-probe-portability`, `w6/guard/vacuum-under-legacy-holder` |
| rejected | 4 | `w6/guard/oracle-cannot-detect`, `w6/queue/deletion-oracle-crossuser`, `w6/tshm/carries-content`, `w6/tshm/presence-is-liveness` |

| confirmed fingerprint | severity | one line |
|---|---|---|
| `w6/liveness/platform-probe-fails-open` | **high** | `SystemProcessProbe` contradicts its own contract off Linux, so D-39 steals a live engine's lock and the guard is designed to reuse that logic |
| `w6/guard/oracle-failopen` | **high** | the `-tshm` oracle is specified by description only and has seven fail-open paths to the pinned silent-row-loss outcome |
| `w6/purge/queue-automates-erasure` | medium | the enqueue has no authorisation or owner gate and its only worked-out version executes at start-up |
| `w6/purge/report-omits-vault` | medium | `not_erased` names one tier; the vault crate documents that the report is untrue without it |
| `w6/erase/promise-overclaims-inventory` | medium | "every file this tool writes" versus six never-reaped `.bak` families, profile snapshots, vault, logs, exports, media copies |
| `w6/erase/disclosure-unreachable` | medium | `PendingErase` has no surface the user sees: frozen single-boolean DTOs, a separate endpoint, a 501 cleanup stub |
| `w6/erase/view-clause-contradicts` | medium | "removed from the agent's view immediately" versus "memories are a separate promise" and verbatim episodic retention |
| `w6/milestone/rewrite-keeps-derived-text` | medium | rewriting the contributor set leaves the text in `summary`, `embedding`, `source_count` and the window bounds |
| `w6/milestone/rewrite-needs-sweep` | medium | an `UPDATE` is physically no safer than a `DELETE`; `episodic.db` is in no sweep set |
| `w6/guard/retirement-nonsequitur` | low | the retirement condition cannot be met by the pre-W6 binaries it defends against |

Two `high` findings → the gate is **REJECTED**.

---

## 2. The purge queue: the minimum safe design (question 1)

**The earlier review's framing is wrong, and its conclusion is still right.** Nothing in this design
is an "unauthenticated cross-process write path" in the sense of a boundary between users: the
storage root is created by `create_dir_all` with the process umask (`connection.rs:206`,
`engine_lock.rs:204`), so under a normal umask another local user cannot write an entry, and decision
8 already replaced the consultant's free-form spool with an engine-owned, row-revalidated queue. That
claim is recorded as rejected (`w6/queue/deletion-oracle-crossuser`).

The real defect is worse in a way that matters for this product. A same-uid writer exists **by
design**: the AI agent runs as the user, executes shell commands, and ingests untrusted content. The
only authorisation a purge has today is `--confirm project:session:agent`
(`mem-bridge/src/purge.rs:79`), which is reproducible by anything that knows the ids — it is a human
gate only because a human types it. Decision 8 proposes to move the deletion behind a durable
request, and the only worked-out drain (`consult-response.md:163`) executes at engine start-up. That
is automatic erasure, which the episodic module's own doc forbids in one sentence:
"Erasure is a SEPARATE operation and is never automatic: no timer, no sweep, no flag, no
consolidation stage calls this module" (`mem-episodic/src/purge.rs:11`). A start-up drain is a
stage.

**Minimum safe design, if a queue is kept at all.** Six properties, each of which is load-bearing;
drop any one and the queue is not safe:

1. **It is a request, not an instruction.** The drain may only mark requests *pending*. Execution
   requires an owner-present action: a TTY-confirmed CLI invocation or an authenticated UI action.
   This is the property that keeps erasure owner-initiated; without it the queue is an erasure
   trigger.
2. **Authorisation is a human-intent gate, not file ownership.** File mode cannot separate the agent
   from the user — they are the same uid. The gate must be something the agent cannot satisfy on its
   own: a `isatty(0)` check on the confirming process, or a UI action carrying the web session. Say
   this in the ADR; do not write "authenticated" and mean "same uid".
3. **Content is ids only, and bounded.** The triple plus a client request id; `SessionScope::new`
   validation reused verbatim (`mem-episodic/src/purge.rs:64-85`); a hard byte cap per entry and a
   hard count of pending entries. On overflow, **refuse loudly** — never evict the oldest, because
   evicting silently loses a deletion request, the exact failure decision 6 forbids.
4. **One writer, one location, no symlink following.** A single engine-owned queue (reuse the capture
   inbox pattern, `capture.rs:639`, which already has idempotent ids and terminal states) rather than
   a directory of free-form files; entries created `O_EXCL` with `O_NOFOLLOW`, `0600`, in a `0700`
   directory.
5. **Revalidated against a live row at drain, and never a deletion oracle.** An entry naming a scope
   that does not exist must be dropped and *not* reported as erased. That removes the "probe the
   store by asking whether the purge succeeded" primitive.
6. **The request is erased when it completes**, and it must be inside the inventory the erasure
   promise enumerates. Ids are not text, so this is cheap — but it must be written down.

**What an attacker who can write one entry gets under that design:** one pending request they cannot
execute; a refusal if the scope is absent; no content, no path, no SQL, no amplification. Under the
design as recorded — no gate, automatic drain — they get irreversible deletion of one session's
episodic rows and the co-contributors' retained text, at the next engine start.

**If that is not worth the complexity, the alternative is better.** Drop the queue. The CLI refuses
with a typed outcome and exit 75 — the same degradation signal D-39 already uses — naming the next
action: *stop the engine and re-run `purge-session`*. Zero new state, zero new write path, no new
thing to sweep, and the existing confirmation gate stays exactly where it is. The cost is one UX
step; the benefit is that "erasure is never automatic" stays literally true. I recommend this.

---

## 3. Does decision 6 misrepresent? (question 2)

**Yes, in three distinct ways, and the disclosure is not reachable where it must bite.**

**(a) The first clause contradicts the second.** "Removes it from the agent's view immediately" sits
next to "memories are a separate promise". The injected context *is* the memories
(`inject.rs:405`), so under the only reading that concerns the agent, the clause is false by the
promise's own next sentence. Under the narrow reading ("the prompt timeline"), it says nothing about
the agent and a user will still read it as forgetting. Independently, the same prompt text is
retained verbatim in `episodic_raw_context` and `episodic_segments`
(`episodic_sql.rs:111,124`) as whole-session rows, and a prompt delete is one `DELETE` against
`user-prompts.db` (`prompt.rs:289`) with no episodic leg. A per-prompt delete cannot remove it from
there at all.

**(b) The second clause over-claims the file set.** See §5.

**(c) The disclosure has no reachable surface.** The routed default keeps `DeletedPromptDto` and
`DeletedMemoryDto` at one boolean each (`dto.rs:294,301`), puts erasure state behind
`GET /api/prompts/{id}/erasure`, and leaves "erase now" to a cleanup endpoint that is a documented
501 stub (`maintenance.rs:234`). A user who deletes a prompt gets `{"deletedPrompt": true}`. The
consultant's own requirement — "the delete call's return ... Default return is `PendingErase`"
(`consult-response.md:352`) — was overruled by the routed default, and nothing replaced it. This is
the honest finding: the design satisfies its own rule in the API and violates it in the product.

**Where the wording must bite — exactly three places, and all three are required:**

1. **The delete response** (or a mandatory, same-interaction read the UI is contractually required to
   render). Not a status page the user must know exists.
2. **The prompt-delete copy**: "removed from the prompt history now; N memories learned from it were
   kept and can be deleted separately; the session transcript still contains it until you purge the
   session." The count is the surprising moment (§2.7 of the consult response is right about this);
   the transcript sentence is the one the design currently omits.
3. **The ADR's own statement of the default configuration**, with the limits paragraph in the same
   section, not a footnote: `database_encryption_enabled = false` means plaintext residue until the
   sweep; encryption does not change the promise because the key outlives the deletion; CoW
   snapshots, SSD wear-levelling, external backups and **the tool's own `.bak` files** are outside
   the promise unless named.

---

## 4. Fail-open analysis of the guard's oracle (question 3 — the highest-value part)

The oracle is correct in principle and is **already implemented inside turso** — and unreachable.
`reject_live_multiprocess_wal_for_legacy_open` (`turso_core` `database.rs:933`) does exactly the
proposed probe: open the `-tshm`, ask `open_mode()`, refuse if `MultiProcess`. But it is only called
from `resolve_default_storage`, which runs only when `OpenOptions::storage` is **unset**; the SDK
`Builder` path the tool uses supplies storage itself (`rsapi` `open()` → `io.open_file` →
`Database::open_async`). So both directions of turso's own cross-mode guard are dead code for every
open this tool performs — which is the mechanism behind the pinned mixed-mode measurement, and the
reason the W6 guard is genuinely load-bearing.

The mechanism it must reproduce: a **1-byte POSIX record lock at offset 0**, taken **shared**, held
for the mapping's lifetime, with the primitive selected at compile time (`F_OFD_SETLK` on Linux,
`F_SETLK` elsewhere). `l_pid` is not populated for OFD locks, so the holder cannot be identified.

Every way the oracle can report **free** while a legacy holder is live:

| # | path to a false "free" | consequence |
|---|---|---|
| 1 | **Wrong lock family.** A `flock(2)`-based probe — the natural Rust choice — does not conflict with fcntl record locks on Linux. | **Data loss.** Every guarded open proceeds. The probe is silently useless and no negative test catches it. |
| 2 | **Holder inside its open window.** The `.db` is open but the `-tshm` does not exist yet / byte 0 is not yet locked. | **Data loss.** A real TOCTOU; closable only by probing *again* after the open (turso probes twice for exactly this reason). |
| 3 | **`-tshm` unlinked while the holder lives** — by the tool's own hygiene, a restore from a backup taken without sidecars, an interrupted rename, or a user following the consultant's tripwire advice. | **Data loss.** The holder is on an orphaned inode; the next opener creates a new coordination region over one WAL. |
| 4 | **Read-only legacy holder with no `-tshm`.** A read-only open takes neither the coordination lock nor the `.db` lock. | **Reader-side inconsistency.** The holder writes nothing, so no data is lost; the sweep's rewrite can hand it a torn image. |
| 5 | **Probe open fails** (EACCES/EIO) and the code maps "cannot read" to "free". | **Data loss.** `read_live_lock` already has this shape (`operation_lock.rs:56`); the guard must not inherit it. |
| 6 | **Non-Linux with the wrong fcntl flavour or a different lock domain.** | Data loss on macOS (where flag-on roots exist); no legacy holders on Windows. |
| 7 | **Windows.** The flag is never on, so no legacy holder exists. | **Clean (vacuous).** Still must be stated, so nobody "fixes" it later. |

And the mirror hazard, which is equally important because it bricks the root: **a stale, zero-length
or garbage `-tshm` must never be read as "live"**. The pinned measurements say such debris is inert;
a guard that uses the file's presence, size or magic as a signal would refuse every open forever
after a crash. **The lock, and only the lock, may be the authority.**

Two further consequences that the design does not address:

- **The guard cannot implement "it is me" by identity.** With OFD locks there is no `l_pid`. Under
  the recorded selectable-WAL intent, a mode-selecting build that chooses multiprocess will hold
  byte 0 on its own databases and a naive guard will refuse to open its own file. The exception must
  be structural (one probe per process per path, cached clearance, taken before this process's own
  mapping exists), not identity-based.
- **Fail-closed has a cost that must be named:** any same-uid process can hold byte 0 of a `-tshm` it
  creates and force permanent exit 75. Under the stated threat model that is a same-account DoS, not
  a boundary crossing — and it is strictly better than silent data loss — but ADR-0025 should say so,
  otherwise the first support ticket reads as a bug.

Also note: decision 8's premise ("the drop makes a flag-off open take an exclusive `fcntl` lock, so
`purge-session` **provably** cannot open a database the engine holds") is true only while
`ENV_DISABLE_FILE_LOCK` is unset — turso skips the `.db` lock entirely when that variable is present
(`unix.rs` `open_file`). "Provably" is too strong for an environment the tool does not control.

---

## 5. Decision 9's rewrite (question 4)

**The rewrite is correctly motivated and incomplete in three ways.**

1. **It leaves the text in the row.** `episodic_milestones` carries, for the same contributor set:
   `summary TEXT NOT NULL` (the merged summary), `embedding F32_BLOB NOT NULL` (its vector, written
   once, never regenerated), `source_count`, `window_start`, `window_end`
   (`episodic_sql.rs:65-83`). Removing one `Contributor` removes its verbatim `summary` — the thing
   decision 9 is about — but the merged summary is a paraphrase of the same sessions, and the
   embedding inverts toward it. Re-deriving the merged summary needs the summarizer, which a sweep
   must not call. So the ADR must choose explicitly: regenerate (not in a sweep), drop (destroys the
   other contributors' knowledge, contradicts "knowledge is independent"), or keep and **report as a
   named residual**. Silence is not an option, because the rewrite currently reads as complete.
2. **An `UPDATE` is physically no safer than a `DELETE`.** The old row image stays in the page and
   the `-wal` until `wal_checkpoint(TRUNCATE)` plus `VACUUM` (`consult` doc:19; the canary test at
   `turso_capabilities.rs:1127` is the shape). So the rewrite reopens the very leak the rule exists
   to close **unless `episodic.db` joins the request's sweep set**. Nothing in the recorded decisions
   puts it there; the sweep-set rule lives only in the consultant's §2.4.
3. **The report arithmetic over-claims.** `EpisodicPurgeReport.milestones` is summed from `DELETE`
   changes (`purge.rs:224`). A rewritten milestone must not be counted as erased; it needs its own
   count and its own residual.

**Safe-write requirements**, which are cheap and should be pinned as tests: decode → filter → encode
with the existing deterministic `encode_contributors` (already byte-stable), one `UPDATE` inside the
existing single write batch, and **skip, with a named residual, any row whose `contributors` JSON does
not decode** — the pre-scan already skips undecodable rows; the rewrite must not be the place where a
corrupt row is blanked. Recompute `source_count` and the window bounds. Never blank fields instead of
removing the element.

---

## 6. The promise versus the real file inventory (question 5)

The current inventory is **not honest**. What the tool writes, and what the sweep reaches:

| artefact | can hold prompt text | in the design's sweep? |
|---|---|---|
| `user-prompts.db` + `-wal` | yes, verbatim | yes |
| `episodic.db` + `-wal` (`raw_context`, `segments`, `agent_sessions.summary`, `milestones.summary`, `contributors`, `embedding`) | yes | **no** — not named anywhere |
| memory shards + `-wal` | derived (K), plus scrubbed C text when the sweep-set rule lands | only under the consultant's rule |
| `user-profiles.db` + `-wal`, incl. `user_profile_changelogs.profile_data_snapshot` (5 versions, `sql.rs:166`) | derived, near-verbatim | **no** — and the profile has no class |
| `metadata.db` | no (counts, registry) | n/a — name it |
| `{root}/vault/{scope_hash}/vault.db` | yes, encrypted | **no** — must be a named exclusion (`mem-vault/src/lib.rs:45` says the report already omits it) |
| `-tshm` | **no** — header counters, owner records, a page→frame index of two integers (`w6/tshm/carries-content`, rejected as a content claim) | n/a — name it |
| six `.bak` families (`.schema-v*`, `.legacy`, `.pre-reembed*`, `.pre-encrypt*`, `.pre-path-migrate*`, quarantine `{reason}-{pid}-{millis}`) | **yes, full copies including `-wal`** | **no** — and `schema_migrations.rs:421` states no code path deletes a `.bak` |
| `~/.opencode-mem/opencode-mem.log` + `.old` (30 days, 5 MiB rotation) | only if a call site logs text; the payload rule forbids raw context and the sink is scoped to one target | **no** — outside the root; needs an explicit exclusion and a lint/test on the payload rule |
| portability export bundles | yes, memory content and profile | **no** — user-directed; name it |
| filesystem/CoW snapshots, SSD wear-levelling, external backups/sync | yes | **no** — the consultant's limits paragraph is right and needed |

Two conclusions. First, the promise's sentence must change ("every database file this tool writes
under the storage root, once the request's sweep set has completed"), with the exclusions enumerated
in the same section. Second, two items are not exclusions but **bugs**: the `.bak` families and the
profile snapshots are the tool's own writing, sit in the root, and are never reaped. Bringing them in
is cheap — reap `.bak` on the next successful sweep, assign the profile a class, and add a `.bak`
grep to the erasure canary test.

---

## 7. Residual leakage in the status endpoint (question 6)

**It is not a finding, and the reason is worth stating.** `/api/*` is token-gated
(`gate.rs:86-97`) by a 0600 file readable by any same-uid process (`api_auth.rs:213`), on a loopback
bind behind a Host allowlist. The residual's audience is therefore the data subject and same-uid
processes, and the storage path is already known from config. Naming a database label and a file path
crosses no boundary that the token has not already granted.

**Minimum the residual should carry**, on other grounds — the *support bundle* and *stolen laptop*
adversaries, and the risk of the status page becoming a second inventory surface:

- the request id, the erasure state, and a **database label** (`prompts`, `episodic`, `memory shard
  (scope)`) — not an absolute path;
- the reason enum (`NotYetSwept`, `LegacyPeerLive`, `SweepFailed`, `Spooled`), without the pid;
- a count of surviving derived artefacts;
- and **nothing derived from content** — no preview, no hash of the text, no scope hash.

Absolute paths in a browser surface leak the user's home directory and username into screenshots,
support bundles and bug reports. That is a low-severity disclosure at most, but the fix is free:
labels in the API, paths only in the server-side log.

---

## 8. Anything that increases attack surface for a local adversary (question 7)

Ranked by what it actually buys an adversary inside the stated model:

1. **The queue, if it executes without an owner gate** — the only item that grants an irreversible
   destructive capability to a same-uid process without human intent. §2.
2. **The guard's exit-75 on a held lock** — a same-uid process can deny service indefinitely. Accept
   and name it; it is better than silent loss.
3. **Retained `-tshm`** — no content, no new reader, ~86 KiB per database. The only new property is
   that it is now load-bearing, so its loss must be a *refusal*, never a silent "free". State that a
   backup restored without `-tshm` defeats the guard.
4. **The status endpoint** — effectively no change (§7).
5. **Rewrite-in-place** — not a new surface; it is the same physical residue as `DELETE`, which is why
   `episodic.db` must be in the sweep set (§5).
6. **Two things *not* in the design that should be**: the storage root's mode is never set
   (`create_dir_all` with no mode), so under a default umask the databases are 0644 and another local
   user can read every prompt; and the `.bak` copies inherit the same mode. The design's own honesty
   paragraph depends on this being a protected directory. Not filed as a W6 finding (it is base
   hardening), but ADR-0025 should not assume a boundary the tool never creates.

---

## 9. What must be settled before ADR-0025 and ADR-0019 are written

The gate is REJECTED. These are the blockers, in the order that unblocks the most:

**Before ADR-0025 (substrate):**

1. **Fix `SystemProcessProbe` and re-derive D-39.** `is_alive` must match its documented contract on
   every shipping platform, `LockState` needs a start-time token, and both directions must be tested
   per platform. Also correct the routed default's description of the failure mode in issue 43 — it
   describes the intended contract, not the shipped code, and gets the Linux direction wrong.
2. **Write the oracle as a contract, not a description**, and pin the positive control first: a child
   holding a real flag-on handle must make the guard refuse. Add the post-open re-probe, the
   "unknown → refuse" rule, the platform matrix, and the "stale debris is never live" rule.
3. **Decide the self-conflict case** for the selectable-WAL future (no `l_pid` under OFD → structural
   clearance, not identity).
4. **State the `ENV_DISABLE_FILE_LOCK` caveat** against decision 8's "provably cannot".
5. **State the same-uid DoS** that a held byte-0 lock gives, and that the remedy text must never
   advise deleting `-tshm` while any process may hold it.

**Before ADR-0019 (erasure):**

6. **Decide the queue or the refusal.** Preference: no queue. If a queue: owner present at execution,
   ids only, bounded with loud refusal, engine-owned single writer, live-row revalidation, and the
   request erased on completion. Do not let a start-up drain execute deletions.
7. **Rewrite the promise in three clauses** — what leaves the prompt history, what is retained
   (memories, session transcript, milestone summaries) with counts, and what the sweep set covers.
8. **Publish the inventory as a table**, with `.bak` reaping, the profile assigned a class, the vault
   named, and logs/exports/media declared. Add the `.bak` and profile snapshots to the canary test.
9. **Give `PendingErase` a reachable surface** at delete time, or accept one additive DTO field.
   Define the residual's fields (labels, not paths).
10. **Finish decision 9**: what happens to `summary`, `embedding`, `source_count` and the window
    bounds; `episodic.db` in the sweep set; a `milestones_rewritten` count distinct from erased.
11. **Extend `NOT_ERASED`** to name the vault (and its `W8c` condition) before anyone reads a purge
    report as complete.
12. **Fix the retirement sentence** to a support-policy floor.

Items 1–3 are code/probe work that can land now. Items 4–12 are ADR text. None requires re-litigating
a pinned measurement.

---

## 10. Coverage statement

Scoped `standard` run: 21 ledger units — 14 candidate, 3 covered, 3 out of scope, 1 not applicable.
**This is partial coverage and is presented as such.** Not reviewed: `web-ui/` (excluded by the spawn
contract), Rust outside the W6 subject, erasure under encryption (pinned as not meaningful), remote
adversaries (outside the threat model). No prior opsec run exists for this repository, so no prior
ledger was carried in. Two things I read but did not file, because they are outside the W6 subject and
would be findings against the base rather than the design: the storage root's umask-derived mode
(§8.6) and `LockState`'s lack of a start-time token (folded into blocker 1 because the guard reuses
it).

## 11. Terminal state

`audit_complete` — all six phase artifacts written, `findings.json` and `coverage-ledger.json` both
pass the vendored validators (`PASS: 17 findings valid`, `PASS: 21 coverage units valid`).
