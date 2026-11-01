# Issue 88 — the OpenCode HTTP API sample, and what it does and does not prove

This directory exists so that "untracked artifact, origin unwritten" stops being true. Everything here is
either the API description itself (`openapi.json.gz`, `openapi-fragments.json`) or a body derived from that
description (`*.sample.json`) — **no live exchange has ever been captured** (see §7 for the path to that).

## 1. Attribution is INFERRED, not document content

The sample is the OpenCode HTTP API description: `openapi: "3.1.0"`, `info.title: "opencode HttpApi"`,
`info.version: "0.0.1"`, `servers: null`, 440 311 bytes decompressed. **It does not say "2.2.5" anywhere.**
The attribution to host 2.2.5 comes from outside it: the MEASURED `hostVersion:"2.2.5"` log line and the
resolving `service.json`, same origin, same day. Write it as an inference. It is never "the 2.2.5 document".

Reproduce the pin (the sha covers the **decompressed** stream, because `gz` bytes are not stable across
compressors):

```sh
gunzip -c openapi.json.gz | sha256sum
# 6cc49ccf6477f0bda02574858a85b010ea72ff01e13ab0d3afcf9f4d4e053c00  -
gunzip -c openapi.json.gz | wc -c
# 440311
```

## 2. How it was obtained: UNKNOWN

Nothing on disk records the fetch, and the document carries no `servers` entry, so its URL is not
recoverable from it. Conventional re-derivation is `GET {origin}/doc` or `GET {origin}/openapi.json` on the
origin `service.json` resolves — **unverified for 2.2.5**. Verify a re-fetch by the decompressed sha above,
never by byte-equality of the `gz`.

## 3. Evidence levels per claim

- **READ (documented, not observed):**
  - `POST /api/session` response is `{"data": Session.Info}` — `required: ["data"]`,
    `additionalProperties: false` (`openapi-fragments.json` → `/api/session#post`).
  - Its request body declares `…, agent, model, …, permissions` (`properties` of the same fragment).
  - `POST /api/session/{sessionID}/generate` takes `{"prompt": string}` — `required: ["prompt"]`,
    `additionalProperties: false` — and answers `{"data":{"text": string}}`.
  - `/api/session/{sessionID}/message` is **GET-only**: the path item has no `post` member.
  - `DELETE /api/session/{sessionID}` exists and answers `204`.
- **MEASURED (observed):** the live failure line `{"errorKind":"protocol","reason":"summarizer_failed"}`
  and, through `decode_json`'s status mapping, the fact that the create call returned **2xx**.
- **INFERRED:** the version attribution (§1); that the tolerated-but-undeclared `permission` key in the
  port's create body is harmless — a 2xx was observed for it, the mechanism was not.
- **NEVER CAPTURED:** any request or response. Both `*.sample.json` files are schema-derived.

The values in `create-body.sample.json` are placeholders: `ses_2_2_5`, `prj_1`, `input: 10`, `created: 1`,
`directory: "/tmp"`. `ses_2_2_5` occurs **zero** times in the sample (`grep -c ses_2_2_5 openapi.json` → 0).

## 4. What the envelope means

Upstream's TypeScript client never sees these bodies: it runs inside the host and reads through the SDK
wrapper that unwraps `data` (`readSdkData`, `example/opencode-mem/src/services/ai/opencode-provider.ts:771-786`,
clone `1ddfdad` v2.33.0). A raw `/api` client must unwrap it itself. `example/` is gitignored, so the
citation lives here rather than in a doc that would break without the clone.

## 5. The `/generate` claim, and its tracked trace

`/generate` is declared by the sample and already tracked in
`docs/adr/0027-host-endpoint-discovery.md` D-27-9 row 3. No tracked code, test or spec acted on it before
issue 88's wave; that wave does, per issue 88's third correction. (The earlier withdrawal of the claim
rested on a negative search of the tree and `example/` — true of both, and irrelevant to the wire.)

## 6. Still unmeasured — do not extrapolate

- Whether the host's tolerance of undeclared create-body keys extends to any key.
- Whether `/generate` accepts the extra `?directory=` query the port still appends.
- Whether the model named in the create body is the model that answers.
- Anything about the live reply except its documented shape.

`additionalProperties: false` in this document is **documentation, not runtime behaviour**: a 2xx was
MEASURED for a body that violates it. No comment, test name or ADR line may claim "the host will reject X".

## 7. Upgrade path to MEASURED

One transcript per step — request headers redacted, bodies verbatim — plus a compensating `DELETE` on the
session it creates. Owner-gated, because it POSTs to the serving host. Append the results as
`create.exchange.json` / `generate.exchange.json` and drop the `NEVER CAPTURED` line for that step (and only
for that step).

## 8. Why the fixtures are read from here at build time

`crates/mem-bridge/tests/host_capture_e2e.rs` `include_str!`s the two `*.sample.json` files through
`CARGO_MANIFEST_DIR`, so one spelling exists for each body. `crates/mem-bridge` already includes `plugin/…`
from outside its crate root (`tests/capture_wrapper_payload.rs:27`), so this is a pre-existing build
condition, not a new one.
