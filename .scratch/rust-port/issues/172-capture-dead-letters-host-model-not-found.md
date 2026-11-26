# 172 — auto-capture dead-letters every attempt: `host_model_not_found` for `opencode-go/beam-free`

Status: needs-triage — filed 2026-11-26 from a post-restart health check; no fix attempted.
Filed: 2026-11-26

## What was observed

The live log (`~/.opencode-mem/opencode-mem.log`, read-only) holds exactly **three** `capture.*` events, and every one
of them is a failure:

```
[2026-11-25T21:50:29.017+07:00] capture.dead_lettered: {"capture_id":"01a2132f-…","component":"capture",
  "detail":"providerID=opencode-go;modelID=beam-free","error_kind":"config","level":"warn",
  "project_id":"/home/opencode/Code/Personal/Rust/opencode-agents-memory","reason":"host_model_not_found","status":404}
[2026-11-25T23:52:38.440+07:00] capture.dead_lettered: … same detail, reason, status
[2026-11-26T10:40:40.494+07:00] capture.dead_lettered: … same detail, reason, status
```

MEASURED counts over the whole log: `capture.dead_lettered` **3**, any success event (`capture.completed`,
`memory.captured`) **0**. So the capture path has not had a single successful run on record.

SOURCE, MEASURED: the live config `/home/opencode/.config/opencode/opencode-mem.jsonc:86` sets
`"opencodeModel": "beam-free"`. Grepping the whole of `~/.config/opencode/` for `beam` returns that line and nothing
else — no other file names the model, and no provider listing in the tree mentions it. The host answers 404 with
`reason: "host_model_not_found"`, which is the host saying the id does not exist rather than the engine failing to
reach it.

## Why it matters

The error class is `config`, not `transient`, and `dead_lettered` means the engine has given up on that capture rather
than scheduling a retry. The visible consequence is silent: memories from those sessions are never captured, and
nothing in the transcript says so — the plugin still injects a block, the engine still serves search, and the log line
sits at `level: warn`.

It also means the memory store's growth is not explained by capture. 5,601 memories exist while capture has zero
successes on record, so they arrived by another path (a manual `memoryApiKey`/`memoryApiUrl` configuration also
present at `:107`, or direct writes). Which path is active, and whether it is intended, is the first thing to settle —
see the triage question below.

## What is not yet established

- **Whether `beam-free` was ever valid.** There is no model list in the tree to check against. If the id was renamed
  upstream, this is a config drift; if it never existed, the config has been wrong since it was written.
- **Whether capture is meant to run at all.** `opencode.jsonc`-level config may select a different provider, and the
  manual API block may be the intended path. Deciding this needs the owner.
- **How often capture is supposed to fire.** Three events in ~15 h could be every session end or a small fraction of
  them; the log does not say what triggers it, so the loss rate is not yet bounded.
- **Whether the dead letters are retried.** `capture.retry_scheduled` does not appear in the log at all, so nothing was
  scheduled — but that is an absence of evidence, not a measured retry policy.

## The triage question

Which capture path is intended: the host-model path (`opencodeProvider`/`opencodeModel` at `:86`, currently broken), or
the manual API path (`memoryApiKey`/`memoryApiUrl`/`memoryModel` at `:107`)? The answer decides whether this is a
one-line config fix or a deeper wiring question, and it is the owner's call rather than a code question.

## Not in scope

- Changing the live config: an owner action, and this issue is filed from a read-only check.
- Issue 151/166 (adapter block lifecycle) — unrelated surface.
- Any engine restaging.
