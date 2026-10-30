# opencode-agents-memory

Persistent memory for [OpenCode](https://opencode.ai) coding agents — a Rust port of
[`opencode-mem`](https://github.com/tickernelz/opencode-mem).

The engine stores project-scoped memories in sharded Turso/libSQL databases, embeds locally with
`Xenova/nomic-embed-text-v1` (768-dim ONNX), and exposes them through a thin OpenCode plugin, a
ten-mode `memory` tool, an optional `memory-usage` skill, and a Leptos web explorer over a REST API.

**Status: pre-1.0.** Phases 0–6 of the port (memory engine, AI client, profile learning, episodic
memory, plugin bridge, web explorer) are implemented and their suites are green. The remaining Phase 6
items are acceptance/smoke closures, and Phase 7 is the hardening and drop-in rehearsal. The upstream
plugin remains the stable release until the live swap has been validated — do not point this port at a
store you have not backed up.

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
| `docs/` | ADRs, agent/testing documentation, evidence runs |

The workspace is Rust **edition 2024**. Dependencies live once in `[workspace.dependencies]`; the
registry root is `~/.opencode-mem/data`, the embedding model
`Xenova/nomic-embed-text-v1` (768 dimensions), and the web explorer's default port is **3742**
(config-overridable).

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

3. The plugin reads the same `~/.config/opencode/opencode-mem.jsonc` as upstream (camelCase, unknown
   keys tolerated). Defaults: `storagePath` `~/.opencode-mem/data`, `embeddingModel`
   `Xenova/nomic-embed-text-v1`. The web explorer listens on port **3742** by default;
   `OPENCODE_MEM_WEB_PORT` overrides it.

Two operational caveats:

- **The embedder never downloads.** It reads `$TMPDIR/.opencode-mem-onnx/Xenova/nomic-embed-text-v1/`;
  provision that cache (weights plus a canonical v2.0 `tokenizer.json`) before relying on
  `search`/`add`. Without it, the rest of the store still works and `search`/`add` answer `-32603`.
- **Opening an upstream store migrates it in place** (`metadata.db` 1→2, `user-prompts.db` 1→2,
  project shards 2→3 lazily on first write). The migration is additive but this build refuses a store
  written by a newer build, so back up the store before the first open. The backup/restore and
  provisioning sequence is documented in `docs/runbooks/live-swap.md`.

A local plugin does not auto-update.

## Compatibility and upstream

The upstream-visible compatibility surfaces are exactly two:

- the OpenCode **plugin-loader contract** — `plugin/` exports a `PluginModule` with a `server` plugin;
- the **REST API** — the hand-written, upstream-shaped routes (the upstream Svelte UI remains usable
  against this server; this port ships its own Leptos explorer).

Everything else is port-owned: the wrapper↔engine stdio protocol, the on-disk store format, and the
episodic/facts/profile tiers. See `docs/adr/` for the decisions (ADR-0003 web stack, ADR-0007 bridge
strategy, ADR-0016 commit gate, and the schema/format ADRs).

The behavioural reference is the vendored `example/opencode-mem/` clone (upstream v2.33.0 at the time
of writing; it is gitignored and never edited). This project is not affiliated with the upstream
project.

## Documentation

- `OVERVIEW.md` — scope authority: the five epics as user stories with acceptance criteria.
- `AGENTS.md` — repository workflow, gates, commands and verification recipes.
- `CONTEXT-MAP.md` — the bounded contexts, one `CONTEXT.md` per crate.
- `docs/adr/` — architecture decisions; `docs/agents/` — testing, issue-tracker and workflow docs.
- `.scratch/rust-port/` — the port's delivery roadmap, PRD and issue history.

## License

MIT — see [`LICENSE`](LICENSE). The upstream MIT copyright notice is retained.
