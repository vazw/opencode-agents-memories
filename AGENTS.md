# AGENTS.md

This repository is a from-scratch **Rust port of `opencode-mem`**, the OpenCode plugin providing persistent agent memory (Turso/libSQL vector search, auto-capture, user-profile learning, web explorer). The port must stay drop-in compatible with the OpenCode plugin contracts and the existing web UI's REST contract (the explorer UI itself is reimplemented in the port — Leptos/Thaw CSR, see `docs/adr/0003-web-explorer-ui-stack.md`), and adds a new third memory tier: per-project agent session (episodic) memory.

## Read first

- **`OVERVIEW.md` is the scope authority.** It lays out 5 epics as user stories with acceptance criteria, each mapping a TypeScript service/test from the original to its Rust replacement.
- TS paths cited there (`src/services/*.ts`, `tests/*.test.ts`) refer to the **vendored clone under `example/`**, not to this repo's tree.

## Reference clone — `example/opencode-mem/`

- Vendored clone of upstream [tickernelz/opencode-mem](https://github.com/tickernelz/opencode-mem) at commit `906038e` (v2.31.3), with its own `.git`.
- **Read-only reference:** never edit it, never `git add -f` it — it is gitignored by design (local reference only). All port work lands in this repo's own code.
- Its `AGENTS.md` governs only that subtree (Bun tooling, Conventional Commits); those rules do not apply to this repo.
- The upstream test suite (`example/opencode-mem/tests/`, 110 `*.test.ts` files) is the behavioral contract for the port — mirror tests as you port.

## Current state & stack

- `src/` holds a hello-world `main.rs` only; the crate (`edition = "2024"`) has no dependencies yet. Check what exists before assuming a module layout.
- When adding dependencies, use the stack recommended in OVERVIEW.md: `tokio`, `libsql`/`turso`, `ort` (ONNX embeddings), `axum`, `serde`/`jsonc_parser`.

## Commands

- Standard cargo workflow: `cargo build`, `cargo test`, `cargo run`, `cargo fmt`, `cargo clippy`.
- No CI, release automation, or extra lint config exists — cargo is the whole toolchain here.
- Don't invoke Bun/npm/eslint as this repo's tooling; that belongs to the reference-only `example/` subtree.

## Agent skills

### Issue tracker

Issues and PRDs for this repo live as local markdown files under `.scratch/<feature>/`. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical triage strings used 1:1 (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Multi-context — `CONTEXT-MAP.md` at the root indexes one `CONTEXT.md` per crate. See `docs/agents/domain.md`.
