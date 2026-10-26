# Architecture — W6 erasure design under audit

**Subject:** the *design* of W6 erasure for `opencode-agents-memory` — not a code review of the
repository. The implementation does not exist; the material audited is (a) two owner decisions
(6, 7) and two amendments (8, 9) recorded in issue 43, (b) four routed defaults, (c) a consultant
response, and (d) the pinned measurements in `crates/mem-store/tests/turso_capabilities.rs`.

## Trust model (given, not re-derived)

Local single-user developer tool. One engine process owns a storage root under an exclusive engine
lock (D-39; a second engine exits 75). Data at rest in local SQLite/turso files: prompt text
(`user_prompts` + `user_profile*`) and memories (sharded DBs + `episodic.db`). No server, no
multi-tenancy, no network surface beyond a token-gated loopback UI. Adversaries in scope: another
local user, physical access / stolen laptop, backups and sync targets, support bundles, and the
tool's own over-claims. Out of scope: remote attackers, root.

Two consequences of that model drive every severity call in this run:

1. **Same-uid is not a boundary.** The AI agent runs as the user and can run shell commands, so any
   control that reduces to "a same-uid process may do X" does not stop the realistic case where
   untrusted model output leads the agent to do X. The only same-uid controls that matter are
   *human-intent gates* (a TTY, an explicit confirmation the agent cannot obtain on its own) and
   *irreversibility* (the operation cannot be undone by a later action).
2. **Metadata is cheap, content is not.** Path labels, DB names and file sizes are not secrets to a
   local reader that already has the storage root; prompt text and its derived forms are.

## Components bound by the design

| component | what it is | why W6 touches it |
|---|---|---|
| `mem-store::operation_lock` | cross-process JSON lock `.turso-operation.lock`, body `{pid, timestamp, operation}`; `read_live_lock` unlinks anything unparsable, empty or dead-pid | the guard's second signal; the liveness oracle the consultant would reuse |
| `mem-store::engine_lock` | `.engine.lock`, D-39, exit 75; same `LockState` body; loser writes `status` | the only pre-existing mixed-version guard, per platform |
| `mem-store::platform::SystemProcessProbe` | `/proc/<pid>` existence | if it is wrong, both locks above are wrong |
| turso 0.8.3 `MappedSharedWalCoordination` | `-tshm` sidecar: byte 0 process-lifetime RDLCK, byte 1 writer, byte 2 checkpoint, 3..N reader slots; page→frame index | the routed guard oracle ("shared lock on byte 0 of `<db>-tshm`") |
| turso 0.8.3 legacy/multiprocess rejection | `reject_live_multiprocess_wal_for_legacy_open` + `reject_live_legacy_wal_for_multiprocess_open` | exists, but unreachable on the SDK open path the tool uses (see below) |
| `mem-bridge::purge` | the one sanctioned erasure path: episodic gate + operation lock, facts leg, episodic leg, `not_erased` | decision 8 replaces it with an enqueue; decision 9 changes its milestone handling |
| `mem-episodic::purge` | one write batch; pre-scan milestones, `DELETE_MILESTONE` per match | decision 9 |
| `mem-profile::prompt` | `user-prompts.db`; `delete_prompt` = one `DELETE`; `vacuum()` exists | the prompt half of decision 6 |
| `mem-web::api::prompts/memories/dto` | delete handlers; `DeletedPromptDto{deleted_memory}`, `DeletedMemoryDto{deleted_prompt}` | the delete-response contract decision 6 must not lie in |
| `mem-web::api::maintenance` | `POST /api/cleanup` — documented **501 stub** (D-15) | the consultant's "erase now" lever has no implementation to attach to |

## Load-bearing source facts established in reconnaissance

