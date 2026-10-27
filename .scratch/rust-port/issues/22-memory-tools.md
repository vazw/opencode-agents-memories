# 22 — W5.4 · Memory tools registration — existing tools + recall (US 1.4)

Status: landed — residual (owned by issue 70) — 2026-10-27, implementing commit `e58e703` (canonical + legacy `recall` and the V2 `recall_project_sessions` tool with per-tool scope enforcement; the four ported behaviours — `tool-scope`, `memory-timeline-orphan` (tool path), `memory-portability-tool`, `profile-tool-runtime` — remain blocked on wire methods the frozen schema cannot add; see `## Comments`)

## What to build

Register all agent-facing memory tools with the host: the ported memory tools plus the new `recall_project_sessions`, with per-tool scope rules and the portability tool path. Wave detail: `.scratch/rust-port/roadmap.md` §W5.4.

## Acceptance criteria

- [ ] Behaviors ported: `tool-scope`, `memory-timeline-orphan` (tool path), `memory-portability-tool`, `profile-tool-runtime`. — **blocked**: each needs a wire method the frozen schema cannot add (see `## Comments`).
- [x] `recall_project_sessions` registered and callable with the W4.2 schema.
- [x] Per-tool scope rules enforced (tools cannot read outside their scope). — the one tool this lane registers; `scope_rejected` / `invalid_params` ×2 / `scope_unavailable` are pinned in `plugin/tests/memory-tools.test.mjs`.

## Blocked by

- 16 · W4.2
- 19 · W5.1

## Agent Brief

**Category:** enhancement
**Summary:** register the agent-facing memory tools and give `recall` a real implementation.

**Current behavior (verified at HEAD `206cf9f`):**
The wrapper exposes no tools at all — `server()` returns only hooks, and no tool registration exists anywhere
in the plugin. The engine advertises `capture/inject/recall/health/shutdown`, but `recall` is a stub that
returns `{count: 0, items: []}` unconditionally. The `recall_project_sessions` input schema and the scope
precedence rules landed under earlier waves and are simply not exposed to a host.

**Desired behavior:**
Every agent-facing memory tool is registered with the host, including the new `recall_project_sessions`, and
each tool enforces its scope rules at the host boundary — a tool cannot read outside its scope. `recall`
returns real rows through the existing store path instead of the hard-coded empty response, and the
portability tool path is registered too. The four behaviours this lane owns are ported: `tool-scope`,
`memory-timeline-orphan` (tool path), `memory-portability-tool`, `profile-tool-runtime`.

**Key interfaces:**
- The host's tool registration surface — absent in the wrapper today.
- The `recall_project_sessions` schema from the earlier wave — reuse it, do not redefine it.
- The scope precedence rules in the core crate — the enforcement point is the tool boundary.
- The engine's `recall` method — replace the stub body; do not add a parallel path.

**Acceptance criteria:**
- [ ] The issue's three ACs, each pinned by a test that fails on today's tree. — two of the three closed; the ported-behaviours AC is blocked on wire methods (see `## Comments`).
- [x] `recall` no longer returns the unconditional empty response: a scope with rows returns them, and a
      project-scoped call never returns another project's rows.
- [x] Per-tool scope enforcement is tested with a negative case per tool.

**Out of scope:**
- The V2 adapter (issue 20) — tools register against the host surface that exists.
- The compaction hooks (issue 33).
- Any change to the tool schema, which is already defined.

## Comments

**2026-10-27T17:42Z — implemented in `e58e7038438b9ac37348d5f311cbf3298bf59a61`.**

Delivered:

- **`recall` (engine).** One wire method, two accepted parameter shapes
  (`crates/mem-bridge/src/recall.rs`). *Canonical* is the W4.2
  `recall_project_sessions` argument set flattened at the top level (camelCase,
  `deny_unknown_fields`) and discriminated by `projectId`; it is served from the
  episodic store — chronological/filtered through `timeline`, semantic through
  `semantic_timeline` (per-item `similarity`, descending), `includeFullSessions`
  through one `session_detail` per summary. *Legacy* is the frozen
  `{query, scope?, tags?, limit?}` shape; it resolves no project, reads no rows,
  and answers one empty page through the named `recall.legacy_shape_unscoped`
  warning whose extras object is a constant `{}` (no `session_id`, query or
  path can travel). The envelope `session_id` is stripped from a clone, so the
  W4.2 filter `sessionId` survives untouched. The wire page stays
  `{"count": int, "items": [...]}` for both shapes; `Engine::new` (no runtime)
  answers `-32001` for the canonical shape instead of an empty page.
- **`recall_project_sessions` (wrapper).** Registered on the V2 `setup` path
  through `ctx.tool.transform` — the only door the installed host
  (`opencode v2.1.4-local`) exposes — with the W4.2 `to_json_schema()` as the
  raw JSON Schema `input` and no `options.namespace`, so the callable name is
  exactly `recall_project_sessions` (`packages/core/src/tool/runtime.ts`
  `effectiveName`). The wire `projectId` is `ctx.location.directory` verbatim;
  the model's `projectId` is only compared after `path.resolve()` on both
  sides. Failures are values with fixed tokens: `scope_unavailable`,
  `invalid_params`, `scope_rejected`, `engine_unavailable` (precedence in that
  order). The tool cleanup is awaited before the adapter cleanup; a host
  without a tool surface keeps capture and injection (fail-open).
- The two sides assert one shared descriptor artifact,
  `plugin/tests/fixtures/recall-tool.json` (Rust drift guard
  `crates/mem-bridge/tests/recall_schema.rs`; wrapper oracle
  `plugin/tests/memory-tools.test.mjs`).

**Blocked behaviours** — each needs a wire method that does not exist, and
`docs/bridge-schema.md` plus `tests/fixtures/bridge/**` are frozen:

- `tool-scope` — needs the multi-mode `memory` tool (upstream's ten modes;
  nine need `add`/`search`/`profile`/`list`/`forget`/`list-shards`/`migrate`/
  `export`/`import`, while the engine advertises only
  `capture`/`inject`/`recall`/`health`/`shutdown`).
- `memory-timeline-orphan` (tool path) — needs `list`.
- `profile-tool-runtime` — needs `profile`.
- `memory-portability-tool` — needs `list-shards`/`migrate`/`export`/`import`
  (the Rust services exist in `mem-store::shard_inventory` and
  `mem-portability`; the engine exposes no method).

**Residual doc staleness.** `docs/bridge-schema.md`'s `recall` params table
(`:84`) still describes only the legacy shape, and its stub paragraph (`:96`)
is falsified for the canonical shape. The file is out of carve; the docs owner
must add the canonical params and drop the stub sentence.

**V1-only-host limitation.** `plugin/package.json` is unchanged, and the live
host reads no manifest for tools. An upstream-style V1 `hooks.tool` entry would
need `zod` instances (`@opencode-ai/plugin` 1.19.0 typings), i.e. a runtime
dependency `plugin/tests/bundle-boundary.test.mjs` forbids — so a V1-only host
gets no memory tool where upstream has one. If a 1.x host becomes a release
target, reopen that dependency question deliberately.

