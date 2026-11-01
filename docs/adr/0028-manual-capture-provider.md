# 0028 — The manual auto-capture provider is wired, as the fallback to a named host

- **Status:** Proposed
- **Date:** 2026-11-01
- **Amended:** 2026-11-01 (same day) — precedence flipped to **host-first**; see
  [Amendment 1](#amendment-1--2026-11-01-same-day-host-first-owner-ruling). The first landing's
  manual-first rule is preserved there as history, not erased.
- **Builds on:** `docs/adr/0027-host-endpoint-discovery.md` (the OpenCode host transport),
  `docs/adr/0001-crate-boundaries.md` (the mem-bridge → mem-ai edge).
- **Owner ruling (original, 2026-11-01):** "a configured manual fallback wins; otherwise the OpenCode
  host path".
- **Owner ruling (amendment, 2026-11-01):** `opencode-go/beam-free` through the OpenCode host is the
  primary capture route; "LiteLLM is the fallback".

## Context

`opencode-mem.jsonc` documents a manual auto-capture provider: `memoryProvider`, `memoryModel`,
`memoryApiUrl`, `memoryApiKey` (literal, `env://…` or `file://…`, resolved at config load). Every
component existed — `MemoryProvider`, `ProviderConfig`, `ProviderRegistry`, and the seven adapters
(`OpenAiChatProvider` included, with `AiProviderStructured`) — but **no production code ever built a
provider from those fields**. `ProviderRegistry::new`/`create`/`resolve_with_fallback` had no call site
outside `mem-ai` tests, and `crates/mem-core/src/config/mod.rs` parsed the four fields into `Config`
where nothing read them. The engine then passed `provider: None`, so composition always dialled the
OpenCode host.

The owner's config sets **both** blocks (`opencodeProvider`/`opencodeModel` *and* the three manual
values), and the OpenCode host is 2.2.5 — its wire contract differs from the port's, so capture cannot
persist through it today (ADR-0027). The manual endpoint is an OpenAI-compatible LiteLLM URL, which the
existing adapter already speaks.

## Decisions

**D-28-1 — A NAMED OpenCode host wins; otherwise a COMPLETE manual block; otherwise the host.**
As amended (see Amendment 1), the ordering is:

1. **Host arm** when the operator explicitly named the host — `opencodeProvider` **or**
   `opencodeModel` present and non-blank. The `LazyHostProvider` is warmed once and handed to
   composition. The manual block is not even evaluated on this branch.
2. **Manual arm** only when the host was *not* named: a COMPLETE manual block is the capture
   provider. "Complete" is the config's own wording — the three values its comment calls "all 3
   lines": `memoryModel` + `memoryApiUrl` + `memoryApiKey`, or just `memoryApiKey` for the two preset
   providers (`atlas-cloud`, `orcarouter`) that fill endpoint and model themselves. This mirrors
   `get_auto_capture_provider_status`'s *manual* arm exactly.
3. **Claimed but unusable, host unnamed** → fail closed (D-28-2).
4. **Neither** → the host default, unchanged.

The inverted wording this decision carried at first landing ("a complete manual block wins") is
recorded in Amendment 1, together with the reason it was reversed.

**D-28-2 — A CLAIMED block fails closed with a NAME, never a silent switch.**
This applies **only on the unnamed-host branch** (D-28-1 step 3): naming the host already answers the
"which provider does the operator want" question, so a stale or half-filled manual block beside it
neither wins nor fails the engine closed.
"Claimed" = `memoryProvider` was **present in the config file** (`Config.memory_provider_claimed`,
carried from `FileConfig` — the resolved value always exists and defaults to `openai-chat`, so it
cannot answer this), OR at least one of `memoryModel`/`memoryApiUrl`/`memoryApiKey` is non-blank. A
claimed block that cannot be built — a required field missing, an obvious placeholder key
(`is_placeholder_api_key`), or the registry rejecting the config — yields
`reason: "memory_provider_unusable"` on the `Auto-capture provider unavailable` line and passes **no**
provider to composition, so the capture path reports `capture_provider_unavailable`. Quietly using
another provider would hide a typo in the endpoint the operator asked for. One reason string covers
the three inputs on purpose: they share one fix, and the line names the layer.