1. **`SystemProcessProbe::is_alive` is `Path::new("/proc").join(pid).exists()`** (`platform.rs:51`).
   Its own doc comment (`platform.rs:36-44`) claims the opposite contract ("answers `true` so a
   possibly-live foreign lock can NEVER be stolen"). Off Linux the code answers **false for every
   pid**, so `read_live_lock` unlinks a **live** holder's lock and the acquire retry succeeds.
2. **`LockState` has no process start time** (`operation_lock.rs:35-42`), so liveness is PID-only on
   Linux — PID reuse reads as a live holder (over-refusal), and `/proc/<pid>` exists for zombies.
3. **turso's cross-mode rejection is dead code on the tool's open path.** `turso` 0.8.3's
   `Builder::build()` → `TursoDatabaseConfig` → `rsapi` opens the file itself and passes
   `OpenOptions::storage`, while the two `reject_live_*` probes live only in
   `Database::resolve_default_storage`, which runs only when `storage` is *unset*. So both
   directions of turso's own mixed-mode guard are skipped by every open this tool performs. This is
   the mechanism behind the pinned mixed-mode measurement, and it means the W6 guard is genuinely
   load-bearing rather than duplicating a live engine check.
4. **The `-tshm` byte-0 lock is a POSIX byte-range record lock, 1 byte at offset 0, taken in
   *shared* mode and held for the mapping's lifetime** (`shared_wal_coordination.rs`
   `PROCESS_LIFETIME_LOCK_OFFSET`, `detect_open_mode`, `Drop`). The primitive is chosen at compile
   time: `F_OFD_SETLK` on Linux, `F_SETLK` elsewhere. `l_pid` is therefore **not** available for
   OFD locks, so the holder cannot be attributed by identity.
5. **A read-only open takes no `.db` lock at all** (`unix.rs` open_file: the lock is skipped when
   `ReadOnly` is set) and a read-only multiprocess open with no `-tshm` degrades without taking
   byte 0 (`database.rs` `shared_wal_coordination()`), so a read-only legacy holder can be invisible
   to the oracle.
6. **`turso` honours an `ENV_DISABLE_FILE_LOCK` environment variable** that skips the `.db` lock
   entirely, so decision 8's "provably cannot open" is conditional on an environment the tool does
   not control.
7. **`-tshm` carries no page bytes**: header (magic, reader-slot count, max_frame, nbackfills,
   transaction_count, visibility_generation, checkpoint seq/epoch, page_size), owner records and a
   `SharedWalFrameIndexEntry { page_id, frame_id }` array.
8. **Six never-reaped `.bak` families exist**: `.schema-v{N}-{millis}.bak` (schema migrations),
   `.legacy.bak`, `.pre-reembed-{nonce}.bak`, `.pre-encrypt-{millis}.bak`,
   `.pre-path-migrate-{nonce}.bak`, `{reason}-{pid}-{millis}.bak` (shard quarantine). Each is a full
   copy including `-wal` (the copy primitive copies every sidecar).
9. **`user_profile_changelogs.profile_data_snapshot` is `TEXT NOT NULL`**, retained five versions
   deep, so profile-derived prompt text survives in five snapshots plus the live row.
10. **`POST /api/cleanup` is a 501 stub** and the delete DTOs are frozen at a single boolean.
11. **`purge.completed`'s `not_erased` names one tier**, and `mem-vault`'s own module doc states that
    "a purge run today erases no vault rows and does not say so".
12. **The storage root is created with `create_dir_all` and no mode**, so its permissions are the
    process umask (typically 0755/0644). Only the encryption key and the web token are 0600.

## Where the design's own claims cross a source fact

| design claim | source fact | consequence |
|---|---|---|
| "the guard's oracle is the shared lock on byte 0" | true, but primitive- and platform-specific, `l_pid` unavailable, and absent/unlinked/read-only cases read free | fail-open is silent row loss (measured shape) |
| "a second engine exits 75" | only if `SystemProcessProbe` is right; off Linux it is inverted | D-39 is not a guard on macOS |
| "purge-session provably cannot open what the engine holds" | `ENV_DISABLE_FILE_LOCK` skips the lock | "provably" is too strong |
| "erased from every file this tool writes" | six `.bak` families, profile snapshots, vault, logs, exports, media copies | over-claim |
| "removed from the agent's view immediately" | prompt text also lives in `episodic_raw_context` / `episodic_segments`; injected context is memories, which the same promise exempts | clause contradicts the next clause |
| "the purge names the residual" | `NOT_ERASED` has one entry; the vault gap is self-documented | false report today |
| "rewrite the contributor set instead of deleting the row" | `summary` and `embedding` are also derived from that contributor; an UPDATE's old image persists in WAL/free pages | leak survives the rewrite |
| "guard permanent until SchemaNewerThanBuild ships in every fielded build" | pre-W6 binaries cannot contain that check | retirement condition unreachable |

## Not audited (declared)

`web-ui/` (out of scope by instruction), every Rust path outside the W6 subject, the encrypted
configuration's erasure properties (pinned as not meaningful), and remote/network adversaries (out
of the threat model). No runtime probe was executed; runtime-decisive questions are recorded as
`needs_validation` with an owner-run plan.
