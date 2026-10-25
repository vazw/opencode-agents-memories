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

## Gates

The repository's commit gate is tracked at `.githooks/pre-commit`. Install it once per clone (the
configuration is per-clone state, not tracked):

```sh
git config core.hooksPath .githooks
```

Every commit then runs, in the worktree:

```sh
cargo clippy --workspace --lib -- -A clippy::all -D clippy::unwrap_used -D clippy::expect_used
cargo clippy --workspace --all-targets -- -D warnings -A clippy::await_holding_lock -A clippy::assertions_on_constants -A clippy::doc_lazy_continuation
```

and, for staged `.rs` files only, `rustfmt --check` over the **index blobs** (`--edition 2024` is
hardcoded; all 434 tracked `.rs` files are edition 2024 — 407 outside `web-ui/`, 27 inside). A
staged `web-ui/` Rust or manifest change also runs **both** gates in that separate workspace
(`exclude = ["web-ui"]`, ADR-0001 wasm exception), in the root's order:

```sh
cd web-ui && cargo clippy --workspace --lib -- -A clippy::all -D clippy::unwrap_used -D clippy::expect_used
cd web-ui && cargo clippy --workspace --all-targets -- -D warnings
```

(the panic-family gap closed 2026-10-25: 7 sites → 0, ADR-0016 D-3). A `web-ui/README.md`-only
commit never invokes cargo. A commit that stages no
`.rs`, `Cargo.toml` or `Cargo.lock` exits before cargo is invoked.

Cost: **≈35 s warm** for a Rust-touching commit; a staged `web-ui/` change adds both web-ui gates
(≈2.5 s warm total on top of the ≈35 s); a cold `CARGO_TARGET_DIR` pays a
full workspace build. The first run on a fresh clone creates `.tmp/scratch` itself (it is gitignored
and may not exist), so the `git config` command above is the only setup step.

This is a **lint and format** gate, not a quality gate: it runs no tests and cannot judge an
evidence claim. `git commit --no-verify` is legitimate only when the failure is pre-existing and
unrelated to what you staged — record that in the wave ledger; otherwise fix it.

Verify the gate itself with:

```sh
bash -n .githooks/pre-commit && bash -n scripts/test-pre-commit.sh
shellcheck .githooks/pre-commit scripts/test-pre-commit.sh
bash scripts/test-pre-commit.sh     # 16 behavioural cases, PASS 16/16
```

See `docs/adr/0016-pre-commit-gate.md` for the decisions, costs and limitations.

## Agent skills

### Issue tracker

Issues and PRDs for this repo live as local markdown files under `.scratch/<feature>/`. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical triage strings used 1:1 (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Multi-context — `CONTEXT-MAP.md` at the root indexes one `CONTEXT.md` per crate. See `docs/agents/domain.md`.
