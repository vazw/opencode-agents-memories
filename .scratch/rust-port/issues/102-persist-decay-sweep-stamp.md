# 102 — The decay-sweep stamp is in-memory, so every engine start re-arms the daily gate

Status: ready-for-agent — 2026-11-01. The premise is MEASURED; the *shape* of the fix is a maintainer
decision, because every candidate changes either the storage root's file set or the `user_profiles`
schema, and this repo's on-disk format carries an explicit refusal rule for newer roots (issue 43
decisions 2–3). Filed with the issue-90 blueprint rather than left as a disclosed bullet, because issue
90 itself exists *because* issue 14's R13 carry-over was a known gap with no owning issue.

## What is missing

`ProfileStore::sweep_decay_daily` gates on a stamp that only lives in memory:

- `last_decay_sweep: Mutex<Option<i64>>` — `crates/mem-profile/src/profile_store.rs:371`.
- Initialised `Mutex::new(None)` — `:413`; cleared back to `None` by `reset()` (`:437`) and
  `rebind()` (`:457`).
- Read by the gate (`:1063-1073`) and written after the loop (`:1113-1119`). Nothing else touches it:
  `grep -rn last_decay_sweep crates/mem-profile/src` yields those sites plus the poison unit test
  (`:1584-1607`). It is never persisted.
- `decay::sweep_is_due(None, now, interval)` is **true** (`crates/mem-profile/src/decay.rs:73-75`).

Once issue 90 gives the sweep a production caller (a startup tick plus an hourly wake-up), every process
start sweeps immediately, regardless of when the previous process swept. The store's own doc (`:1050`)
calls it "the persisted daily sweep" and the gate is `SWEEP_INTERVAL_MILLIS = 86_400_000` (`decay.rs:33`);
the 24 h cadence is stated as the contract and does not survive a restart.

## Consequence (INFERRED — arithmetic from the tree's kernel, no run)

`sync_confidence` (`profile_store.rs:1462-1470`) is `ema_update(prior, alpha/(alpha+beta), 0.3) ×
confidence_decay_factor`, and the sweep applies it **twice** per pass (the disclosed doubled derivation,
pinned by `crates/mem-profile/tests/user_profile_decay.rs` — the 0.8 → 0.26675 case). For a frequency-1
item the per-sweep map at decay factor `f` is `c ↦ f·(0.3·0.8333 + 0.7·c)`; at the pinned 90-day fixture
`f = 0.5`, so `c ↦ 0.125 + 0.35c` with fixed point `c* = 0.125/0.65 = 0.1923`.

Two restarts in a day therefore land nearer `0.1923` than the one-sweep value (`0.26675`), and N restarts
converge on the fully-decayed value instead of collapsing to zero. Nothing is crossed on the way:
`crates/mem-profile/src/profile_context.rs:320-322` reads confidence as a continuous weight
(`unwrap_or(0.0)`), and no `confidence >= …` constant exists anywhere under `crates/`.

So this is **over-decay against the stated daily contract** — bounded, self-limiting, no data loss and no
threshold to trip — which is why it did not block issue 90. It is still a correctness gap that must have
an owner.

## What to build

Persist the stamp so the 24 h gate survives a process restart. Triage decides between:

- a sidecar file beside `user-profiles.db` (the cold-buffer snapshot at `crate::cold_buffer::cold_buffer_path`
  is the existing precedent for derived state kept off the DB);
- a column on `user_profiles` (a schema/format change: the migration set is versioned and a newer root
  already refuses to open);
- an existing watermark — `update_profile` binds wall-clock `now_millis()` to `last_analyzed_at`, and the
  sweep writes a `Decay sweep: confidence decayed` changelog row, so a "last sweep" query is derivable;
  it needs a decision on which profile's row answers when several exist.

## Acceptance criteria

- [ ] A restart **inside** the interval does not sweep again: seed, `sweep_decay_daily(T)` on one store,
      drop it, construct a new store over the SAME root, `sweep_decay_daily(T + 1 h)` returns
      `profiles_scanned == 0`.
- [ ] A restart **after** the interval does sweep: the same shape at `T + 25 h` returns
      `profiles_scanned >= 1` and persists a lower confidence.
- [ ] A lost optimistic-version CAS keeps its current contract: logged at `debug`, excluded from
      `profiles_updated`, no inline retry, deferred to the next *due* sweep.
- [ ] A missing, unreadable or corrupt persisted stamp degrades to "due" (fail toward decaying, never
      toward skipping forever) and produces ONE named event, not one per tick.
- [ ] Whichever storage shape is chosen, the refusal/downgrade rule for an older or newer build is
      recorded where the format is documented — no silent second on-disk surface.
- [ ] The "daily decay sweep" row of `crates/mem-profile/CONTEXT.md` describes the gate as durable, and
      the `sweep_decay_daily` doc comment (`profile_store.rs:1050-1061`) stops describing it as
      store-lifetime state.

## Relation

Gated on issue 90 (without a caller there is nothing to compound). Does **not** block issue 90: the caller
is correct and the over-decay is bounded, so landing 90 first is intended.

## Evidence

- Tree `8c6bfb2`: the anchors above, re-read for this issue.
- Plan gate 1 for the issue-90 blueprint: `.tmp/progress/profile-decay.gate-1.md` §Gate 2 (adjudicated the
  bound and required this issue to exist). `.tmp/` is gitignored — the arithmetic above is therefore
  reproduced here in full rather than referenced.

## Comment — 2026-11-03: exposure is zero today, and the runway is ~6 days

MEASURED (read-only) and reasoned after a triage cross-check found this issue ranked too low:

- `crates/mem-profile/src/profile_store.rs:413` — `last_decay_sweep: Mutex::new(None)`, **in memory
  only**, never persisted; `sweep_is_due` at `:1076` therefore returns true on **every engine start**.
- `crates/mem-bridge/src/engine.rs:1678` — `spawn_profile_decay_lifecycle` runs a startup tick, added by
  `39dadf3` (2026-11-01), so this is live now.
- `profile_store.rs:1473` — `confidence = blended * decay_factor(item, now)`; `decay_items` never
  advances `last_seen`, so a second pass multiplies the already-decayed value again.
- `profile_store.rs:288` and `:1456` both call `sync_confidence` → **twice per pass**.
- `crates/mem-profile/src/decay.rs:53-55` — `decay_factor` is exactly `1.0` inside the grace window.

**Why nothing is decaying yet (owner: the install is 24 days old):** the grace is
`userProfileConfidenceDecayDays = 30` — the default (`schema.rs:602`) and the live value
(`~/.config/opencode/opencode-mem.jsonc:252`). No profile item can be older than the install, so every
`decay_factor` is `1.0` today and the double blend multiplies by one. **Exposure now: zero.**

**The runway is ≈ 6 days.** From the day the install passes 30 days, every engine start applies the
blend to the oldest items — silently, irreversibly, and twice per pass. The fix (persist the stamp, and
remove the duplicate `sync_confidence`) must land **before** that, not when someone notices a number
moving.
