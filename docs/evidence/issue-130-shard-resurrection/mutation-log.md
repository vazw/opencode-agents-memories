# Mutation log — issue 130 shard-resurrection wave

Worktree: `.tmp/worktrees/issue-130` (branch `fix/issue-130-shard-resurrection`).
Suite under test: `cargo test -p mem-web --test shard_resurrection` (acceptance, A/B/C/E/D),
`cargo test -p mem-web --lib` (4a unit pins), `cargo test -p mem-store --test shard_open_premise`
(4b/4c premises). Every mutant below was applied to a clean tree, observed, and reverted
(`git status` clean between kills — verified each time, recorded inline).

## Step 4 — premise mutant (deleted creation, demonstrated once)

**Mutant M0** — "deleted creation": `open_database`
(crates/mem-store/src/connection.rs) made to refuse instead of creating:

```rust
// applied at the head of open_database:
return Err(StoreError::Io {
    message: "mutant: create-on-open deleted".to_string(),
    os_code: None,
    kind: None,
});
```

Mechanism: the connection seam no longer creates a missing file (the ADD-refusal
shape — the exact wire shape Step-5 kill 3a produces). Demonstration target: pin 4b
(`get_connection_creates_a_missing_file`).

**Observed (MEASURED):**

```
running 2 tests
test open_shard_creates_a_missing_file ... FAILED
test get_connection_creates_a_missing_file ... FAILED
```

