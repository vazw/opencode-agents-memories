# 88 — host arm fails with `errorKind:protocol`: the live host wraps session creation in `{"data":{…}}` and the client reads a bare `{id}`

Status: ready-for-agent — 2026-11-01 (the create-step mismatch is MEASURED: the live body is recorded
in-tree as `MEASURED_2_2_5_CREATE_BODY` (`crates/mem-bridge/tests/host_capture_e2e.rs:30`, added in
`d3f4e07`) and pinned by a green test that asserts the dead-letter. Work will modify
`crates/mem-ai/src/provider/opencode_host.rs` (`create_session`) and re-point that row.)

## What is missing

Since host-first precedence went live, auto-capture selects the named OpenCode host and fails:

```
Auto-capture provider ready: {"baseUrl":"http://127.0.0.1:49374/api","hostVersion":"2.2.5","providerId":"opencode-go"}
Auto-capture provider failed: {"errorKind":"protocol","reason":"summarizer_failed"}
```

MEASURED 2026-11-01T16:59-17:05 (+07) (the staged engine at that moment was sha `588437264bac`, built from
HEAD `13e0014`; the tree has since moved to `585956d`, which is **not** staged). `errorKind:"protocol"` is
new information: before the re-stage the same log line appeared with the manual arm and was a *parse*
failure; on the host arm a protocol error points at the exchange itself.

**The create-step mismatch, MEASURED.** The live host answers `POST {origin}/api/session` with the envelope

```json
{"data":{"id":"ses_…","projectID":"…", …}}
```

recorded verbatim as `MEASURED_2_2_5_CREATE_BODY` (`crates/mem-bridge/tests/host_capture_e2e.rs:30`), while
the port's `create_session` (`crates/mem-ai/src/provider/opencode_host.rs:355-383`, URL built at `:360`)
reads a bare `{id}`. Every capture therefore stops at session creation and is classified `Protocol`.

Neither side is simply "wrong": upstream's TypeScript client runs inside the host and reads through the SDK
wrapper, which unwraps `data` (`readSdkData`, `example/opencode-mem/src/services/ai/opencode-provider.ts:771-786`);
the raw HTTP body is the envelope. A client speaking to the raw `/api` must unwrap it. An earlier revision
of this issue asserted the opposite — that the shape was "not the finding" — which was wrong; this
paragraph is the correction.

**What is NOT measured — do not act on it.** A `/generate` endpoint appears nowhere in this tree or in
`example/` (the only `generate` hit there is the word `generateDiff` in a comment), so the claim that prompt
generation moved to `/generate` is withdrawn. `:385-410` is `prompt_session` only (`delete_session` starts
at `:412`). Whether the prompt and structured-output steps also mismatch **cannot be observed until session
creation succeeds** — do not rewrite them on speculation.


**Why `reason` cannot discriminate.** The log shows `errorKind:protocol` with `reason:summarizer_failed`,
and an earlier revision read that as evidence that `last_failure` was `None` and the label was a fallback
(`capture.rs:1013-1025`). On the host arm `last_failure` **is** `Some`: `summarize` records the
classification at `capture_prompt.rs:383`, and `classify_provider_error`
(`crates/mem-bridge/src/capture_prompt.rs:339-356`) maps only `Auth`/401/403 and `Config` to their own
labels — every other kind, `Protocol` included, falls through the `else` at `:348-349` to
`"summarizer_failed"`. (The audit cited `:321-338`; the r5 landing at `585956d` moved the function down
~18 lines, so cite the function, not the old offsets.) The fix that would make the log discriminating is
therefore **here**, not in `capture.rs`: give `Protocol` its own label.

## What would settle the rest

The create-step body is captured; the envelope is known. What is still open is what happens **after**
creation succeeds — the prompt body and the structured-output reply. One captured pair for each of those
closes it. Until then, only the create step may be changed.

Separately, the same observation closes the one real gap in issue 83's case: the engine logs a fixed reason
label and throws away the error text (`capture.rs:799`).

## What lands

- `create_session` accepts the host's `{"data":{…}}` envelope (and still tolerates a bare `{id}`, so the
  client is not broken against a host that does not wrap).
- The downstream prompt body is verified against a captured exchange before any change to it is made —
  no rewriting on speculation.
- The failure is reported as a named, field-carrying reason rather than a generic `summarizer_failed` —
  an operator must be able to tell "the host changed its shape" from "the model returned prose". The
  mapping site is `classify_provider_error` (`capture_prompt.rs:339-356`).
- The recorded exchange is promoted from a test constant into `docs/evidence/` with host version and date.

## Acceptance criteria

- [ ] `create_session` unwraps `{"data":{…}}` from the live host and still parses a bare `{id}`.
- [ ] The green pin that asserts the 2.2.5 dead-letter is re-pointed: the create step now succeeds, and
      the row asserts the **next** failure or success rather than the protocol error at creation.
- [ ] The recorded pair (create, and — once reachable — the prompt) lives under `docs/evidence/` with
      host version and date, not only as a test constant.
- [ ] A wire-shape mismatch produces a distinct named reason, not `summarizer_failed` — pinned by a row
      on `classify_provider_error`.
- [ ] Nothing is claimed about the prompt or structured-output steps until a pair for each is captured.


## Parent

Slice issue 92 (`92-slice1-capture-persists-end-to-end.md`).

## Comments

**The measurement that closes "it never succeeded" (MEASURED 2026-11-01T11:00Z, read-only over
`~/.opencode-mem/`):** all 22 log files (21 dated `opencode-mem-YYYY-MM-DD.log` plus the current
`opencode-mem.log`) contain **0** lines of the Rust format — `Auto-capture memory persisted: {"captureId":…}`.
The 16 persisted lines on 2026-10-31 are **TypeScript-plugin** lines, carrying `prompt_…`/`mem_…` ids
(`{"promptId":"prompt_178…","sessionID":"ses_…","memoryId":"mem_178…"}`), and no
`Auto-capture provider ready` line appears anywhere on that day. The audit's "16 persisted on the swap
day" must not be re-inherited as evidence that the port has ever persisted a capture.

**Why this is first in the chain:** with host-first precedence, the host arm is what production uses.
Issue 84 repaired the manual/fallback arm and therefore changes nothing observable while this stands;
issue 83's payload fix sits behind both, because a capture never reaches `commit_episodic` until a
summary exists.

**Not a regression:** the host arm was selected only from `eb2edd9` onwards, and the staged binary
carrying it went live at 16:45 on 2026-11-01. Before that, the same dead letters came from the manual
arm's parse failure (issue 84).
