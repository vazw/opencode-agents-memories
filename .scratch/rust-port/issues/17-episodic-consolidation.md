# 17 — W4.3 · Episodic consolidation & compaction (US 1.3, 1.5)

Status: needs-triage

## What to build

Age-based consolidation of episodic records into milestones, promotion of durable facts into permanent memory, and compaction that preserves agent identity/attribution. Interacts with the confidence/decay layer (W3.3) via clean service seams. Wave detail: `.scratch/rust-port/roadmap.md` §W4.3.

## Acceptance criteria

- [ ] Behavior ported: `compaction-agent-preservation` (agent attribution survives compaction).
- [ ] Age-threshold tests: eligible records consolidate; ineligible untouched (thresholds from config, W0.2).
- [ ] Promoted facts land in the permanent memory tier with normal scope rules.
- [ ] Cleanup/dedup interplay verified (no double-processing with W3.3 cleanup).

## Blocked by

- 09 · W2.1
- 11 · W2.3
- 15 · W4.1
