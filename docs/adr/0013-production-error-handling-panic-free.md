# 0013 — Production Error Handling: Panic-Free Paths

- **Status:** Accepted (owner decision, 2026-10-24)
- **Date:** 2026-10-24
- **Context source:** the owner's instruction of 2026-10-24, quoted verbatim in Context, plus the
  three scoping answers given with it; a full-tree survey at HEAD `9a523b5` and clippy runs on the
  same tree, both measured on 2026-10-24 (D-5 quotes the commands and their results).
- **Extends:** ADR-0002 (typed, per-crate `thiserror` errors) from error *shape* to **panic
  policy**. ADR-0002's bans stay in force: no `anyhow`, no `Box<dyn Error>`/`String` payloads in
  public signatures, sources always preserved. **Supersedes:** nothing.
- **Binding on:** every crate in the workspace, production code only (D-1).

## Context

The owner's instruction, verbatim:

> "revisit all other crate as well, no `?` unless handled no `.unwarp()` at all cost as well as to
> avoid `.expect("")` if posible."

Three scoping answers were given with it, and they are part of the decision:

1. **Scope is production only.** The sites before any `#[cfg(test)]` module. Test code is exempt —
   its `expect("…")` messages are the diagnostic tool.
2. **Poisoned locks are classified per site.** Recover where the guarded state is self-contained;
   propagate where a cross-field invariant could be half-updated. Every site gets a written
   rationale.
3. **Enforcement is a clippy lib-only gate.**

### The tree as measured (2026-10-24, HEAD `9a523b5`)

97 production sites — `.unwrap()` or `.expect(` before the first `#[cfg(test)]` line — in 31 files
across 8 crates. Root `src/` — including the workspace's only binary target, `src/main.rs` — is clean
(0 sites).

| crate | `unwrap` | `expect` | total |
|---|---|---|---|
| `mem-store` | 37 | 8 | 45 |
| `mem-web` | 0 | 20 | 20 |
| `mem-ai` | 0 | 11 | 11 |
| `mem-profile` | 0 | 10 | 10 |
| `mem-bridge` | 0 | 5 | 5 |
| `mem-skills` | 0 | 3 | 3 |
| `mem-vault` | 0 | 2 | 2 |
| `mem-core` | 0 | 1 | 1 |
| **total** | **37** | **60** | **97** |

Two independent counts agree on 97: the static line cut, and
`cargo clippy --workspace --lib` with the two restriction lints promoted to warnings (D-5). In every
one of the 29 files carrying a `#[cfg(test)]` module that module is the file's last item — measured,
no top-level item follows it — so the line cut does not hide production code today.

**Classifying each of the 97 sites** (measured, not line-local): **61 acquire a lock guard**
(57 `.lock()`, 2 `.write()`, 2 `.read()`, then `.unwrap()`/`.expect(…)`), and **36 do not**. The
61 split by crate as `mem-store` 36, `mem-ai` 11, `mem-profile` 9, `mem-skills` 3, `mem-vault` 2.
A line-local count of the same tree yields 43 for the guard class because 18 acquisitions are split
across lines (`.lock()` on one line, `.unwrap()` on the next); **the 43 figure undercounts and must
not be used to size the wave.**

The 36 non-guard sites are the "infallibility by construction" class, in recurring shapes: 12
`Response::builder()…body(..).expect("…a valid HTTP response")` in `mem-web`, 8
`serde_json::to_string(x).expect("…serializable")` (5 in `mem-web`, 2 in `mem-bridge`, 1 in
`mem-store`), 2 `PathBuf::from(opt.expect("checked above"))` in `mem-store`'s `legacy_migrator`, 2
`.last().expect("…non-empty")`, 1 `String::from_utf8(..).expect("…ASCII")`, 1
`i64::try_from(u64).expect("…fits")` inside a `From` impl.

**Test code, which this decision leaves alone:** 2,560 `expect` occurrences across 192 files under
`crates/*/tests`, 169 across 9 files under `tests/`, and 69 inside inline `#[cfg(test)]` modules.

