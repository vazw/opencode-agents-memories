# 79 — plugin · The `memory-usage` skill is not registered through the V2 skill bridge

Status: ready-for-agent — 2026-10-30 (creates `plugin/skills/memory-usage/SKILL.md`, `PLUGIN_SKILL_ID` +
`pluginSkillEntry` in `plugin/src/skills.js` (+ its `dist/` twin), the `readPluginSkill` seam, and the
`"skills"` entry in `plugin/package.json`'s `files`; plugin suite 26 files / 160 tests).

## What is missing

The V2 skill bridge (`plugin/src/skills.js`, landed as issue 69) registers exactly the engine's `skills`
catalogue. A `memory-usage` skill that teaches the agent the `memory` tool's ten modes — the scope rule,
the four portability modes, and failures-as-values — is not shipped at all, so an agent has no in-band
description of the tool it was given. This is a port-owned addition (owner-directed in issue 70 wave 3);
upstream has no counterpart, so no parity row moves.

## What lands

- `plugin/skills/memory-usage/SKILL.md`: frontmatter plus the ten-mode text.
- One static `Skill.Info` (`opencode-mem.memory-usage`) appended **last** to the validated catalogue, with
  an absolute `path` resolved from `import.meta.url`, `content` re-read on every refresh, `autoinvoke:
  false`, never written back to `skills.db`.
- The null law: a failed catalogue read keeps every entry already registered and only re-reads the plugin
  entry; a present-but-empty catalogue still reconciles. An unreadable or empty file logs
  `skills.plugin_entry_unreadable` and skips only that entry; nothing throws.
- Packaging: `package.json` `files` gains `"skills"`; the test harness stages `skills/` so the e2e cases
  read the real file.

## Acceptance criteria

- [ ] `plugin/skills/memory-usage/SKILL.md` exists with `name`/`description` frontmatter (the description
      double-quoted) and the ten modes.
- [ ] `registerSkills` registers the entry last beside the catalogue; S1 (merge/order/absolute path/
      collision/skill body — the mode column compared as a sorted set against `MEMORY_TOOL_MODES`),
      S2 (survives an empty-present reconcile), S3 (unreadable/empty/throwing reader, two-argument
      calls), S4 (catalogue `[alpha]` then `null` → `alpha` still registered, plugin re-read, no throw)
      pass.
- [ ] The eleven existing expectations the static entry moves are updated (six unit, five driver); test
      (7) is renamed to `test_register_skills_registers_only_the_plugin_entry_when_the_catalogue_is_unavailable`.
- [ ] `package.json` `files` includes `"skills"`, pinned by the loader-contract test; `stagePackageTree`
      copies `skills/`.
- [ ] An 11-mutant battery (both twins, `package.json` for MS10, `SKILL.md` for MS11) exits non-zero on
      every mutant and restores byte-identically (`sha256sum -c`, 4 paths), under
      `docs/evidence/issue79-memory-usage-skill-mutations-20261030/`.
- [ ] Plugin suite 26 files / 160 tests / 0 fail / 0 skipped; `skills-bridge.test.mjs` 18;
      `memory-tools.test.mjs` 34.
- [ ] Plugin-only delta: no `.rs`, no `Cargo.toml`/`Cargo.lock`, no cargo invocation; issue 70 untouched.

## Reopens when

The host's skill contract changes (a registration entry point other than `ctx.skill.transform`, or a
`Skill.Info` field this entry must carry), or the owner rules that the entry must be writable/DB-backed. A
change to the ten modes the skill text teaches reopens it too: the text is a contract with the tool's
`help`.

## Out of scope

- Editing issue 70 (closed at `6b518fe`) or reopening the portability wave.
- The engine catalogue's own entries, `skills.db`, and `readSkillCatalogue`.
- `docs/bridge-schema.md` (this entry is not on the wire) and `docs/agents/testing.md` (no upstream test).

## Comments

- **2026-10-30 — filed from the issue 70 wave-3 split.** The r4 plan for that wave bundled this skill with
  the four portability modes; its gate rejected the skill design on R1 (a one-argument `registerSkills`
  call that hit the no-op), R2 (a null-catalogue refresh that evicted every DB skill and was untested) and
  R3 (the file set contradicted itself). Issue 70 landed and closed without the skill; this issue owns it,
  with S4 as the R2 test and the two-argument calls pinned by S3.
