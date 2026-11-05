# 88 — host arm fails with `errorKind:protocol`: three documented wire-contract mismatches (create envelope, prompt route, prompt reply)

Status: done — 2026-11-04. Evidence levels are stated per claim, because two earlier revisions of
this file — and a commit correcting one of them — over-claimed in both directions. The 2.2.5 **spec** is
**READ** (`.tmp/openapi.json`, untracked, fetched 2026-11-01T09:44; it declares the create and prompt routes
and their envelopes). The *live failure* is **MEASURED** (`errorKind:"protocol"` on the host arm). The body
recorded in-tree as `MEASURED_2_2_5_CREATE_BODY` is **NOT a capture** (synthetic, see below). No runtime
exchange has ever been captured, so every shape below is documentation, not observation. Work modifies
`crates/mem-ai/src/provider/opencode_host.rs`, re-points the pin, and renames the constant.

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

**The create-step mismatch: envelope READ, instance synthetic, locus unconfirmed.**

The 2.2.5 OpenAPI sample documents that responses carry a `data` property
(`.tmp/openapi.json`, untracked under gitignored `.tmp/`), which is the same wrapping upstream's TypeScript
client unwraps through `readSdkData` (`example/opencode-mem/src/services/ai/opencode-provider.ts:771-786`);
a client speaking to raw `/api` must unwrap it. The port's `create_session`
(`crates/mem-ai/src/provider/opencode_host.rs:355-383`, URL built at `:360`) reads a bare `{id}`.

**The recorded body is not wire evidence.** `crates/mem-bridge/tests/host_capture_e2e.rs:30` names the
constant `MEASURED_2_2_5_CREATE_BODY`, but its values are placeholders — `"projectID":"prj_1"`,
`"tokens":{"input":10,"output":20}`, `"time":{"created":1,"updated":2}`, `"directory":"/tmp"` — and
`ses_2_2_5` appears zero times in the OpenAPI sample. It is a hand-authored instance built from the
documented schema, not a captured response. The green test at `:135` therefore pins the *port's behaviour
against an assumed shape*, which is useful, but no live create response has ever been captured.

**The prompt step is mismatched too — documented, not inferred.** The same sample declares
`POST /api/session/{sessionID}/generate` (required body `{"prompt": string}`) and
`GET /api/session/{sessionID}/message`. The port POSTs to `/session/{id}/message`
(`crates/mem-ai/src/provider/opencode_host.rs:394`, error label at `:408`). A POST to a GET-only route cannot
succeed. `/generate` answers `{"data":{"text": string}}` — a JSON object, **not** a stream, which is exactly
the non-streaming shape `585956d`'s structured call already assumes.

**Correction to the previous revision of this file.** That revision said `/generate` "appears nowhere in this
tree or in `example/`" and withdrew the claim. That search was true of the tree and of `example/`, and false
of the host spec, which is what actually governs the wire. The withdrawal is withdrawn. What is still true:
no captured exchange proves any of these shapes at runtime, and no Rust code in this tree references
`/generate`.

**The failure's location is unconfirmed — and no longer load-bearing.** `Protocol` is returned both by a
create-parse failure and by `parse_prompt_response` when the model answers prose, so the live
`errorKind:"protocol"` does not by itself name the step. But two of the three steps are documented-broken
independently of each other, so the fix does not need the locus: **tolerate both create shapes**, and **call
the documented prompt route** (substitution, since a GET-only route cannot accept a POST). A captured
transcript is what converts this file's READ columns into MEASURED ones.


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

`create_session` is correct under either shape once it tolerates the envelope, and the prompt call moves to
the route the spec declares. What is still open is whether any of it behaves as documented at runtime —
one captured live exchange for create and one for the prompt close that. The structured-output reply shape
is the last unknown, and the spec gives it (`{"data":{"text":…}}`).

Separately, the same observation closes the one real gap in issue 83's case: the engine logs a fixed reason
label and throws away the error text (`capture.rs:799`).

## What lands

- `create_session` accepts both `{"data":{…}}` and a bare `{id}`, so the client is correct whichever shape
  the host sends — no locus confirmation is needed for the fix to be sound.
- `prompt_session` POSTs to `/session/{id}/generate` with `{"prompt": …}` and reads `{"data":{"text":…}}`,
  instead of POSTing to the GET-only `/session/{id}/message`.
- The constant that says `MEASURED` is renamed to say what it is: a schema-derived sample.
- The failure is reported as a named, field-carrying reason rather than a generic `summarizer_failed` —
  an operator must be able to tell "the host changed its shape" from "the model returned prose". The
  mapping site is `classify_provider_error` (`capture_prompt.rs:339-356`).
- The sample is promoted into `docs/evidence/` labelled by what it is (`READ`, not `MEASURED`), with host
  version, date and **how it was obtained**, and a recorded path to upgrade it once a real transcript exists.

## Acceptance criteria

- [ ] `create_session` accepts `{"data":{…}}` **and** still parses a bare `{id}`, so it is correct whichever
      shape the live host sends.
