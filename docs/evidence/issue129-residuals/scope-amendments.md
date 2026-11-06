# Issue 129 residuals r9 — scope amendments (syseng rulings, durable record)

Tree: worktree `.tmp/worktrees/issue-129`, branch `issue-129/wave-residuals`, base `eda2efd` +
uncommitted shard diffs. Written 2026-11-06 by the coder rework pass, transcribing the syseng
ruling (session `ses_e4eb3541bffezzOuDRv7X3YBW4`, answers Q1=ACCEPT, Q2=ACCEPT-STOP, delivered to
the orchestrator the same day). This file exists because a ruling that lives only in `.tmp/` is one
`rm -rf` from gone; the canonical ledger still carries the Step 7 pointers.

## Amendment 1 — `web-ui/Cargo.toml` ownership grant (Finding 2, Q1=ACCEPT)

The blueprint's `[FAN-OUT PLAN]` forbids `web-ui/Cargo.toml` to both shards. Shard B added two
web-sys features there (`web-ui/Cargo.toml:38` `"HtmlDivElement",` and `:40` `"NodeList",`, `+2 -0`,
nothing else moved). Review-1 flagged it BLOCKER (blueprint-forbidden file edited). The syseng gate
ruled **ACCEPT**: the two feature lines stand under a recorded grant; no revert, no code rewrite.

Basis (each leg re-derived on this tree, 2026-11-06):

- `NodeList` is load-bearing: exactly one call site, `web-ui/src/dialog_focus.rs:78`
  (`container.query_selector_all(FOCUSABLE)`); removing the feature fails with E0599 "no method
  named `query_selector_all`" at that line.
- `HtmlDivElement` backs `NodeRef<html::Div>` at `web-ui/src/confirm.rs:125` and
  `web-ui/src/core_screens.rs:2632` (the dialog surface wrappers' keydown-trap refs).
- The lockfile diff is empty: `git diff HEAD -- Cargo.lock` (and the whole tree) shows no
  `Cargo.lock` change — same crate, same version, feature flags only.

**Binding scope: THIS WAVE ONLY** (`issue-129/wave-residuals`, r9). Any later wave that touches
`web-ui/Cargo.toml` needs its own ownership grant; the default rule (the file is forbidden to
executor shards) is unchanged.

## Amendment 2 — Step 4 closes as diagnosis (Finding 3, Q2=ACCEPT-STOP)

The blueprint names `test_partial_storage_root_answers_structured_unavailable` as a Step 4
criterion. Shard A STOPPED: a distinct typed missing-store signal needs
`crates/mem-web/src/api/mod.rs` (`port_failure`, outside the shard's file list) and re-baselines the
frozen `crates/mem-web/tests/scope_wide_delete.rs:373` 500 pin; the open-first read routes detect a
missing shard only by *recreating* it, which no 503 may enshrine. The syseng gate ruled
**ACCEPT-STOP**: Step 4 closes as diagnosis; the test is NOT authored; no code changes for Step 4.

- Full decision block: the `Decision:` line in
  [partial-store-route-matrix.md](partial-store-route-matrix.md) (appendix below).
- Follow-up candidate, NOT authorized here: stat-before-open on the open-first reads — filed as
  `.scratch/rust-port/issues/130-shard-resurrection-on-open-first-read.md` (needs-triage, TO BE
  GATED).
- Inverted world (what would reopen Step 4 as measurement): a route where a distinct typed
  missing-store signal exists WITHOUT touching `port_failure` or the frozen `scope_wide_delete`
  500 — that observation reopens the criterion.

## Appendix — the grant text as recorded in the matrix file

Same content as the `Decision:` block appended to `partial-store-route-matrix.md`; repeated here so
the amendment file is self-contained:

> Step 4 closes as DIAGNOSIS per the syseng ruling Q2=ACCEPT-STOP, 2026-11-06 (session
> `ses_e4eb3541bffezzOuDRv7X3YBW4`): `test_partial_storage_root_answers_structured_unavailable` is
> not authored, no code changes for Step 4. The route matrix above is the measurement the criterion
> closes on. Inverted world: a route with a distinct typed missing-store signal that requires
> neither the `port_failure` arm in `crates/mem-web/src/api/mod.rs` nor a re-baseline of the frozen
> 500 in `crates/mem-web/tests/scope_wide_delete.rs:373` reopens Step 4 as measurement. The
> stat-before-open follow-up is filed as issue 130 (needs-triage, gated separately). Ownership note
> (review-1 LOW-2 settled): this matrix file is granted as shard A's fourth evidence file for the
> r9 wave.
