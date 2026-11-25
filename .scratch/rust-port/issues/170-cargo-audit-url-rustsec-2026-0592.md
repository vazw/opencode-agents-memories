# 170 — `cargo audit` reports `RUSTSEC-2026-0592` against `url 2.5.8`

Status: ready-for-agent — measured: url and rustix both lockfile-patchable; chrono needs an accept entry; cargo audit must exit 0 after
Filed: 2026-11-25, noticed by the issue-169 review (`.tmp/progress/issue-169-score-determinism.review-2.md`)

## The observation

REPORTED by the issue-169 review, not yet re-derived by me: `cargo audit` flags `RUSTSEC-2026-0592` against
`url 2.5.8`. The advisory text, the severity, the affected range and the reachable call path are all unread.

Pre-existing and unrelated to that diff — the 169 change adds no dependency and does not touch `Cargo.toml` or
`Cargo.lock`. Which means it has been there for some time without being caught, since no CI runs `cargo audit`
(`AGENTS.md`: "There is no CI, no release automation").

## Why this is filed rather than fixed

The repo has a specific posture for this: `cargo audit` runs from the repository root only, and `.cargo/audit.toml`
is the sole file it reads accepted findings from, and that file documents its own drift check. So there are exactly
three possible states and they need different actions:

1. **Already an accepted finding in `.cargo/audit.toml`** and the report is stale or the ignore list has drifted —
   then the fix is the drift check, not the dependency.
2. **Not in `.cargo/audit.toml` and the advisory fires on the ignored path** — then either the ignore entry is
   missing or the advisory was published after the list was last touched. Read the advisory before writing anything.
3. **Not in `.cargo/audit.toml` and `url 2.5.8` is reachable from a path that matters** — then this is a real
   security item and outranks ranking work.

Nothing here should be "fixed" by bumping `url` before establishing which state holds: `url` is pulled in by the
dependency tree, not chosen directly, and a bump may need a parent bump instead.

## First step (what settles it)

Run `cargo audit` at `HEAD` from the repository root, then read `.cargo/audit.toml` and compare. Report: the
advisory's own id text, the severity and the affected range as the advisory states them, the dependency path to
`url 2.5.8` (`cargo tree -i url@2.5.8`), and whether an ignore entry exists and why. Do not write an ignore entry
before reading the advisory — an accepted finding nobody re-read is the same vacuity this project keeps finding in
its own gates.

## Inverted world

Closed as *measurement, not a deliverable* if `cargo audit` at `HEAD` reports no `RUSTSEC-2026-0592` (the review's
observation was of a different tree or a stale database), or if the advisory is already accepted in
`.cargo/audit.toml` with a reason that still holds and the drift check confirms it.

## Not in scope

- Any dependency bump: that is the follow-up if state 3 holds.
- Issue 169's scoring change, which is unrelated and lands separately.

## Measured 2026-11-25 — state 3, plus two more findings and a register gap

`cargo audit` at `faa9620`, from the repository root (the only directory where `.cargo/audit.toml` is read), 495
dependencies, advisory DB 1568 entries:

| ID | crate | class as RustSec records it | patch exists | in `ignore`? |
|---|---|---|---|---|
| `RUSTSEC-2026-0592` | `url 2.5.8` | **vulnerability** — stack overflow in `Url::origin()` on nested `blob:` URLs | **yes, 2.5.9** | no |
| `RUSTSEC-2026-0486` | `rustix 1.1.5` | **unsound** — `xattr` marks unwritten bytes initialised on a zero-length buffer | **yes, 1.1.6** | no |
| `RUSTSEC-2026-0543` | `chrono 0.4.45` | unmaintained | no fixed upgrade | no |

So it is state 3, and two findings beyond the one the review noticed. `cargo audit` exits 1 on the vulnerability and
treats unsound/unmaintained as warnings, which is why only `url` failed the run.

**Both patched crates are lockfile-only changes.** `Cargo.toml:36` declares `url = "2"`, so `2.5.9` is in range and
`cargo update -p url --precise 2.5.9` suffices with no manifest edit. `rustix` is transitive in two lines
(`0.38.44` and `1.1.5`); only `1.1.5` is affected, and `cargo search` reports `1.1.6` as current. Reachability from
the workspace: `url` enters through `mem-bridge`, `mem-web`, `reqwest` and `tower-http`, so the vulnerable call is
compiled into the HTTP surface, not only into dev tooling.

**The register gap is the finding worth keeping.** `.cargo/audit.toml` states that "every entry below is an ACCEPTED
finding" and lists unmaintained as one of the classes it records, yet `chrono` has been unmaintained since
2026-09-14 and no entry exists. `chrono` is a direct workspace dependency (`Cargo.toml:45`) used by `mem-core`, so
this is not a stray transitive. Either an accept entry is owed with a reason and a revisit condition, or the
dependency is owed a replacement — and the file's own doctrine says an accepted entry must not be written before the
finding is read, which is what this triage did.

## Fix shape

1. `cargo update -p url --precise 2.5.9` — lockfile only, patch bump within `"2"`.
2. `cargo update -p rustix@1.1.5 --precise 1.1.6` — lockfile only, transitive patch bump.
3. `chrono`: add an accept entry in the file's existing format (class, blocked-by crate, why no escape, revisit
   condition), since no fixed upgrade exists and replacement is a separate change. Do not silently leave it absent.
4. Verification, each measured: `cargo audit` exits **0** (not merely "the vulnerability is gone"); `cargo test
   --workspace --no-fail-fast` shows no new red against the four pre-existing ones recorded in issue 169; the store
   format is untouched (no embedder or tokenizer change here, so no re-embed question arises).

## Inverted world

Closed as *measurement, not a deliverable* only if `cargo update --precise` for either crate fails to resolve, or if
the bump moves behaviour the gates can see — in which case the finding goes back to the accept list with the measured
reason, not to a silent pin.
