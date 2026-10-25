# Issue 45 — the shared discriminating measurement (A vs B), raw output and verdict

Run 2026-10-25 23:0x +07 on `Ocean` (`Linux 7.2.11_1 #1 SMP PREEMPT_DYNAMIC … x86_64`), rustc/cargo 1.98.0,
repo **code** at `710a6df` (the only later commit, `c18d0b1`, is docs-only: it adds issue 45's O1–O6, which
landed *during* this wave; §7 accounts for it). Everything below is a single measurement applied identically
to both designs. Labels: `MEASURED` / `READ` / `INFERRED`.

Harness (throwaway, **not a deliverable**): `.tmp/scratch/issue-45-ab/issue45_ab_measure.rs`
(≈ 900 lines, re-execs itself as a child process; `fcntl` declared directly so **no** `Cargo.toml` /
`Cargo.lock` change and no `libc` dependency was added for the measurement).

Reproduce:

```sh
sh .tmp/scratch/issue-45-ab/apply-and-run.sh      # copies, runs, removes; the tree is clean afterwards
```

Raw logs (kept, set `--nocapture --test-threads=1`): `run-all.log`, `t1.log`…`t5.log`, `size.log`,
`existing-mixed.log` in the same directory.

---

## 1. What the measurement had to cover

| arm | question | result |
|---|---|---|
| **G1** (issue 45's gate) | with a live flag-on holder child, does an open **through `TursoConnectFactory`** succeed and reproduce the pinned row loss? | **yes — premise holds** |
| **S** (self-conflict) | does the raw probe refuse a second open of a database this process already holds? | probe A **yes**, probe B **no**; and probe B is *destructive* to our own lock |
| **T3** (clearance evidence) | does each design clear the same-process case **without** also clearing a foreign holder of a sibling database? | A refuses correctly; **B clears and loses a row** |
| **C** (cost) | the real per-open price, including the "two fcntl calls" | one probe = 2 fcntl + open/close = **5.70 µs**; marker read = **7.12 µs** |
| **T5** (decision table) | the writability asymmetry as a pure function | 14 cells printed |
| **45.7** | `StoreError` size | **112 bytes** (re-derived, not inherited) |

---

## 2. Raw output, verbatim

### 2.1 `run-all.log` — all arms in one process

```
running 6 tests
test m45_child ... ignored, re-exec'd worker: Kid::spawn runs it with --ignored
test m45_t1_g1_unguarded_open_through_the_factory ... [m45] t1 probes_live_flag_on_holder probe_a=Held probe_b=Free
[m45] t1 opener=OPEN-OK (premise holds: the guard does not exist)
[m45] t1 RESULT opener=OPEN-OK opener_before=[1, 2] opener_after=[1, 2] final=[1, 3] integrity=ok lost_row_2=true
ok
test m45_t2_self_conflict ... [m45] t2 S1 own_flag_on probe_a=Held second_factory_open=ok
[m45] t2 S5 probe_a_does_not_mutate_tshm while_held=Held before_len=Some(86016) after_len=Some(86016) identical=true
[m45] t2 S2 stale_tshm_no_holder probe_a=Free (G3: must be Free)
[m45] t2 S3 control_no_probe_foreign_flag_off_open=OPEN-ERR Locking error: Failed locking file '/tmp/mem-store-m45-self-24733-1792944254907292517/own_off_control.db'. File is locked by another process
[m45] t2 S4 own_flag_off probe_b_wholefile=Free foreign_open_after=OPEN-ERR Locking error: Failed locking file '/tmp/mem-store-m45-self-24733-1792944254907292517/own_off_whole.db-wal'. File is locked by another process
[m45] t2 S6 own_flag_off probe_b_one_byte=Free foreign_open_after=OPEN-ERR Locking error: Failed locking file '/tmp/mem-store-m45-self-24733-1792944254907292517/own_off_byte.db-wal'. File is locked by another process
[m45] t2 S7 own_flag_off probe_b_ofd_byte=Held foreign_open_after=OPEN-ERR Locking error: Failed locking file '/tmp/mem-store-m45-self-24733-1792944254907292517/own_off_ofd.db-wal'. File is locked by another process
ok
test m45_t3_clearance_evidence_ab ... [m45] t3 marker_written={"pid":24733,"start_time":28848369,"mode":"multiprocess"}
[m45] t3 inputs a=Missing b=Held registry={(34, 1864669): 1} a_own=NotOurs b_own=true a_decision=Refuse("legacy-authority") b_decision=Clear
[m45] t3 B_path_open=OPEN-OK
[m45] t3 B_path_result before=[1, 2] after=[1, 2] final=[1, 3] integrity=ok lost_row_2=true
[m45] t3 DECISION A=Refuse("legacy-authority") B=Clear differ=true
ok
test m45_t4_cost ... [m45] t4 probe_a_tshm: n=2000 mean=5.668us p95=5.600us max=53.591us
[m45] t4 probe_b_db_one_byte_fsetlk: n=2000 mean=5.695us p95=5.640us max=25.458us
[m45] t4 bare_open_close_baseline: n=2000 mean=3.253us p95=3.247us max=8.717us
[m45] t4 marker_read_parse: n=2000 mean=7.115us p95=7.224us max=33.533us
[m45] t4 fcntl_ofd_lock_unlock_only: n=2000 mean=2.240us p95=2.235us max=8.917us
[m45] t4 fcntl_setlk_lock_unlock_only: n=2000 mean=2.285us p95=2.275us max=15.519us
[m45] t4 registry_contains_key_with_stat: n=200000 mean=1735.504ns
[m45] t4 registry_contains_key_precomputed: n=200000 mean=225.733ns
ok
test m45_t5_decision_table ... [m45] t5 writability=Writable probe_a=Free probe_b=Some(Free) => Clear
[m45] t5 writability=Writable probe_a=Free probe_b=Held => Refuse("legacy-authority")
[m45] t5 writability=Writable probe_a=Held probe_b=Some(Free) => Refuse("multiprocess-authority")
[m45] t5 writability=Writable probe_a=Held probe_b=Held => Refuse("multiprocess-authority")
[m45] t5 writability=Writable probe_a=Missing probe_b=Some(Free) => Clear
[m45] t5 writability=Writable probe_a=Missing probe_b=Held => Refuse("legacy-authority")
[m45] t5 writability=Writable probe_a=Unknown(5) probe_b=Some(Free) => Refuse("probe-a-unknown")
[m45] t5 writability=Writable probe_a=Unknown(5) probe_b=Held => Refuse("probe-a-unknown")
[m45] t5 writability=Writable own=Ours => Clear
[m45] t5 writability=ReadOnly probe_a=Free probe_b=None => Clear
[m45] t5 writability=ReadOnly probe_a=Held probe_b=None => Refuse("multiprocess-authority")
[m45] t5 writability=ReadOnly probe_a=Missing probe_b=None => Clear
[m45] t5 writability=ReadOnly probe_a=Unknown(5) probe_b=None => Refuse("probe-a-unknown")
[m45] t5 writability=ReadOnly own=Ours => Clear
ok

test result: ok. 5 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.62s
```

`t5` exercises the **pre-amendment** decision function (probe A present). It is kept because it validates the
primitives and the old A3 table; the amended table (mode × writability, probe B only) is a design artefact in
draft A §2 and is not claimed as measured.

### 2.2 `size.log` — 45.7, re-derived

```
[m45] size_of StoreError = 112 ; Option<StoreError> = 112 ; PathBuf+u8 = 32 ; PathBuf = 24 ; String = 24
```

### 2.3 `existing-mixed.log` — the tree's own pinned mixed-mode matrix, re-run (raw `turso::Builder`, not the seam)

```
[cell cap-lock-cell-4-flag-on-vs-flag-on] opener=Ok(())
[cell cap-lock-cell-4-flag-on-vs-flag-on] final_ids=Ok([1, 3, 100]) final_integrity=Ok("ok")
[cell cap-lock-cell-5-flag-off-vs-flag-on] opener=Ok(())
[cell cap-lock-cell-5-flag-off-vs-flag-on] final_ids=Ok([1, 3]) final_integrity=Ok("ok")
[cell cap-lock-cell-3-flag-on-vs-flag-off] opener=Ok(())
[cell cap-lock-cell-3-flag-on-vs-flag-off] final_ids=Ok([1, 3]) final_integrity=Ok("ok")
[cell cap-lock-cell-6-flag-off-vs-flag-on-no-holder-write] final_ids=Ok([1, 100]) final_integrity=Ok("ok")
```

`test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 38 filtered out`.

Reading, one row per fact:

- **`t1` G1 — premise holds.** With a live flag-on holder child, the open through `TursoConnectFactory` with
  the multiprocess feature removed returns a live handle; the opener reads `[1,2]`, the holder commits `3`
  afterwards, the opener re-reads `[1,2]`, and after both close a fresh shipping handle reads `[1,3]` with
  `integrity_check = ok`. **The opener's committed row 2 is gone and nothing errored.** This is the first
  time the loss is measured *through the production seam* rather than through `turso::Builder` (43's
  `b42905c` was the latter).
- **`t1` also measures A2:** with that same live flag-on holder, **probe A = `Held`, probe B = `Free`** — a
  multiprocess holder holds `.tshm` byte 0 and **no** lock on the database file. Both A2 claims are now
  runtime observations, not code reads.
- **`t2 S1` — the pre-amendment self-conflict is real and belongs to probe A.** Our own flag-on handle makes
  probe A answer `Held` (would refuse our own second open), while the second factory open currently
  **succeeds** (`second_factory_open=ok`).
- **`t2 S2` — G3:** `.tshm` present with **no** holder ⇒ probe A `Free`. Presence is not liveness; the
  `43:157` rejection reproduces.
- **`t2 S5` — 45.14's mechanism:** `.tshm` is **byte-identical** across a probe (`identical=true`, 86 016
  bytes). A byte lock does not mutate the file.