- [ ] `prompt_session` uses `POST /session/{id}/generate` with a `{"prompt": …}` body and decodes
      `{"data":{"text":…}}`; no code path POSTs to `/session/{id}/message` any more.
- [ ] The green pin that asserts the 2.2.5 dead-letter is re-pointed: create and prompt now both succeed
      against the recorded shapes, and the row asserts the **next** observable step rather than a protocol
      error at creation.
- [ ] The constant named `MEASURED_2_2_5_CREATE_BODY` is renamed to reflect that it is a schema-derived
      sample: its values are placeholders (`prj_1`, `input:10`, `created:1`) and `ses_2_2_5` appears in no
      captured artifact. A name that asserts an evidence level it does not have is the defect being fixed.
- [ ] The sample lives under `docs/evidence/` labelled `READ` (not `MEASURED`) with host version, date and
      how it was fetched, and the file states what would upgrade it: one captured live exchange.
- [ ] A wire-shape mismatch produces a distinct named reason, not `summarizer_failed` — pinned by a row
      on `classify_provider_error`, with prose replies still classified as prose.
- [ ] No test fixture claims to be a host transcript when it is a schema sample; the ledger and the evidence
      README both say which columns are READ and which are MEASURED.


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

**Evidence-level correction, 2026-11-01T11:40Z (this file's second, and mine, not an agent's).** An earlierrevision of this issue called `MEASURED_2_2_5_CREATE_BODY` 'recorded verbatim' from the live host, and
commit `8c6bfb2` restated it as MEASURED. Re-derived against the artifact: the constant's values are
placeholders — `"projectID":"prj_1"`, `"tokens":{"input":10,"output":20}`, `"time":{"created":1,"updated":2}`,
`"directory":"/tmp"` — and `ses_2_2_5` occurs **zero** times in the 2.2.5 OpenAPI sample
(`.tmp/openapi.json`, untracked, fetched 09:44). It is a hand-authored instance built from the documented
schema. The correct ledger is therefore: envelope **READ** from the spec; live failure **MEASURED**;
failure **locus unconfirmed**, because `Protocol` is also what a prose reply produces. The over-claim was
inferring an evidence level from a constant's *name* — which is exactly the defect class this issue set
exists to fix, so it is recorded here rather than quietly edited away.

**`/generate` reversal, 2026-11-01T11:52Z (this file's third correction, mine again).** The previous revision
withdrew the `/generate` claim on a search of this tree and of `example/` — true of both, and irrelevant,
because the host spec is what governs the wire. The 2.2.5 sample declares
`POST /api/session/{sessionID}/generate` with required body `{"prompt": string}` answering
`{"data":{"text": string}}`, and declares `/api/session/{sessionID}/message` as **GET only**. The port
(`:394`) POSTs to `/message`, so the prompt step cannot succeed either way. Two lessons are recorded rather
than smoothed over: a negative search result is evidence about the search, not about the host; and the
spec was sitting in `.tmp/` untracked, unread, while three revisions argued about shapes it states.
The scope therefore widened from create-only to the three documented mismatches.

[decision] 2026-11-04 — live wire-contract capture approach chosen (issue 88 residual; issue 126).
Approach: operator-executed .tmp/scratch/capture-live-88.sh against http://127.0.0.1:49374 (OpenCode 2.2.5). Auth: HTTP Basic opencode:$PASS from ~/.local/state/opencode/service.json (matches crates/mem-ai/src/provider/host_auth.rs). Steps: POST create -> /api/session with Model.Ref body; POST generate -> /api/experimental/generate/text; compensating DELETE on the created session. Evidence: docs/evidence/issue-88-host-wire-contract-20261101/create.exchange.json and generate.exchange.json, headers REDACTED, bodies verbatim, JSON {request, response, captured_at}. No production code; docs/fixture portion TDD-exempt. Rationale: every shape was READ (schema-derived) or INFERRED; the drift claim stayed NEEDS_VALIDATION until one captured live create and one captured live generate exchange existed.

[fact] 2026-11-04 — issue 88 evidence capture complete (MEASURED).
- POST /api/session -> 200, data envelope, session ses_e59a04f07ffeElHt9jTxpGsGP7, compensating DELETE issued.
- POST /api/experimental/generate/text -> 200, data.text envelope.
- Wrote create.exchange.json and generate.exchange.json (headers REDACTED, bodies verbatim).
- README §3 evidence levels promoted READ→MEASURED for create/generate/DELETE; §7 marked COMPLETE.
- cargo test --workspace green, zero failures; clippy clean.
- Git state uncommitted, no push: modified .githooks/pre-commit, README.md (pre-existing), docs/adr/0016-pre-commit-gate.md, docs/evidence/issue-88-host-wire-contract-20261101/README.md, plugin/tests/wrapper-glue.test.mjs, scripts/test-pre-commit.sh; new create.exchange.json, generate.exchange.json, plugin/tests/dist-sync-v2.test.mjs.
- Issue 88 residual closed (code already landed; no production change).
