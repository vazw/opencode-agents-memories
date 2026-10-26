# 57 — Test harness · `purge_cli::binary_path()` falls back to `target/debug/deps`, so `cargo test --workspace` needs `CARGO_TARGET_DIR` to be green

Status: needs-triage — 2026-10-26. **Seven `purge_cli` tests fail on an otherwise clean tree unless
`CARGO_TARGET_DIR` is exported: the fallback resolves the root binary under `target/debug/deps/` while cargo
builds it at `target/debug/`.** Reproduced independently on the unchanged tree by two agents; not caused by any
wave.

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
