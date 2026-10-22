# mem-core

**Purpose:** shared kernel: memory scopes, tags, config loading, typed errors, shared utilities.

**Ubiquitous language:**
- **Config resolution chain** — defaults < global file (`~/.config/opencode/opencode-mem.jsonc`/`.json`, first parseable wins) < project file (`<project>/.opencode/opencode-mem.jsonc`/`.json`) < environment-derived values; loaded through injected `ConfigPaths` + `EnvSource` seams.
- **Global-only fields** — `embeddingApiUrl`, `embeddingApiKey`, `memoryProvider`, `memoryApiUrl`, `memoryApiKey` (rejected in project configs by key presence, before secret resolution) and the dropped project keys `autoCleanupEnabled`, `autoCleanupRetentionDays`.
- **Typed error tree** — `Error` (crate seams) wrapping `ConfigError`/`LoggingError` `#[non_exhaustive]` variants; validation errors always name the offending key.
- **Log sink** — cheap-clone handle over the file direction of the mirrored upstream logger: session banner, 5 MiB/daily rotation, 30-day cleanup; `tracing` facade with the `SinkLayer` rendering `[<ts>] <message>[: <json>]` lines without log levels.
- **Provider status** — `AutoCaptureProviderStatus` (`Ready(mode: opencode|manual)` / `NotReady(issues)`) computed from opencode provider/model + memory provider/endpoint/key settings; placeholder API keys never count as configured.
- **Context budget truncation** — `utils::context_limit`: UTF-8 byte budget; unchanged when under budget; budget ≤ marker → head slice only; else `floor((budget − marker)/2)`-byte head + `\n[... truncated ...]\n` marker + remaining tail; slicing never splits multi-byte characters; branch order is the upstream contract (roadmap R9).
- **Project name extraction** — `utils::paths::get_project_name`: last segment after separator normalization (`\`→`/`) and lexical `.`/`..` collapse; falls back to the raw input; `normalize_separators` is the shared cross-platform idiom.
- **Similarity math** — `utils::math`: cosine similarity with f64 accumulation (length mismatch or zero magnitude → 0.0); L2 normalization returns zero-norm inputs unchanged.
- **Scope hash & container tag** — `scope`: a container tag is `{prefix}_{user|project}_{16hex}`; the hash is the first 16 hex characters of SHA-256 over the identity string (`sha256::hex(...)[..16]`); `ScopeHash` is a newtype whose invariant (16 lowercase hex) makes traversal segments structurally unrepresentable; `ScopeRef.hash = None` means "every shard of that scope" (upstream's empty string) and must never become a filesystem path segment.
- **Scope resolution** — `resolve_memory_scope`: `all-projects` spans both scopes as `user`-then-`project` wildcard refs (user-scope memories must not be silently excluded); `project` resolves the container tag's own scope+hash. `resolve_scope_selection`: an explicit request scope beats the resolved config default (`memory.defaultScope`, default `project`).
- **Project identity & container tags** — `tags`: identity precedence `path:{marker root}` (`.opencode-mem-project`) > `git-common:{git common dir}` (worktrees share one identity) > `remote:{origin url}` > `path:{directory}`; marker presence drops `git_repo_url`; container tags are `{prefix}_project_{hash}` / `{prefix}_user_{hash}`; user fallback chain name → `USER` → `USERNAME` → `anonymous`; git is probed through the `GitProbe` seam, and a repo-local `git` executable is never trusted.
- **Tag migration** — `tags::migration`: skips memories that have both tags and a tags vector; soft failures are recorded (message names the memory), leave the memory unprocessed, and still advance the batch cursor; a completed pass resets counters so soft failures can be retried; the tags embedding text is `Topics: a, b`.
- **Dedup grouping** — `dedup::dedup_groups(candidates, options)`: candidates `(id, key, embedding)` → `Exact` groups by `key` (score `1.0`, first-appearance order) and `Conflict` groups inside the `[0.65, 0.9)` cosine band (union-find; score = maximum pairwise cosine). Output is deterministic: exact groups first, then conflict groups by descending score, ties broken by the group's smallest candidate index. Consumed by W3.3's cleanup scope and (planned) W4.3's episodic consolidation.

**W7.1 additions (sound memory, five tiers):**

- **AgentRole** — the closed ADR-0009 set `main | explorer | reviewer | worker`;
  `parse` trims and is ASCII case-insensitive, and its error is FIXED text that
  never echoes the rejected input. `AGENT_ROLE_HEADER = "x-agent-role"` is the
  lowercase HTTP header name (no extractor ships: D3).
- **AgentAccess / Access** — the ADR-0009 D-4 matrix as code, column order
  `profile | facts | own_episodic | other_episodic | vault` (`None`, `Read`,
  `ReadScoped`, `ReadWrite` with `can_read`/`can_write`/`is_scoped`).
- **MemoryTier** — `profile | semantic | procedural | resource | episodic |
  vault` (wire strings, exact parse). The vault tier's crate lands in W7.2;
  the model row is here.
- **CompressionLevel / `compression_level_for`** — the ADR-0010 D-3 budget as
  code, never config: Profile→L2, the three fact tiers→L1, Episodic→L4,
  Vault→L0.
- **FactTier** — `semantic | procedural | resource`, the three tiers
  `facts.db` holds; `as_tier()` maps to `MemoryTier`.
- **`classify_fact_tier`** — pure, total, deterministic; fixed precedence:
  exact-artifact marker (URL, path, backtick/token fence, `fn` signature) →
  Resource, imperative/step marker → Procedural, else Semantic.
- **Feature flags** — `featureFlags.soundMemory` and
  `featureFlags.promotionPipeline` (both default OFF); `soundMemory
  .rawContextRequired` and `promotion.ttlFallbackDays` (integer 1..=3650,
  default 7). Off means no retention writes and no `facts.db`.