- **`t2 S3` — the probe-B control:** a live flag-off holder refuses a second flag-off opener, and the engine's
  own text names the **database file**. (That is the one integration direction O6 retains.)
- **`t2 S4/S6/S7` — new finding, not in the issue:** any open+close of the database file in a process that
  holds a POSIX record lock on it **releases that lock**. After the probe, the foreign flag-off opener gets
  *past* the db-file lock and is refused only by `…db-wal`; identical for `F_SETLK` whole-file (`S4`),
  `F_SETLK` one byte (`S6`) and `F_OFD_SETLK` one byte (`S7`) — the release comes from `close()`, not from
  the fcntl. `S7` also shows `F_OFD_SETLK` **reports** our own POSIX lock as `Held` instead of replacing it.
- **`t3` — the A/B discriminator.** This process holds `one.db` (marker `{"pid":…,"mode":"multiprocess"}`
  written, registry entry `(34, 1864669)`), while a **foreign** process holds `two.db` in legacy mode
  (probe B = `Held`; no `.tshm` ever existed ⇒ probe A = `Missing`). Design A's evidence (identity-keyed
  registry, `two.db` absent) ⇒ `Refuse("legacy-authority")`. Design B's evidence (live marker names this
  pid+start_time) ⇒ `Clear`. Executed: B's open succeeds, writes row 2, the legacy holder commits 3 and
  closes, and the fresh read is `[1,3]` with `integrity=ok`: **row 2 lost, silently.**
