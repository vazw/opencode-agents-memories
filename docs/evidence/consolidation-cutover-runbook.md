# Cutover runbook — lossless shard consolidation (Steps 11–13)

**Status: EXECUTED 2026-11-16 18:12 +0700 — the window ran against the live root.**
Custody `recovery/consolidation-1794827545-7788a5b952ddd3c5` holds the journal
`StagedVerified(1) → Installing(2) → InstalledUnverified(3) → Verified(4)`; measured
before → after: 37 shards / **5939** memory rows → 35 shards / **5939** rows, identity
rewrite `/home/vaz` → `/home/opencode` complete (0 rows left with a vaz-rooted
`project_path`), 300 children reparented with 0 dangling references. The deployable
for that run was H3 `e07df280…`; H4 `872bcede…` (wildcard-bind host fix) was staged
and activated at 19:59. **Everything below is kept unchanged as the record of what
was actually run** — it was written as a DRAFT and is not a standing authorisation
for a second run: a re-run needs its own approval and its own custody.

Every command below is written against the tree at `07d4440`. Every claim carries
`MEASURED` (a command was run and its output is quoted) or `READ` (source was read,
nothing was executed). Anything unlabelled is an instruction, not a fact.

**Driver:** `docs/evidence/consolidation-cutover-window.sh` implements §2–§11 with a
hard gate on every step (`status` · `stop` · `window` · `all`). It refuses to start
without `EXPECTED_SHA256` matching the artifact, and it aborts with custody held on
the first non-zero exit.

---

## 0. Three blockers found while writing this — read before anything else

### B1 — the bound deployable was STALE; RE-BOUND at HEAD. `MEASURED`

| Binary | sha256 | bytes | `--mode=consolidate-acquire` |
|---|---|---|---|
| `plugin/engine/linux-x64/opencode-agents-memory` (installed, Nov 10) | `746290cb…` | 81749024 | **unknown mode** |
| old artifact (bound Nov 13 20:16) — **DO NOT DEPLOY** | `24b02b3566bc5a31e8b6bc8ca7d2ed99bcef33eecf63f53542a38c1d427e7756` | 83097040 | **exit 64** |
| artifact at HEAD `07d4440` — **SUPERSEDED 2026-11-14, DO NOT DEPLOY** | `edc5783306ead3c3d5646a31ef0046348bbae777b9d1a9eb309fd6202f8c0266` | 83197328 | **exit 77** |
| **H2, `cargo build --release --offline` at HEAD `a6e9d57` (bookkeeping-phase fix)** — **SUPERSEDED 2026-11-16, DO NOT DEPLOY** | `9d7073efd4900449e64c36089c79f0d6855ea239a1cb3389beffdcde5e4ff7cd` | 83197752 | **exit 77** |
| **H3, `cargo build --release --offline` at HEAD `13171c9` (sshd fail-closed exemption)** | `e07df28057d05891ab45c2d79b70a5a865d394a17f346b6b9db4ccaa9e8911fa` | 83200696 | **exit 77** |

**H3 is the deployable.** It fixes the defect that blocked four cutover attempts:
`consolidate-acquire` refused 77 `acquire_writer_active` whenever any SSH session of
the same uid was open, because `live_process_holds_fd_under` failed closed on
sshd's `PR_SET_DUMPABLE=0` session processes. H3 exempts `comm == "sshd-session"`
in that arm only; every other refusal is unchanged. MEASURED on the same host, same
shell, with the three `sshd-session` leaders alive: **H2 → 77, H3 → 0 with a
reservation minted**, and rehearsal-6's full driver window completed with acquire
succeeding while the unreadable dirs were printed in its own log
(`docs/evidence/consolidation-rehearsal-6-20261116.md`, `SETTLEMENT: PASSED`).
H2 is preserved beside it as `.tmp/cutover-artifact/opencode-agents-memory.9d7073ef`.

