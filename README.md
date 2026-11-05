# opencode-agents-memory

Persistent memory for [OpenCode](https://opencode.ai) coding agents — a Rust port of
[`opencode-mem`](https://github.com/tickernelz/opencode-mem).

The engine stores project-scoped memories in sharded Turso/libSQL databases, embeds locally with
`Xenova/nomic-embed-text-v1` (768-dim ONNX), and exposes them through a thin OpenCode plugin, a
ten-mode `memory` tool, an optional `memory-usage` skill, and a Leptos web explorer over a REST API.

**Status: pre-1.0.** Phases 0–6 of the port (memory engine, AI client, profile learning, episodic
memory, plugin bridge, web explorer) are implemented and their suites are green. The remaining Phase 6
items are acceptance/smoke closures, and Phase 7 is the hardening and drop-in rehearsal. The live swap
on the owner's host executed on 2026-10-31 (`docs/runbooks/live-swap.md`, re-verified at
`docs/evidence/deploy-20261103-1959.md`), while the one-week dogfood window (issue 81) and manual
rehearsal sign-off (issue 28) remain open — do not point this port at a store you have not backed up.

## What it does

- **Durable, project-scoped memory** — add, search, list, forget, and inspect shards through one
  ten-mode `memory` tool; a `memory-usage` skill teaches the modes to the model.
- **Automatic capture and context injection** — session capture, idle learning, and compaction hooks
  keep memory current without explicit calls.
- **User profile learning** — prompt capture, language detection, confidence/decay, and a rendered
  profile for the model's context.
- **Episodic memory** — session timeline queries, consolidation into milestones, and promotion of
  durable facts (agent attribution lives on the milestone).
- **Portability** — `export` / `import` of an upstream-shaped JSON bundle, plus path migration for
  moved projects.
- **Web explorer** — a Leptos/Thaw SPA over a token-gated REST API (upstream-compatible); health,
  memories, tags, profile, sessions, maintenance, and the agent contribution view.

## Layout

| path | what |
|---|---|
| `crates/*` | 12 crates: `mem-store`, `mem-embed`, `mem-ai`, `mem-profile`, `mem-episodic`, `mem-facts`, `mem-vault`, `mem-skills`, `mem-portability`, `mem-web`, `mem-bridge`, `mem-core` |
| `src/` | composition root: CLI/arg parsing and the engine entry (`opencode-agents-memory`) |
| `plugin/` | the OpenCode plugin — plain Node ESM, no npm dependencies; `dist/` bundles, `engine/` staging, `skills/memory-usage/` |
| `web-ui/` | Leptos 0.7 + Thaw 0.4.8 CSR explorer (an excluded wasm workspace, built with `trunk`) |
| `example/opencode-mem/` | vendored upstream reference clone — read-only, gitignored, not part of the build |
| `opencode-mem.example.jsonc` | shipped config example — curated, every live key at its default (see Configuration) |
| `docs/` | ADRs, agent/testing documentation, evidence runs |

The workspace is Rust **edition 2024**. Dependencies live once in `[workspace.dependencies]`; the
registry root is `~/.opencode-mem/data`, the embedding model
`Xenova/nomic-embed-text-v1` (768 dimensions), and the engine's web server listens on **3742** by
default (`mem_bridge::engine::DEFAULT_WEB_PORT`; `OPENCODE_MEM_WEB_PORT` overrides it at launch).

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

Prerequisites: a stable Rust toolchain (edition 2024), Node for the plugin, and — for the web UI —
the `wasm32-unknown-unknown` target plus `trunk`. The repo-local `.cargo/config.toml` pins
**clang** and the `wild` linker at `/home/vaz/.cargo/bin/wild` for `x86_64-unknown-linux-gnu`
(a measured, machine-local choice). On another machine either install `wild` at that path or delete
`.cargo/config.toml` to fall back to rustc's bundled linker.

Install the commit gate once per clone:

```sh
git config core.hooksPath .githooks
```

## Run with OpenCode

1. Build the UI and stage the engine beside the plugin:

   ```sh
   cd web-ui && trunk build && cd ..
   plugin/scripts/stage-engine.sh
   ```

   The script builds the release engine and stages it (plus the built UI) under
   `plugin/engine/<platform>-<arch>/` — gitignored build output.

2. Point OpenCode at the local plugin in `~/.config/opencode/opencode.jsonc`:

   ```jsonc
   {
     "plugin": ["/absolute/path/to/opencode-agents-memory/plugin"]
   }
   ```

3. Configuration is read from `~/.config/opencode/opencode-mem.jsonc` (camelCase; unknown keys are
   tolerated). `opencode-mem.example.jsonc` at the repo root is the shipped example. Path precedence,
   the keys this build parses but does not read, the `OPENCODE_MEM_*` overrides and the
   `webServerPort`-versus-engine-port distinction are in [Configuration](#configuration).

Two operational caveats:

- **The embedder never downloads.** It reads `<storagePath>/.cache/<embeddingModel>/` (default
  `~/.opencode-mem/data/.cache/Xenova/nomic-embed-text-v1`) and needs the weights plus a canonical
  v2.0 `tokenizer.json`; `scripts/provision-onnx-cache.sh` writes exactly that. Without it, the rest
  of the store still works and `search`/`add` answer `-32603`.
- **Opening an upstream store migrates it in place** (`metadata.db` 1→2, `user-prompts.db` 1→2,
  project shards 2→3 lazily on first write; the port's own `episodic.db` sits at `user_version = 7`).
  The migration is additive but this build refuses a store written by a newer build, so back up the
  store before the first open. The backup/restore and provisioning sequence is documented in
  `docs/runbooks/live-swap.md` (with the 2026-11-03 live restart verified in
  `docs/evidence/deploy-20261103-1959.md`).

A local plugin does not auto-update.

## Configuration

The engine resolves one global file and, when the host launches it for a project, one project file,
layered lowest to highest:

| order | source |
|---|---|
| 1 (lowest) | built-in defaults (`DEFAULTS`, `crates/mem-core/src/config/schema.rs`) |
| 2 | `~/.config/opencode/opencode-mem.jsonc`, then `~/.config/opencode/opencode-mem.json` |
| 3 | `<project>/.opencode/opencode-mem.jsonc`, then `<project>/.opencode/opencode-mem.json` |
| 4 (highest) | environment-derived values (home resolution, secret references, the fallbacks below) |

Within one side the first candidate that parses wins: an existing but unparseable `.jsonc` is logged
and skipped, and the `.json` candidate is tried instead.

`opencode-mem.example.jsonc` at the repo root is the shipped example. It is **curated, not
exhaustive**: every live key in it is set to its built-in default, so copying it verbatim changes
nothing, and a key it does not list is at its default. For the complete key list read `FileConfig`
and `DEFAULTS` in `crates/mem-core/src/config/schema.rs`.

**Unknown keys are tolerated by design** (upstream forward compatibility). A misspelled key is
accepted, resolves to nothing and changes no behaviour — silently, because nothing validates key
names at runtime. Compare an edit against `FileConfig`; `crates/mem-core/tests/config_example.rs`
keeps the shipped example itself honest, and it does so against the real schema, not only against a
test-local copy of it.

### Keys that parse but this build does not read

These resolve (and mostly validate) but no code reads them, so their values change no behaviour.
They are kept for upstream-config compatibility; none of them is broken. Measured at commit
`25dec37`: for each key, `grep -rn '\b<resolved field>\b' crates src --include=*.rs`, filtered of
`*/tests/`, of `crates/mem-core/src/config/` (where a key is defined and resolved) and of comment
lines (`:[0-9]+:[[:space:]]*//` — a doc comment naming a field is not a reader), returns nothing.
The filter is textual: an access through an intermediate binding (`let cm = &config.chatMessage;
cm.maxMemories`) would not be matched. Re-run it before trusting a row. First taken at `1d423e8`;
re-stamped at `25dec37` because both pipelines print `0` on every row at both commits — at
`1d423e8` that is 22 loop rows plus the nested row (**23**), and after this wave it is 21 loop rows
plus the nested row (**22**); the difference is `chatMessage.captureChildSessions`, which is live
now and therefore not in the table above.

| key | note |
|---|---|
| `maxProfileItems` | the injection caps are `userProfileInject{Preferences,Patterns,Workflows}` (issue 44.9) |
| `autoUpdate` | the binary ships through the OS package manager; there is no self-update path (issue 27) |
| `chatMessage.excludeCurrentSession`, `chatMessage.injectOn` | only `chatMessage.enabled`, `chatMessage.filterInjectedPrompts`, `chatMessage.injectionMarkers` and `chatMessage.captureChildSessions` are read |
| `chatMessage.maxAgeDays` | assigned from the file and never consulted |
| `chatMessage.maxMemories` | the nested key is unread; the **top-level** `maxMemories` is the `search` default. Shared field name, so its census is the separate command below |
| `similarityThreshold` | `search` takes no threshold parameter in this build |
| `aiSessionRetentionDays`, `autoCleanupRetentionDays` | `autoCleanupEnabled` is read; neither count is |
| `autoCaptureMaxIterations`, `autoCaptureIterationTimeout`, `autoCaptureLanguage` | only `autoCaptureEnabled`, `autoCaptureMaxRetries` and `autoCaptureMaxContextBytes` are read; the capture prompt's language line is fixed in code |
| `opencodeTimeoutMs` | resolved and clamped, never consulted |
| `memoryTemperature` | normalized and never read; the manual provider reads `forceToolChoice` and its endpoint/model/key |
| `memoryExtraParams` | parsed and copied, never read, for the same reason |
| `userProfileMaxContextBytes`, `userProfileDisplay{Preferences,Patterns,Workflows}` | profile rendering uses the `userProfileInject*` caps |
| `showAutoCaptureToasts`, `showUserProfileToasts`, `showErrorToasts` | no toast surface in this build |

`chatMessage.captureChildSessions` is deliberately **not** in this table: it is read. It gates both the
memory-context restore for child sessions at compaction time (issue 114) and idle session capture in
`fireIdleCapture` (`plugin/src/plugin.js`, issue 86, `e6951c5`), stamping `parent_session_id` when
enabled; the pre-compaction **ingest** (`experimental.session.compacting`, ruling D-114-2) and the
pre-turn `chat.message` inject (D1) stay ungated. Its default is
`false`, and the shipped example carries that default so the opt-in is discoverable. The
child-session restore wave landed before this census was taken, so this section was re-derived
after it.

The `chatMessage.maxMemories` row is verified by its own pipeline, because the bare field name also
matches the **read** top-level key and would be a false positive:

```sh
grep -rn 'chat_message\.max_memories\|chat_message_max_memories' crates src --include=*.rs \
  | grep -v '/tests/' | grep -v 'crates/mem-core/src/config/' \
  | grep -vE ':[0-9]+:[[:space:]]*//' | wc -l    # 0
```

### Global-only and project-dropped keys

A project file that carries any of `embeddingApiUrl`, `embeddingApiKey`, `memoryProvider`,
`memoryApiUrl`, `memoryApiKey` is **rejected** — key presence is enough, even `null` — because
provider transport and keys are a global decision. `autoCleanupEnabled` and
`autoCleanupRetentionDays` are accepted in a project file and ignored.

### Environment

| variable | effect |
|---|---|
| `HOME`, `USERPROFILE` | home used for `~` expansion and the config paths |
| `OPENCODE_MEM_STORAGE_ROOT` | the engine's storage root (DBs, lock, status); `--storage-root` wins over it. **`storagePath` in the JSONC does not move the store** — it is the embedder's `<storagePath>/.cache` root |
| `OPENCODE_MEM_MODE` | engine launch mode (`stdio`, `standalone`, …) |
| `OPENCODE_MEM_WEB_HOST`, `OPENCODE_MEM_WEB_PORT` | the engine's own web bind (default `127.0.0.1:3742`) |
| `OPENCODE_MEM_WEB_DIST` | packaged UI directory (`--web-dist` wins) |
| `OPENCODE_MEM_LOG_FILE` | log file (default `~/.opencode-mem/opencode-mem.log`) |
| `OPENCODE_MEM_ALLOW_UNGUARDED_OPEN` | the guard override: any non-empty value enables it, `0`/`false`/`no`/`off` disable it, and it wins over `allowUnguardedOpen` |
| `OPENAI_API_KEY` | embedding key fallback, used only when `embeddingApiUrl` is set and `embeddingApiKey` is absent (an explicit `""` blocks the fallback) |
| `ATLASCLOUD_API_KEY` | memory key fallback when `memoryProvider` is `atlas-cloud` and `memoryApiKey` is unset |

`memoryApiKey`, `embeddingApiKey`, `webServerAuthPassword` and `webServerApiToken` accept
`env://NAME` or `file:///path` instead of a literal secret; a missing variable or file is a config
error.

### The two ports

`webServerPort` (default 4747) configures the standalone `mem-web` server. The engine's own server
binds **3742** (`mem_bridge::engine::DEFAULT_WEB_PORT`) and `OPENCODE_MEM_WEB_PORT` overrides it at
launch — measured 2026-10-31: the live engine bound 3742 while the config said 4747.

## Compatibility and upstream

The upstream-visible compatibility surfaces are exactly two:

- the OpenCode **plugin-loader contract** — `plugin/` exports a `PluginModule` with a `server` plugin;
- the **REST API** — the hand-written, upstream-shaped routes (the upstream Svelte UI remains usable
  against this server; this port ships its own Leptos explorer).

Everything else is port-owned: the wrapper↔engine stdio protocol, the on-disk store format, and the
episodic/facts/profile tiers. See `docs/adr/` for the decisions (ADR-0003 web stack, ADR-0007 bridge
strategy, ADR-0016 commit gate, and the schema/format ADRs).

The behavioural reference is the vendored `example/opencode-mem/` clone (upstream v2.39.0 at the time
of writing; it is gitignored and never edited). This project is not affiliated with the upstream
project.

## Documentation

- `OVERVIEW.md` — scope authority: the five epics as user stories with acceptance criteria.
- `AGENTS.md` — repository workflow, gates, commands and verification recipes.
- `CONTEXT-MAP.md` — the bounded contexts, one `CONTEXT.md` per crate.
- `docs/adr/` — architecture decisions; `docs/agents/` — testing, issue-tracker and workflow docs;
  `docs/runbooks/` — `live-swap.md` and `adopt-embedding-model.md`; `docs/evidence/` — measured run
  artifacts (including `deploy-20261103-1959.md`).
- `.scratch/rust-port/` and `.scratch/web-identity-and-project-picker/` — the port's delivery
  roadmap, PRDs and issue history.

## License

MIT — see [`LICENSE`](LICENSE). The upstream MIT copyright notice is retained.
