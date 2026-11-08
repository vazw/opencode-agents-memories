# 0025 — The Mixed-Version Open Guard

- **Status:** Proposed
- **Date:** 2026-11-09
- **Builds on:** ADR-0021 (schema-version discipline: newer-file/older-build is the
  refusal direction), ADR-0007 (D-31/D-39: multiprocess WAL is architecturally
  unnecessary — one engine per storage root). **Supersedes:** the routed
  "permanent until `SchemaNewerThanBuild` ships in every fielded build AND W6 bumps
  `user_version`" retirement default (issue 43 `:120-122`).

## Context

Issue 43 W6a dropped `MultiprocessWal` from
`ConnectOptions::for_config` — the shipping option set is `[Encryption]` on every
platform — so every open this tree performs is turso's single-process (legacy) mode
and the engine again owns the database file's exclusive `F_SETLK` lock
(`turso_core-0.8.3/io/unix.rs:70`, `lock_file`). That restore is also the mixed-version
hazard: a still-launchable pre-drop (pre-issue-45) binary opens the same file in
multiprocess WAL, whose coordination lives in the `-tshm` sidecar and is invisible to
turso's `.db` lock. Two openers in two WAL modes on one file lose rows silently, which
is the failure this guard exists to refuse rather than hide.

The guard therefore lives at one seam, `ConnectFactory::connect`, and answers from a
pure decision table plus one probe. This ADR records the eleven decisions that table
and its surroundings encode, so the next mode change re-derives them instead of
rediscovering them. Evidence base: the opsec report §9 blockers 1–5 and 12, issues
45/46/60, and f6e3ff1 (no file-format change ⇒ no migration).

## Decisions

- **D-25-1 — One seam, two flights.** The guard lives at `ConnectFactory::connect`;
  preflight refuses before any turso call; postflight re-probes after the open and
  drops the handle on `Held`; `Unknown` refuses, never degrades to `Free`
  (`crates/mem-store/src/db.rs`, `crates/mem-store/src/open_guard.rs`).
- **D-25-2 — The oracle primitive, per platform.** Linux queries with `F_OFD_GETLK`,
  every other Unix with `F_GETLK`, one byte at offset 0, **queried and never taken**
  (issue 60: a taken probe made two conforming openers refuse each other for
  20–85 µs); the lock flavour is compile-time-selected and must be re-pinned if turso
  changes backend, and clearance is **structural** — mode + writability + probe on a
  fresh descriptor before this process's own mapping exists — never identity-based,
  because an OFD lock carries no `l_pid` and the guard can never learn who holds.
