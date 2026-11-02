# 111 — the capture prompt budget is 2 bytes short of the request it builds

Status: done — 2026-11-02 — fixed in 4af852a (local commit, nothing pushed)

Filed by issue 108's blueprint (stateless-capture wave) as a TRACKED deferral: the accounting is off by a
constant, and the wave that found it did not own the arithmetic.

## Evidence

`crates/mem-bridge/src/capture_context.rs::build_bounded_summary_prompt` promises
`system + prompt + output_reserve + overhead <= total_request_bytes`. It charges
`system_prompt.len()`, the schema block (`SCHEMA_SEPARATOR` + `SCHEMA_ANSWER_INSTRUCTION` + `"\n"` +
`schema_json`), the output reserve and `SUMMARY_REQUEST_OVERHEAD_BYTES`, then truncates `context` +
`SUMMARY_ANALYSIS_SUFFIX` to the remainder.

The request the host actually sends is assembled by `crates/mem-ai/src/provider/opencode_host.rs::prompt_text`:

```rust
if system.trim().is_empty() { return user_text.to_owned(); }
format!("{system}\n\n{user_text}")
```

The `"\n\n"` fold is 2 bytes and is charged by nothing. So the real request is `charged + 2` bytes, and a
prompt that fills the budget exactly overshoots by two.

MEASURED: `cargo test -p mem-bridge --test capture_context` → 7 passing rows; none asserts the folded
request length, so the gap is invisible to the suite today.

## Consequence

Two bytes over `autoCaptureMaxContextBytes` (default `131_072`) on a maximally sized capture. Harmless on
every model seen so far; it matters only when a host enforces the budget exactly, and then the failure would
be an opaque context-length rejection attributed to the model rather than to the accounting.

## Shape of the fix (not decided here)

Charge the join separator where it is spent — either have `build_bounded_summary_prompt` reserve
`SYSTEM_USER_SEPARATOR.len()` (and keep `prompt_text` in step), or fold the system side inside the budget
builder so one function owns every byte. A test should assert the assembled request's byte length against
the budget rather than only the context slice.

## Decision

**D-111-1 — shape (a): charge the separator in the builder, from ONE shared constant that lives in
`mem-ai`.** `pub const SYSTEM_USER_SEPARATOR: &str = "\n\n"` sits in
`crates/mem-ai/src/provider/opencode_host.rs`, is re-exported by the `pub use opencode_host::{…}` list
in `provider/mod.rs`, and has exactly two spenders: `prompt_text`'s `format!` and one extra
`saturating_sub` in `build_bounded_summary_prompt`. Because the bytes exist once, the builder cannot
drift from the provider without the constant itself changing — which the wire-shape row then reds.
Shape (b), folding the system side inside the budget builder, was rejected as larger and seam-breaking:
it would force `capture_prompt.rs` to stop calling `.with_system(CAPTURE_SYSTEM_PROMPT)`, and the
remote providers send `system` as a separate field (`effective_system_prompt`,
`crates/mem-ai/src/provider/remote/plumbing.rs`), so three provider families would lose their capture
system prompt to fix two bytes in a fourth. A bare `+ 2` in the builder was rejected too: two literals
drift, and that is how the gap was born.

**D-111-2 — the separator is charged UNCONDITIONALLY, not only when `system_prompt` is non-blank.**
`prompt_text` spends nothing when the system side is blank, so an unconditional reserve leaves 2 bytes
of context unused in that case. That is the safe direction: over-reserving wastes two bytes,
under-reserving is the defect this issue records; and a conditional reserve re-opens the gap the
moment the builder's `system_prompt` argument and the request's `system` field diverge.

**D-111-3 — two rows with distinct claims, and the wire row derives nothing from the code under
test.** A1 (`host_capture_provider::test_issue_111_the_assembled_host_prompt_fits_the_capture_budget`)
measures the ASSEMBLED request the fake host records; its second assertion keeps the clamp honest
(slack ≤ 64) so an over-reserve cannot pass. A2
(`opencode_provider::test_issue_111_the_host_route_folds_the_system_side_with_the_separator_constant`)
pins the wire shape against the LITERAL `"SYS\n\nUSR"` FIRST — an expectation built from the constant
would move with the mutant and could never red — then against the constant. A1 lives in
`host_capture_provider.rs` rather than `capture_context.rs` because the issue asks for the assembled
request, and that file already owns a real `LazyHostProvider → HostStructured →
ProviderCaptureSummarizer` chain over a recording `FakeHost`. The 7 rows in `capture_context.rs` are
unmodified.

MEASURED (this wave):
- RED before the fix, on `0d13111`: `cargo test -p mem-bridge --test host_capture_provider issue_111`
  → `the assembled request fits the budget: 131074 > 131072`.
- GREEN after: charged `131072`, slack 0; `cargo test -p mem-bridge --test capture_context` 7 passed
  (unchanged); `cargo test -p mem-ai --test provider` 38 passed before the A2 row and 39 after.
- M1 (`SYSTEM_USER_SEPARATOR = "\n"`): A2 RED (`SYS\nUSR` vs `SYS\n\nUSR`), A1 **green** — both sides
  shrank together, which is exactly why A1 alone is insufficient.
- M2 (`prompt_text` hard-codes `"\n\n\n"`, the constant untouched): A2 RED **and** A1 RED
  (`131073 > 131072`) — the observation that proves the constant is actually consumed.
- M3 (`prompt_text` hard-codes `"\n"`): A2 RED, A1 **green** (1 byte of slack) — A1 is not a
  fold-shape oracle.

**Pre-existing limit, recorded not fixed:** the invariant is claimed only for budgets that can hold the
schema block plus reserves. At `autoCaptureMaxContextBytes` near 1,024 a capture exceeds its budget by
construction — the schema block, the output reserve and the 1,024 overhead alone exceed it — which
predates this wave and is unchanged by it.

## Parent

Issue 108 (capture should use the host's stateless generate route), ADR-0029 D-29-11.
