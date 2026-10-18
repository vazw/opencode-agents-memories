# 16 — W4.2 · Timeline queries & recall tool schema (US 1.3, 1.4)

Status: needs-triage

## What to build

`mem-episodic::timeline` — chronological, semantic and filtered queries over sessions (time range, `last_n`, touched files, agent id); orphan-free linkage between episodic records; and the typed `recall_project_sessions` tool schema (registration happens in W5.4). Wave detail: `.scratch/rust-port/roadmap.md` §W4.2.

## Acceptance criteria

- [ ] Behavior ported: `memory-timeline-orphan` (no orphaned timeline entries after delete/link operations).
- [ ] Filter-matrix tests: time range × last_n × touched files × agent id combinations behave per upstream semantics.
- [ ] Semantic query path reuses vector search (exact fallback per W1.3).
- [ ] `recall_project_sessions` schema defined and documented (registration deferred to W5.4).

## Blocked by

- 15 · W4.1
