# 88 — host arm fails with `errorKind:protocol`; cause unconfirmed

Status: needs-triage — 2026-11-01 (the failure is MEASURED live; the **cause** is NOT confirmed. This
issue deliberately records an observable and no mechanism. See "What would settle it" below. With
host-first precedence this is the top of the capture dependency chain.)

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

**The wire shape is not the finding.** An earlier revision of this issue prescribed a rewrite of the host
client to a `{data: …}` envelope and a `/generate` endpoint, on an audit's report that 2.2.5 wraps session
creation. That prescription is **unsupported and is withdrawn**:

- upstream's own client posts `POST {base}/session` and reads a bare `{id}` —
  `example/opencode-mem/src/services/ai/opencode-provider.ts:812-829` (`createSession`;
  `const url = `${base}/session${buildQuery(directory)}``, `body.id` at `:822-828`);
- it posts `POST {base}/session/{id}/message` — same file, `:891-904` (`promptSessionRaw`);
- `{data}` is the **SDK wrapper's** envelope, read by `readSdkData` at `:771-786`, not by the HTTP client;
- `/generate` appears nowhere in `example/` except a prose comment in
  `src/services/user-profile/ai-cleanup.ts:140`.

The port is a faithful port of that client: `create_session` is
`crates/mem-ai/src/provider/opencode_host.rs:355-383` (URL built at `:360`, `{base}/session{directory_query}`),
and `:385-410` is `prompt_session` only (`delete_session` starts at `:412`). An earlier revision also
described `:385-414` as "session creation" — it never was.

**Why `reason` cannot discriminate.** The log shows `errorKind:protocol` with `reason:summarizer_failed`,
and an earlier revision read that as evidence that `last_failure` was `None` and the label was a fallback
(`capture.rs:1013-1025`). On the host arm `last_failure` **is** `Some`: `summarize` records the
classification at `capture_prompt.rs:383`, and `classify_provider_error`
(`crates/mem-bridge/src/capture_prompt.rs:339-356`) maps only `Auth`/401/403 and `Config` to their own
labels — every other kind, `Protocol` included, falls through the `else` at `:348-349` to
`"summarizer_failed"`. (The audit cited `:321-338`; the r5 landing at `585956d` moved the function down
~18 lines, so cite the function, not the old offsets.) The fix that would make the log discriminating is
therefore **here**, not in `capture.rs`: give `Protocol` its own label.

## What would settle it

One captured request/response pair from the live host — the wire body the engine sends and the host's
verbatim reply — is the observation that closes this issue. Everything else is inference. Note the same
observation also closes the one real gap in issue 83's case (the discarded `EpisodicError` string): the
engine currently logs a fixed reason label and throws away the error text (`capture.rs:799`).

## What lands (once the cause is known)

- Whatever the measurement shows: the host client speaks 2.2.5's actual contract, recorded under
  `docs/evidence/` with the host version and date — or the version claim is withdrawn.
- The failure is reported as a named, field-carrying reason rather than a generic `summarizer_failed` —
  an operator must be able to tell "the host changed its shape" from "the model returned prose". The
  mapping site is `classify_provider_error` (`capture_prompt.rs:339-356`).
- A test that pins the request body shape against a recorded host response, so a future host change
  fails loudly.

## Acceptance criteria

- [ ] The exact 2.2.5 request/response pair is recorded under `docs/evidence/` as the observed contract,
      with the host version and the date — or the record states that the pair could not be captured and
      this issue is closed as `wontfix` with that measurement.
- [ ] Capture against the live host reaches the summarizer (whether it then succeeds depends on issues
      83 and 84).
- [ ] A wire-shape mismatch produces a distinct named reason, not `summarizer_failed` — pinned by a row
      on `classify_provider_error`.
- [ ] The host client's request body is pinned by a test built from the recorded exchange.

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