Naming `memoryProvider` alone (`"memoryProvider": "anthropic"`, no other field) is therefore a claim:
it fails closed. It previously fell back to the host silently, because only the three values were
consulted — the exact quiet fallback this decision exists to prevent.

**D-28-3 — The wiring now follows `get_auto_capture_provider_status`'s order.**
That function is a pinned port of upstream `getAutoCaptureProviderStatus` and answers "opencode branch
first" (`crates/mem-core/tests/provider_status.rs`); it is untouched and now agrees with the selector.
The first landing deliberately diverged from it (manual-first) and recorded the consequence — with
both blocks configured the status said `Ready { OpenCode }` while `openai-chat` was live. The
amendment removes that divergence: the status report and the live provider now name the same route.
The `Auto-capture provider ready` line's `providerId` remains the authoritative statement of which
provider is live, because only it is produced by the selector that actually feeds composition.

**The production path is `run_in_runtime` → `engine_capture_provider` → `build_composed_runtime`.**
The first landing of this ADR defined the selector but never called it from `run_in_runtime`, which kept
building, warming and passing the HOST provider: the manual block was dead code and the ready line named
the host — a mechanism that reads as satisfied but cannot fire. `engine.rs`'s `run_in_runtime` now passes
the selector's result, and `tests/engine_capture_provider.rs` spawns the built engine over a private
`HOME` to assert the ready line names the provider the selector chose. Since the amendment that row
carries **both** blocks and a private `service.json`, and asserts `providerId: "opencode-go"` with no
`openai-chat` ready line — the owner's live shape.

**D-28-4 — The key rides as a per-call bearer header, never on the struct.**
`Config.memory_api_key` holds the **resolved** secret. It is seated in a `StaticSecretResolver` under
the synthetic reference `memoryApiKey`, and `RemoteProviderConfig.api_key_ref` names that reference, so
the adapter resolves it per request through the existing `AuthStrategy::Bearer` path. The secret is
never stored on the provider, never logged, never formatted (`RemoteProviderConfig`'s `Debug` shows the
reference name only).

**D-28-5 — A transport seam for the test, not for production.**
`ProviderRegistry::register_builtin_with(Arc<dyn HttpTransport>)` registers the same seven real factories
over an injected transport; `register_builtin()` is the production spelling (`reqwest`). This is what
lets a test drive config → `ProviderConfig` → real adapter → recording transport and assert the endpoint
and the `Authorization: Bearer` header **without touching the network**. `RemoteBuild` became a boxed
closure to carry the transport.

**D-28-6 — A cleartext non-loopback endpoint is a NAMED warning, not a refusal (mandate 5).**
The key is materialised as `Authorization: Bearer <secret>`. Over `http://` to a NON-loopback host any
observer on the network path can read it and impersonate the owner. The owner's live endpoint
(`http://10.90.11.16:4000/v1`, a private-LAN LiteLLM proxy) is exactly that case, so refusing it would
have disabled the one working capture path; the decision is to **report** it: one
`Auto-capture provider insecure endpoint` line per engine start, payload
`{ baseUrl, scheme: "http", hint }` with userinfo stripped (`AUTO_CAPTURE_PROVIDER_INSECURE_ENDPOINT`).

Rules:
- `https://` — allowed, no line.
- `http://` on a **loopback** host (`127.0.0.0/8`, `::1`, `localhost`) — allowed, no line: that is the
  documented local-provider case (Ollama-style endpoints), where the credential does not leave the box.
- `http://` on anything else, with a key attached — allowed **and warned**.
- An unparsable `memoryApiUrl` is not this predicate's business; the registry rejects what it cannot use.

**Residual risk, accepted and stated:** while that endpoint stays `http://`, the credential is exposed to
the LAN. Remediation is the owner's: terminate TLS on the proxy, or reach it through a loopback tunnel.
The follow-up (a hard refusal for non-loopback `http://` once no live config depends on it) is not
recorded here as done.

