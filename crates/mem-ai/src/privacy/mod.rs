//! `mem-ai::privacy` — the single seam guaranteeing that nothing persisted or
//! sent to an embedding/LLM API passes unfiltered.
//!
//! Four independent pieces plus the middleware that deploys them:
//!
//! - [`tags`] — the `<private>` tag stripper ([`strip_private_content`],
//!   [`is_fully_private`], [`REDACTED`]): a hand-written cursor/depth byte
//!   scanner. No `regex`, no pair matching.
//! - [`injection`] — the injected-prompt filter ([`contains_injection_marker`],
//!   [`is_injected_part`], [`filter_injected_parts`], [`InjectionFilter`],
//!   [`is_internal_prompt`]): drops host/plugin-injected text so it is never
//!   recorded as if the user had typed it.
//! - [`secret`] — the file-backed secret resolver ([`FileSecretResolver`]),
//!   which implements the W2.1 `SecretResolver` trait (it does *not* define a
//!   competing interface).
//! - [`layer`] — [`PrivacyFilterLayer`], an `AiProvider` middleware wrapping any
//!   provider with the three filters above.
//!
//! # Locked decisions and divergences (do not "fix" silently)
//!
//! - **D1 — `SecretString` vs. the locked W2.1 trait.** [`SecretString`] is the
//!   resolver's *own* API (zeroize-on-drop, redacting `Debug`); the
//!   `SecretResolver::resolve` impl unconceals it via `expose_secret()`, because
//!   the W2.1 trait returns `Option<String>` and is consumed by
//!   `provider::remote::auth`. Residual exposure: the `String` handed to
//!   `auth.rs` is a copy the HTTP header builder owns for the duration of the
//!   call.
//! - **D2 — permission violations REJECT, they do not warn.** Upstream calls
//!   `console.warn` and still returns the secret; the port returns `Err` with
//!   upstream's exact sentence (so ported substring assertions survive).
//!   Consequence, recorded rather than worked around: an
//!   owner-unreadable-but-group/other-exposed file (`0o060`, `0o006`, `0o007`,
//!   `0o077`) is denied at `open(O_RDONLY)` before the permission check can run,
//!   so it reports the open denial instead of the permission sentence. A
//!   diagnostic `stat` by path would reintroduce the TOCTOU the open-handle
//!   metadata removes.
//!
//!   **Accept-table re-partition (AC4).** Upstream's `SAFE_MODES` /
//!   `EXPOSED_MODES` are *mock* modes. Against real fixture files the accept
//!   table splits three ways: (a) readable owner-only modes (`0600`/`0400`/
//!   `0700`) are `Ok`; (b) owner-readable exposed modes
//!   (`0644`/`0640`/`0604`/`0606`/`0404`/`0777`) are rejected with the
//!   permission sentence; (c) owner-*unreadable* modes (`0000`/`0200`/`0060`/
//!   `0006`/`0007`/`0077`) are proved twice — by the pure
//!   [`secret::group_or_other_bits`] predicate and by a real read-denied `Err`
//!   whose message names the path only. The decision tests that need mode-based
//!   denial print `SKIP: process bypasses file modes` when the process runs with
//!   root / `CAP_DAC_OVERRIDE` semantics; the pure-predicate tests run
//!   everywhere.
//!
//!   **Bounded read.** The size cap is enforced by
//!   `Read::take(MAX_SECRET_BYTES + 1)` plus an explicit overflow `Err`, never a
//!   `metadata.len()` pre-check: `len()` is advisory (a file can grow between
//!   the stat and the read — the same TOCTOU class the open-handle metadata
//!   removes), while the take-bound caps allocation at `MAX_SECRET_BYTES + 1`
//!   bytes regardless. A file of exactly `MAX_SECRET_BYTES` is accepted.
//!   Symlinks are followed (upstream `statSync` parity), so the target's mode is
//!   what is enforced; `O_NOFOLLOW` is a recorded non-goal.
//!   A secret file is one-shot per auth resolution — blocking IO in the async
//!   trait method is deliberate and bounded, and per-call re-read caching is a
//!   recorded non-goal.
//! - **D3 — a missing env var is `Ok(None)`, not a throw.** The W2.1 trait
//!   defines `Ok(None)` as "simply not configured"; file-permission and
//!   file-read failures remain hard `Err`s. An explicitly configured but
//!   empty/whitespace-only value is a *shape* `Err`, not "not configured".
//! - **D4 — no `regex`; explicit tag-whitespace set.** The predicate is
//!   `char::is_whitespace() || c == '\u{FEFF}'`. Two recorded parity boundaries
//!   against JS `\s` (upstream's `/<(\/?)private\s*>/gi`):
//!   - **U+FEFF (ZWNBSP) is recognized.** `'\u{FEFF}'.is_whitespace()` is
//!     `false` in Rust but `\s` matches it in JS, so bare `char::is_whitespace`
//!     would leave `<private\u{FEFF}>secret</private>` unredacted — a leak in
//!     the direction this wave forbids.
//!   - **U+0085 (NEL) is accepted while JS `\s` rejects it.**
//!     `'\u{0085}'.is_whitespace()` is `true` in Rust; upstream leaves
//!     `<private\u{0085}>x</private\u{0085}>` verbatim. Accepted and documented
//!     because it errs *toward* redaction (fail-closed).
//!   - Every other code point agrees in both directions.
//! - **D5 — whole-string scanner; no chunk-incremental API.** A
//!   `push(chunk)/finish()` API is deferred: upstream has none, no ported
//!   behavior needs it, the only persistence path stores whole strings (W3.1),
//!   and chunked tag-splitting would be a *new* leak surface.
//! - **D6 — "allowlist" = internal prompts are exempt from the injection
//!   blocklist.** [`is_internal_prompt`] is the faithful port of upstream
//!   `isStructuredSummaryPromptMessage`. Phase 5's `plugin-host-config` wave must
//!   *import* this predicate rather than port it a second time.
//! - **D7 — shape validation is malformed-content rejection, not a vendor
//!   allow-list.** Unknown key prefixes must stay acceptable (Atlas Cloud /
//!   OrcaRouter take arbitrary keys); the rule set is conservative (non-empty,
//!   no interior whitespace, no control characters, length ≥ 8, not a
//!   placeholder, not a JSON/PEM opener).
//! - **D8 — layer semantics: REQUEST-SIDE ONLY, plus failure/alignment.**
//!   Filtering is applied to what the layer *sends* (completion `messages` and
//!   `system`, embed inputs, `count_tokens` text, the structured request).
//!   Everything coming back — completion chunks, embed vectors, token counts,
//!   model lists, structured values — is forwarded **verbatim**. `complete`
//!   drops injected user messages and returns one `Err` chunk when the request
//!   *had* messages and all were dropped (never send an empty conversation);
//!   assistant/system messages are never dropped. `embed` **never drops inputs**
//!   (index alignment with the returned vectors is an invariant) — a
//!   fully-private input stays in place as `[REDACTED]`. `model`, `json_schema`,
//!   `temperature`, `max_tokens`, `timeout`, `cancellation` and `extra_params`
//!   pass through untouched.
//! - **D9 — config↔resolver wiring is out of scope.** This wave ships an
//!   injectable [`FileSecretResolver`]; the bootstrap wave that reads
//!   `memoryApiKey` from config calls
//!   `ProviderRegistry::new(Arc::new(FileSecretResolver::new(...)))`.
//!
//! # Deferred (recorded, not forgotten)
//!
//! D5 chunk-incremental stripper; D9 config→resolver wiring; per-call secret
//! re-read caching; a better message for owner-unreadable exposed modes; and
//! Phase 5 importing [`is_internal_prompt`] instead of re-porting it.

pub mod injection;
pub mod layer;
pub mod secret;
pub mod tags;

pub use injection::{
    DEFAULT_INJECTION_MARKERS, DEFAULT_INTERNAL_PROMPT_MARKERS, InjectionFilter,
    InjectionFilterable, MessagePart, contains_default_injection_marker, contains_injection_marker,
    filter_injected_parts, is_injected_part, is_internal_prompt, normalize_injection_markers,
};
pub use layer::{PrivacyConfig, PrivacyFilterLayer, PrivacyFilterProvider};
pub use secret::{
    FileSecretResolver, PermissionPolicy, SecretSource, SecretString, group_or_other_bits,
    validate_secret_shape,
};
pub use tags::{REDACTED, is_fully_private, is_tag_whitespace, strip_private_content};
