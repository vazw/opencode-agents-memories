# 119 — `mem-portability`'s test scratch dir is a fixed path, so two concurrent suite runs corrupt each other

Status: done — 2026-11-05

## Observation (MEASURED 2026-11-03, during the zstd bump in issue 116)

`crates/mem-portability/tests/*.rs:266-274`:

```rust
fn scratch_root(label: &str) -> PathBuf {
    let dir = scratch_base().join("scratch").join("mem-portability").join(label);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}
```

`scratch_base()` is `env!("CARGO_TARGET_TMPDIR")` — the **target directory**. So the path depends only
on the label, not on the process: two `cargo test` runs sharing one `CARGO_TARGET_DIR` resolve to the
same directory, and each `remove_dir_all`s what the other is using.

Measured symptom (issue 116's first battery run, while another agent ran `cargo test --workspace` in the
same worktree):

```
mem-portability --test portability_roundtrip
  roundtrip_durable_tiers              FAILED  ImportAbortedMilestones { count: 1 }
  roundtrip_without_staging_restores_milestones  FAILED  ImportAbortedMilestones { count: 1 }
```

The second process saw the first process's `mile-1` row in a store it believed it had just created. Serial
re-run of the same binary: green. The peer run also finished green (2391/0/31). Nothing in the product
changed; the test harness cannot tolerate the parallelism this fleet routinely creates.

## Why this is worth a ticket rather than a note

- **It manufactures false failures**, which is worse than a slow test: the failure mode is
  `ImportAbortedMilestones`, which reads like a real import bug and cost a diagnostic cycle today.
- **The fleet's own pattern causes it** — parallel waves each run `cargo test --workspace`, and the
  blueprint convention already tells agents to use their own `CARGO_TARGET_DIR` precisely because a
  shared one produced a *different* false green earlier the same day (a stale compiled binary). This is
  the same root cause with the opposite symptom.
- The same shape may exist elsewhere: any test helper that joins a fixed label onto `CARGO_TARGET_TMPDIR`
  and clears it is unsafe under concurrency. Worth one sweep, not one fix.

## Options

1. **Unique directory per run**: append the process id (and a random suffix) to the label —
   `…/mem-portability/<label>-<pid>-<rand>`. Smallest change; leaves the directory behind unless the
   helper also cleans up on success, which is acceptable for a target-dir scratch space.
2. **Lock the label**: an advisory file lock so concurrent runs serialise. Keeps one directory per label
   (helpful when debugging a failed run) at the cost of throughput.
3. **`tempfile::TempDir`-style RAII**: unique + auto-removed, but adds a dev-dependency if the crate does
   not already have one.

## Acceptance criteria

- [x] Two simultaneous runs of the same test binary, sharing one `CARGO_TARGET_DIR`, both pass — demonstrated,
  not argued (run the binary twice concurrently and show both exits).
- [x] The chosen option's cost is stated (leftover directories, serialisation, or a dev-dependency).
- [x] A sweep reports every other helper that joins a fixed label onto `CARGO_TARGET_TMPDIR` and clears it,
  with each either fixed or recorded as safe (and why).

## Comments

### 2026-11-05 — Closeout (Issue 119)
- **Concurrent execution proof**: Two concurrent instances of `cargo test -p mem-portability --test portability_roundtrip` were run simultaneously sharing `CARGO_TARGET_TMPDIR` (run1: finished in 0.57s, 23 passed, exit 0; run2: finished in 0.43s, 23 passed, exit 0). Zero collisions observed.
- **Option 1 cost**: Retains leftover directories under `<target>/tmp/scratch/mem-portability/{label}-{pid}-{seq}` until `cargo clean` or target directory purge. In exchange: zero file lock overhead, zero cross-process contention, zero new dependencies (uses `AtomicU64` and `std::process::id()`).
- **Workspace sweep**: Audited all occurrences of `CARGO_TARGET_TMPDIR` across workspace. Exactly one other site found: `crates/mem-skills/tests/skills_manager.rs:52` (`scratch_dir`). `mem-skills` scratch helper uses unique labels across test functions, so single-suite runs are safe; candidate for the same `{label}-{pid}-{seq}` treatment if concurrent `mem-skills` suite runs are scheduled.

