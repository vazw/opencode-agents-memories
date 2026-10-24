# 0016 — Pre-Commit Gate (staged-file rustfmt + clippy gates, via `core.hooksPath`)

- **Status:** Proposed
- **Date:** 2026-10-24 — measured on HEAD `1f66f08`, blueprint
  `20261024-2106-precommit-gate-r3` (SYS-APPROVED 2026-10-24T21:32Z).
- **Amended:** 2026-10-25, after review F1–F3 — the `web-ui/` phase runs one gate (D-3), and the
  root workspace's `exclude = ["web-ui"]` boundary is recorded. See Context and Limitations.
- **Binding on:** `.githooks/pre-commit` (the gate) and `scripts/test-pre-commit.sh` (its
  behavioural self-test). No Rust source, manifest or dependency is touched.
- **Supersedes / amends:** amends ADR-0013 D-5's *"no enforcement"* consequence by adding an
  optional, per-clone commit gate that runs D-5 unchanged. ADR-0013's rejection of a
  pre-commit hook is recorded as supplied by this ADR, not withdrawn there.

## Context

Measured at `1f66f08` before this wave:

- `git config --get core.hooksPath` → exit 1 (**unset**); `.git/hooks/` held 14 sample files
  and no live hook; `.githooks/` and a root `scripts/` did not exist. Nothing gated a commit.
- The two clippy commands the repository treats as law (ADR-0013 D-5) were review-enforced
  only: gate 1 `cargo clippy --workspace --lib -- -A clippy::all -D clippy::unwrap_used
  -D clippy::expect_used` → exit 0 in 8 s; gate 2 `cargo clippy --workspace --all-targets --
  -D warnings -A clippy::await_holding_lock -A clippy::assertions_on_constants
  -A clippy::doc_lazy_continuation` → exit 0 in 25 s (root gates: 33 s total, published as
  ≈35 s warm). `cd web-ui && cargo clippy --workspace --all-targets -- -D warnings` → exit 0,
  42.88 s (gate-measured 2026-10-24 against a cold `web-ui/target`; 0.22 s warm on 2026-10-25).
- The **raw** form (gate 2 without its three `-A`) exits 101 with 17 pre-existing errors across
  three lints: 14 × `clippy::await_holding_lock` in `crates/mem-skills/tests/skills_manager.rs`,
  1 × `clippy::doc_lazy_continuation` in `crates/mem-bridge/src/vault_feeder.rs:15:5`, and
  2 × `clippy::assertions_on_constants` in `crates/mem-vault/tests/no_write_path.rs:124:21`
  and `:133:21`. All three exclusions are load-bearing: dropping any one blocks every commit
  on code this wave must not touch.
- **`web-ui` is a separate workspace and the root gates never saw it.** Root `Cargo.toml` carries
  `exclude = ["web-ui"]`; `cargo metadata --no-deps` at the root lists 12 `crates/*` plus the root
  crate and no `web-ui`, and a root-gate log carries 0 `web-ui` units. ADR-0013's "97 sites today,
  0 at close" was therefore **root-workspace-scoped** — the string `web-ui` appears nowhere in it —
  and that boundary was undocumented until this amendment.
- **The `web-ui` panic-family gate is red.** `cd web-ui && cargo clippy --workspace --lib -- -A
  clippy::all -D clippy::unwrap_used -D clippy::expect_used` → **exit 101**, 7 pre-existing errors
  (reviewer-measured 2026-10-25 at `1f66f08`, ≈1.9 s warm, reproduced twice; all 7 are production
  code under `web-ui/src/`, `#[cfg(test)]` occurs 0 times in that tree because `web-ui` tests live
  in `web-ui/tests/`, and `git status -- web-ui` is empty). Sites: `core_screens.rs:41`,
  `export_import.rs:25`, `routes.rs:155`, `screens.rs:130` and `skill_library.rs:29` are
  `iter().find(...).expect(...)` table lookups; `skill_library.rs:160` is
  `as_object_mut().unwrap()`; `:425` is `get_untracked().clone().unwrap()`. (Clippy reports the
  span start, 38/22/152/127/29/160/425; both line sets name the same 7 sites.) Every blame commit
  predates ADR-0013's enforcement commit `3136c97`, so the panic-free wave neither added nor saw
  them.