The earlier note below about "the file rehearsed was no longer the file that shipped"
applies to H3 only in the sense that H3 was built at `13171c9` and then rehearsed —
do not rebuild between the rehearsal and the install.

`64` is `EXIT_PLAN_INVALID` (`src/shard_consolidation.rs:37`), i.e. the mode is not
recognised. The artifact bound for Step 11 was built *before* rev-15/FOLLOW-ON-ACQ
landed, so it contained no `consolidate-acquire`, no `consolidate-release`, no fence.
The "deploy the exact file rehearsed, never rebuild" rule is still correct — but the
file rehearsed was no longer the file that shipped.

**Re-bound 2026-11-14 14:10 +0700** by `install -m 0755 target/release/opencode-agents-memory .tmp/cutover-artifact/`. Mode-surface probe on the new artifact, all
`READ` from the emitted `code` field: `consolidate-acquire` 77 · `consolidate-release`
77 (`nonce required`) · `snapshot`/`verify-snapshot`/`verify`/`recover`/`journal` 77 ·
`consolidate-shards --dry-run` 64 `plan_unreadable` (correct: the probe plan does not exist).

**The staleness discriminator, keep it:** probe `--mode=consolidate-acquire
--storage-root /nonexistent-probe-root`. A build with the mode answers **77**; a build
without it answers **64**. A hash binding proves the bytes did not change — never that
those bytes contain the slice.

The Step-10 rehearsal was run against the OLD artifact, so its verdict does not cover
these bytes. **Rehearsal-4 does**: `SETTLEMENT: PASSED` against
`edc57833…0266`, Part A 34/34 memory groups exact and 7/7 aux tables equal after the
§C.5a reparent oracle, Part B B1–B11 measured, frozen manifest `e0fe0276…d258e`
unchanged before and after. See `.tmp/progress/20261114-rehearsal-4.md`.

### B2 — `consolidate-acquire` cannot run while the engine is alive. `READ` + fd census `MEASURED`

**Enforced by the driver since window attempt 2 (2026-11-15): `preflight` greps
`opencode.jsonc` for the uncommented plugin line and refuses the run while it is
enabled; `do_window` re-runs `verify_stopped` immediately before `acquire`.**
Attempt 2 proved the hazard live: a fresh host + engine appeared mid-window,
between the first `verify_stopped` and `acquire`, and `acquire` correctly refused
77 `acquire_writer_active` against a holder the driver's earlier scan no longer
saw.

**And since attempt 4 (2026-11-16): `fence_no_unreadable_fd` names the processes
whose unreadable `/proc/<pid>/fd` would make `acquire` fail closed.** That was the
blocker behind attempts 2–4: `live_process_holds_fd_under` fails closed on an
unreadable fd dir, and **sshd sets `PR_SET_DUMPABLE=0` on its session processes**,
so `consolidate-acquire` refused 77 whenever any ssh session of this uid was open.

**H3 removes that limitation** (commit `13171c9`, blueprint
`20261116-sshd-fail-closed-exemption-r2`): the engine now exempts
`comm == "sshd-session"` in the unreadable-fd arm only. MEASURED on this host with
three `sshd-session` leaders alive — H2 → 77, H3 → 0 with a reservation minted —
and rehearsal-6's full driver window completed with acquire succeeding while the
unreadable dirs appeared in its own log. The driver's fence therefore **no longer
refuses on sshd sessions**: it prints `note: N sshd-session process(es) … exempt in
H3, not blocking` and dies only on a same-uid process of **any other** `comm` with an
unreadable fd dir — which H3 still fails closed on. `status` prints both classes:
`unreadable fd dirs: sshd-session (exempt in H3)=N, other (fail-closed)=…`.

Consequence for the operator: **the window now runs over SSH.** No local console
login is needed, and no ssh session has to be closed.

