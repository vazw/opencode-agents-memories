# AGENTS.md

This repository is a from-scratch **local agent-memory engine in Rust, inspired by `opencode-mem`**, the
OpenCode plugin that gives coding agents persistent memory (Turso/libSQL vector search, auto-capture,
user-profile learning, web explorer).

Its compatibility promise splits by **what upstream can see**. Upstream sees exactly two surfaces: the
OpenCode **plugin-loader contract** — `plugin/` must export `type PluginModule = { id?: string; server:
Plugin; tui?: never }` (ADR-0007 §Context, guarded upstream by `tests/plugin-loader-contract.test.ts`) —
and the **REST API** (ADR-0003 keeps the upstream Svelte build a drop-in UI over it). Everything else is
ours alone: **no upstream counterpart** exists for the wrapper↔engine stdio protocol (ADR-0007 D-43 —
equal major + wrapper floor), nor for the on-disk format (shard v3, prompts v2; no downgrade path, and
a root written by a newer build refuses to open — issue 43 decisions 2–3). Upstream is a behavioural
reference, never a co-tenant of the storage root. Beyond its two tiers the model grows per-project
episodic/session memory, the durable facts DB (`facts.db`), the encrypted vault (`vault.db` per scope),
skills (`skills.db`) and a portability/staging tier — bounded contexts indexed by `CONTEXT-MAP.md`.

## Read first

- **`OVERVIEW.md` is the scope authority.** It lays out 5 epics as user stories with acceptance criteria,
  each mapping a TypeScript service/test from the original to its Rust replacement.
- TS paths cited there (`src/services/*.ts`, `tests/*.test.ts`) refer to the **vendored clone under
  `example/`**, not to this repo's tree.

## Workflow — evidence over claims

**Don't trust, verify.** Owner directive, 2026-10-25. No claim is accepted because it is stated
confidently — not by an agent, not by a report, not by a review. If it can be tested, test it.

```
architect A/B drafts  →  measure (A wins / B wins / NEITHER)  →  syseng gate
                      →  coder implements  →  opsec review loop
```

- **A/B means two competing designs**, each with pros, cons, a predicted outcome, the observation that
  would refute it, and one shared **discriminating measurement**. Not two implementations, not two model
  runs — if a behavioural A/B is ever meant, say so explicitly.
- **"Neither" is a valid result.** A measurement that dissolves the work is a success; close the item as
  *measurement, not a deliverable*.
- **syseng gates the design; opsec reviews the implementation.** Neither substitutes for the other, and
  neither substitutes for the measurement.
- Label every substantive claim `MEASURED` / `READ` / `REPORTED` / `INFERRED`, cite the artifact, and
  name what would settle an inference.
- **Re-derive second-hand figures, never inherit them.** Counts from reports, audits and reviews have
  been wrong here — one was inverted.
- **Every gate carries an inverted world:** state the observation that would close it as measurement.
- **A reviewer's prescription is a claim to check, not an instruction to apply.** Reviewers in this repo
  have been wrong about which files to edit and which routes exist.
- **Never claim a measurement that was not run.** "Not covered by any evidence here" is a valid answer.

Record every run: both drafts' pros/cons (keep the loser), the decision (ADR if architectural, issue
comment if scoped), and the measurement with its artifact path. **Never only in `.tmp/` — it is
gitignored** and a ruling that lives only there is one `rm -rf` from gone.

Full process: `docs/agents/workflow.md`.

## Reference clone — `example/opencode-mem/`

