# opencode-agents-memory

Local agent-memory engine in Rust, inspired by [`opencode-mem`](https://github.com/tickernelz/opencode-mem).
Sharded Turso/libSQL stores, local embeddings (`Xenova/nomic-embed-text-v1`, 768-dim ONNX), an OpenCode
plugin, a ten-mode `memory` tool, and a Leptos explorer over a token-gated REST API.

**Pre-1.0.** Phases 0–6 are implemented and green; Phase 7 (hardening, drop-in rehearsal) is in progress.
Running on the owner's host since 2026-10-31. Two things bite:

- **Back up the store before the first open.** Opening an upstream store migrates it in place, and a store
  written by a newer build is refused (`docs/runbooks/live-swap.md`).
- **The embedder never downloads.** It reads `<storagePath>/.cache/<embeddingModel>/` and needs the weights
  plus a canonical v2.0 `tokenizer.json`; `scripts/provision-onnx-cache.sh` writes exactly that. Without it
  the store still works and `search`/`add` answer `-32603`.

## Layout

| path | what |
|---|---|
| `crates/*` | 12 crates: `mem-store`, `mem-embed`, `mem-ai`, `mem-profile`, `mem-episodic`, `mem-facts`, `mem-vault`, `mem-skills`, `mem-portability`, `mem-web`, `mem-bridge`, `mem-core` |
| `src/` | composition root: CLI parsing and the engine entry (`opencode-agents-memory`) |
| `plugin/` | the OpenCode plugin — plain Node ESM, no npm install; `dist/` bundles, `engine/` staging, `skills/memory-usage/` |
| `web-ui/` | Leptos + Thaw CSR explorer (excluded wasm workspace, built with `trunk`) |
| `example/opencode-mem/` | vendored upstream clone — read-only, gitignored, not built |
| `opencode-mem.example.jsonc` | shipped config example, every live key at its default |
| `docs/` | ADRs, agent/testing docs, runbooks, evidence runs |

Rust **edition 2024**. Dependencies live once in `[workspace.dependencies]`. Store root
`~/.opencode-mem/data`; the engine's own server binds **3742** by default.

## Build and test

```sh
cargo build --release
cargo test --workspace

# plugin — plain node, no npm install
cd plugin && node --test tests/*.test.mjs

# web UI — wasm only
cd web-ui && trunk build
cd web-ui && cargo test --target wasm32-unknown-unknown
```

Needs a stable Rust toolchain, Node for the plugin, and `wasm32-unknown-unknown` + `trunk` for the UI.
`.cargo/config.toml` pins clang and the `wild` linker at a machine-local path; elsewhere install `wild`
there or delete the file.

Install the commit gate once per clone — it runs clippy and, for `plugin/` changes, a src↔dist byte compare:

```sh
git config core.hooksPath .githooks
```

## Run with OpenCode

```sh
cd web-ui && trunk build && cd ..
plugin/scripts/stage-engine.sh        # builds the engine + UI into plugin/engine/<platform>-<arch>/
```

Point the host at the plugin in `~/.config/opencode/opencode.jsonc`:

```jsonc
{ "plugin": ["/absolute/path/to/opencode-agents-memory/plugin"] }
```

A local plugin does not auto-update.

## Configuration

One global file and, when the host launches the engine for a project, one project file, lowest to highest:

| order | source |
|---|---|
| 1 (lowest) | built-in defaults (`DEFAULTS`, `crates/mem-core/src/config/schema.rs`) |
| 2 | `~/.config/opencode/opencode-mem.jsonc`, then `.json` |
| 3 | `<project>/.opencode/opencode-mem.jsonc`, then `.json` |
| 4 (highest) | environment-derived values |

Within one side the first candidate that parses wins; an existing but unparseable `.jsonc` is logged and
skipped. `opencode-mem.example.jsonc` is curated — copy it verbatim and nothing changes. For the full key
list read `FileConfig` and `DEFAULTS`.

**Unknown keys are tolerated by design**, silently: a misspelled key is accepted, resolves to nothing, and
changes no behaviour. Nothing validates key names at runtime, so compare an edit against `FileConfig`;
`crates/mem-core/tests/config_example.rs` keeps the shipped example honest against the real schema.

A project file carrying any of `embeddingApiUrl`, `embeddingApiKey`, `memoryProvider`, `memoryApiUrl`,
`memoryApiKey` is **rejected** — key presence is enough, even `null` — because transport and keys are a
global decision. `autoCleanupEnabled` and `autoCleanupRetentionDays` are accepted in a project file and
ignored.

### Keys that parse but nothing reads

Kept for upstream-config compatibility; none is broken, and setting one changes nothing. Verified per key by
`grep -rn '\b<resolved field>\b' crates src --include=*.rs` filtered of `*/tests/`, of
`crates/mem-core/src/config/`, and of comment lines — re-run it before trusting a row; a read through an
intermediate binding would not match.

| key | note |
|---|---|
| `maxProfileItems` | the injection caps are `userProfileInject{Preferences,Patterns,Workflows}` |
| `autoUpdate` | ships through the OS package manager; no self-update path |
| `chatMessage.excludeCurrentSession`, `chatMessage.injectOn` | only `enabled`, `filterInjectedPrompts`, `injectionMarkers` and `captureChildSessions` are read |
| `chatMessage.maxAgeDays` | assigned, never consulted |
| `chatMessage.maxMemories` | nested key unread; the **top-level** `maxMemories` is the `search` default |
| `similarityThreshold` | `search` takes no threshold in this build |
| `aiSessionRetentionDays`, `autoCleanupRetentionDays` | `autoCleanupEnabled` is read; neither count is |
| `autoCaptureMaxIterations`, `autoCaptureIterationTimeout`, `autoCaptureLanguage` | only `autoCaptureEnabled`, `autoCaptureMaxRetries` and `autoCaptureMaxContextBytes` are read; the prompt's language line is fixed in code |
| `opencodeTimeoutMs` | resolved and clamped, never consulted |
| `memoryTemperature`, `memoryExtraParams` | normalized/copied, never read; the manual provider reads `forceToolChoice` and its endpoint/model/key |
| `userProfileMaxContextBytes`, `userProfileDisplay{Preferences,Patterns,Workflows}` | rendering uses the `userProfileInject*` caps |
| `showAutoCaptureToasts`, `showUserProfileToasts`, `showErrorToasts` | no toast surface in this build |

`chatMessage.captureChildSessions` is **read**: it gates the child-session restore at compaction and idle
capture, stamping `parent_session_id`. Default `false`. The pre-compaction ingest and the pre-turn
`chat.message` inject stay ungated.

### Environment

| variable | effect |
|---|---|
| `HOME`, `USERPROFILE` | home for `~` expansion and config paths |
| `OPENCODE_MEM_STORAGE_ROOT` | storage root; `--storage-root` wins. `storagePath` in JSONC does **not** move the store — it is the embedder's cache root |
| `OPENCODE_MEM_MODE` | engine launch mode (`stdio`, `standalone`, …) |
| `OPENCODE_MEM_WEB_HOST`, `OPENCODE_MEM_WEB_PORT` | the engine's own web bind (default `127.0.0.1:3742`) |
| `OPENCODE_MEM_WEB_DIST` | packaged UI directory (`--web-dist` wins) |
| `OPENCODE_MEM_LOG_FILE` | default `~/.opencode-mem/opencode-mem.log` |
| `OPENCODE_MEM_ALLOW_UNGUARDED_OPEN` | guard override: any non-empty value enables it, `0`/`false`/`no`/`off` disable it, wins over `allowUnguardedOpen` |
| `OPENAI_API_KEY` | embedding key fallback, only when `embeddingApiUrl` is set and `embeddingApiKey` is absent (an explicit `""` blocks it) |
| `ATLASCLOUD_API_KEY` | memory key fallback when `memoryProvider` is `atlas-cloud` |

`memoryApiKey`, `embeddingApiKey`, `webServerAuthPassword` and `webServerApiToken` accept `env://NAME` or
`file:///path`; a missing variable or file is a config error.

**Two ports, do not confuse them.** `webServerPort` (default 4747) configures the standalone `mem-web`
server; the engine's own server binds 3742 and `OPENCODE_MEM_WEB_PORT` overrides it at launch. Measured
2026-10-31: the live engine bound 3742 while the config said 4747.

## Compatibility and upstream

Two upstream-visible surfaces, and only two: the OpenCode **plugin-loader contract** (`plugin/` exports a
`PluginModule` with a `server` plugin) and the **REST API** (upstream-shaped routes; the upstream Svelte UI
stays usable against this server, though this project ships its own explorer). Everything else is ours: the
wrapper↔engine stdio protocol, the on-disk format, and the episodic/facts/profile tiers. See `docs/adr/`
(0003 web stack, 0007 bridge, 0016 commit gate, plus the schema/format ADRs).

`example/opencode-mem/` is the behavioural reference and is never edited. Not affiliated with upstream.

## Documentation

- `OVERVIEW.md` — scope authority: five epics as user stories with acceptance criteria.
- `AGENTS.md` — workflow, gates, commands, verification recipes.
- `CONTEXT-MAP.md` — bounded contexts, one `CONTEXT.md` per crate.
- `docs/adr/`, `docs/agents/`, `docs/runbooks/`, `docs/evidence/` — decisions, process, runbooks
  (`live-swap.md`, `adopt-embedding-model.md`), measured runs.
- `.scratch/` — delivery roadmap, PRDs, issue history.

## License

MIT — see [`LICENSE`](LICENSE). The upstream MIT notice is retained.
