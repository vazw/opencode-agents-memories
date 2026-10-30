---
name: memory-usage
description: "How to use the memory tool: its ten modes, the scope rule, the four portability modes, and failures as values."
---

# Memory usage

`memory` is the agent-facing half of opencode-mem. The engine stores memories in per-project
shards; the plugin resolves the project for you. Every call returns one JSON value, and a failure
is `{"success": false, "error": "<token>"}` — never a thrown error.

## The ten modes

| mode | use it when |
|---|---|
| `add` | store a durable fact: what will still matter next session (a deploy gate, a hard-won gotcha, a user preference). `content` is required; `type` and `tags` are optional |
| `search` | find memories by technical keywords; `query` is required |
| `list` | browse recent memories, newest first |
| `profile` | read the user profile; pass `content` to save one explicit preference |
| `forget` | remove one memory by its `memoryId` |
| `list-shards` | inspect the project's shards and orphaned path associations before a migration |
| `migrate` | after a project directory moved, reassociate its orphaned shards |
| `export` | write this project's memories to a portable JSON file |
| `import` | load memories from a portable JSON file (merge-only; aborts on duplicate ids) |
| `help` | list what this build serves |

## Scope and identity

- `scope` defaults to `project`; pass `all-projects` explicitly to search or list across projects.
- The project is resolved by the plugin from the host directory. Do not try to address another
  project: a `projectId` argument is only compared with the host directory, and a mismatch is
  refused.
- The session id comes from the call context. Never pass one, and never invent one.

## Durability

A memory is durable, project-scoped knowledge, not a scratchpad. Prefer `add` for facts that
outlive the session; do not store transcripts, secrets or transient chatter. Content that is
private in its entirety is refused before anything is embedded or written.

## Portability

- `export` and `import` take an absolute path ending in `.json`; the engine writes and reads the
  file. Run `import` with `dryRun: true` first.
- `migrate` takes `fromPath` or `fromHash` (find them with `list-shards`); `dryRun: true` previews
  without touching a file. A source path that still exists is refused unless `allowLinkedSource`
  is true.
- These four modes depend on the engine build. When a mode's capability is absent the tool
  answers `capability_unavailable` and sends nothing.

## Failures

A wire refusal arrives as the tool's `error` — `blank`, `private`, `limit`, `path`,
`from_required`, `import_aborted`, … Read the token instead of retrying blindly.
`engine_unavailable` means no handshake, a transport failure, a timeout, or a malformed answer.