`fence_no_opencode` (since attempt 3) separately refuses while ANY opencode
process lives: a host started while the plugin line was enabled carries the
plugin in memory — commenting the config does not unload it — and spawns engines
on demand. The acquire step is also wrapped in a 15 s `/proc` watcher that prints
any holder observed during the step.

`acquire` runs its checks in this order (`crates/mem-store/src/maintenance_reservation.rs:371-377`):

```
check_root_permissions(parent)   -> 77 acquire_root_permissions_invalid
check_root_permissions(S)        -> 77 acquire_root_permissions_invalid
check_foreign_engine_holder(...) -> 77 acquire_foreign_engine_holder
check_active_writer(S)           -> 77 acquire_writer_active
```

`check_active_writer` walks `/proc/*/fd` across same-uid processes and refuses when
**any** process other than itself holds a descriptor resolving under `S`
(`:256-330`, skip set = caller pid only). The live engine holds at least six
(`MEASURED`: `/proc/16234/fd` → `episodic.db`, `episodic.db-wal`, `skills.db`,
`skills.db-wal`, `user-profiles.db`, `user-profiles.db-wal`, `metadata.db`).

Consequence: **acquire must run after the stop, not before it.** That inverts design
§E.2 / §G row S1a ("acquire FIRST … before the S2 quiesce"), and it voids the design's
auto-respawn argument, which assumed the fence already existed when the stop began.
The unfenced interval is `[stop, acquire]`, closed only by the operator's post-stop
re-check (§3 below). This is a design/implementation divergence for the owner to rule
on — do not paper over it by moving the step silently.

Not executed: probing acquire against the live root could mint a reservation on
production. The refusal is inferred from source plus the fd census, not observed.

**RESOLVED OPERATIONALLY, 2026-11-14.** The plugin entry was commented out of
`/home/opencode/.config/opencode/opencode.jsonc` (line 220, file hash now
`d7f408a802ee53c28da78c0406ebcc02012a7ec4e80fb53489ada2e77a5a731d`) and the host was
restarted. `MEASURED` after the restart: no `opencode-agents-memory` process, **0** fd
holders under `S`, `$S/.engine.lock` removed, no reservation, no custody dir. The
respawn hazard is therefore closed by the disabled plugin rather than by the fence, and
the window order is **stop → acquire** as a documented fact rather than a defect.
`check_foreign_engine_holder` tolerates an absent `.engine.lock`
(`maintenance_reservation.rs:218`, `Err(NotFound) => Ok(())`), so an absent lock file is
not a refusal.

### B3 — a crash mid-retake is unrecoverable on the currently bound artifact. `MEASURED`

Found by the NV-1(d) crash fixture, 2026-11-14. A crash immediately after the retake
writes its bookkeeping record leaves the journal's last line as
`{"phase":"Retaken","retake_seq":N}`, which carries no `storage_root`
(`src/shard_consolidation.rs:5067`). Then:

- `effective_phase` (`:3654`) returns `"Retaken"` — although its own doc comment
  (`:3652-3653`) says it skips bookkeeping records;
- `settle` (`:5495`) reads `storage_root` as `""` and refuses
  **77 `journal names a different storage_root`** on the *same* root;
- `consolidate-recover` (`:5237`) has no `Retaken` arm and refuses
  **77 `journal phase requires an operator decision`**.

So on `edc57833…0266` a crash inside that window ends the run with no designed exit —
only a hand-edited journal, which every other part of this design forbids. The design
says otherwise (rev-15 gate ask 11(a): "the effective-phase definition (bookkeeping
records skipped)"; §H.1: a crash after writing `Retaken` "re-enters through that same
row"), so this is a conformance defect, not a design question.

**Do not run the window on `edc57833…0266`.** The fix supersedes it; the runbook's
hash is re-bound after rehearsal-5 passes on the new artifact. If a retake crash is
somehow hit on the old bytes, stop and escalate — do not hand-edit the journal.

---

## 1. Preconditions