- Consequence, and the reason D-3 is narrower than "both gates everywhere": a hook running both
  gates in `web-ui/` refuses **every** commit staging a `web-ui/**/*.rs`, `web-ui/Cargo.toml` or
  `web-ui/Cargo.lock` path — on files its author may never have touched — and the only way through
  is `git commit --no-verify`, which skips the fmt phase and both root gates as well.
- `rustfmt --check <file>` signals a diff with exit 1 and a clean file with exit 0, but on
  **stdin** it prints `Diff in <stdin>:1:` and exits 0 — so the staged content has to be
  materialised before it can be judged.
- Redirected colour: 6 ANSI escape sequences by default, 0 with `--color never`.

The gates were green but unenforced; the ADR-0013 lesson is that a gate nobody runs is
documentation, and this repository's actual recurring defect is an *evidence* defect that no
commit-time lint can see (see Limitations).

## Decision

**D-1 — A tracked hook at `.githooks/pre-commit`, not `.git/hooks/pre-commit`.** A tracked hook
is reviewable in the diff and survives a re-clone; `.git/hooks/` is per-clone state. Installation
is one command per clone, applied by the operator:

```
git config core.hooksPath .githooks
```

The configuration is not tracked, so the hook is a default, not an invariant. `githooks(5)`:
hooks without the executable bit are ignored — hence `100755` on both files (this repository had
zero tracked executables before this wave).

**D-2 — rustfmt is scoped to staged `.rs` files and judges the index blob.** The hook lists
staged added/copied/modified/renamed paths NUL-delimited (`git diff --cached --name-only -z
--diff-filter=ACMR`), materialises each `.rs` blob with `git show ":0:$path"`, and runs

```
rustfmt --check --color never --edition 2024 --config skip_children=true <blob>
```

`skip_children=true` is mandatory for a materialised blob (a copied `lib.rs` otherwise fails to
resolve its modules); because every staged `.rs` is checked as its own blob, a staged unformatted
child is still caught by its own entry. The index is the subject because the commit is: a foreign
dirty worktree must never block a commit whose own content is clean. Zero staged `.rs` files skip
the phase entirely. Renames are safe: `--name-only` prints only the **destination** path for an `R`
entry, so `git show ":0:<path>"` reads the new blob and the vanished old path is never used
(verified in a scratch repository — `R100 old.rs new.rs`, `show :0:old.rs` → rc 128,
`:0:new.rs` → rc 0).

**D-3 — clippy is workspace-wide and cannot be file-scoped.** In the repository root both commands
run verbatim from repository law:

```
cargo clippy --workspace --lib -- -A clippy::all -D clippy::unwrap_used -D clippy::expect_used
cargo clippy --workspace --all-targets -- -D warnings -A clippy::await_holding_lock -A clippy::assertions_on_constants -A clippy::doc_lazy_continuation
```

Measured on the clean tree: exit 0 in 8 s and exit 0 in 25 s. Gate 1 is ADR-0013 D-5's command
byte-for-byte; gate 2 is the all-targets form recorded in `AGENTS.md` §Gates and
`docs/agents/testing.md` — D-5 carries the `--lib` command only, not this one.

When a staged path is under `web-ui/` (and `web-ui/Cargo.toml` exists) **one** gate runs inside that
separate workspace, with `CARGO_TARGET_DIR=<toplevel>/web-ui/target` so the root target dir never
absorbs a second workspace:

```
cd web-ui && cargo clippy --workspace --all-targets -- -D warnings          # exit 0, 0.22 s warm
```