- **`t4` — cost, §3 below.**
- **`existing-mixed`** — the tree's raw-builder matrix still behaves exactly as pinned: mixed directions lose
  the opener's row (`[1,3]`), same-mode shares coherently (`[1,3,100]`), and the loss needs the other side's
  next commit (`[1,100]`). No new claim, but re-derived this wave rather than inherited.

---

## 3. Cost (45.12) — numbers, not guesses

Debug test profile (`unoptimized + debuginfo`), n = 2 000 per probe, 200 000 registry lookups:

| measurement | mean | p95 | max |
|---|---|---|---|
| probe A: `open`+`F_OFD_SETLK`+`F_UNLCK`+`close` on `.tshm` | **5.668 µs** | 5.600 | 53.591 |
| probe B: same on the database file, 1 byte, `F_SETLK` | **5.695 µs** | 5.640 | 25.458 |
| bare `open`+`close` of the database file (baseline) | 3.253 µs | 3.247 | 8.717 |
| **marker read + `serde_json` parse** (design B's per-open cost) | **7.115 µs** | 7.224 | 33.533 |
| **fcntl lock+unlock pair alone, `F_OFD_SETLK`** | **2.240 µs** | 2.235 | 8.917 |
| **fcntl lock+unlock pair alone, `F_SETLK`** | **2.285 µs** | 2.275 | 15.519 |
| registry `contains_key`, precomputed key | 225.7 ns | — | — |
| registry `contains_key`, with a `metadata` per op | 1 735.5 ns | — | — |

What this says:

- **"Two `fcntl` calls per open" is 4.5 µs, and each probe as a whole is ~5.7 µs** because it also opens and
  closes a file. The issue's phrasing under-counts by the open/close (3.25 µs of the 5.7).
- **Under O3 there is one probe, not two** ⇒ a writable multiprocess open pays ~5.7 µs of guard cost, and an
  own-process open pays the same (there is nothing to clear). Design B adds 7.1 µs to **every** open on top
  of that; design A's registry (if shipped) would add 1.7 µs + 0.2 µs and only on the probing path.
- Syscalls dominate; the debug profile inflates the in-memory rows most. **Re-measure in release before
  ADR-0025 quotes absolutes** — the ranking (A < B) is safe, the absolute numbers are profile-bound.

---

## 4. Verdict

### **A wins**, on the one measurement T3.

`[m45] t3 DECISION A=Refuse("legacy-authority") B=Clear differ=true` →
`[m45] t3 B_path_result before=[1, 2] after=[1, 2] final=[1, 3] integrity=ok lost_row_2=true`

A's clearance is per file identity and therefore refuses a foreign holder of a sibling database; B's
clearance is root-granular and therefore clears it, after which the pinned loss reproduces silently. Cost and
the failure-mode list point the same way (B pays 7.12 µs per open for a clearance that is unsound where it
differs from A), but **they are not what decided it** — T3 is.

### Rider that the amendment forces, and that matters more than the verdict

O2/O3 (landed during this wave, §7) delete probe A, and with it **the reachable need for either clearance**:
probe B runs only for a multiprocess open, a multiprocess open takes no lock on the database file (`t1
probe_b=Free`), and a legacy open is refused by construction, so `preflight` can never inspect a description
this process owns. `45.6` is satisfied **by proof plus a passing test**, not by a mechanism
(`t2 S1 second_factory_open=ok`).

So: **A is the design to take; neither clearance mechanism needs to ship in this issue.** The syseng gate
should take A's rule contract, ship probe B + `UnsupportedOpenMode` + the O4/O5 override + the A5 marker
write, and land A's registry only when a legacy opener returns (draft A §11 states the trigger and the tests
that must come with it).

