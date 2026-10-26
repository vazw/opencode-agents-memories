# 57 — Test harness · `purge_cli::binary_path()` falls back to `target/debug/deps`, so `cargo test --workspace` needs `CARGO_TARGET_DIR` to be green

Status: landed 2026-10-27 in `672625fc2b5daf3a6e93c94ea0337b9537946241` — the root-binary resolution is now a
pure, total `resolve_binary_path(&Path, Option<&OsStr>)` that strips exactly one `deps/` segment; measured
**21/21** in `purge_cli` with `CARGO_TARGET_DIR` unset and **21/21** with it set. `cargo test --workspace` with
the variable unset still exits 101 on one **pre-existing, unrelated** `mem-skills` test
(`v2_bridge_skill_path_absolute` — the same env-fallback class); out of this issue's scope, see Comments.
Originally filed `needs-triage` 2026-10-26: **seven `purge_cli` tests failed on an otherwise clean tree unless
`CARGO_TARGET_DIR` was exported** — the fallback resolved the root binary under `target/debug/deps/` while
cargo builds it at `target/debug/`; reproduced independently by two agents, not caused by any wave.

## PREMISE — the fallback, read from the source

`crates/mem-bridge/tests/purge_cli.rs:936-947`:

```rust
fn binary_path() -> PathBuf {
    let name = format!("opencode-agents-memory{}", std::env::consts::EXE_SUFFIX);
    let binary = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(target_dir) => PathBuf::from(target_dir).join("debug").join(name),
        None => std::env::current_exe()
            .expect("current exe")
            .parent()            // = <target>/debug/deps
            .expect("deps/")
            .join(name),         // = <target>/debug/deps/opencode-agents-memory
    };
```

The `Some` arm is correct. The `None` arm is wrong: an integration test's `current_exe()` lives in
`<target>/debug/deps/`, while the root package's binary is built at `<target>/debug/`. The fallback therefore
looks one directory too deep.

## Symptom

On a clean tree with `CARGO_TARGET_DIR` **unset**, `cargo test --workspace` fails exactly 7 `purge_cli` tests,
all with `the root binary is missing at …/target/debug/deps/opencode-agents-memory`. With
`CARGO_TARGET_DIR=$PWD/target` exported, the whole workspace is green (229–232 suites across waves). Both the
issue-45b coder and its reviewer reproduced this on the unmodified tree, and `crates/mem-bridge` is byte-identical
to base across the waves that hit it.

## Why it matters

It makes the repo's primary verification command **conditionally green**: an agent or a human who runs
`cargo test --workspace` without the variable sees 7 failures and must first conclude they are not real. This
session lost time to exactly that twice, and the reviewer had to measure both arms to characterise it. A test
harness whose failure mode is "looks like 7 real regressions" is worse than one that fails loudly and correctly.

## Suggested fix

Prefer the profile directory, not `deps/`:

- derive from `current_exe()`: if the parent directory's name is `deps`, use `parent.parent()`;
- otherwise fall back to the existing behaviour;
- keep the `CARGO_TARGET_DIR` arm as-is (it is correct and is what the waves have been setting).

A regression test is cheap here: assert `binary_path()` exists after a build, in both var states. Note the
existing `OnceLock` build trigger in the same function — do not disturb it.

## Gate

`READ` from `crates/mem-bridge/tests/purge_cli.rs:936-947` at `5c06275`; the symptom is `MEASURED` twice
independently (two arms, `CARGO_TARGET_DIR` set and unset). No manifest, dependency or production-code change
is involved — it is a test harness defect only.

## Comments

### Outcome (record-only; implementation commit `672625f`)

**Change.** `crates/mem-bridge/tests/purge_cli.rs` is the only file. The whole decision moved into a pure,
total `resolve_binary_path(current_exe: &Path, target_dir: Option<&OsStr>) -> PathBuf`. The `Some` arm is
unchanged; the `None` arm strips exactly one `deps` segment from the executable's parent — any other parent
is taken as the profile directory itself, and a parentless path yields the bare file name. `binary_path()`
keeps its `OnceLock` build trigger and its `assert!(binary.exists(), …)` verbatim. No env mutation, no
filesystem probe inside the resolver; the join is the literal hyphenated `opencode-agents-memory` +
`EXE_SUFFIX`, so no `deps/` scan can ever select a sidecar.