```sh
S=/home/opencode/.opencode-mem/data          # storage root
P=/home/opencode/.opencode-mem               # parent of S
R=$P/.opencode-mem-maintenance.lock          # reservation
# C is NOT set here. It is derived after acquire (§7) from the window token, because
# the custody directory NAME is part of the release contract — see §11.
```

No pid belongs here. The host pid changes on every restart, and the plugin is disabled
(§0 B2) so there is normally no engine child at all. Identify processes by argv[0], not
by a number copied from an earlier session:

```sh
engine_pids() { pgrep -f '^([^ ]*/)?opencode-agents-memory( |$)' || true; }
```

- `S` and `P` are owner-owned and not group/other-writable — F12 preflight
  (`MEASURED`: `755 opencode:opencode` both).
- `$P/recovery` is `0700` (`MEASURED`: `drwx------`). The recovery flock opens it
  `O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC` and any other mode is `77` with no designed repair.
- `$R` is absent (`MEASURED`: `No such file or directory`).
- `/home` has ≥ 3× `du -sh $S` free (`MEASURED`: 447 G free, 3.2 G in use).
- The bound artifact has been rebuilt at HEAD and its `--mode=consolidate-acquire`
  probe on a throwaway root returns **77**, not 64 (§0 B1).

---

## 2. Stop

The wrapper owns the ladder; the custodian originates no signals beyond the host's
own teardown (design §G S2).

`MEASURED` (`plugin/dist/plugin.js:1253-1360`, constants `:52-53`):

1. `SIGTERM` to the host → `shutdownHandler` → `shutdownAll` → per-state `runShutdown`.
2. Capture drain (best-effort).
3. Wire `shutdown` request, `session_id: "oc-bridge-<host pid>"`.
4. `scheduleEscalation`: wrapper-owned `SIGTERM` at **2000 ms**, `SIGKILL` at **5000 ms**
   after the request returns or times out.
5. Reap children, then re-deliver the received signal to the host.

**Do not hand-roll this.** `opencode service stop` performs the teardown; signalling a
pid you copied from an earlier session signals nothing, because that pid is gone.
`MEASURED` 2026-11-14: the host is now pid 16843 under the new tree, and pid 16181 no
longer exists.

**Stopping the host from inside an opencode session kills that session** — the engine
and the shell are its descendants. Run it from a terminal outside, or detached:

```sh
setsid -f bash -c 'opencode service stop; sleep 5' </dev/null >/dev/null 2>&1
```

Budget: ≤ ~30 s (design §G S2), then abort the window.

With the plugin disabled (§0 B2) there is no engine child, so the ladder has nothing to
escalate: the stop is a plain `SIGTERM` to the host, and §3's verification should show
zero fd holders immediately.

---

## 3. Post-stop verification (must pass before acquire)

```sh
opencode service status        # expect the literal word: stopped  (it exits 0 either way)
engine_pids                    # expect: no output
for p in /proc/[0-9]*; do
  ls -l "$p/fd" 2>/dev/null | grep -q "$S/" && echo "HOLDER ${p#/proc/}"
done                           # expect: no output
```

The fd loop must print **nothing**. A `grep` for the repo path is not a substitute: it
matches any shell whose command line mentions the checkout, including the one running
the check — the driver hit exactly this and now anchors on argv[0].

Any surviving fd holder → **ABORT**, do not acquire. This is the only guard on the
`[stop, acquire]` interval that B2 opens.

---

## 4. Deploy the bound artifact

While stopped, replace the staged engine with the re-bound build and verify the bytes:

```sh
install -m 0755 .tmp/cutover-artifact/opencode-agents-memory \
  plugin/engine/linux-x64/opencode-agents-memory
sha256sum plugin/engine/linux-x64/opencode-agents-memory
# must equal e07df28057d05891ab45c2d79b70a5a865d394a17f346b6b9db4ccaa9e8911fa   (H3)
```