If the orchestrator wants a single word, the answer to "A or B" is **A**. If it wants the honest engineering
answer to "what do we implement now", it is **A's rule without A's table** — the measurement dissolved the
clearance deliverable the same way `NEITHER` is meant to.

### Recommendation for the syseng gate

1. **Implement**: `ConnectFactory::connect` → `preflight` (`WalMode::Legacy ⇒ UnsupportedOpenMode`;
   `Multiprocess ∧ Writable ⇒ probe B`; read-only ⇒ `Clear` + recorded divergence) → open → `postflight`
   (probe B) → RAII-free handle. Refusals `OpenBlocked{db_path, direction}` +
   `UnsupportedOpenMode{db_path, mode}`; `StoreError` stays 112.
2. **Probe B**: one byte at offset 0, `F_OFD_SETLK` on Linux / `F_SETLK` elsewhere, `O_RDWR` **without**
   `O_CREAT`; `Missing` is a known answer, `Unknown` refuses unless the O4 override is active.
3. **Do not implement** probe A or any clearance table now; record A §11's trigger as an ADR precondition.
4. **Settle, before the tests go red**, the disposition of the two seam-based legacy probes
   `turso_capabilities.rs:2550` and `:2840`, which `UnsupportedOpenMode` will refuse (test-only constructor vs
   raw builder — a 45.1 row decision).
5. **Carry into ADR-0025**: O2's matrix (with A3's superseded rule as history), O3's construction refusal and
   its dependency, A5's marker location, A4's doc-vs-code contradiction, the POSIX close-releases-locks
   finding, the refusal shape and its no-identity posture, and the seam-completeness precondition.

### The inverted world for the recommendation

The claim is: *"under O2/O3 no clearance mechanism is needed, because probe B can never inspect a lock this
process holds."* It closes as **measurement** the moment either observation appears:

- a guarded **multiprocess** open whose probe B answers `Held` while the only holder is this same process —
  i.e. run T2's S-shape with the fixture opened through the seam (not the raw builder) and read `Held`; or
- a state in which a legacy handle can exist in the same process as a guarded open (a new legacy opener, or a
  production `turso::Builder` call) — then the registry ships in that same change, with the two tests in
  draft A §11.

Conversely, the **A-vs-B** verdict closes as measurement if T3's `differ=true` and `lost_row_2=true` are not
reproduced: e.g. a marker form whose clearance is per-file would make the two designs tie on T3, and the
decision would fall to cost — which the same run already answers in A's favour.

---

## 5. What the issue — and its own amendment — assert that did **not** reproduce

1. **The amendment's closing consequence is wrong.** O1–O6 say: *"with probe A gone, the self-conflict
   problem is entirely a probe-B concern on Linux — an exclusive lock on the database file conflicts between
   open file descriptions, including two in the same process, and our own cached handle is exactly such a
   description."* `MEASURED`: probe B against our own live **multiprocess** handle answers **`Free`** (`t1`),
   because that handle holds no lock on the database file at all. There is no probe-B self-conflict on the
   only mode this tree can produce. The destructive effect that exists is different — `close()`, not the
   lock (`t2 S4/S6/S7`) — and it needs a *legacy* handle in-process, which O3 removes. The consequence of
   this correction is that `45.6` is vacuous and the clearance deliverable is not needed now.