**RED → GREEN ladder** (every run `env -u CARGO_TARGET_DIR`, same command):

| moment | total | passed | failed | exit | log (`.tmp/scratch/`) |
|---|---|---|---|---|---|
| `a6e9b34`, before any edit | 16 | 9 | 7 | 101 | `issue57-red-baseline.log` |
| after faithful extraction + 5 resolver tests | 21 | 11 | 10 | 101 | `issue57-red-resolver.log` |
| after the `None`-arm fix | 21 | 21 | 0 | 0 | `issue57-green.log` |

Step 2's three resolver failures were `…_prefers_the_profile_dir_when_the_exe_sits_in_deps`,
`…_ignores_dwo_sidecars_in_deps`, `…_falls_back_to_a_bare_name_when_the_exe_has_no_parent`; the two controls
`…_uses_cargo_target_dir_when_it_is_set` and `…_treats_a_non_deps_parent_as_the_profile_dir` passed on the
buggy body by design, which is what makes the other three discriminating.

**Environment measured that day (Step 1 log).** `target/debug/opencode-agents-memory` present (195 493 096 B);
`target/debug/deps/*.dwo` = **19065** in total, of which **93** carry the underscore stem
`opencode_agents_memory-` and **103** entries sit under that stem with or without the extension. The count is
a build property (`split-debuginfo = "unpacked"`), not a constant of the tree.

**`Some` arm (regression).** `CARGO_TARGET_DIR=$PWD/target cargo test -p mem-bridge --test purge_cli` →
exit 0, 21 passed.

**Workspace with the variable unset — measured, and not fully green.** `cargo test --workspace` exits **101**
on exactly one target: `-p mem-skills --test skills_manager`, 15/16, failing `v2_bridge_skill_path_absolute`
("the OpenCode Skill.INFO path is absolute") at `crates/mem-skills/tests/skills_manager.rs:379`
(`.tmp/scratch/issue57-workspace.log`). Cause: `scratch_dir()` (line 45) falls back to the relative `target`
when the variable is absent and this test asserts the resulting path is absolute — the **same env-fallback
class as this issue**, independent of mem-bridge (it fails alone under `env -u CARGO_TARGET_DIR`:
`issue57-memskills-attribution.log`). `purge_cli` itself reads `21 passed` inside that workspace run, and a
`--no-fail-fast` re-run (`issue57-workspace-nofailfast.log`) gives 230 `test result: ok` with exactly that one
FAILED. Not repaired here: outside this issue's scope ceiling; recorded as the residual.

**Trees Step 5 creates in the checkout (recorded, out of scope).** With the variable absent,
`crates/mem-portability/tests/portability_roundtrip.rs:199` and `crates/mem-skills/tests/skills_manager.rs:45`
write `crates/mem-portability/target/` and `crates/mem-skills/target/`. `.gitignore:1` is `/target`
(root-anchored), so `git check-ignore -v crates/mem-skills/target/x` exits 1 with no output — untracked, not
ignored. Both were removed with `find … -depth -delete`, never `rm -rf`.

**No doc edit.** `docs/agents/testing.md` needed none: grep returns 0 hits for `binary_path` and 0 for
`CARGO_TARGET_DIR`; its §Hermeticity line 57 (never `set_var`, inject via the `EnvSource`/`ConfigPaths` seams)
is exactly the rule the pure-resolver design obeys and remains true verbatim.

**Corners reasoned, not measured.** No Windows runner and no `--target <triple>` layout was exercised. The
strip is relative to `current_exe()`'s actual parent, so `<target>/<triple>/debug/deps/<test>` resolves to
`<target>/<triple>/debug/` by construction; `EXE_SUFFIX` covers `.exe`.