4b REDs on `create-on-open must answer Ok for a new path` (get_connection answers
`Err(Io{mutant})`, file never created). 4c REDs identically — `open_shard` delegates to
`get_connection` (shard_manager.rs:282-284). The mem-web suite was not run against M0
(kill 3a's official observation, including the fixture setup-abort recording, happens in
Step 5's dedicated run). Kill recorded: the premise pins discriminate the
refusal-instead-of-create mutant. Reverted with `git restore crates/mem-store/src/connection.rs`;
`git status` clean after revert.

## Step 5 — the six kills (each applied on a clean tree, observed, reverted)

Suite under test: `cargo test -p mem-web --test shard_resurrection` unless noted.

---

### Kill 1 — single-guard deletion: `all_memories`

Edits: the head line `self.stat_shard_file(shard).await?;` removed from `all_memories` ONLY
(store_adapter.rs). Guards of the other three lanes kept.

**MEASURED:** exactly the pre-registered victim —

```
test test_deleted_registered_shard_read_refuses_without_resurrecting_get_memories ... FAILED
thread ... panicked at shard_resurrection.rs:223:5: (the no-resurrection assertion)
test result: FAILED. 4 passed; 1 failed
```

A REDs (file recreated + the read pops the generic 500). A's lane (`all_memories`) is the only
one this request walks; B/C stay green (their routes hit their own lanes' intact guards first),
D/E green. Kill 1 = A only, as pinned. Reverted (guard line re-added); `git status` verified clean.

### Kill 2 — single-guard deletion: `list_memories_by_container_tag`

Edits: guard head line removed from `list_memories_by_container_tag` ONLY.

**MEASURED:**

```
test test_deleted_registered_shard_read_answers_503_get_memories_tagged ... FAILED
thread ... panicked at shard_resurrection.rs:321:5 ("row 3: the deleted registered shard answers the typed 503")
test result: FAILED. 4 passed; 1 failed
```

C REDs on the 503 status (leg re-answers 500 with the recreated file caught by `!exists`).
A/B/D/E green — C owns this leg alone, as pinned. Reverted; clean.

### Kill 2b — single-guard deletion: `distinct_tags`

Edits: guard head line removed from `distinct_tags` ONLY.

**MEASURED:**

```
test test_deleted_registered_shard_read_answers_503_get_tags ... FAILED
thread ... panicked at shard_resurrection.rs:272:5 ("row 1: the deleted registered shard answers the typed 503")
test result: FAILED. 4 passed; 1 failed
```

B REDs — `GET /api/tags` traverses `collect_distinct_tags` → `distinct_tags` BEFORE
`all_scope_rows` (tags.rs:114→122), so the walk hits the deleted guard first. A/C green,
as pinned (A's unfiltered leg with `includePrompts=false` never calls `distinct_tags`;
C's tagged leg with `includePrompts=false` reaches `list_memories_by_container_tag`,
intact). Reverted; clean.

### Kill 3a — hoist, pinned reading: ADD a refusal inside `get_connection`

First re-id: the Step-4 M0 mutant is mechanically the SAME ADD shape; the Step-5 run adds the
asking the gate pinned for the producer-side over-application. Edits: a metadata-refusal
inserted at the head of `ConnectionManager::get_connection` (mem-store), BEFORE turso creates;
mem-web guards all remain.

**MEASURED (all three lobs):**

- 4b premise: `get_connection_creates_a_missing_file` RED — `create-on-open must answer Ok
  for a new path: Io { message: "mutant: connection-layer refusal for a missing file" ... }`
- 4c premise: `open_shard_creates_a_missing_file` RED — same refusal (delegation,
  shard_manager.rs:282-284).
- mem-web acceptance suite: **all five tests fail in SETUP** — fixture expect-panic at
  `real_store.rs:111` ("the fixture active shard is read: Io {...}"), because
  `create_shard`'s fresh-shard path and `ensure_initialized`'s metadata-db path are BOTH
  not-yet-created files. Per the gate-corrected trace: the setup-abort (expect-panic output)
  is the recorded mechanism; NO request executes and no request-level assertion moves.
- mem-web lib (4a): `shard_file_guard_refuses_when_metadata_errors`,
  `shard_file_guard_carries_no_free_text` and `shard_file_guard_sits_at_the_method_head`
  all GREEN 17/17 — the refusal never reaches mem-web code (layer separation).

Kill: BOTH mem-store premises RED while 4a pins stay green. Reverted (`connection.rs`
restored); `git status` clean.

### Kill 3b — hoist, pinned reading: MOVE the mem-web guards into `open_shard**

Edits: the four mem-web method-head guard calls deleted; a metadata-refusal inserted at the
head of `ShardManager::open_shard` answering a `StoreError` (mem-store cannot name the
mem-web-typed variant — `tests/workspace_deps.rs` forbids the edge), so the refusal surfaces
`StoreError → PortError::Store → frozen 500`.

**MEASURED (all three lobs):**

- 4c premise RED: `open_shard_creates_a_missing_file` panics on the refusal — the move owns
  its lane. **4b GREEN**: `get_connection_creates_a_missing_file` passes (`get_connection`
  untouched — `create_shard`'s path, shard_manager.rs:769). Disjoint victim sets vs 3a, exactly
  the observable layer separation the two-layer pattern demands.
- 4a: `shard_file_guard_sits_at_the_method_head` RED — "the read lane answered the generic
  Store refusal — the guard call is not at the method head (the MOVE mutant)". The two
  helper seam pins (`..._refuses_when_metadata_errors`, `..._carries_no_free_text`) stay green
  (the private helper still exists; the pin records the method-head leg as the 4a victim).
- Acceptance: **B and C RED** on the 503 status (panic lines 272/321 — the reads reach
  `open_shard`, which refuses with the Store-flavored error → frozen 500; the 503
  assertions fail: this is the pre-registered status-flip — the flip is 503→500, and the
  body half stays clean). D/E green (create path + stats lane untouched).
- **A GREEN — recorded green-with-reason per pinned reading:** the heredoc lists A as a 3b
  victim, but A's pinned assertion set (the gate's own five-test Step-2 truth: A carries the
  no-resurrection pin without a status assertion so it stays green under the interim
  Store→500 mapping) makes A's 3b observable {status 500, fixed-500 envelope body, file
  absent} IDENTICAL to its own green state — no wire assertion can separate them. Blueprint
  Step 5 authorizes precisely this: "A green named victim is recorded green-with-reason in
  mutation-log.md; every named victim RED is satisfied per the pinned reading only."

Reverted (four guard lines re-added, `shard_manager.rs` restored); `git status` clean;
mem-web lib 17/17 and acceptance 5/5 green again.

### Kill 4 — wrong-arm mapping (two sub-mutants)

**(i) the new arm returns `internal_error()`** (observed twice: against the literal-arm build,
then re-observed against the final single-source arm):

- First run: B and C RED on the 503 status (500; body = fixed 500 envelope; A/D/E green).
- Final-arm re-run: B and C RED on the 503 status, panic lines 272/321; A/D/E green.
- Killed exactly as pinned; reverted.

**(ii) `ShardFileMissing` folded into `response_status`'s Store arm** (the pair separates
arm-adding from status-adding):

- Against the interim literal-arm build the mutant SURVIVED the suite — diagnosed before any
  suspect patching: the arm's `envelope::json(StatusCode::SERVICE_UNAVAILABLE, ...)`
  constructed the wire status independently of `response_status` (which feeds only the
  structured-stderr diagnostic), so folding the response_status side alone never reaches the
  wire. The blueprint's OWN doc line for `response_status` ("the diagnostic line names the
  status the client actually received") implies the single-source linkage, so:
  **code deviation (recorded in the ledger + handoff):** the port_failure arm now builds its
  wire status from `response_status(&error)` (`from_u16` + `unwrap_or(SERVICE_UNAVAILABLE)`).
  Wire behavior in the correct build is unchanged — full mem-web suite green (including
  api_port_failure_class 6/6, api_diagnostic_stderr 2/2, shard_resurrection 5/5).
- Against the final arm, **MEASURED:** B and C RED on the 503 status (the wire built 500)
  while the body still carried the 503 fixed message ("the body arm still correct") —
  exactly the pinned separation. Reverted; suite green 5/5 again.

After all kills: `cargo test -p mem-web --test shard_resurrection` 5/5; `git status` shows only
the wave's own files; fmt clean.