**Propagation is already the house style:** production contains 893 `?;` occurrences, so `?` is not
an exotic construct here; it is how every crate seam reports failure. Production also contains 2
`unreachable!()` and 0 `panic!`, 0 `todo!`, 0 `assert!` — the panic family is essentially absent,
which is what makes D-3's ban on *adding* to it cheap to hold.

### Why this needs a decision rather than a sweep

ADR-0002 fixed the *shape* of errors and said nothing about panics. Nothing in the tree prevents a
new `.unwrap()` from landing in a production path: there is no CI (measured: no `.github`, no CI
config), so the documented battery in `docs/agents/testing.md` runs when someone runs it. A sweep
without a rule would remove 97 sites and leave the 98th to the next wave. This ADR is the rule, and
D-5 is the only mechanism the tree can currently offer.

## Decisions

### D-1 — Scope: production code only; tests are exempt and stay exempt

"Production" means **the code compiled into the shipped library or binary**, operationalised as
**before any `#[cfg(test)]` module in a file**. Integration tests under `tests/` are exempt, as are
doctests and examples.

The exemption is not a convenience. In a test, `expect("…")` *is* the diagnostic surface: it names
the assertion that failed and the value that surprised it. Rewriting it as `?` would convert a
located failure into a returned error that the harness prints without a location, which is strictly
worse for the person reading it. Test code is where a panic is the correct report.

The boundary is therefore a location, not a syntax: a new `expect` inside `#[cfg(test)]` is not a
policy violation; the same `expect` in a `pub fn` above that module is. This decision converts none
of the 2,798 test-side occurrences and forbids converting them "for consistency".

**Rejected:** exempting tests but requiring `Result`-returning test functions (loses the failing
location and the panic message — the two things a test failure has to carry); treating `tests/` as
production because it is compiled (it is compiled *as a test*; D-5's `--lib` makes the same cut).

### D-2 — Poisoned locks: classify per site, and say what recovery means

A poisoned lock means a thread panicked while holding the guard. Rust guarantees the guarded value is
not torn in memory; it does not guarantee the *logical* update finished. Two dispositions are
admissible, and the choice is made per site by this test:

1. **Recover** — `PoisonError::into_inner()` — only where the guard protects a **single,
   self-contained** value (a collection, a primitive, an `Option`, a cell) whose own operations
   cannot leave it malformed, **and** no other state or external resource is left at a step that
   this value would misrepresent.
2. **Propagate** — return the crate's typed error — where a **cross-field invariant** could be
   half-updated, where the guarded state is **key material or an encrypted-vault structure**, or
   where the guarded value gates a **side effect** that a stale value would let run twice.

**What recovery means at runtime, stated plainly:** the process continues, silently, with the guarded
value exactly as the panicking thread left it. There is no log line, no degraded-mode flag, and no
second chance to notice. That is acceptable for a listener registry whose worst case is a
notification that fires or does not. It is **not** acceptable for vault key material, where
continuing means deriving content ids and opening databases from a key state nobody validated, and
it is not acceptable where a half-applied multi-step update is observable — a `pending` entry whose
initializer panicked is a later caller joining a signal that never completes.

**The rationale must name what is inside the mutex.** "Mutex poisoned" is a symptom, not a
justification: it says a thread died, not whether continuing is safe. Each site carries a one-line
comment naming the guarded value and the disposition, and the wave's ledger records the list. The
comment is the durable record; the ledger is transient.

Three applications of the test, from the clusters this ADR was written against:

- **`mem-vault`'s `scopes` map — propagate.** Its value is `ScopeState { manager, handle,
  key: OnceLock<KeyHex> }`: per-scope key material plus the manager built from it. The key-material
  clause above decides it, and ADR-0012 D-3's fail-closed order is the reason — a vault that cannot
  vouch for its key state must refuse, not continue.
- **`mem-store`'s `ConnectionManager` maps — propagate, on the protocol test.** Each of
  `connections`, `pending`, `closing_connections`, `closing_all` is a single self-contained map, so
  the field-level test passes. The four together encode one protocol: `pending` says an open is in
  flight, `connections` says it finished. Recovering can leave a `pending` entry whose initializer
  panicked, and the next caller joins it and waits for a completion that will never arrive. The
  protocol, not the field, decides.
- **`mem-skills`' listener registry — recover.** A single `Vec<(u64, Arc<dyn Fn()>)>` of
  notification handles; the worst case is one listener registered or unsubscribed inconsistently,
  nothing persistent depends on it, and the id-keyed guard makes a duplicate removal harmless.

Propagation uses the crate's typed error. Where no variant fits, the crate adds a
`#[non_exhaustive]` variant naming the guard and the operation (ADR-0002 (a)); its payload is a
`&'static str` lock name drawn from a closed set of literals, **not** a `String`, and `PoisonError`'s
`Box<dyn Any>` payload never reaches a public signature.

