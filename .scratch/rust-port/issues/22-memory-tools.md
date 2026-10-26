# 22 — W5.4 · Memory tools registration — existing tools + recall (US 1.4)

Status: ready-for-agent — 2026-10-27 (verified absent at HEAD: the wrapper registers no tools and `recall` is a hard-coded empty stub; brief under `## Agent Brief`)

## What to build

Register all agent-facing memory tools with the host: the ported memory tools plus the new `recall_project_sessions`, with per-tool scope rules and the portability tool path. Wave detail: `.scratch/rust-port/roadmap.md` §W5.4.

## Acceptance criteria

- [ ] Behaviors ported: `tool-scope`, `memory-timeline-orphan` (tool path), `memory-portability-tool`, `profile-tool-runtime`.
- [ ] `recall_project_sessions` registered and callable with the W4.2 schema.
- [ ] Per-tool scope rules enforced (tools cannot read outside their scope).

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
- [ ] The issue's three ACs, each pinned by a test that fails on today's tree.
- [ ] `recall` no longer returns the unconditional empty response: a scope with rows returns them, and a
      project-scoped call never returns another project's rows.
- [ ] Per-tool scope enforcement is tested with a negative case per tool.

**Out of scope:**
- The V2 adapter (issue 20) — tools register against the host surface that exists.
- The compaction hooks (issue 33).
- Any change to the tool schema, which is already defined.
