# 0027 — Host endpoint discovery: the live origin, `{origin}/api`, basic auth, and named failures

- **Status:** Proposed
- **Date:** 2026-11-01
- **Builds on:** `docs/adr/0007-bridge-strategy.md` (D-43 wire versioning), `docs/adr/0013-production-error-handling-panic-free.md`
  (fail closed, never panic), `docs/adr/0014-stats-log-signature-emission.md` (log signatures),
  `docs/adr/0021-schema-version-discipline.md` (a newer root refuses to open).
- **Blueprint:** `blueprint-20261101-0322-host-transport-r7` (sha256 `80d79b2a2446`, `SYS-APPROVED 2026-11-01T10:26Z`),
  after seven gate revisions (`r1`…`r7`).
- **Amended:** 2026-11-01 (issue 88, the wire wave) — D-27-9 row 1 is DECODED (the `{"data": Session.Info}`
  envelope, with the bare `{"id": …}` as a permanent `#[serde(untagged)]` fall-through, so a pre-2.2.5 host
  still dials); rows 3–4 land (`POST …/generate`, body `{"prompt"}`, reply `{"data":{"text"}}`, and
  `format`/`tools`/`parts` are gone — no code path POSTs `…/message`); row 2 **partially** lands —
  `model: Model.Ref{id, providerID, variant?}` and `agent: Agent.ID` now travel on the create body, while
  `permission` is untouched and still a rebuild; handoff items 1 (decode) · 2 · 3 · 4 land, 1's create-body
  rebuild and 5 (`requeue-captures`) do not. The JSON schema no longer travels as a `format` field: it travels
  in the prompt TEXT (`capture_context::SCHEMA_ANSWER_INSTRUCTION`). `note_structured_step` had no transport
  caller before this wave and still has none.

## Context

Auto-capture persisted nothing after the 2026-10-31 host swap: last success 16:04:39, the swap at 16:26:40,
the first dead-letter at 16:39:12, and ~130 dead rows with zero persisted afterwards. The cause was not the
wire contract but the **transport layer**: the composition root passed `provider: None`
(`crates/mem-bridge/src/engine.rs`), which built an `OpenCodeHostProvider` over
`OpenCodeHostConfig::default()` — `http://127.0.0.1:4096` — while the live host listens on an ephemeral
port recorded in `~/.local/state/opencode/service.json` and requires HTTP basic auth on `{origin}/api`.
The failure was invisible because the retry-exhausted dead-letter path logged no reason.

This ADR records the transport half only. It does **not** restore capture.

## Grounds

### Grounds 1 — the origin is neither fixed nor guessable

`opencode serve` records `{id, url, pid, password, version}` in a `0600` file under
`$XDG_STATE_HOME/opencode/` (measured: `{"version":"2.2.5","url":"http://127.0.0.1:49374","pid":…}`).
The file's mtime equals the host's `lstart` second, so a startup-only read can see the **previous** run's
dead pid. A compiled-in port is therefore always wrong eventually: the process that owns the old port
after a restart may be a stranger (measured second loopback listener at `127.0.0.1:8384`).

### Grounds 2 — three carrier options, one survives secret handling