**D-28-7 — The ready line logs a SANITIZED endpoint.**
`memoryApiUrl` is operator input; an embedded `user:pass@` would otherwise be written to the sink
verbatim (the host arm logs a resolved origin and `HostAuthorization` is redacted). The manual arm logs
the parsed URL with userinfo cleared, keeping scheme/host/port/path (the diagnosable part, e.g. `/v1`).
An unparsable value is stripped textually rather than trusted. The key itself is never logged on any arm.

## Amendment 1 — 2026-11-01 (same day): host-first (owner ruling)

**What changed.** The selector's order was flipped: the **host arm runs first** when the operator
named it (`opencodeProvider` or `opencodeModel` set), and the manual block is the **fallback** it was
always documented as. Only the *selection* moved; `manual_provider.rs`'s validation, its redaction and
`sanitized_base_url` are untouched. Nothing else in D-28-2 … D-28-7 changed.

**Why.** The host path is the product's default: the wrapper exists to give OpenCode its memory, and
`opencode-go/beam-free` through the host is the route the owner actually wants captures to take. The
manual block exists for environments **without a usable host provider** (a LiteLLM-compatible proxy on
a box the OpenCode host cannot be reached from), not as an override of a host the operator named. With
both blocks filled in — the owner's live config — manual-first made `beam-free` unreachable and the
host transport dead code, which is the opposite of what naming `opencodeProvider`/`opencodeModel`
asks for.

**History of the first landing, kept deliberately.** The original rule recorded in this ADR was the
inverse: *"a COMPLETE manual block wins; otherwise the OpenCode host provider applies"*, on the
reasoning that "filling in the manual block is the operator naming an endpoint". That reasoning
measured the *config* and not the *product*: the host block was present in the same file and lost
silently. Reversal came from the owner, not from a defect report — the behaviour matched the recorded
decision, which is why the decision is what changed. The first landing's tests (manual-first rows) were
flipped in the same commit; the earlier wording is quoted above so the reversal stays legible.

**What the flip means for the arms.**
- A named host with a **complete** manual block → host arm; the manual block is never built (`memoryApiUrl`
  never reaches the sink) and its cleartext-endpoint warning never fires.
- A named host with a **claimed-but-unusable** manual block → host arm, not fail-closed: the manual
  block is the fallback and is not evaluated (contrast D-28-2, unnamed-host branch only).
- Host **not** named + complete manual block → manual arm, `providerId` = the manual provider.
- Host **not** named + claimed-but-unusable block → fail closed, `memory_provider_unusable`.
- Neither → the host default, unchanged.

**Not changed, and why it matters now.** The wrapper still forwards no `LITELLM_API_KEY`, so an
`env://` manual key that cannot resolve still fails `Config::load` and exits the engine 70 — even
though the host arm would win and never use that key. Config resolution is provider-agnostic and runs
before selection. Use the `file://` form (below) until the wrapper's child-environment policy changes.

**Startup cost, stated for the drain wave.** The awaited startup capture drain blocks on the provider
the selector chose, so host-first makes the **host** the provider a startup drain waits on. Measured
today (2026-11-01, 2.2.5 host): the host arm fails fast — a named `protocol` reason — rather than
hanging. The drain taking the whole memory tool down when a provider stalls is a separate defect and a
separate wave; this amendment neither fixes nor worsens its trigger beyond that provider swap.

## Known consequence: `env://` references need the variable in the ENGINE's process

The engine is a child of the wrapper, and the wrapper builds its environment from a fixed whitelist
(`plugin/src/plugin.js` `childEnvironment()`: `PATH`, `HOME`, `XDG_STATE_HOME`, `TMPDIR`, `LANG`,
`LC_ALL`, `OPENCODE_MEM_WEB_DIST`, `OPENCODE_MEM_ALLOW_UNGUARDED_OPEN`). `LITELLM_API_KEY` is **not**
forwarded, so with `"memoryApiKey": "env://LITELLM_API_KEY"` the engine cannot resolve it: `Config::load`
fails with `Environment variable not found: LITELLM_API_KEY` and the engine exits 70 — named, loud, and
before any capture runs (not a silent `None` provider).

