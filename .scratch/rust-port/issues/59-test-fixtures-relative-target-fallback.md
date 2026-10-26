# 59 — Test harness · two scratch fixtures resolve `target/` relatively, so with `CARGO_TARGET_DIR` unset one suite fails and another writes into the checkout

Status: ready-for-agent — 2026-10-27 (fully specified: premise measured, acceptance criteria and the open design question are in the issue). **With `CARGO_TARGET_DIR` unset, `cargo test --workspace` on a clean
tree exits 101** on `mem-skills`' `v2_bridge_skill_path_absolute`, and `mem-portability`'s suite passes while
silently creating `crates/mem-portability/target/` inside the checkout (untracked — `.gitignore`'s `/target`
is root-anchored). Found while landing 57, which fixed the third member of the same class; filed because the
repo's primary command is conditionally green and a test run dirties a clean tree.

## PREMISE — measured, not inherited

Both fixtures read the same variable and fall back to a **relative** path.

`crates/mem-skills/tests/skills_manager.rs:44-50` — `scratch_dir()`:

```rust
fn scratch_dir(label: &str) -> PathBuf {
    let base = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("target"));
    let dir = base.join("scratch").join("mem-skills").join(label);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
```

`crates/mem-portability/tests/portability_roundtrip.rs:198-204` — `scratch_root()`, the identical shape, with
**19 call sites** (20 matches of `scratch_root(`, one of which is the definition).

A cargo test process's CWD is its **package root**, so a relative `target` resolves *inside the checkout*:
`crates/mem-skills/target/…` and `crates/mem-portability/target/…`.

## THE TWO CONSEQUENCES — measured 2026-10-27 on merged `main` (`cdd37e4`)

| suite | `CARGO_TARGET_DIR` **unset** | consequence |
|---|---|---|
| `-p mem-skills --test skills_manager` | exit **101**, `15 passed; 1 failed` — `v2_bridge_skill_path_absolute` panics at `skills_manager.rs:379` on `path.is_absolute()` | the workspace cannot be green on a clean checkout |
| `-p mem-portability --test portability_roundtrip` | exit **0**, `20 passed` | passes, but creates `crates/mem-portability/target/scratch/mem-portability/…` inside the checkout; `git status` shows `?? crates/mem-portability/target/` |
| both with `CARGO_TARGET_DIR=$PWD/target` | exit 0 in both | the variable hides the defect entirely |

Attribution control: `mem-skills` is byte-identical to base (`git diff a6e9b34..HEAD -- crates/mem-skills` is
empty) and its suite is **16/16 green with the variable set** — the failure is the variable's absence alone,
not any wave's diff.

Ignore asymmetry (MEASURED): `.gitignore` has four rules — `/target`, `/example/`, `/tmp/`, `/.tmp/`. `/target`
is the only *target* rule and it is **root-anchored**, so:

```
git check-ignore -v target/x                      → .gitignore:1:/target   (ignored)
git check-ignore -v crates/mem-skills/target/x     → exit 1, no output     (NOT ignored)
git check-ignore -v crates/mem-portability/target/x → exit 1, no output    (NOT ignored)
```

## WHY IT MATTERS

- `cargo test --workspace` — the command `AGENTS.md` lists — is **conditionally green**: it needs
  `CARGO_TARGET_DIR` exported, and nothing in the repo says so.
- A test run on a clean tree leaves untracked directories. A commit staging a broad path could pick them up;
  `.gitignore` will not stop it.
- The failure surfaces as a panic about an unrelated assertion (`path.is_absolute()`), not as "you did not
  set the variable", so the diagnosis cost is real — it is the one item that made 57's AC-7 unachievable and
  cost that wave a rejected-then-revised blueprint.

## THIRD MEMBER — fixed, for context

`crates/mem-bridge/tests/purge_cli.rs` had the same fallback shape. It was fixed in 57 (`672625f`, merge
`cdd37e4`): root-binary resolution is now a pure, total `resolve_binary_path(&Path, Option<&OsStr>)` that
derives from the test executable's own location rather than the CWD. These two suites are the residue of that
class.

## NOT AFFECTED — checked, so a fix does not widen

- `tests/engine_shutdown.rs:804,873` sets `CARGO_TARGET_DIR` explicitly on a spawned child to an **absolute**
  scratch path — deliberate, not a fallback.
- `crates/mem-store/tests/episodic_v6_convergence.rs:30` mentions the variable in a doc comment only.
- `crates/mem-store/tests/common/mod.rs::lock_holder_binary_path()` does relative-free `examples/` arithmetic
  and was ruled **correct** during 57's gate; do not touch it here.

## ACCEPTANCE — what would close it

1. `env -u CARGO_TARGET_DIR sh .tmp/scripts/cargo-serialized.sh cargo test --workspace` exits **0**, with all
   231 result lines green on a clean tree (the merged-`main` count with the variable set), `mem-skills`' test
   included.
2. `find crates -maxdepth 2 -name target -type d` prints nothing after that run, and `git status --porcelain`
   is empty.
3. The fix is pinned by at least one test that **fails on the current bodies**. "Both suites pass with the
   variable present" is already true today and proves nothing.
4. Label isolation preserved: ADR-0016 D-4 records that concurrent suites collide on `mem-portability` scratch
   dirs keyed by test label, so any new derivation must keep `<base>/scratch/<suite>/<label>`.

## OPEN DESIGN QUESTION — for the blueprint, not decided here

Both fixtures deliberately avoid `/tmp` ("A scratch directory under the target dir (never `/tmp`)"). Candidates,
each needing measurement rather than preference:

- derive the base from `env!("CARGO_MANIFEST_DIR")` and walk to the workspace root — correct regardless of CWD,
  but couples each fixture to its crate's depth;
- `std::env::current_dir()?.join("target")` — preserves today's exact layout, but still trusts the CWD that
  produced the bug;
- one shared helper used by both suites (and any future one), so the fallback exists once and is tested once.

Whichever is chosen: a test must assert the resolved path is **absolute**, and the assertion must be on the
fixture rather than only on the consumer (`is_absolute` on a path derived downstream is the symptom, not the
seam).

## Comments

- **2026-10-27** — filed from the issue-57 review. Evidence: `.tmp/scratch/issue57-workspace-nofailfast.log`
  (230 `test result: ok` + exactly 1 FAILED = this `mem-skills` test), `.tmp/scratch/issue57-memskills-attribution.log`,
  `.tmp/scratch/issue59-portability-unset.log` (20/20 green **and** the checkout dirtied). The reviewer
  reproduced both independently, including the variable-present control (`mem-skills` 16/16) and the empty
  `git diff` against base. Nothing here is inherited from a report.