- **D-25-3 — The direction matrix, post-drop.** A writable `Legacy` (production) open
  probes the target's `-tshm` sensor; a `Multiprocess` writable open is refused by
  construction (the inverted O3: no option set in this tree carries the token); a
  read-only open skips the probe (victim, never aggressor —
  `turso_core-0.8.3/database.rs:991-993` returns early on `ReadOnly` whatever the
  holder's mode); Windows has no probe at all (`Divergence::NoProbeOnPlatform`), the
  platform selection is untested off Linux and **named here so nobody "fixes" it**,
  and post-drop Windows opens work for the first time (today every Windows open is
  refused `UnsupportedOpenMode`) — untested, no Windows runner.
- **D-25-4 — Stale debris is never live; only a conflicting lock reads `Held`.**
  Presence, size, header and magic are never signals; `Missing` is a known-free answer
  post-drop because the sidecar sets are disjoint; a `-tshm` unlinked (or a backup
  restored without sidecars) while a flag-on holder lives defeats the oracle — a named
  residual, never claimed as covered, mitigated only partially by D-25-6's marker
  cross-check, and the remedy text never advises deleting a `-tshm` while any process
  may hold it.
- **D-25-5 — The marker is mode truth, never a clearance input.** Shape
  `{pid, start_time, operation: null, mode}` in `{storage_root}/.wal-mode`; a live
  owner with an absent marker is a pre-issue-45 binary — exactly the mixed-version
  window — so the probe decides and the marker corroborates or conflicts; marker
  liveness inherits the hidepid limit and never clears anything by itself, and the
  start-time token defeats PID reuse; any future evolution of that token must be
  additive and lenient (`serde(default)`), never a type change that renders live
  records corrupt and reclaims a live lock.
- **D-25-6 — The sweep cross-check.** A live `.wal-mode` marker recording
  `multiprocess` refuses the shard sweep regardless of the probe outcome, which closes
  the unlink hole as far as shipped artifacts can; the mechanism lands with the W6c
  sweep work.
- **D-25-7 — `ENV_DISABLE_FILE_LOCK` caveat.** turso skips the `.db` lock when that
  variable is present, so "purge-session provably cannot open a database the engine
  holds" holds only while it is unset; the guard's `-tshm` probe is independent of the
  `.db` lock and remains the enforcement, and the variable degrades one layer, never
  the guard (the supersession chain for the earlier decision: enqueue was refused by
  ADR-0019 D-19-4, and the safety property is delivered by the flag drop's restored
  `.db` lock plus this caveat).
- **D-25-8 — Same-uid DoS, accepted and named.** Any same-uid process can hold byte 0
  of a `-tshm` it creates and force a refusal / exit 75 indefinitely; that is strictly
  better than silent row loss, and the support phrasing lives here, never "delete the
  `-tshm`".
- **D-25-9 — Retirement is a support-policy floor.** The guard is permanent while any
  pre-guard (pre-issue-45) binary remains launchable; `SchemaNewerThanBuild` protects
  only the newer-file/older-build direction, so a retirement date is a
  minimum-supported-version policy plus a release-note obligation, never a schema
  fact; the "W6 bumps `user_version`" clause is dropped, because f6e3ff1 ruled no
  migration is needed (the flag drop is not a file-format change) and the W6c
  sweep-stamp bumps are ordinary ADR-0021 migration rows with no bearing on the guard.
- **D-25-10 — The override is process-wide, two legs, named residual.** The env leg
  (`OPENCODE_MEM_ALLOW_UNGUARDED_OPEN`) wins over the config key, nothing is settable
  per open, and the residual is two distinct `StoreConfig` values inside one process
  (the documented footgun; `test_hazard_requires_two_distinct_resolved_configs`).
- **D-25-11 — `Vacuum ∧ MultiprocessWal` refuses at preflight.** A typed refusal one
  layer above a mid-VACUUM translation error, because the engine refuses VACUUM under
  multiprocess WAL at translation; the mechanism lands with the W6c erasure surface,
  is unreachable post-flip by construction, and is kept as the durable rule for any
  future selectable-WAL work.

## Consequences

- Every open pays one `fcntl` query on a sidecar path that usually does not exist
  (`Missing`), plus one post-open re-query for a writable legacy open — no probe runs
  under the override, for a read-only open, or on a platform without `fcntl`.
- A pre-drop holder now refuses the new build instead of corrupting the file; the
  refusal is typed (`StoreError::OpenBlocked`, direction `legacy-authority`), names
  the database file, and names the documented override.
- `purge-session` surfaces the engine-held root as exit **75** `purge.busy` from
  `store.ready()` (the engine's `.db` lock), not only from the operation lock.
- The two openers of one root are now serialised by turso's own file lock, so a
  second process's legacy open is refused by the engine rather than by the guard —
  the honest post-drop contract, and the reason the issue-60 two-conformer test
  became a single-owner test.

## Security and edge cases

- **Fail closed.** An unanswerable probe (`Unknown`) refuses; only a proven `Free` or
  a `Missing` sidecar clears a writable open.
- **No path echo beyond the database.** Errors name the `.db` the user knows, never
  the sidecar, the pid of a holder (unknowable: D-25-2), or any row content.
- **Override breadth.** The escape is process-wide by construction and covers both
  flights; a caller cannot switch it per open, and a second `StoreConfig` in one
  process is the named residual (D-25-10).
- **Platforms.** The Windows cell clears with `NoProbeOnPlatform` recorded — the guard
  is absent there, not proven unnecessary, and the divergence is a value in the table
  test rather than a comment.
- **Marker leniency.** Marker parsing is additive-only (`serde(default)` on new
  fields), so a future field can never make a live record look dead and reclaim a
  live lock.