- Vendored clone of upstream [tickernelz/opencode-mem](https://github.com/tickernelz/opencode-mem) with
  its own `.git`, gitignored (`/example/`): **`9086c65` (v2.39.0), 140 `*.test.ts` files** — the commit and
  the test count move together; re-derive both before citing either. (Updated 2026-11-02 from
  `1ddfdad`/v2.33.0/117 files; the parity matrix is still pinned to an older commit — see
  `docs/agents/testing.md`.)
- **Read-only reference:** never edit it, never `git add -f` it. All of this project's work lands in this repo's code.
- Its `AGENTS.md` governs only that subtree (Bun tooling, Conventional Commits); those rules do not apply.
- The upstream suite is the behavioural contract, but not every file is mirrored: each takes exactly one
  disposition — `port`, `covered-by` or `N-A` — per `docs/agents/testing.md`.

## Current state & stack

Counts below were re-derived 2026-11-01 and are a snapshot: re-run "Verify this file" before quoting any of
them, and prefer a description over a numeral when the numeral cannot be kept cheaply true.

- Root package `opencode-agents-memory` is the **composition root** over the **12 crates under
  `crates/*`**: `src/main.rs` is sync arg-parse and dispatch (a `purge-session` mode included), `src/lib.rs`
  exposes `web_dist` (`src/web_dist.rs` resolves the built UI), and root `tests/` holds
  18 workspace-level suites (`tests/workspace_deps.rs` enforces ADR-0001's allowed edges;
  `tests/web_identity.rs` is the web-identity resolver's contract, added by issues 115/116).
- Every tracked `Cargo.toml` is `edition = "2024"` (members inherit `[workspace.package]`); `web-ui/` is an
  excluded workspace of its own (Leptos/Thaw CSR, wasm-only, `trunk`-built).
- `plugin/` is the OpenCode wrapper — plain `node`, no npm/Bun dependency, no lockfile: `plugin/dist/`
  bundles, 28 `*.test.mjs` files, and `plugin/scripts/stage-engine.sh`, which stages a built engine
  beside the packaged UI under the gitignored `plugin/engine/<platform>-<arch>/`.
- Dependencies live once in `[workspace.dependencies]`, consumed as `{ workspace = true }`: `turso` 0.8,
  `tokio`, `ort` + `tokenizers`, `axum` 0.8 + `tower-http`, `serde`/`jsonc-parser`, `tracing`, `chrono`,
  `zeroize`. Check that table before adding a crate to a member.
- `opencode-mem.example.jsonc` at the repo root is the shipped config example: curated, every live
  key at its built-in default, keys that parse but are unread marked as such. Its guard is
  `crates/mem-core/tests/config_example.rs` — a misspelled key fails the test-local
  `deny_unknown_fields` mirror, and a key that is not a `FileConfig` field fails a production-side
  check that hands every shipped key an object value and requires `ConfigError::InvalidShape`.
  Were one of the six `Value`-typed keys shipped, the same check would red it too, one way or
  the other: memoryExtraParams, opencodeVariant and opencodeTimeoutMs accept the object and the
  load answers Ok, while memoryTemperature, embeddingDtype and injectionMarkers surface as
  InvalidValue/InvalidValueExpected. Add a key to the example and to the mirror together.

## Commands

- Rust: `cargo build`, `cargo test --workspace`, `cargo fmt --all --check`, the two clippy gates below,
  `cargo run`. Web UI: `cd web-ui && trunk build`, tests wasm-only (`cargo test --target
  wasm32-unknown-unknown`). Wrapper: `cd plugin && node --test tests/*.test.mjs`.
- `cargo audit` runs from the repo root only — `.cargo/audit.toml` is the sole file it reads accepted
  findings from, and it documents its own drift check. The repo has exactly two tracked cargo configs:
  `.cargo/config.toml` (pins the **wild** linker for `x86_64-unknown-linux-gnu` host links — measured
  2026-10-27: 1.22–1.45 s vs rust-lld's 1.89–2.01 s per debug link, −20 % link RSS, evidence in
  `docs/evidence/wild-linker-2026-10-26/`) and `web-ui/.cargo/config.toml` (the wasm test runner, nothing
  else). There is no CI, no release automation, no `clippy.toml`, `rustfmt.toml` or `rust-toolchain.toml`.
- Never invoke Bun/npm/eslint: that tooling belongs to the reference-only `example/` subtree.

## Gates

The commit gate is tracked at `.githooks/pre-commit`; install it once per clone (per-clone state, not
tracked) with `git config core.hooksPath .githooks`. Every commit then runs, in the worktree:

```sh
cargo clippy --workspace --lib -- -A clippy::all -D clippy::unwrap_used -D clippy::expect_used
cargo clippy --workspace --all-targets -- -D warnings -A clippy::await_holding_lock -A clippy::assertions_on_constants -A clippy::doc_lazy_continuation
```

and, for staged `.rs` files only, `rustfmt --check` over the **index blobs** (`--edition 2024` is
hardcoded — safe, because every tracked `.rs` in this repository is edition 2024). Any commit that gets
past the fast path and also stages a `web-ui/` path runs **both** gates in that separate workspace
(`exclude = ["web-ui"]`, ADR-0001 wasm exception), in the root's order:

```sh
cd web-ui && cargo clippy --workspace --lib -- -A clippy::all -D clippy::unwrap_used -D clippy::expect_used
cd web-ui && cargo clippy --workspace --all-targets -- -D warnings
```

The web-ui panic-family gate exists again as of 2026-10-25 (7 sites → 0, ADR-0016 D-3). A commit staging
no `.rs`, `Cargo.toml` or `Cargo.lock` exits before cargo is invoked — so docs-, `plugin/`- and
`web-ui/README.md`-only commits never pay for it.

Cost: **≈35 s warm** for a Rust-touching commit; a staged `web-ui/` change adds both web-ui gates
(≈2.5 s warm on top); a cold `CARGO_TARGET_DIR` pays a full workspace build. Those are ADR-0016's
measurements, not re-measured since — re-measure before quoting them. The first run on a fresh clone
creates `.tmp/scratch` itself (gitignored, possibly absent), so `git config` above is the only setup.

This is a **lint and format** gate, not a quality gate: it runs no tests and cannot judge an evidence
claim. `git commit --no-verify` is legitimate only when the failure is pre-existing and unrelated to what
you staged — record that in the wave ledger; otherwise fix it.

Verify the gate itself with:

```sh
bash -n .githooks/pre-commit && bash -n scripts/test-pre-commit.sh
shellcheck .githooks/pre-commit scripts/test-pre-commit.sh
bash scripts/test-pre-commit.sh     # 17 behavioural cases in throwaway repos; prints PASS 17/17 on success
```

See `docs/adr/0016-pre-commit-gate.md` for the decisions, costs and limitations.

## Agent skills

| topic | where |
|---|---|
| Issue tracker — `.scratch/<feature>/PRD.md`, `issues/NN-<slug>.md`, a `Status:` line per issue | `docs/agents/issue-tracker.md` |
| Triage labels — five strings used 1:1: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix` | `docs/agents/triage-labels.md` |
| Domain docs — multi-context: `CONTEXT-MAP.md` indexes one `CONTEXT.md` per crate | `docs/agents/domain.md` |
| Testing — dispositions, where tests live, RED→GREEN | `docs/agents/testing.md` |
| Workflow — the evidence-gated pipeline in full | `docs/agents/workflow.md` |

## Verify this file

Every figure above is a snapshot; re-derive it before relying on it — each command's output is the claim:

```sh
git -C example/opencode-mem log --oneline -1; grep -m1 '"version"' example/opencode-mem/package.json
ls example/opencode-mem/tests/*.test.ts | wc -l; ls -1d crates/*/ | wc -l; ls crates/*/CONTEXT.md | wc -l
ls tests/*.rs | wc -l; ls plugin/tests/*.test.mjs | wc -l; grep -c '^case_[0-9]' scripts/test-pre-commit.sh
git ls-files '*Cargo.toml' | xargs grep -h '^edition = ' | sort -u   # expect only edition = "2024"
grep -rliE 'jsonrpc|json-rpc|"hello"' example/opencode-mem/src     # expect no output: no upstream bridge
```
