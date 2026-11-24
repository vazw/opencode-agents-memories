# 160 — two schema maxima in `mem-store`'s CONTEXT.md drifted from the source constants

Status: done — 2026-11-25 (shipped `1a97781`; `cargo test --test context_docs` passes, repo-wide grep clean. Triage
ruled it a direct correction rather than a wave — see "Size" below.)

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

## Corrected directly — 2026-11-24 (triage: doc-only, no wave)

Took the direct path: both `file:line` anchors were already READ, the acceptance is a grep, and a planner/gate/coder
round over two numerals would cost more than the edit. Re-reading the sentence as the issue asked turned up **two more
drifts of the same class** in the same sentence, so the edit is four changes rather than two:

| correction | what the sentence said | what the source says |
|---|---|---|
| shard maximum | `MEMORY_SHARD_MAX_KNOWN = 3` | `= 4` — `schema_migrations.rs:74` |
| episodic maximum | `EPISODIC_MAX_KNOWN = 7` | `= 8` — `schema_migrations.rs:78` |
| episodic legs | "v6 and v7 are applied by `apply_episodic_schema`" | v6, **v7 and v8** — the v8 leg is at `:725-737`, gated `max_known >= 8`, and stamps `user_version = 8` |
| the "every set" list | four constants | five — `USER_PROFILES_MAX_KNOWN = 2` (`:72`) goes through the same `apply_schema_migrations` runner with `max_known: USER_PROFILES_MAX_KNOWN` at `crates/mem-profile/src/profile_store.rs:670` |

The third is worth naming separately: the sentence's parenthetical is not decoration, it explains *why* the maximum is
not derived from the migration slice. Leaving it saying "v6 and v7" while the maximum reads 8 would have been a correct
numeral inside a false explanation — the exact shape that reads as verified because a nearby figure is right.

The fourth makes the sentence's own claim ("every set carries its maximum") true; the list omitted one set that does.

**Verified after the edit**, all five sentences numerals against source:

```
METADATA_MAX_KNOWN        ctx = 2  src = 2
USER_PROMPTS_MAX_KNOWN    ctx = 4  src = 4
MEMORY_SHARD_MAX_KNOWN    ctx = 4  src = 4
EPISODIC_MAX_KNOWN        ctx = 8  src = 8
USER_PROFILES_MAX_KNOWN   ctx = 2  src = 2
```

`cargo test --test context_docs` passes. A repo-wide grep for the two old restatements now matches only this issue's own
table, the issue-159 review note, and gitignored scratch copies — no tracked doc restates them.

**Not touched, deliberately:** `crates/mem-store/tests/prompt_tombstone_schema.rs`, the opposite-policy tripwire that
must red on a legless constant bump.
