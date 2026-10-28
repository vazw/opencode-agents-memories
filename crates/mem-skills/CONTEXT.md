# mem-skills

**Purpose:** the skills manager: one `skills.db` table plus the per-skill
`SKILL.md` content tree, with the upstream id validation, the
content-file-iff-enabled rule and the change-notification seam.

## Ubiquitous language

| Term | Meaning |
|---|---|
| **skill** | one row of the `skills` table: an id, a name, a description, a body, the `autoinvoke` flag, the `enabled` flag and an optional container tag. |
| **skill id** | the skill's stable handle — lowercase kebab-case (`^[a-z0-9]+(-[a-z0-9]+)*$`, 1–64 characters). It is also the content directory's name and the catalog entry's name. |
| **content file** | `{storage_path}/skills-content/{id}/SKILL.md` — the OpenCode-compatible markdown. It exists **iff** the skill is enabled. |
| **catalog entry** | one `{name, version, files}` triple of the OpenCode HTTP catalog. `name` is the skill id, `version` is the `updated_at` stamp, `files` is the single `{id}.md`. |
| **catalog markdown** | the rendered `SKILL.md` of one **enabled** skill; `None` for an unknown or disabled skill. |
| **change listener** | a callback registered through `on_skills_changed`; the manager notifies every listener exactly once per mutation (create, update, delete). |

## Rules the crate owns

- The id rule is a **whitelist**: every byte outside `[a-z0-9-]` is refused,
  so a path-traversal attempt (`../escape`, `a/b`, `%2F`) can never reach
  the filesystem.
- `create` rejects a duplicate id; `update` rejects a missing row.
- The defaults are `autoinvoke = true`, `enabled = true` and a trimmed
  (possibly empty) description.
- `list` is ordered `updated_at DESC, id ASC`; a container-tag filter keeps
  the global (NULL-tag) skills visible.

## Boundaries

- `mem-core` — the shared kernel (no direct use at the moment; the crate
  owns no configuration of its own).
- `mem-store` — the connection seam (`ConnectionManager`) and the `Db`
  handle the `skills.db` is opened through.

The crate owns NO web surface: `mem-web` adapts it to the REST contract
through the `SkillPort` seam. The bridge's `skills` method (issue 69) reads
this same `list` through `mem-bridge`'s `SkillsService`; the wire carries
`skill_path`'s absolute path.