The panic-family gate does **not** run there. It exits 101 on the 7 pre-existing sites recorded in
Context — code the root workspace has never linted — so running it would refuse every commit that
stages a `web-ui/` Rust source or manifest regardless of what that commit changed. This loses no
coverage that ever existed (the root gates cannot see `web-ui`: `exclude = ["web-ui"]`); it defers
one gate until the sites are repaired, which is the recorded follow-up in Limitations.

Published cost: **≈35 s warm** for a Rust-touching commit; a staged `web-ui/` change adds one
all-targets gate (0.22 s warm measured; 42.88 s gate-measured on 2026-10-24 against a cold
`web-ui/target`). A cold `CARGO_TARGET_DIR` pays a full build. The hook is therefore also a compile
check for the **worktree**: a worktree that does not compile is blocked. An index-only breakage
(staged blob broken, worktree fixed) is caught by the fmt phase's parse error or not at all — cargo
cannot lint an index blob without checking it out.

**D-4 — No test execution.** `cargo test --workspace` is minutes, and two concurrent suites
collide on `mem-portability` scratch dirs keyed by test label under `CARGO_TARGET_DIR`
(`Store(Turso { message: "database is locked" })`). The suite stays a deliberate, separate act.

**D-5 — Fast path before any cargo work.** A staged set with no `.rs`, no `Cargo.toml` and no
`Cargo.lock` (at any depth) exits 0 without invoking cargo; proven by a `cargo` shim that writes a
marker and exits 1, not by assertion. Splitting the two workspaces into separate flags would save
≈33 s of root clippy on a web-ui-only commit (the web-ui gate itself is sub-second warm);
**explicitly declined** as added surface — one gate everywhere, and under-running a gate is worse
than 33 s.

**D-6 — One scratch root, created on first use.** The hook exports
`CARGO_TARGET_DIR=<toplevel>/.tmp/scratch/cargo-target` and `TMPDIR=<toplevel>/.tmp/scratch`,
then runs `mkdir -p "$TMPDIR"` **before** `mktemp -d "$TMPDIR/precommit.XXXXXX"`. This is not
optional: `.tmp/` is gitignored, so a fresh clone has no `.tmp/scratch`, and `mktemp -d` against
a missing parent fails with `No such file or directory`. A `mkdir` failure is loud
(`pre-commit: FAIL environment — cannot create the scratch directory <path>`) and never falls
back to an inherited `TMPDIR`. The per-run directory is removed by `trap 'rm -rf "$tmp"' EXIT`, and
`trap 'exit 1' INT TERM HUP` guarantees a signalled hook both cleans up and stops instead of
resuming into cargo with nothing checked.
**A fresh clone needs only the `git config core.hooksPath .githooks` command.**

**D-7 — The root clippy commands are copied byte-for-byte; all three exclusions are load-bearing.**
17 pre-existing sites (table above) fire under the raw form. If the baseline is ever repaired, the
exclusions and ADR-0013 D-5 change together in a Rust wave — a shell script is not the place to
redefine the gate.

**D-8 — The self-test is committed at `scripts/test-pre-commit.sh`.** A gitignored harness dies
with the wave and cannot re-prove the hook after a later edit; bats would be a new dependency. It
runs 16 behavioural cases in throwaway repositories under
`<repo>/.tmp/scratch/precommit-selftest/`, carries a copy of the *tracked* hook, configures
`core.hooksPath .githooks`, and refuses to run if its work dir resolves to the repository root or
outside `.tmp/scratch`. It makes no writes to this repository's index or worktree.

**D-9 — Toolchain presence is checked before the fast path.** A missing `cargo` or `rustfmt`
exits 1 naming the tool even for a docs-only commit: a hook that silently degrades to a no-op is
worse than one that refuses. Cost: none (both present; `cargo`/`rustfmt` are rustup proxies in
`~/.cargo/bin`, and the system also carries `/usr/bin/rustfmt`).

