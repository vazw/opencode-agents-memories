# Issue 152 — host capability for shell liveness (tier 1, read-only)

Measured 2026-11-24 on the host this machine runs. No code or config change, no restart. Every claim
carries its level and its file:line.

## The host

```
$ command -v opencode
/home/opencode/.local/bin/opencode
$ readlink -f "$(command -v opencode)"
/home/opencode/Code/Personal/JS/opencode/packages/cli/dist/cli-linux-x64/bin/opencode
$ opencode --version
opencode v2.3.2
$ git -C /home/opencode/Code/Personal/JS/opencode log -1 --format='%h %ci %s'
b7e2ea5e01 2026-11-18 14:57:04 +0700 fix(cli): point cors test at the relocated service config path
```

MEASURED. The binary is dated `Nov 18 14:57`, the same minute as that commit, and the source tree is
clean — so this tree is a fair reading of the running host. Version `2.3.2` in the tree's
`package.json` matches `opencode --version`.

## V1 hook names do not exist in this host

```
$ rg -a -o --no-heading '(chat\.message|experimental\.session\.compacting|tool\.execute\.(before|after)|shell\.create\.before|execute\.before|execute\.after|session\.compacted|shell\.exited|shell\.created)' \
    /home/opencode/Code/Personal/JS/opencode/packages/cli/dist/cli-linux-x64/bin/opencode | sort | uniq -c | sort -rn
      7 execute.after
      5 execute.before
      3 shell.exited
      3 shell.created
```

MEASURED. `chat.message`, `experimental.session.compacting`, `tool.execute.before`,
`tool.execute.after` and `session.compacted` appear nowhere in the running binary; over the whole host
source, the `tool.execute.*` spelling appears only in the v1 docs under `services/site`. The v1 plugin
hook API is gone from this host generation, so the names in the local SDK 1.19 declaration document a
host this machine no longer runs.

The wrapper detects this at runtime and reports it: the live log line carries
`"host_contract":"setup"` (`~/.opencode-mem/opencode-mem.log`, 2026-11-24T11:01:30), and the wrapper
exports both contracts — `plugin/src/plugin.js:1811` `export async function setup(ctx)` and
`:1838` `export default { id, server, setup }`. The `hooks = { "chat.message": … }` block at
`plugin/src/plugin.js:1398` is the v1 registration path: this host never calls those names, and the
`setup` entry reaches the same handler functions through `plugin/src/v2/adapter.js`.

## Q1 — does the host deliver shell / tool lifecycle to a plugin? **YES**

Three independent surfaces, all reachable from the `setup(ctx)` contract
(`packages/plugin/src/promise/plugin.ts` — `Context.event` at `:35`, `Context.shell` at `:53`,
`Context.tool` at `:56`, `Plugin.setup` at `:66`):

1. **Shell bus events.** `packages/schema/src/shell.ts:58-60`:
   `shell.created` carries the whole `Info`; `shell.exited` carries `{ id, exit, status }`;
   `shell.deleted` carries `{ id }`. The core publishes them at
   `packages/core/src/shell.ts:473` (created), `:1079` (exited), `:336-337`, `:651-652` (killed
   paths), and they are in the client's subscribable event union —
   `packages/client/src/promise/generated/types.ts:4546-4547`.
2. **The wrapper already subscribes to that bus.** `plugin/src/v2/adapter.js:131`
   `for await (const raw of ctx.event.subscribe({ signal: controller.signal }))`, no type filter, and
   every event is forwarded to the legacy `event` hook at `:132-135`, which branches on exactly one
   type — `hookInput.event.type === "session.compacted"` (`plugin/src/plugin.js:1467`) — in the v1
   hook map. So a shell event needs no new subscription; whether `ephemeral` events are delivered to a
   plugin subscriber at all is residual (a), and it is one logged line to settle.
3. **A shell pre-create hook.** `packages/core/src/shell.ts:904`
   `yield* hooks.trigger("shell", "create.before", invocation)`; payload
   `ShellCreateBefore { command, cwd, timeout, shell, env }`
   (`packages/plugin/src/promise/shell.ts:3-13`, whose `ShellHooks` declares `create.before` only —
   no create.after, no exit).

Tool-level lifecycle is separately available: `ToolHooks` declares `execute.before` and
`execute.after` with tool name, sessionID, agent, messageID, call id and input
(`packages/plugin/src/promise/tool.ts:44-61`), exposed as `ToolDomain.hook`
(`:77`) and triggered in the tool runtime at `packages/core/src/tool.ts:129` (before),
`:168` and `:180` (after, with status/result/error).

## Q2 — does anything carry a PID? **YES**

`Shell.Info` has `pid: optional(NonNegativeInt)` (`packages/schema/src/shell.ts:49`), set from the
spawner handle at `packages/core/src/shell.ts:976` (`draft.pid = handle.pid`) and from the process
daemon at `:466`. `shell.created` carries the whole `Info`, so the PID arrives with creation. The
engine's observer takes exactly that shape: `observe_process(pid: u32)`
(`crates/mem-store/src/platform.rs:345`, Linux-gated at `:343`).

## Verdict

- **Q1: YES.** The host emits shell lifecycle and every V2 plugin gets `Context.event`; this
  wrapper's subscription is already open, so no new capability has to be requested — only a branch
  added.
- **Q2: YES.** A creation event carries the shell's PID.

Both answers are yes, so per this issue's acceptance the branch is "the file gains a `ready-for-agent`
brief, and then a planner writes a blueprint".

Named limits and the two runtime residuals are in the issue's outcome section.

## Inverted world

This file is wrong if a plugin subscriber receives neither `shell.created` nor `shell.exited` at
runtime. Two residuals settle it, each in one logged line after the next host restart:

- (a) whether `ephemeral` events (`packages/schema/src/event.ts:128` — no `durable` field) are
  delivered to a plugin subscriber, not only to the UI's data reducer;
- (b) whether `pid` is populated for the background shells this wrapper itself starts.

If both fail, Q1 drops to the `tool.hook("execute.before")` surface alone, which names a shell tool
call but no lifecycle, and 149's action line is the ceiling after all. `shell.list` over the API is
the fallback source for (a) and is a poll, not an event.