**Rejected:** one blanket policy for all 61 sites (either silently continues past a panic in key
custody, or turns a listener registry into a fallible interface for no benefit); recovering
everywhere because "std's collections are panic-safe" (true of the collection, false of the
multi-step protocol around it); treating a poisoned lock as fatal to the process (the process may
have no supervisor, and ADR-0012's fail-closed rule is per scope, not per process).

### D-3 — The other 36: eliminate first, then propagate; retention is an exception class

**Eliminate** — remove the error rather than plumb a branch for it — is preferred wherever the
infallibility claim is a *design* artifact rather than a fact:

- carry the already-validated value instead of its `Option`: `legacy_migrator`'s
  `PathBuf::from(staged.expect("checked above"))` sits five lines below the `if staged.is_none()`
  that already rejected `None`; binding both paths in that one `if let` removes the claim;
- carry the index beside the value: `mem-web`'s `LinkedPair { memory: Option<usize>, prompt:
  Option<usize> }` is constructed only from `(Some, Some)` and then `expect`ed at both fields
  (`api/memories.rs`); a pair type holding two `usize` removes both;
- bind the `Some` in the guard that tests for it: `sqlite_files.rs` checks `retry_delay.is_none()`
  and then unwraps the same `Option` eight lines later;
- use a constructor that is infallible by signature: `mem-web`'s 12 `Response::builder()` sites can
  go through axum's `(StatusCode, headers, body)` `IntoResponse` impl (axum-core 0.5.6,
  `into_response.rs:424`), which returns no `Result`;
- let the type carry the invariant: `String::from_utf8(..).expect("base36 digits are ASCII")`
  (`mem-profile/src/ids.rs`) becomes a `String` built by pushing the digit `char`s, which has no
  fallible step; and `impl From<u64> for Value`
  (`mem-store/src/db.rs`) is a false infallibility claim — a `From` impl may not fail, so the trait
  choice is the defect and `TryFrom` with propagation at the call site is the fix.

**Propagate** when the failure is real and the caller can act on it: return the crate's typed error
per ADR-0002, add a variant where none fits, and preserve the source. A `serde_json::to_string`
over a caller-supplied `Serialize` type genuinely can fail; a helper that returns `String` should
either return `Result` or take an already-encoded value.

**Retention of a panic is an exception class, not a loophole.** A site may keep its panic only with a
written argument that the invariant is **type-enforced** — the type makes the failing state
unrepresentable, not merely unlikely. Each retention is listed by the wave's ledger with the type
that enforces it, so a reviewer can check the argument rather than trust it. This class is expected
to be small and is empty until a site earns its place in it.

**Banned as a way to satisfy this policy:** `unreachable!()`, `panic!()`, `todo!()`, `assert!()`.
Moving a panic is not removing it. The tree shows why the ban is needed: `sqlite_files.rs:221`
unwraps `retry_delay` four lines above `unreachable!("attempt counter bounded by the delay-table
check")` in the same function — swapping one for the other would satisfy a `-D unwrap_used` gate and
change nothing an operator could observe. The 2 existing `unreachable!()` sites are not converted by
this decision; the ban is on *using* them as a discharge.

**Rejected:** propagating everywhere (turns infallible-by-signature helpers into fallible ones and
pushes `Result` through call sites that cannot act on it — the opposite of D-4's "context survives");
rewriting a panic as an `assert!` with a better message (same panic, later); `#[allow]` on the site
(the gate then reports nothing and the claim is still unverified).

### D-4 — The `?` instruction, interpreted

"No `?` unless handled" cannot mean removing `?`: production has 893 `?;` occurrences and `?` is
Rust's normal propagation mechanism, so a literal ban would be a different, far larger wave than the
97 sites this ADR is about. The operative rule, recorded so no later wave has to guess:

`?` is admissible **only** when the error is meaningful and the context survives:

- the error type is a typed per-crate variant (ADR-0002), never `Result<_, ()>`, never `String`,
  never `anyhow`;
- the source error is preserved (`#[from]`/`#[source]`), not replaced by a formatted message;
- nothing is swallowed on the way out — no `let _ =`, no `.ok()`, no `unwrap_or_default()` used to
  silence a failure that the caller needed to see;
- context is added at boundaries, in the variant, not in a string concatenated at the call site.

Under that reading the owner's sentence is satisfied: "handled" means the caller can act on a typed
failure, not that every `?` disappears.

**Explicitly deferred:** auditing the existing 893 `?;` occurrences is a **separate decision**. It is
not in scope for the wave this ADR authorises, and a wave that starts rewriting `?` has left its
brief.

**Rejected:** a literal ban on `?` (a 893-site rewrite with no error-shape improvement, and it would
push callers toward `unwrap`); banning `?` only outside `Result`-returning functions (that is already
impossible); treating `?` into a crate-level `Error` as "handled" (it is, provided the variant keeps
its source — which is the rule above).

### D-5 — Enforcement: a clippy lib-only gate, and the honest limit of a gate with no CI

The canonical invocation, verified on 2026-10-24:

```
cargo clippy --workspace --lib -- -A clippy::all -D clippy::unwrap_used -D clippy::expect_used
```

**The `--` is load-bearing.** Without it the same command fails before compiling anything:
`cargo clippy --workspace --lib -A clippy::all …` → `error: unexpected argument '-A' found`, exit 1.
Lint flags belong after `--`, where they are passed to the compiler driver.

Why each part:

- **`--lib`** lints exactly the in-scope surface: library targets, compiled without `--test`, so
  `#[cfg(test)]` modules and `tests/` are not built and cannot be linted. Measured: with the two
  lints at warn, `--lib` reports 97 diagnostics, one per production site in the table above, and the
  root package's binary target (`src/main.rs`, clean today) is outside the gate.
- **`-A clippy::all`** sidesteps the known-red baseline without repairing it. Measured per crate on
  this tree: `cargo clippy -p mem-skills --tests` → 14 × `await_holding_lock` in
  `crates/mem-skills/tests/skills_manager.rs`, exit 0; `cargo clippy -p mem-vault --lib` → 1 ×
  `unnecessary_lazy_evaluations` in `crates/mem-vault/src/store.rs` (the
  `ok_or_else(|| VaultError::InvalidKeyHex)` inside `ready`), exit 0. Because of `--lib` the 14 test
  warnings are excluded by the target selection, **not** by the allow; the allow covers the
  lib-target one. That distinction matters: dropping `--lib` would not be covered by the allow.
- **`-D clippy::unwrap_used -D clippy::expect_used`** are restriction lints, outside `clippy::all`,
  so the allow does not touch them. Order is significant and correct: the two denials come last.

**Measured result on today's tree — the gate is red, and it hides most of its own findings.** With
`-D` the run exits 101 after **one** diagnostic (`crates/mem-core/src/utils/context_limit.rs`, the
`str::from_utf8(..).expect("slice boundaries are re-aligned…")`), and stops: `mem-core` is a
dependency of every other crate, so no dependent library can be checked until it is clean.
`--keep-going` does not help — measured, still 1 diagnostic — for the same reason. The 96 other sites
are invisible to the gate until `mem-core` is fixed, then the next crate in dependency order, and so
on.

The measurement form that sees everything, and the form the wave should use while it works:

```
cargo clippy --workspace --lib --keep-going -- -A clippy::all -W clippy::unwrap_used -W clippy::expect_used
```

Measured: exit 0, **97 warnings**, per-crate counts identical to the static survey (45 / 20 / 11 / 10
/ 5 / 3 / 2 / 1). The wave closes when this count is 0 **and** the `-D` form exits 0.

**The honest limitation: this repository has no CI.** The gate is therefore a documented command
plus a recorded expectation, not an enforced invariant. It is recorded here, in D-5, as the canonical
invocation; a wave that touches production code runs both forms at close and records the exit codes
and the warning count in its ledger. A gate nobody runs is documentation, and this ADR says so
rather than implying otherwise. Until a CI job exists, the only thing standing between the tree and
regrowth is review against this paragraph.

Two further limits, stated rather than implied:

- the gate covers `.unwrap()`/`.expect()` only; it says nothing about `panic!`, `todo!`,
  `unreachable!` or `assert!` (D-3's ban is review-enforced), and nothing about the `?` rule (D-4);
- `docs/agents/testing.md` documents the battery line
  `cargo clippy --workspace --all-targets -- -D warnings`, which cannot pass on this tree: the 14
  `await_holding_lock` and 1 `unnecessary_lazy_evaluations` warnings measured above become errors
  under `-D warnings`. This ADR does not repair that baseline and does not silently redefine the
  battery; it defines a narrower gate that can be green.

**Rejected:** `--all-targets` (lints the exempt test surface and inherits the red baseline);
`RUSTFLAGS="-D clippy::unwrap_used"` (applies to dependencies too, and is invisible in the command);
a `#![deny]` attribute in each crate's `lib.rs` (equivalent in effect, but one more place to drift per
crate, and it cannot be scoped away from tests per target); a pre-commit hook (a repository write
outside this decision's scope, and still not a gate anyone is obliged to install).

## Rejected alternatives

- **Removing every panic from the tree, tests included.** The test-side `expect` messages are the
  diagnostic surface (D-1); converting 2,798 of them would make failures harder to read and buy
  nothing the gate needs.
- **A single poisoned-lock policy.** Either it silently continues past a panic in vault key custody
  or it makes a listener registry fallible for no benefit; the boundary is what the guarded value is
  (D-2).
- **Replacing `unwrap` with `unreachable!`/`assert!` to satisfy the gate.** Same panic, same
  invisible failure, now with a green gate (D-3).
- **A literal ban on `?`.** 893 sites, no improvement in error shape, and it pushes callers toward
  `unwrap` (D-4).
- **Enforcing with `--all-targets -D warnings`.** Red today for reasons unrelated to this policy,
  so it would either be ignored or force an unrelated cleanup into every wave (D-5).

## Consequences

**Positive**

- Panics in production become a reviewed decision with a named disposition, instead of a reflex.
- Poisoned-lock handling gets a written rationale per site, naming the guarded value — the thing a
  future reader needs and the thing "mutex poisoned" never said.
- The gate is runnable, measured, and reports exactly the in-scope surface: 97 today, 0 at close.
- Test diagnostics are untouched, so failures keep their location and message.

**Negative**

- 97 sites of work, 45 of them in `mem-store` — 36 of those on lock guards, and its guard protocols
  (the connection maps, the shard init slots, the ready gate) make propagation the likely answer
  there, hence the larger diff.
- The gate cannot report its own backlog in one pass while `mem-core` is dirty; the wave measures
  with the `-W` form and closes with the `-D` form.
- Retention exceptions are a judgement call enforced by review, not by a mechanism.
- With no CI, the gate is a convention. A wave that forgets to run it is not stopped by anything in
  the tree.

**Neutral / forward**

- The `?` rule is a reading of the owner's instruction, recorded so it is not re-litigated; the
  893-site audit remains an open, separate decision.
- Error enums may grow a poisoned-lock variant per crate; `#[non_exhaustive]` (ADR-0002 (a)) keeps
  that a non-breaking addition inside the workspace.
- ADR-0002 is extended, not amended: its error-shape rules are the mechanism this policy propagates
  through, and nothing here relaxes them.

## References

- ADR-0002 — Error & Logging Strategy: typed per-crate `thiserror` enums, no `anyhow`, preserved
  sources. This ADR propagates through those types and adds nothing to them.
- ADR-0012 D-3 — the vault's fail-closed open order, the reason `mem-vault`'s poisoned scope map
  propagates rather than recovers.
- `docs/agents/testing.md` — the documented battery, whose clippy line is red today (D-5).
