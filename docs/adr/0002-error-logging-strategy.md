# ADR-0002: Error & Logging Strategy

**Status:** Accepted
**Date:** 2026-10-18
**Context source:** issue 02 (W0.2 — config/jsonc + error & logging strategy) + roadmap Rev-2 §W0.2, §0b

## Context

Wave 0.1 (ADR-0001) split the port into eight `mem-*` crates with a frozen dependency
graph and zero external dependencies. This wave makes `mem-core` the functional shared
kernel every later wave builds on: JSONC config loading, the repo-wide typed error
tree, and the `tracing`-based logging strategy. It also introduces the repo's first
external dependencies (serde/serde_json/jsonc-parser/serde_path_to_error/thiserror/
tracing/tracing-subscriber/chrono) via `[workspace.dependencies]`.

## Decisions

### (a) Errors — typed, per-crate, no anyhow

- Every crate seam returns `Result<_, ConfigError/LoggingError/Error>`; the single
  cross-seam type is `mem_core::error::Error` (`Result<T, E = Error>` alias). No
  stringly-typed error payloads (`String`/`Box<dyn Error>` are banned from public
  signatures), no `anyhow` at any seam (revisit only if the composition root needs it).
- `ConfigError`/`LoggingError` are `#[non_exhaustive]` `thiserror` enums. Variant
  fields stay structured (callers branch on data, not on `Display` strings);
  `Display` mirrors the upstream user-facing messages verbatim where they exist
  (e.g. `ProjectRemoteProviderFields`, secret-resolver errors, `Invalid {key}
  config: {value}`).
- Cross-crate conversion only via `#[from]`. The `Error::{Config,Logging}` variants
  use `#[error("{0}")]` Display delegation rather than thiserror's
  `#[error(transparent)]`, because `transparent` forwards `source()` past the
  `Error` hop and would break the guaranteed chain
  `Error → ConfigError → underlying cause`.
- Sources are always preserved (`#[source] serde_json::Error`, parser error,
  io errors) for programmatic inspection.
- `serde_path_to_error` covers shape errors produced by serde deserialization so
  validation errors name the offending key (exact field path, e.g.
  `webServerPort`).

### (b) Config

- `Config` is value-typed and passed explicitly — the upstream process-global
  mutable `CONFIG` is deliberately not ported; resolution seams (`ConfigPaths`,
  `EnvSource`) make `load` hermetic and injectable (tests never touch the process
  environment or the real home directory).
- Resolution chain: defaults < global file < project file < environment-derived
  values. Environment participates only as `env://`/`file://` secret refs named in
  file values and as fill-in fallbacks for unset keys (`OPENAI_API_KEY`,
  `ATLASCLOUD_API_KEY`) — an explicit file value is never overridden by an ambient
  env var, mirroring upstream's `??` nullish chains.
- JSONC parsing uses `jsonc-parser` with strict-JSON-plus-extras options
  (comments + trailing commas tolerated; missing commas, single quotes, loose
  property names, hex/unary-plus/leading-dot/non-finite numbers, and extended
  escapes rejected — drop-in for upstream `stripJsoncComments` + `JSON.parse`).
  The crate's default loose JSON5 options are intentionally not used.
- The full upstream schema (`OpenCodeMemConfig`) is transcribed, plus the new-tier
  threshold `episodic.consolidationAgeDays` (default 30) so later tuning needs no
  recompile. Unknown keys stay tolerated (no `deny_unknown_fields`) — forward
  compatibility.
- Project files are rejected when they carry any global-only remote-provider field
  (`embeddingApiUrl`, `embeddingApiKey`, `memoryProvider`, `memoryApiUrl`,
  `memoryApiKey`) by key PRESENCE (a `null` value still counts) — checked before
  any secret resolution so a project file can never trigger inheritance of global
  keys. `autoCleanupEnabled`/`autoCleanupRetentionDays` from project files are
  dropped before merge (global/defaults decide); this is load policy, not
  validation.
- Invalid candidates are skipped (unreadable/invalid/non-object), first parseable
  candidate wins, with a `tracing::warn!` for observability; only the merged,
  parseable object is validated, and validation failures are `Err` (key-named),
  never silent.
- Documented divergence class: serde shape errors are stricter than upstream's
  silent JS coercion for non-normalized keys, but they always name the offending
  key. Deliberate fail-fast; do not "fix" by loosening types.

### (c) Logging

- `tracing` is the repo-wide log facade; the file sink mirrors upstream
  `src/services/logger.ts`: env-overridden path (`OPENCODE_MEM_LOG_FILE`)
  otherwise `<home>/.opencode-mem/opencode-mem.log`, session banner
  `\n--- Session started: <local ISO ts> ---\n`, 5 MiB / mtime-date rotation,
  30-day archive cleanup, line format `[<local-ISO-timestamp>] <message>` (and
  `: <compact-json>` when data is attached), and an ENOENT re-init retry.
  Rotation/cleanup failures are swallowed (upstream parity); other write errors
  surface as `LoggingError::Io`.
- Upstream's free-function global logger maps to a cheap-clone `LogSink` handle;
  the composition binary owns the handle and installs it once via
  `init_logging` (`registry()` + `LevelFilter` + `SinkLayer`).
- Later waves log through `mem_core::logging::log(message, data)` — an `INFO`
  tracing event with a `message` field and, when data is attached, a `data_json`
  field — rendered by `SinkLayer` in the same format as direct sink writes. Lines
  never carry log levels; `LevelFilter` controls capture only.
- `chrono` (clock/std only) is used solely by `mem-core::logging` because local-time
  timestamps cannot be produced safely with std or `time`'s local-offset API in
  multithreaded programs.

## Rejected alternatives

- Process-global mutable `CONFIG` (upstream) — unsound across threads, untestable
  without env mutation; replaced by value-typed `Config` + injection seams.
- Hand-rolled `stripJsoncComments` port — error-prone (comment-like text in
  strings); the parser crate with strict options is both safer and tested.
- `anyhow` at crate seams — stringly context chains lose typed variant structure.
- Loose JSON5 parsing (crate default options) — accepts configs upstream's
  `JSON.parse` would reject (single quotes, bare numbers), breaking drop-in
  strictness.
- Logging without `tracing` — loses structured capture and level filtering while
  keeping the upstream file format.

## Consequences

- Every later wave inherits these seams: config values arrive validated and
  typed; errors compose into a single cross-crate type; logging is a one-call
  facade. External-dependency surface grows only through `[workspace.dependencies]`
  with recorded justifications.