Two routes, both outside this ADR's change:
1. **No code change:** use a file reference — `"memoryApiKey": "file://~/.config/litellm-key.txt"` — the
   documented alternative, which the config loader already supports.
2. A wrapper allowlist knob (e.g. `OPENCODE_MEM_CHILD_ENV=LITELLM_API_KEY`) — a deliberate widening of
   the child environment's secret policy, which is why it is not smuggled into this wiring commit.

## Known consequence: a `file://` key must be created `0600` — nothing enforces it

`resolve_secret_value` (`crates/mem-core/src/config/normalize.rs`) reads `file://` (with `~/` expansion)
and trims the content, but performs **no permission check**: a `0644` key file under a default umask is
accepted silently. The requirement is the operator's:

```sh
install -m 600 /dev/stdin ~/.config/litellm-key.txt   # then paste the key, Ctrl-D
# or: umask 077 && printf '%s' "$KEY" > ~/.config/litellm-key.txt
```

Follow-up (not this commit): `resolve_secret_value` should refuse a `file://` secret that is group- or
world-readable (mandate 4/10), naming the path and the mode.

## Known consequence: a `dummy` key proves WIRING only

`"dummy"` is **not** in `PLACEHOLDER_API_KEYS` (`crates/mem-core/src/config/mod.rs`), so
`LITELLM_API_KEY=dummy` builds a live provider that sends `Authorization: Bearer dummy`. The proxy
normally answers `401`: the wiring row is then green (`providerId: openai-chat`, a named provider failure
in the log) while no `Auto-capture memory persisted` line can appear. Only a REAL key can produce a
persisted capture — that is the acceptance signal, and only the owner can observe it.

## Verification

- `crates/mem-bridge/tests/memory_provider.rs` (as amended): a NAMED host beats a complete manual block
  beside it, logging `providerId: "opencode-go"` and never the manual endpoint; a named host with only a
  claimed-but-unusable manual block is still the host arm; the host **not** named plus a complete manual
  block gives `providerId: "openai-chat"`; the host not named plus a partial block fails closed with
  `memory_provider_unusable`; a `memoryProvider`-only claim fails closed the same way; neither block
  gives the host; the configured `memoryApiUrl` receives the completion with `Authorization: Bearer
  <key>` and the resolved key never reaches the log; the ready line strips URL userinfo; a cleartext
  non-loopback endpoint emits exactly that named line while the loopback/`https` predicate rows stay
  silent; a missing `env://` reference names the variable; every `memoryProvider` value maps to its
  adapter.
- `tests/engine_capture_provider.rs` (root package — `CARGO_BIN_EXE_…` is only defined where the binary
  is declared): a SPAWNED engine over a private `HOME` whose config carries **both** blocks (the owner's
  live shape) and a private `service.json` logs exactly one ready line with `providerId: "opencode-go"`,
  `hostVersion` and the resolved `…/api` base, no `openai-chat` ready line, the manual endpoint never in
  the sink, no `memory_provider_unusable`, no key, and exits 0 on stdin EOF. This is the row that pins
  the production call site; it failed against the pre-flip tree by logging
  `providerId: "openai-chat"` from a spawned engine.
- Mutation checks, this wave: restoring manual-first fails the two host-named unit rows and the spawned
  row; `host_is_named → false` fails the same two unit rows; removing the fail-closed arm fails both
  unnamed-host claim rows; never building the manual arm fails the fallback row and the userinfo row;
  returning `None` when neither block is set fails the neither row.
- **The end-to-end proof is the first live `Auto-capture memory persisted` line** — only the owner can
  observe it, and only once the engine is restaged on a host build whose wire contract this port speaks
  (ADR-0027) — the 2.2.5 host still fails a capture with a named `protocol` reason.