Do not rebuild between this point and restart. The bytes installed are the bytes the
window runs.

---

## 5. Plan generation

The plan is an input; no shipped mode derives one
(`docs/evidence/consolidation-plan-generator-20261113/README.md`, script sha256
`924b1dcf8fc4661840b8b247879887e4c933ffdb0eca6088686be9581322e343`).

```sh
python3 docs/evidence/consolidation-plan-generator-20261113/build_plan.py \
  "$S" "$P/plan-$(date -u +%Y%m%dT%H%M%SZ).json"
```

The generator searches `<parent>` then `<parent>/custody` for `manifest.sha256` or
`baseline.sha256`, and a baseline anywhere else makes it exit 1 with
`no manifest.sha256 or baseline.sha256 in (...)` — before any mode runs. **No shipped
mode writes such a file outside a custody snapshot, and the window's own custody is
created at the snapshot step, after plan generation**, so `do_window` builds
`$P/manifest.sha256` itself right before this step: `sha256  ./relpath` over the
stopped, quiescent `$S`, excluding `status` and `.engine.lock`, matching the custody
manifest's format. A manual invocation outside the driver needs that file first.
Confirm `baseline_source` in the printed summary names the file you expect.

The plan's three hash fields are bound into the plan digest and never verified against
the tree. Safety comes from the per-row `source_hash` census and from `baseline.sha256`
in custody — not from those three fields.

---

## 6. Dry run (custody-free)

**This runs BEFORE acquire (§7), and the order is not cosmetic.** The dry-run arm is
fenced by `refuse_if_reservation_held` (`src/main.rs:874`), which has no nonce
admission — unlike the apply arm, which goes through `admit_window_step`. A dry-run
under a held reservation is therefore refused **76 by design**. Plan generation (§5)
is likewise read-only and runs before acquire.

```sh
OPENCODE_MEM_MAINTENANCE_NONCE= \
  $E --mode=consolidate-shards --storage-root "$S" --plan "$PLAN" --dry-run
echo "exit=$?"
```

`--storage-root` is passed explicitly on **every** mode. Without it the mode silently
uses the config default, so an operator who overrides `S` would dry-run one root and
apply to another.

Apply is triggered by the **absence** of `--dry-run`, not by the `--apply` token
(`src/main.rs:795`). A valued `--apply` or any `--confirm` is refused `64`.

---

## 7. Acquire (S1a)

```sh
E=plugin/engine/linux-x64/opencode-agents-memory
$E --mode=consolidate-acquire --storage-root "$S"; echo "exit=$?"
```

Exit `0` ⇒ reservation minted at `$R`, mode `0600`
(`crates/mem-store/src/maintenance_reservation.rs:36`).

**It prints no nonce.** `run_acquire` (`src/shard_consolidation.rs:4954`) returns the
exit code only. The nonce lives in the reservation body:

```sh
NONCE=$(jq -r .nonce "$R")
WT=$(printf '%s' "$NONCE" | sha256sum | cut -c1-16)   # <wtok> for custody names
C=$P/recovery/consolidation-$(date -u +%s)-$WT
[ ! -e "$C" ] || echo "REFUSE: custody already exists: $C"
```

