# 26 — W6.4 · Portability & skills manager (US 4.3)

Status: needs-triage

## What to build

Export/import across all memory tiers — including agent-session histories from day one — plus the skills manager and its bridge. Round-trip fidelity is the product promise. Wave detail: `.scratch/rust-port/roadmap.md` §W6.4.

## Acceptance criteria

- [ ] Behaviors ported: `memory-portability`, `shared-api-types`, `v2-skills-bridge`, `skills-manager`.
- [ ] Export→import round-trip preserves all tiers including episodic records.
- [ ] Missing/extra-tier combinations handled without data loss.

## Blocked by

- 15 · W4.1
- 24 · W6.2
