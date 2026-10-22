# 26 — W6.4 · Portability & skills manager (US 4.3)

Status: needs-triage

## What to build

Export/import across the **durable** memory tiers — project/global facts, user profiles, and **milestones with their contributors** — plus the skills manager and its bridge. The **raw staging tier** (agent-session rows) is exportable but **opt-in**. Round-trip fidelity of the durable tiers is the product promise. Wave detail: `.scratch/rust-port/roadmap.md` §W6.4.

**The bundle's shape (user ruling, 2026-10-21 — the epic-1 tier is staging, so the default bundle is the durable part):**
- **Always included:** tier-1 project facts, tier-2 global facts, user profiles, and **milestones with their `contributors`** — the durable session history, which upstream has no equivalent of and which the port therefore adds to the export schema.
- **Opt-in:** the raw `agent_sessions` staging rows (an explicit export flag), because they are transient and unbounded.
- A milestone is **self-contained** for portability: its `contributors` embed the attribution and each contributor's retained summary inline, so an imported milestone carries the full recap even when the referenced session rows were not exported (their row ids then dangle as references — accepted, and the import must tolerate it).

## Acceptance criteria

- [ ] Behaviors ported: `memory-portability`, `shared-api-types`, `v2-skills-bridge`, `skills-manager`.
- [ ] Export→import round-trip preserves the durable tiers (facts, profiles, **milestones with contributors**) exactly.
- [ ] The raw staging tier is **opt-in** via an explicit export flag, and a round-trip *without* it still restores the milestones' full recaps (the import tolerates contributor row ids that point at rows it never received).
- [ ] Missing/extra-tier combinations handled without data loss.

## Blocked by

- 15 · W4.1
- 24 · W6.2
