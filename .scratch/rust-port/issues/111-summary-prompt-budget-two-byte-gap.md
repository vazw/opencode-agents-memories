# 111 — the capture prompt budget is 2 bytes short of the request it builds

Status: needs-triage

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

## Parent

Issue 108 (capture should use the host's stateless generate route), ADR-0029 D-29-11.
