# 160 — two schema maxima in `mem-store`'s CONTEXT.md drifted from the source constants

Status: ready-for-agent — 2026-11-24 (both sides READ; correction is two numerals in one sentence.
Decide at triage whether this is a direct correction or needs a wave — see "Size" below.)

## The defect

`crates/mem-store/CONTEXT.md`, in its "Version discipline" bullet, states four schema maxima in one
sentence. Two of the four are wrong:

| stated in CONTEXT.md | actual source constant | verdict |
|---|---|---|
| `MEMORY_SHARD_MAX_KNOWN = 3` | `pub const MEMORY_SHARD_MAX_KNOWN: i64 = 4;` — `crates/mem-store/src/schema_migrations.rs:74` | stale |
| `EPISODIC_MAX_KNOWN = 7` | `pub const EPISODIC_MAX_KNOWN: i64 = 8;` — `crates/mem-store/src/schema_migrations.rs:78` | stale |
| `USER_PROMPTS_MAX_KNOWN = 4` | `:69`, `= 4` | correct |
| `METADATA_MAX_KNOWN = 2` | `:63`, `= 2` | correct |

Evidence class: READ from both sides on 2026-11-24. The stale pair is the same failure mode as issue
159 — a production constant moved and the thing that restates it did not — but here the thing that
restates it is a tracked domain doc rather than a test, so nothing goes red and nothing catches it.

The sentence's own framing makes the drift worse than a typo: it presents itself as the authority for
"every set carries its maximum", and then lists two maxima that no longer exist.

## Fix

Correct the two numerals in that one sentence. Nothing else. The surrounding prose ("v6 and v7 are
applied by `apply_episodic_schema`, not by the v1..v5 slice") describes the split, not the maximum,
and stays true — but whoever makes the edit should re-read it against `apply_episodic_schema`, since
the max moving to 8 means the slice boundary claim deserves one check rather than an assumption.

## Size — the triage call

This is a doc-only correction of two numerals in one sentence, with both sides already read. A full
blueprint wave (planner → Plan Gate → coder → reviewer) for that is disproportionate, and the
reviewer's re-execution would be re-reading two `pub const` lines. The honest options:

- **Direct correction**, citing this issue and the two `file:line` anchors, committed as
  `docs(mem-store)`. Cheapest and proportionate.
- **A small wave** if the owner wants the episodic-slice sentence verified by someone other than the
  editor, which the direct path would leave as a self-check.

Either way the acceptance is the same and is grep-able: every numeral in that sentence matches its
`pub const` in `crates/mem-store/src/schema_migrations.rs`.

## Not in scope

- `crates/mem-store/tests/prompt_tombstone_schema.rs` — a deliberate opposite-policy tripwire that is
  supposed to red on a legless constant bump. Do not "fix" it.
- Any other doc that restates a maximum; a repo-wide grep for the numerals is cheap and worth running
  once while the file is open, but a sweep of every `CONTEXT.md` is a separate decision.
- Issue 159's test changes, which are shipped.

## Ceilings

- Correcting the two numerals does not prevent the third drift. Nothing reds when a `CONTEXT.md`
  numeral goes stale, and no mechanism is proposed here; the project's answer to that class is the
  `CONTEXT.md` per-crate review, not a test.
