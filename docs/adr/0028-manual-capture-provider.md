# 0028 — The manual auto-capture provider is wired, and it wins when complete

- **Status:** Proposed
- **Date:** 2026-11-01
- **Builds on:** `docs/adr/0027-host-endpoint-discovery.md` (the OpenCode host transport),
  `docs/adr/0001-crate-boundaries.md` (the mem-bridge → mem-ai edge).
- **Owner ruling:** 2026-11-01 — "a configured manual fallback wins; otherwise the OpenCode host path".

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

**D-28-1 — A COMPLETE manual block wins; otherwise the OpenCode host provider applies.**
"Complete" is the config's own wording — the three values its comment calls "all 3 lines":
`memoryModel` + `memoryApiUrl` + `memoryApiKey`, or just `memoryApiKey` for the two preset providers
(`atlas-cloud`, `orcarouter`) that fill endpoint and model themselves. This mirrors
`get_auto_capture_provider_status`'s *manual* arm exactly; only the precedence differs (D-28-3).

**D-28-2 — A CLAIMED block fails closed with a NAME, never a silent switch.**
"Claimed" = at least one of the three values is non-blank (`memoryProvider` alone cannot say it: it is
not an `Option` in `Config` and defaults to `openai-chat`). A claimed block that cannot be built — a
required field missing, an obvious placeholder key (`is_placeholder_api_key`), or the registry rejecting
the config — yields `reason: "memory_provider_unusable"` on the `Auto-capture provider unavailable` line
and passes **no** provider to composition, so the capture path reports
`capture_provider_unavailable`. Quietly using another provider would hide a typo in the endpoint the
operator asked for. One reason string covers the three inputs on purpose: they share one fix, and the
line names the layer.

**D-28-3 — Deliberate divergence from `get_auto_capture_provider_status`'s order.**
That function is a pinned port of upstream `getAutoCaptureProviderStatus` and answers "opencode branch
first" (`crates/mem-core/tests/provider_status.rs`). It stays untouched. The **wiring** is manual-first
per the owner ruling, because filling in the manual block is the operator naming an endpoint. Consequence
recorded: with both blocks configured, the status report says `Ready { OpenCode }` while the capture
provider in use is `openai-chat`. The `Auto-capture provider ready` line's `providerId` is the
authoritative statement of which one is live; aligning or retiring the status function is a follow-up,
not a silent change to a ported contract.

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

## Verification

- `crates/mem-bridge/tests/memory_provider.rs`: manual wins over a configured host block; unconfigured
  falls back to the host; a partial block fails closed with `memory_provider_unusable`; the configured
  `memoryApiUrl` receives the completion with `Authorization: Bearer <key>` and the resolved key never
  reaches the log; a missing `env://` reference names the variable; every `memoryProvider` value maps to
  its adapter.
- Mutation checks recorded in the handoff: forcing the selector to `None` (host-only) fails 2 rows;
  dropping `api_key_ref` fails the endpoint/credential row.
- **The end-to-end proof is the first live `Auto-capture memory persisted` line** — only the owner can
  observe it, and only once the key is reachable in the engine's process (see above).