**`C` must live at `$P/recovery/consolidation-<epoch>-<wtok>`.** `run_release`
(`src/shard_consolidation.rs:4577-4600`) resolves custody by scanning `$P/recovery` for
a directory whose name starts `consolidation-`, is not one of the retired/corrupt/
failed/quarantine/escalation prefixes, and **ends in `-<wtok>`** — it **ignores
`--custody` entirely**. A custody dir at `$P/custody`, or at any other name, is
invisible to release and the window ends in `77 release_phase_nonterminal` ("no custody
journal for window") with the install already done. This was a real defect in the first
draft of this runbook and in the window driver; both are corrected.

Body shape (`ReservationBody`, `maintenance_reservation.rs:54-77`): `pid`, `start_time`,
`operation`, `root_basename`, `nonce` (16 CSPRNG bytes, standard base64),
`created_utc`, `retake_seq?`, `s_dev?`, `s_ino?`. Treat `nonce` as window identity,
not as a secret — it appears in directory names.

Refusals: `77 acquire_reservation_exists` · `77 acquire_root_permissions_invalid` ·
`77 acquire_foreign_engine_holder` · `77 acquire_writer_active`.

Every later window step is a **new process** and must carry the nonce per command
(never exported, never in a file):

```sh
export -n NONCE   # if it is in the environment, unset it; pass it inline instead
```

---

## 8. Snapshot (S3)

Custody must **not** exist — `snapshot` creates it `0700` and fails on `EEXIST`
(`src/shard_consolidation.rs:3510-3544`). Use the `C` derived in §7; nothing else is
reachable by release.

```sh
OPENCODE_MEM_MAINTENANCE_NONCE="$NONCE" \
  $E --mode=consolidate-snapshot --storage-root "$S" --custody "$C"
echo "exit=$?"
OPENCODE_MEM_MAINTENANCE_NONCE="$NONCE" \
  $E --mode=consolidate-verify-snapshot --storage-root "$S" --custody "$C"
echo "exit=$?"
```

Artifacts written (`READ`, `:3519-3543`): `mode-table` (first, captured pre-chmod),
`baseline.sha256`, `excluded`, then `data/` (the snapshot tree), then a re-hash that
must equal the baseline, then a freeze.

---

## 9. Apply (S4 → staged tree)

```sh
REPORT=$(OPENCODE_MEM_MAINTENANCE_NONCE="$NONCE" \
  $E --mode=consolidate-shards --storage-root "$S" --plan "$PLAN" --custody "$C"); rc=$?
echo "exit=$rc"
printf '%s\n' "$REPORT"
STAGED=$(printf '%s' "$REPORT" | jq -r .staged_root)   # needed by §10
```

`--custody` is mandatory and must be absolute; otherwise `64 consolidation.custody_required`
(`src/shard_consolidation.rs:299-305`).

Refusals: `71` divergent rows · `72` identity · `73` embedding · `74` store/IO ·
`77` custody family (`window_not_held`, `window_busy`, `window_not_applyable`).

---

## 10. Staged verify, install, settlement

The staged directory name is **non-deterministic** — `staged_path` hashes
`plan_sha256 | pid | nanos` (`src/shard_consolidation.rs:1459-1467`) — so §9 read it
back from the apply report as `staged_root` rather than guessing it.

Both steps below need `--staged`:

```sh
OPENCODE_MEM_MAINTENANCE_NONCE="$NONCE" $E --mode=consolidate-staged-verify --storage-root "$S" --custody "$C" --staged "$STAGED"
OPENCODE_MEM_MAINTENANCE_NONCE="$NONCE" $E --mode=consolidate-install      --storage-root "$S" --custody "$C" --staged "$STAGED"
OPENCODE_MEM_MAINTENANCE_NONCE="$NONCE" $E --mode=consolidate-verify       --storage-root "$S" --custody "$C"
```

`consolidate-install` **requires** `--staged` even though its guard's message says only
`--custody <absolute-dir> is required` (`src/main.rs:633-650`: the arm matches when
`staged.is_some()`, and only `consolidate-recover` is exempt). `consolidate-verify`
takes `--custody` alone and reads the staged path from the journal.

**Never open the staged or installed tree with `sqlite3` between apply and settlement —
not even read-only.** `manifest_of` (`src/shard_consolidation.rs:3105`) hashes the whole
tree and skips exactly two names, `M1_EXCLUDED = ["status", "engine.lock"]` (`:3110`).
`-shm`, `-wal` and `-tsh` are **not** excluded, and a read-only SQLite connect still
creates them (measured: 11–93 sidecar files). The staged manifest then changes and
`staged_manifest_sha256` no longer matches, which turns a clean window into a 77.
Inspect a **copy**, never the live staged or installed tree.

- `consolidate-verify` **is** the settlement term: a plain-SQL dangling-reference count.
  It is the true no-op — a repeat `consolidate-shards --apply` at `Verified` refuses
  `77 window_not_applyable` by design (ruling F1').
- Settlement pass ⇒ journal `Verified` ⇒ S5 continues. Fail ⇒ `RollingBack`.
- Journal is `$C/journal.json` (`src/shard_consolidation.rs:3567`) and it is **JSONL** —
  one record per line. `jq '.records[-1]'` is wrong and prints every phase.
- **The last line is not always the phase.** `Retaken` and `TailDiscarded` are
  bookkeeping records: they carry a `phase` name but are never phases, and the code
  skips them (`effective_phase`, `src/shard_consolidation.rs:3652-3653`). To read the
  status yourself:

  ```sh
  jq -c 'select(.phase != "Retaken" and .phase != "TailDiscarded")' "$C/journal.json" | tail -n1
  ```

  A window whose raw last line is `Retaken` is *mid-retake*, not stuck; the effective
  phase is whatever precedes it.

---

## 11. Custody release (S5, before restart)

A held reservation refuses the restart by design — the restarted engine's own check is
the enforcement.

```sh
OPENCODE_MEM_MAINTENANCE_NONCE="$NONCE" \
  $E --mode=consolidate-release --storage-root "$S"
echo "exit=$?"
jq -r .nonce "$R" 2>/dev/null   # expect: cannot open / no such file
```

Release is gated: nonce + dead recorded holder + terminal journal state (`Verified` or
`RolledBack`) + EV-M/EV-R/EV-G/EV-N evidence agreement. It is custodian-invoked only.

What release does **not** check: fd holders under `S`. The `/proc/*/fd` scan is an
acquire-time refusal, not a release-time one, and the approved §E.2 release gate names
only nonce, dead recorded holder and journal phase. That is safe because a held
reservation refuses engine startup, so nothing legitimate can be holding `S` at release
time; an fd scan here would guard a bypass, not the designed path. Recorded as a
limitation, not a defect.

The startup refusal is not the CLI fence. `refuse_if_reservation_held` (`src/main.rs:369`)
guards only the four CLI writer modes; the **engine** has its own check at the top of
`run_in_runtime` (`crates/mem-bridge/src/engine.rs:1516`), before
`acquire_engine_lock`'s `create_dir_all`, so a refused engine creates nothing. Both
`stdio` and host modes reach it — `run_in_runtime` is the one production assembly path
(`engine.rs:279`). The module header states the same (`maintenance_reservation.rs:9`).

`MEASURED` 2026-11-14 on the bound artifact: a held reservation in `stdio` mode exits
**76 `maintenance.reservation_held`, `operation: engine`**, stdout empty, and the root
tree is byte-for-byte unchanged (218 entries, same path/size/mode/type) — the engine
creates nothing, not even `status` or `.engine.lock`. Releasing the reservation lets
the same artifact start. So while a window holds the reservation, no legitimate engine
can be holding `S`, which is what makes the release-side fd gap a limitation rather
than a defect.

If the release refuses and the journal is terminal, the recovery path is
`--mode=consolidate-recover` (§H.1 row for the effective phase), never a hand-deleted
reservation file.

---

## 12. Restart

The host is managed by opencode's own service manager — not by runit, not by hand.
`MEASURED` 2026-11-14 on `v2.2.7`:

```sh
opencode service status     # -> http://0.0.0.0:49374, exit 0
opencode service restart
opencode service stop
```

`service status` **exits 0 whether or not the host is running** — when it is down it
prints the literal word `stopped` (`READ`: `packages/cli/src/commands/handlers/service/status.ts`).
Read the output, never the exit code; a driver that branches on `$?` here is always
wrong in the same direction.

`opencode --help` lists `service start|restart|status|stop|get|set|unset`, plus a
top-level `reload` ("Reload configuration"). A config-only change therefore needs no
restart at all:

```sh
opencode reload
```

Path facts, `MEASURED`: the install moved from `/home/opencode/Code/opencode/…` to
`/home/opencode/Code/Personal/JS/opencode/…`; the old tree is gone. The name on `PATH`
is the symlink `/home/opencode/.local/bin/opencode` → the new tree, relinked
2026-11-14 15:58:23 (`opencode --version` → `v2.2.7`, exit 0). **If that link is left
dangling after a move, every `opencode …` invocation dies with `command not found`
(exit 127) while the running host keeps serving normally** — check the link before
concluding anything about the service. runit supervises only `9router` and
`vpn-reconnect` under `/home/opencode/.local/services`; it does not own the host.

Do **not** run `opencode service get` casually: it prints the service environment,
API keys included.

**Re-enable the plugin first.** The window runs with it commented out (§0 B2), so a
restart as the config stands brings the host back with **no engine** — a silent
degradation that looks like a successful cutover. Uncomment the plugin line in
`/home/opencode/.config/opencode/opencode.jsonc` (line 220), then restart:

```sh
opencode service restart
```

Startup smoke: the engine takes `$S/.engine.lock`, the reservation file stays absent,
and `consolidate-verify` still exits 0. With the plugin still disabled, no engine
spawns at all — that is the expected state for the *window*, and a failure for the
*restart*.

---

## 13. Exit codes (`READ`, `src/shard_consolidation.rs:37-49`, `engine_lock.rs:50`)

| code | constant | meaning |
|---|---|---|
| 64 | `EXIT_PLAN_INVALID` | bad plan, unknown mode, valued `--apply`, missing/relative `--custody` |
| 65 | `EXIT_UNSUPPORTED_SHAPE` | schema deviation from the plan's expectations |
| 71 | `EXIT_DIVERGENT_ROWS` | divergent payloads refused before any write |
| 72 | `EXIT_IDENTITY_REFUSAL` | identity census disagreement |
| 73 | `EXIT_EMBEDDING_REFUSAL` | embedding identity mismatch |
| 74 | `EXIT_STORE_OR_IO` | store open / IO |
| 75 | `ENGINE_LOCK_LOST` | engine lock lost |
| 76 | held | reservation held by a foreign holder |
| 77 | `EXIT_CUSTODY_REFUSAL` | every custody/window refusal (`window_not_held`, `window_busy`, `window_not_applyable`, `release_phase_nonterminal`, acquire family) |
| 78 | — | MCP mode unimplemented |

---

## 14. Abort

Any refusal before install: the reservation stays held. Run
`--mode=consolidate-recover` for the effective phase's §H.1 row. Restore is whole-root
and idempotent; the displaced tree is renamed to
`$P/recovery/consolidation-retired-<UTC>-<wtok>/` at `0700` and retained indefinitely.

No `rm`, no `rm -rf`, no deletion path exists in any plan or journal command surface.
Cleanup of scratch uses `find <dir> -mindepth 1 -delete`.

---

## 15. What this runbook does not authorise

- It does not authorise the cutover. Step 11 needs an explicit, recorded user approval.
- It does not authorise deploying the stale artifact `24b02b35…`.
- **It does not authorise deploying `edc57833…` either, as of B3.** Rehearsal-4 passed on
  those bytes, but B3 is a crash-window defect found afterwards; the artifact is
  superseded by the fix and must not be used for a window until the new hash is bound
  and rehearsal-5 passes on it.
- It does not settle B2 (acquire-after-stop vs design S1a-first). That is an owner ruling.
- It does not cover the §L crash-boundary fixtures `NV-1(d)`–(g), which are still
  unrun on the host filesystem and remain a Step-11 precondition.
