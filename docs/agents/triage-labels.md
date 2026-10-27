# Triage Labels

The skills speak in terms of five canonical triage roles. This file maps those roles to the actual label strings used in this repo's issue tracker. In this repo's local-markdown tracker, the strings below are written as the `Status:` line of each issue file.

| Label in mattpocock/skills | Label in our tracker | Meaning                                  |
| -------------------------- | -------------------- | ---------------------------------------- |
| `needs-triage`             | `needs-triage`       | Maintainer needs to evaluate this issue  |
| `needs-info`               | `needs-info`         | Waiting on reporter for more information |
| `ready-for-agent`          | `ready-for-agent`    | Fully specified, ready for an AFK agent  |
| `ready-for-human`          | `ready-for-human`    | Requires human implementation            |
| `wontfix`                  | `wontfix`            | Will not be actioned                     |

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), use the corresponding label string from this table.

Edit the right-hand column to match whatever vocabulary you actually use.

## Other local states (not mattpocock roles)

`paused` / `PAUSED` — blocked on another lane or an owner ruling. Keeps the issue out of the triage board
until the blocker moves; the status line names the blocker. In use: 39 (behind 43), 41 (behind 39), 58
(three unfixable zstd advisories).

## Local closure convention (not a mattpocock role)

`done` — terminal state for finished issues: wave completed and verified, evidence digest under `## Comments`. Close-out applies it as `Status: done — <yyyy-mm-dd>`. Adopted 2026-10-18 (issues 01–04 backfilled).