- **A — the `0600` service file.** The password never enters argv (`/proc/<pid>/cmdline`) or the
  environment (`/proc/<pid>/environ`, and the wrapper's child env is a fixed whitelist).
- **B — env/argv.** Rejected: both are readable by any same-UID process and both appear in crash dumps
  and process listings.
- **C — an operator `opencodeBaseUrl` override.** Accepted **with** the file, as the escape hatch for a
  host whose record is absent or wrong.

### Grounds 3 — `/api`, not the root

Measured: root paths serve the SPA; `/api/session` unauthenticated answers `401`; bearer tokens are
rejected while basic auth with the file's password is accepted. There is no root-API fallback: an
unverifiable fallback would mask the next drift behind a plausible-looking 200.

### Grounds 4 — `GET /api/config` is an array of documents

Measured (`2.2.5`): the response is `Config.Entry[]` — a top-level array whose document entries carry
`type: "document"` and an `info.providers` **object** keyed by provider id, each with `models` an
**object** keyed by model id, and some entries carrying only `{"disabled":…}`. The previous reader looked
for `value.get("providers")` and returned nothing, which silently disconnected the connected-provider
gate.

## Decisions

**D-27-1 — Two disjoint resolution branches; the override makes the file credential-only.**
Branch **A**: a non-blank `opencodeBaseUrl` (trimmed; blank is unset) is the authority. It is validated
(loopback, explicit port, supported path) and the service file is consulted for **nothing but credential
binding**: no resolvable state dir or no file ⇒ `Ok` with no credentials; unreadable/malformed/URL errors
propagate by name; a valid file contributes credentials **only when its origin equals the override's
origin**. No liveness or recycle probe runs on branch A. Branch **B**: the file is the authority —
state dir → file → pid liveness → pid recycling → URL shape → credentials → model ref. Written as
`if/else`, never as fall-through. Neither branch logs a version on A: the file there describes whatever
host wrote it, not necessarily the override's.

**D-27-2 — One read+parse helper, wrap-at-parse.**
Both branches call `read_service_file`, the only place the password is touched: read the text into
`Zeroizing<String>` → deserialize into a `Deserialize`-only wire struct (never `Debug`, never through
`serde_json::Value`, unknown keys ignored) → **immediately** wrap the password in `Zeroizing` → only then
the `pid != 0`/`u32` and `Url::parse` validations. Every rule that can return `Err` after the parse
therefore already holds a zeroizing secret, on either branch. Accepted residuals, stated not claimed
away: `reqwest`'s `HeaderValue` copy lives for the request, and serde's transient buffer on a **malformed**
body is outside our reach. `HostAuthorization`'s `Debug` is manual and prints `<redacted>`; there is
deliberately no `Display`.

**D-27-3 — `{origin}/api` is the base; the provider appends `/session` and `/config`.**
A path policy shared by both branches accepts only `""`, `/`, `api`, `/api`; `api` is never doubled. A
violation is `host_override_invalid` on branch A (the operator named an unusable base) and
`host_url_unsupported` on branch B (the host wrote an unusable origin). A 404 on a real host surfaces as
`errorKind: "http-status", status: 404`.

**D-27-4 — Lazy resolution, cache on success, clear on ANY inner error.**
`LazyHostProvider` resolves on first use because the service file is rewritten at host start. `Network`/
`Timeout` alone is too narrow: after a restart the old port can be rebound by a foreign process, so the
request returns a real `401`/`404`/junk-200 and the cache would pin a stranger's origin for the engine's
life. Any `Err` chunk from the inner provider clears the cache; the next capture re-resolves. Accepted
residual, recorded and not fixed: the **first** attempt after a recycle is still named by what the
stranger answered (`auth`/`http-status`/`protocol`); the correct name appears on the next attempt.
`warm()` is the startup probe: it resolves once, caches on success and never caches a failure.

**D-27-5 — The security boundary is same-UID, and the URL must be loopback.**
A measured second loopback listener means loopback is not identity: a forged or stale `service.json`
naming a live same-UID loopback origin is accepted, with prompt content as the exposure. Do not claim
"nothing leaves the machine". Shape rules: scheme `http|https`; empty userinfo (a userinfo URL is an
identity smuggle ⇒ `host_url_not_loopback`); no query/fragment; `host_str()` ∈ {`127.0.0.1`, `::1`,
`localhost`}; explicit port.

**D-27-6 — Pid liveness, then start-time ordering.**
A recorded pid that is not alive ⇒ `host_pid_dead`. The host starts **before** the engine the plugin
spawns, so a recorded start time **later** than the engine's is provably recycled ⇒ `host_pid_recycled`.
Either side unobservable (non-Linux, or a process that vanished mid-read) ⇒ accept. Known false
positive, accepted as conservative: an **orphan engine** (one that outlived its host) rejects the new
host's pid until the engine restarts.

**D-27-7 — Fail closed with a NAME: ten host reasons, plus the capture reasons.**
Host resolution (each with exactly one pinning input; the table is a test):

| Reason | Branch | Pinning input |
| --- | --- | --- |
| `host_override_invalid` | A | override `http://127.0.0.1:5000/not-api` |
| `state_dir_unknown` | B | env with neither `XDG_STATE_HOME` nor `HOME`/`USERPROFILE`, no override |
| `service_file_missing` | B | valid state dir, no `service.json` |
| `service_file_unreadable` | A and B | a directory at `…/opencode/service.json` |
| `service_file_malformed` | A and B | `not json`; no `pid`; `pid: 0`; no `password` |
| `host_pid_dead` | B | `pid: 4242`, probe answers dead |
| `host_pid_recycled` | B | recorded start 900, engine start 800 |
| `host_url_unsupported` | A and B | `file:///tmp/x`; `ftp://…`; `/session`; a file-branch `/session` path |
| `host_url_not_loopback` | A and B | `http://192.168.1.10:49374`; `http://127.0.0.1:49374@evil.example:80` |
| `host_model_unresolved` | A and B | model unset or the literal `"inherit"` |

Capture: `capture_provider_unavailable`, `summarizer_failed`, `host_auth_rejected`, `host_wire_shape_mismatch`,
`scope_unresolved`, `episodic_write_failed`, `tier1_write_failed`, `summary_missing`. `summarizer_failed` is
the only default; a provider error adds `errorKind` (`auth`, `network`, `timeout`, `http-status`, `protocol`,
`wire-shape`, `config`…), and a provider-side JSON/parse failure adds none — the summarizer's record is
cleared on entry to every `summarize`, so a stale reason can never name a later attempt. `wire-shape` means
the host answered a shape the port cannot read and is never the model's fault; `protocol` covers a reply
whose TEXT was not JSON. The set grows by APPENDING tokens, never by re-counting a numeral in five places.

**D-27-8 — `opencodeModel: "inherit"` is dropped; the wiring supplies the model.**
`OpenCodeHostProvider` ignores `CompletionRequest.model` and names its own `model_ref()` — since the wire
wave, in the create body's `model` field (`Model.Ref{id, providerID, variant?}`), with `agent` naming the
structured-output agent registered in `plugin/src/plugin.js:924-933`. An unset or `"inherit"` model fails
closed as `host_model_unresolved` instead of forwarding a literal the host would reject opaquely. Resolution
is the LAST step of both branches, so the error names the model, not a host that in fact resolved.

**D-27-9 — Transport and wire are two waves.**
The host is `2.2.5` and, as this ADR was written, the port spoke the pre-2.2.5 HTTP contract in four
**documented** places:

| Break | Label |
| --- | --- |
| `POST /api/session` 200 is `{data: Session.Info}` (`required: ["data"]`, `additionalProperties:false`), so the port's `value.get("id")` was always `None` ⇒ a named `Protocol` error. **Decoded by the wire wave** (envelope first, bare `{"id": …}` as the fall-through) | READ, `docs/evidence/issue-88-host-wire-contract-20261101/` |
| The create body accepts `permissions: Ruleset` and `model: Model.Ref{id,providerID,variant?}`; the port sent `permission` and no model, so a `400 InvalidRequestError` FOLLOWS from the strict schema. **The model/agent half landed in the wire wave** (`model: Model.Ref`, `agent: Agent.ID` on the create body); `permission` is untouched, so an undeclared key still rides the create. A 2xx was **MEASURED** for a create body carrying it: `additionalProperties:false` is documentation, not observed rejection | **INFERRED** — the 400 was never observed; the tolerated 2xx was |
| `POST /api/session/{id}/message` does not exist (GET only); prompt is `POST /api/session/{id}/generate` → `{data:{text}}`. **Landed in the wire wave** | READ |
| `StructuredOutput`/`json_schema` occur **0×** in the 440 KB spec. **Landed**: the schema travels in the prompt TEXT. `format`/`tools`/`parts` left the wire; the assistant-side error name/data is a recorded loss | READ |

Shipping transport alone therefore yielded a **named** `protocol` (or `http-status/400`) dead-letter at
`POST /api/session` — the correct, diagnosable outcome. **A green transport wave did not mean capture
works.** No model was added to the create body in that wave: that was the create-body rebuild, and it
would have flipped a deliberately-pinned fixture row.

## The wire-wave handoff (the next blueprint owns all of this)

1. Decode `{data: Session.Info}` (`data.id`) and rebuild the create body on 2.2.5's shape
   (`permissions: [{action, resource, effect}]`, `model: Model.Ref{id, providerID, variant?}`); the first
   live POST settles the INFERRED 400. — **Half landed** (issue 88): the decode and `model`/`agent` are on
   the wire; `permissions` is still the undeclared shape, so the create-body rebuild remains open.
2. Replace `POST …/message` with `…/generate` (`{data:{text}}`) or `…/experimental/generate/text`. —
   **Landed** on `…/generate`.
3. Drop `format`/`tools`/`parts`; rely on text + the existing `parse_capture_output` fallback. —
   **Landed**, with one correction: the schema is not merely dropped, it is appended to the prompt text,
   so the answer is still schema-bound.
4. Rebuild `crates/mem-ai/tests/provider/opencode_provider.rs`'s fixtures from `.tmp/openapi.json` and
   delete `test_2_2_5_session_create_response_is_a_named_protocol_error`. — **Landed**; the row's
   still-true claims became `test_2_2_5_create_envelope_yields_the_prompt_call`, and the samples live in
   `docs/evidence/issue-88-host-wire-contract-20261101/`.
5. Ship `requeue-captures` (`episodic_sql`/`CaptureInbox`/`src/main.rs`/`tests/requeue_cli.rs`/
   `tests/workspace_smoke.rs`). **Dropped from this wave on purpose:** the CLI is useless until capture
   can reach the host, so it belongs with the wire contract. — **NOT landed**: still the next wave.
6. Optional: `opencodeModel: "inherit"` via `Config.InfoEncoded.model` (the document array names the
   default as `"provider/model#variant"` or `{providerID, model, variant}`). — **Not done** (unchanged).
7. Correct this ADR's direction if the port's fixtures were ever read as "current": the port is the
   **older** shape; 2.2.5 is current. — **Done** by the amendment header above.

## Live verification (owner runbook)

Sink `~/.opencode-mem/opencode-mem.log`; dated archives appear only after rotation, named from the file's
**local** mtime date (`opencode-mem-<date>.log`, and rotation may also leave `opencode-mem.log.old`) —
never grep `opencode-mem-$(date -u +%F).log`.

1. Baseline first (the counts drift): `sqlite3 -readonly` over a **copy** of `episodic.db` — never open
   the live DB, that writes `-shm` into the live root — plus `grep -c` of the four counters.
2. Restage the engine and wrapper, quit OpenCode (the engine lock must free), start OpenCode.
3. `grep "Auto-capture provider ready" "$L" | tail -1` ⇒ `"baseUrl":"http://127.0.0.1:<port>/api"` and
   `"hostVersion":"2.2.5"`. The version is now in the log rather than inferred. A new
   `Auto-capture provider unavailable` line names the resolution layer instead.
4. One real chat turn, then `grep -c "Auto-capture provider failed"` (baseline + 1..5) and read the
   reason/errorKind/status from `tail -n 12`.
5. `errorKind:"wire-shape"` ⇒ the host answered a shape the port cannot read (a new host minor, not a
   model problem); `errorKind:"protocol"` ⇒ the reply TEXT was not JSON (the model's miss);
   `http-status`/400 ⇒ the create body; `http-status`/404|405 ⇒ the route moved; `auth`/401 ⇒ credentials
   **or** a stale origin rebound after a restart (the first attempt after a recycle is misnamed — check
   `service.json` against `ss -ltn` before touching credentials); `network`/`timeout` ⇒ the resolved origin
   is not the live one. **Calibration:** the API description's `additionalProperties:false` is
   documentation, not runtime behaviour — a 2xx was MEASURED for a create body carrying an undeclared key,
   so "the spec forbids it" is not evidence that the host rejected anything.
6. Drain arithmetic: `drain_once` runs **32 rows at startup** and **32 per enqueue** — there is no startup
   loop. The queued rows at attempts 1-4 die over the next 1-5 failed drains, each carrying a named
   reason; the `dead` backlog stays until the wire wave's `requeue-captures` (32 rows per drain).
7. **Inverted world that closes this wave as not working:** no `Auto-capture provider ready` line at all,
   or a new `Auto-capture final error` line without `reason`/`errorKind`. A `wire-shape` outcome is a NAMED
   mismatch (the diagnosable outcome), not a failure of the transport layer; a capture that persists is the
   wire wave's acceptance, and unmeasured until one is observed.

## Consequences

- `crates/mem-ai` gains `base64`; `crates/mem-bridge` gains `url` and `zeroize` (both already workspace
  dependencies). `mem-bridge` still takes **no** `tokio-util`.
- `OpenCodeHostConfig::default().base_url` is now the empty string — not dialable. Production always
  passes a resolved `{origin}/api`; the registry factory still rejects an empty base.
- Composition no longer fabricates a provider: with `provider: None` the capture path fails closed with
  `capture_provider_unavailable`, and consolidation falls back to `UnavailableMilestoneSummarizer`. That
  arm is the tests/degraded-composition path; production passes the lazy provider.
- Three new log lines (`Auto-capture provider ready` / `unavailable` / `failed`), deliberately outside the
  web's `NAMED_ERROR_BUCKETS`: the stats classifier matches bucket/counter prefixes and none of these
  starts with one, so `/api/stats` is unchanged.
- The wrapper forwards `XDG_STATE_HOME`; without it the child would resolve `$HOME/.local/state` and dial
  a stale origin.
- The retry ladder still treats a wiring outage as terminal (5 drains ⇒ dead); clear-on-any-error means
  each failed attempt re-resolves once, so a transient outage recovers on the next capture.