**D-10 — The fixture is pinned, clean and budgeted.** `mk_workspace()` writes a one-crate
workspace (`probe`, edition 2024) whose `lib.rs` is rustfmt-clean and clippy-clean under both
gates, generates and **commits** a `Cargo.lock` so clippy never re-resolves mid-case, and commits
it with `--no-verify` (scaffolding must not be gated, and the fixture must start with no
`.tmp/scratch` so later cases prove the hook bootstraps it). Budget: ≈5 s per Rust-touching case;
the measured whole-harness wall time is recorded per run.

## Consequences

**Positive**

- A commit whose staged Rust is unformatted, or whose worktree fails either root clippy gate, is
  refused with the failing gate, the offending path and a reproduce command.
- A commit staging a `web-ui/` Rust source or manifest runs that workspace's all-targets gate
  (exit 0 on today's tree), so the previously ungated second workspace now has the one gate it can
  pass.
- Existing gates are not weakened: the hook runs the same two root commands, and its fmt scope is
  narrower than `cargo fmt --all -- --check`, which remains the wave-close battery check. The
  hook is a **floor under the commit**, not the battery.
- Docs-only commits never invoke cargo (fast path), so the gate costs nothing on markdown waves.

**Negative**

- Warm Rust-touching commits pay ≈35 s; a staged `web-ui/` change adds one all-targets gate
  (sub-second warm, tens of seconds on a cold `web-ui/target`), and a cold `CARGO_TARGET_DIR` pays
  a full build.
- **`web-ui` panic families are not gated.** 7 pre-existing sites remain (Context). The root law
  gate never covered them either — `web-ui` is `exclude`d from the root workspace — so this is the
  status quo since 2026-10-21, now documented instead of silently blocking every `web-ui/` commit.
  ADR-0013's "0 at close" is root-workspace-scoped, and this ADR is where that scope is stated.
- The clippy phase lints the whole **worktree**, so a concurrent wave with a non-compiling or
  lint-failing edit can block a commit for reasons that are not that commit's own. The correct
  response is to wait for the other wave to land, not to reach for `--no-verify`.
- The gate is installed per clone; a fresh clone has no hook until the operator runs the command.

**Failures and their contract.** On failure the hook prints
`pre-commit: FAIL fmt — <path> is not rustfmt-clean (staged content)` or
`pre-commit: FAIL clippy [gate N/N: <name> — <root workspace|web-ui>]`, the captured output (fmt
diff bounded to 60 lines, clippy log tail 40 lines), a
`pre-commit: reproduce: cd <dir> && CARGO_TARGET_DIR=<dir> <command>` line and the `--no-verify`
note. The gate numbering is per workspace: root publishes `gate 1/2` (panic families) and
`gate 2/2` (all-targets warnings); `web-ui` publishes `gate 1/1: all-targets warnings — web-ui`,
one gate, because D-3 runs one there. Success is silent except the index/worktree `WARN` and
`PRECOMMIT_VERBOSE=1`. The hook always exits 1 on failure and never passes a tool's exit code
through.

## Limitations

**What this hook does not catch — stated plainly.** It is a lint and format gate, not a quality
gate. The defect class that actually cost this repository three waves is the **evidence defect** —
a ledger row whose timestamp was never measured, a test whose fixture is wired to nothing, an ADR
claim the code contradicts, a "verified" line recording a number nobody ran. Three of its blind
spots are structural:

1. **The evidence lives in gitignored files.** Ledgers, blueprints and wave logs are under
   `.tmp/`, which `.gitignore` excludes; a pre-commit hook never sees them.
2. **Truth is not a lint.** A well-shaped timestamp can be invented; a fixture can point at a
   real-looking fake; an ADR sentence can be fluent and false.
3. **Test-code panics are exempt by design** (ADR-0013 D-1); `panic!`/`todo!`/`assert!` (D-3) and
   the `?` rule (D-4) stay review-enforced.

Also not covered: formatting of non-`.rs` files, secrets in staged files, commit-message quality
(a `commit-msg` hook is out of scope), `shellcheck` **of the hook itself** (installed and run at
wave close, deliberately not wired into the commit path — that would be a gate on the gate), and
any Rust whose index and worktree differ (clippy sees the worktree; the hook warns but cannot
fix). Cargo may still write the worktree: `cargo clippy` can create or rewrite `Cargo.lock` when a
staged manifest changes resolution — the hook cannot prevent that and does not try; the fmt phase
runs before cargo, so a fmt failure leaves the tree untouched.

**Recorded follow-up — restore the `web-ui` panic-family gate (carry-over).** Deferred out of this
wave; a separate Rust wave owns it. Both `web-ui` lookup tables are exact bijections today
(`EndpointId` 34 variants ↔ `ENDPOINTS` 34 rows, `RouteId` 9 ↔ `ROUTES` 9, no duplicates and no
gaps), so the five `find(...).expect(...)` sites become infallible by construction:

```rust
// paths.rs, after ENDPOINTS
const _: () = assert!(ENDPOINTS.len() == EndpointId::DeleteProjectSession as usize + 1);
pub fn endpoint(id: EndpointId) -> EndpointSpec { ENDPOINTS[id as usize] }

// routes.rs, after ROUTES
const _: () = assert!(ROUTES.len() == RouteId::Stats as usize + 1);
pub fn route(id: RouteId) -> &'static RouteSpec { &ROUTES[id as usize] }
```

Drift then becomes a build failure (`error[E0080]` on a length mismatch), never a runtime panic.
The remaining two sites are local: `skill_library.rs:160` builds the object directly with `Map`
instead of `as_object_mut().unwrap()` (the `if let Some(map)` form silently drops `id`), and `:425`
reads the signal once — `match editing { Some(id) => id, None => allocate_skill_id(..) }`. Total:
7 edits in 5 files, 2 const assertions, 1 new public accessor, 4 duplicated local helpers deleted,
no call-site signature change. When it lands, D-3 regains the second `web-ui` gate and the failure
contract returns to `1/2`–`2/2` for both workspaces. Re-measure both `web-ui` commands first,
including whether every target under `web-ui/tests/**` lints on the host target (that tree is
outside `src/`), and record the rcs.

**`--edition 2024` is hardcoded** in the rustfmt invocation and correct for all **434 tracked
`.rs` files** today (407 outside `web-ui/`, 27 under `web-ui/`). An edition change must revisit
this script; the clippy commands themselves carry no edition flag.

## Rejected alternatives

- **A repo-wide `cargo fmt --all -- --check` in the hook.** Its colour flips with whoever is
  mid-edit at that moment, it blocks a clean commit because of a foreign dirty worktree, and it
  trains `--no-verify`.
- **A githook under `.git/hooks/`.** Per-clone, untracked, invisible in review.
- **Bats.** A new dependency for eight lines of assertion helpers.
- **Running the test suite in the hook.** Minutes, and concurrent suites collide (D-4).
- **Splitting the fast-path flag per workspace.** Saves ≈33 s of root clippy on a web-ui-only
  commit, adds surface; declined (D-5).
- **`cargo fmt` in write mode or `cargo clippy --fix`.** The gate must never author the change it
  judges; the hook only reads the index and runs check-only tools.
- **`shellcheck` inside the hook.** Would gate the gate and add a second failure surface mid-commit;
  it runs at wave close instead.
- **Disabling `--no-verify`.** Not possible from a hook; documented instead, with the
  legitimate/illegitimate split above.

## References

- ADR-0013 — Production error handling; D-5 defines gate 1 (the `--lib` command) and the 17-site
  baseline; gate 2 is the all-targets form recorded in `AGENTS.md` §Gates and
  `docs/agents/testing.md`. Amended by this ADR to record the hook, the re-measured baseline and the
  root workspace's `exclude = ["web-ui"]` boundary.
- `docs/agents/testing.md` — the documented battery now names D-5's two gate commands.
- `AGENTS.md` §Gates — operator-facing install, commands, cost, escape hatch.
- `scripts/test-pre-commit.sh` — the 16-case behavioural self-test.
