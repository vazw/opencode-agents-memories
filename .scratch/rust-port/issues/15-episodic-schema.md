# 15 — W4.1 · Episodic schema & scoping — new agent-session tier (US 1.1)

Status: needs-triage

## What to build

`mem-episodic` — the new per-project agent session (episodic) memory tier: `AgentSession`/`Episodic` record type bound to `project_id`/`session_id`/`agent_id`, structured metadata payload, per-session embeddings, and schema migrations extended for the new tables. This is net-new domain (no upstream service to port) — write the Rust acceptance tests for the record shape first, mirroring the intent of upstream child-session capture. Wave detail: `.scratch/rust-port/roadmap.md` §W4.1.

## Acceptance criteria

- [ ] New Rust tests define and verify the episodic record shape (scope binding, metadata payload, embedding linkage).
- [ ] Fresh-DB migration test creates the episodic schema from scratch; existing `schema-migrations` suite stays green (upgrade path).
- [ ] Records are scoped per project/session/agent (no cross-project bleed).
- [ ] Embedding column lives behind the same vector plumbing as W1.3 (reuses `mem-store`).

## Blocked by

- 04 · W1.1
- 06 · W1.3
