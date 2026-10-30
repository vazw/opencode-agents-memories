# Issue 79 — the `memory-usage` skill: 11-mutant battery

MEASURED 2026-10-30 in the wave worktree (`.tmp/worktrees/issue-79-memory-usage-skill`, branch
`issue-79-memory-usage-skill`, off `6b518fe`). Driver:
`.tmp/scripts/issue79_skill_mutants.mjs` (gitignored scratch). Before the first mutant it snapshotted the
four production files to `.tmp/scratch/issue79_skill_mutants/snapshots/` and wrote
`.tmp/scratch/issue79_skill_mutants/pre.sha256`; after **every** mutant it restored all four from the
snapshots and verified `sha256sum -c pre.sha256` → 4 × `OK`. No `git checkout --` (the wave is uncommitted)
and no `git status --porcelain` gate (non-empty by construction). Each log below is the full oracle output,
including its `# oracle exit code:` line.

| mutant | file(s) patched | oracle | exit |
|---|---|---|---|
| M01 `plugin-entry-dropped` | `plugin/src/skills.js`, `plugin/dist/skills.js` | `skills-bridge.test.mjs` — S1 + the no-engine e2e witness | 1 |
| M02 `plugin-entry-prepended` | `plugin/src/skills.js`, `plugin/dist/skills.js` | `skills-bridge.test.mjs` — S1 | 1 |
| M03 `plugin-entry-reconciled-away` | `plugin/src/skills.js`, `plugin/dist/skills.js` | `skills-bridge.test.mjs` — S2 | 1 |
| M04 `null-catalogue-evicts` | `plugin/src/skills.js`, `plugin/dist/skills.js` | `skills-bridge.test.mjs` — S4 | 1 |
| M05 `null-catalogue-returns-early` | `plugin/src/skills.js`, `plugin/dist/skills.js` | `skills-bridge.test.mjs` — renamed (7) + S4 | 1 |
| M06 `plugin-read-throws` | `plugin/src/skills.js`, `plugin/dist/skills.js` | `skills-bridge.test.mjs` — S3 | 1 |
| M07 `empty-file-registers` | `plugin/src/skills.js`, `plugin/dist/skills.js` | `skills-bridge.test.mjs` — S3 | 1 |
| M08 `throwing-read-registers-empty` | `plugin/src/skills.js`, `plugin/dist/skills.js` | `skills-bridge.test.mjs` — S3 | 1 |
| M09 `plugin-path-relative` | `plugin/src/skills.js`, `plugin/dist/skills.js` | `skills-bridge.test.mjs` — S1 | 1 |
| M10 `package-files-skills-dropped` | `plugin/package.json` | `v2-loader-contract.test.mjs` | 1 |
| M11 `skill-body-truncated` | `plugin/skills/memory-usage/SKILL.md` | `skills-bridge.test.mjs` — S1 | 1 |

Totals: **11/11 mutants exited non-zero; 11/11 restores verified by `sha256sum -c` (4 × `OK` each);
0 vacuous.**

Notes:

- MS03 inserts the **guarded** delete after `const nextIds = new Set(...)`:
  `if (registeredIds.has(PLUGIN_SKILL_ID)) nextIds.delete(PLUGIN_SKILL_ID);`. The guard is the whole
  difference: the unguarded form is vacuous because it also strips the id from `registeredIds` (the
  repopulation at `plugin/src/skills.js:94-95` in the base file), so the reconcile never sees it. The
  guarded form fires on the second refresh and S2 reds on `removes === ["alpha"]` (measured
  `removes === ["alpha", "opencode-mem.memory-usage"]`).
- MS08 trips S3 leg (a): the throwing reader returns an entry with `content: ""` instead of `null`. The
  README claims nothing about leg (d) for MS08.
- Both twins are patched by every code mutant because the unit cases import `plugin/src/skills.js` and the
  driver cases import the staged `plugin/dist/`; a src-only mutant leaves the e2e witness green.
- A mutant whose oracle passes is a vacuous guard and blocks landing; none did.