2. **The pre-amendment `45.6` framing was still right about probe A** (`t2 S1 Held`), so the amendment
   deleted the probe that created the risk and then attributed the risk to the probe that does not.
3. **`45.12`'s "two `fcntl` calls per open" under-counts**: two fcntl calls are 4.5 µs, but each probe is a
   file open+close too (5.7 µs). Under O3 it is one probe, so the figure is ~5.7 µs; the issue's number
   describes a design the amendment then removed.
4. **O2's premise is confirmed, and it was the issue's real defect**: with a live flag-on holder, probe A
   answers `Held` (`t1`) — so the old "probe A every open" rule would refuse the legitimate second
   multiprocess opener (`purge-session`), and the tree's own cell 4 shows same-mode sharing is coherent
   (`[1,3,100]`, integrity ok). O2 is right; A3's rule was wrong.
5. **O3's premise is confirmed** (`READ`): `ConnectOptions::for_config` never consults config for the mode
   (`db.rs:253-269`), and no production `turso::Builder` call exists — so no legacy opener is reachable.
   **But O3 has an unlisted cost**: two existing tests open *through the factory* with the multiprocess
   feature removed (`turso_capabilities.rs:2550`, `:2840`), and `UnsupportedOpenMode` refuses them. Neither
   the issue nor the amendment mentions this; it is a 45.1 disposition decision.
6. **45.1's open-path table is incomplete** beyond the four rows it names: `encryption_migrator`'s four
   connect sites (including a staged `*.encrypt-*.tmp` path), `mem-profile`'s per-operation coordination DB
   (`learning_lock.rs:790`), and the fact that the largest test class (`turso_capabilities`) deliberately
   bypasses the seam. Draft A §6 adds them.
7. **No contradiction found in the rest**: G1 holds; the `-tshm` version/magic/resize citations (`READ`);
   the OFD lock carrying no `l_pid` (`READ`); `StoreError = 112` (re-measured); stale `-tshm` is not
   liveness (`MEASURED`); a flag-on holder holds no database-file lock (`MEASURED`).
8. **A3's "our tree opens no read-only database today"** holds (`READ`: no open-level read-only flag or
   call; the `read_only` hits are shard metadata).

---

## 6. Not measured / limits (stated, not implied)

- Only Linux/x86_64. `F_SETLK` on other unix and the Windows vacuous path are **untested**.
- Debug profile only; no release numbers.
- The post-open probe direction table is `INFERRED` design reasoning; its refuting test is named in draft A
  §10.4 and not run (there is no implementation to invert).
- The marker **write** path (fsync cost, `create_new` race) was not measured; only the read+parse was.
- The "`-wal` lock survives a checkpoint+unlink?" inference is **unsettled**; the settling test is named in
  draft A §3.
- The end-to-end harm in T3 was measured for the `legacy holder + multiprocess opener` direction (the one O6
  keeps). The mirrored direction's loss is the tree's pinned `cell 3`, re-run above, not re-derived through
  the factory (the amended guard no longer covers it).

---

## 7. `OWNER DECISION NEEDED`

**`OWNER DECISION NEEDED:`** `45.6`'s risk basis was refuted (§5.1). Two admissible postures, and the choice
is a risk-cost trade the owner has been making elsewhere in this issue:

- **(i) Proof, no mechanism.** Ship probe B + `UnsupportedOpenMode`; `45.6` is satisfied by the vacuity proof
  and its test, and draft A §11's trigger binds the first legacy opener to land the registry in the same
  change. Cost: 0 µs, no new state, no defence today for a raw-builder legacy handle (which is outside the
  seam's contract anyway).
- **(ii) Defence in depth now.** Ship A's identity-keyed registry even though it cannot fire: 1.9 µs per
  probing open, a global `OnceLock<Mutex<…>>`, a per-key async gate and its own concurrency tests, for a case
  O3 makes unrepresentable.

The measurement supports (i); the amendment's text assumes (ii)'s premise. **The architect does not decide
this** — it changes the risk posture the owner set in O1/O3/O4, not a mechanism detail.

---

## 8. Reproducibility and tree state

- Harness: `.tmp/scratch/issue-45-ab/issue45_ab_measure.rs`; runner: `.tmp/scratch/issue-45-ab/apply-and-run.sh`
  (copies in, runs, removes via `trap`).
- No `Cargo.toml` / `Cargo.lock` change: `fcntl` is declared directly, `libc` was **not** added for the
  measurement (it is still the implementation's dependency to add, 45.11).
- `git status --porcelain` is empty; `.tmp/` is gitignored. The tree was left at `c18d0b1`.
