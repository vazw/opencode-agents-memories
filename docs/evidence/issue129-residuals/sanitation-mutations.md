# Issue 129 — Step 3f: mutation proof for the startup-audit guards

Method: a scratch worktree `.tmp/scratch/mut` at base `eda2efd` with the GREEN files copied in
(`crates/mem-bridge/src/{lib,composition,startup_audit}.rs`, `tests/production_api_wiring.rs`). The
shard worktree is never mutated. No commit exists at the time of this run (the integrator serializes
commits), so the "GREEN commit" is that copied tree; `git status --short` in the scratch worktree before
the first run showed exactly those four files, and the unmutated baseline exited **0**
(`.tmp/progress/r9-shard-A/mut-baseline.log`).

Per guard: apply ONLY that guard's single-line mutation, run

```
cargo test --test production_api_wiring test_startup_audit_sanitizes_names -- --exact --nocapture
```

record the exit status and the FULL violation list, restore the file from the shard worktree, rerun.
Runner: `.tmp/scratch/mut-run.sh`; transcripts `.tmp/progress/r9-shard-A/mut-<guard>[-restore].log`.

G4 is **not** mutated: Step 2b measured that startup never reaches the shard registry, so no shard entry
exists and there is no mutation site. Recorded as ABSENT BY CONSTRUCTION (invariant: no entry field
carries a free-form shard name), never counted as killed.

| guard | function | mutation diff | mutated rc | own label present | extra labels (why) | restore rc |
|---|---|---|---|---|---|---|
| G1 root redaction | `root_name` | `escape_control(&basename)` → `escape_control(&root.display().to_string())` | 101 | **yes** `[G1]` | `[G2]` — the leaked root_name carries the full path, and the captured error text contains the same path, so 12-byte runs match; `[EQ]` — root_name is no longer the escaped basename. No `[G3]`: controls are still escaped, as predicted. | 0 |
| G3 control escaping | `escape_control` | first statement `return input.to_owned();` (body becomes unreachable) | 101 | **yes** `[G3]` | `[EQ]` — the identity escaper leaves `\n`/`\t` raw, so root_name ≠ the literal escaped basename. No `[G1]`/`[G2]`: no path or error text involved. Exactly the predicted pair. | 0 |
| G2 raw-error redaction | `episodic_entry` (Err arm) | `reason: refusal_reason(error).to_string()` → `reason: error.to_string()` | 101 | **yes** `[G2]` | `[G1]` — the store text starts with the absolute path and the marker; `[G3]` — it also contains the basename's raw newline and tab. Exactly the predicted triple. | 0 |

Verdicts: **G1 KILLED**, **G3 KILLED**, **G2 KILLED** (own label present in each case).
**G4 ABSENT BY CONSTRUCTION / claim dropped.** An unkilled mutant count: 0.

### Review-1 M2 — the presence/symlink boundary (rework, 2026-11-06)

Review-1's mutant M2 survived the original suite (`symlink_metadata` → `metadata` at
`startup_audit.rs` `presence()`, rc=0, 5/5 audit tests passed): no fixture placed a symlink at
`episodic.db`, so the fail-closed boundary had a vacuous oracle. The rework adds
`test_audit_dangling_symlink_at_episodic_is_unknown` — the DANGLING link is the distinguishing
input, because `symlink_metadata` reads the link itself (Ok → `Presence::Other` → `unknown`), while
the M2 mutant follows it (`Err(NotFound)` → `Presence::Absent`) and the eager actor then creates
through the link, so the outcome flips `unknown` → `created`.

| mutant | function | mutation diff | mutated rc | own assertion | extra labels | restore rc |
|---|---|---|---|---|---|---|
| M2 / presence | `presence` (`crates/mem-bridge/src/startup_audit.rs:73`) | `match std::fs::symlink_metadata(path) {` → `match std::fs::metadata(path) {` | 101 | **yes** — `left: String("created") right: "unknown"` on `entry["outcome"]` (`tests/production_api_wiring.rs`, the dangling-symlink test) | none — the companion escaping-link test stays green under M2 (collinear, labelled NOT-an-oracle) | 0 |

Measured (rework run, scratch copy `.tmp/scratch/m2-rework-r6cT` made via `git archive HEAD` +
the four shard files, own `CARGO_TARGET_DIR`, shard worktree never mutated):
mutated run `.tmp/progress/r9b-rework/m2-mutant.log` rc=101 with exactly
`left: String("created") right: "unknown"`; correct-tree runs
`.tmp/progress/r9b-rework/f1-dangling-green.log` and `f1-escaping-green.log` both rc=0; post-mutation
correct-tree re-run rc=0 (worktree untouched — verified by the passing rerun and `git status`).
Verdict: **M2 KILLED** (own assertion `outcome == unknown` present).

Violation lists verbatim (first line each):

```
G1: [G1] "<full escaped path>/HOSTILE_MARK_7f3a/root-\\n\\t-x"
    [G2] "<same value>"
    [EQ] root_name "<full escaped path>" != "root-\\n\\t-x"
G3: [G3] "root-\n\t-x"
    [EQ] root_name "root-\n\t-x" != "root-\\n\\t-x"
G2: [G1] [G2] [G3] "Refusing to open database <path>/HOSTILE_MARK_7f3a/root-\n\t-x/episodic.db: …"
```

The `[EQ]` oracle is a LITERAL expected escaped string (`root-\\n\\t-x`), not a call to
`escape_control`, so the test compiles and runs on the pre-change tree and a wrong or empty sanitizer
cannot pass. G3's framing note holds: `serde_json` escapes control characters itself, so "no raw newline
in the NDJSON line" proves nothing about G3 — the decoded string is what the test inspects.
