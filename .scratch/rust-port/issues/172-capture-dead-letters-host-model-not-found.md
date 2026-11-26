# 172 — auto-capture dead-letters every attempt: `host_model_not_found` for `opencode-go/beam-free`

Status: wontfix — 2026-11-26 owner ruling: `opencodeModel: beam-free` is a deliberate model choice, not a defect.
Filed: 2026-11-26 — **the original premise is falsified; see the correction below before reading on.**

## Owner ruling 2026-11-26

`beam-free` at `opencode-mem.jsonc:86` is an intentional model selection, not a misconfiguration. Owner's words:
*"172 yeah, just a choice."* The three `host_model_not_found` dead letters are the accepted cost of that choice,
against **146** persisted captures. Nothing to fix; the issue stays open only as the record of the contract.

Reopen trigger (written down so a future reader does not re-litigate the choice): dead letters stop being rare —
i.e. `Auto-capture provider failed` approaches the count of `Auto-capture memory persisted`, or the 404 arrives with a
reason other than `host_model_not_found`.

## Correction 2026-11-26 14:05 — "zero successful captures" was a census artifact

The original filing counted `capture.*` **JSON event names** and concluded that the capture path had never had a
success. That count was right and the conclusion was wrong: the log carries auto-capture in **two different line
shapes**, and only one of them is a JSON `capture.*` event. Whole-log census by line family, MEASURED over 10,798
lines:

| line family | count |
|---|---|
| `Auto-capture memory persisted` | **146** |
| `Auto-capture provider ready` | 15 |
| `Auto-capture skipped` (`reason: provider_skip`) | 4 |
| `Auto-capture provider failed` | 4 |
| `Auto-capture final error` | 3 |
| `capture.dead_lettered` (JSON) | 3 |

So capture succeeds overwhelmingly. Store growth corroborates it: `GET /api/memories` reports **`total` 5,845**, up
from 5,601 when the census for issue 167 was taken, and a further capture landed 40 s later (5,846). The plain-text
family is not a JSON event at all — no `level`, no `component` — which is exactly why a `capture\.[a-z_]+` grep could
not see it. **Rule: census by line family, never by the one event shape you expect.**

What survives of the original observation, MEASURED:

- The three `host_model_not_found` 404s are real (2026-11-25 21:50:29, 2026-11-25 23:52:38, 2026-11-26 10:40:40),
  all `providerID=opencode-go;modelID=beam-free`, all classed `config`, each followed by an `Auto-capture final
  error` — those three captures are lost, and 3 failures against 146 successes needs corroborating evidence before
  it is called a systematic defect.
- Retries **do** happen. The fourth failure (`2026-11-26T13:59:52`, `errorKind: network`, `reason:
  summarizer_failed`, captureId `01a216a7-16d3-7524-9ab9-f5b6faf05546`) was persisted at `14:00:20` **under the same
  captureId**, across the engine restart. The original claim "dead_lettered means no retry" is therefore true only of
  the config-class failures, not of the network-class one.

Revised reading: the host resolves a summarizer model per request, and only some requests ask for `beam-free`. The
question is no longer "which capture path is intended" but "which sessions resolve to `beam-free`, and is that the
intended default?" — 3 dead letters against 146 successes is a narrow, intermittent defect, not a silent total loss.

## What was observed (original filing, premise now corrected)

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
- **Whether the dead letters are retried.** `capture.retry_scheduled` does not appear in the log at all. Superseded
  by the correction: retry behaviour is visible directly (the 13:59:52 network failure persisted at 14:00:20 under the
  same captureId), so the absence of that event name means nothing.

## The triage question

Revised after the correction: which sessions resolve to `beam-free`? The host-model path works for 146 of 150
attempts, so this is not "pick a capture path" — it is "why does one model id 404 on some requests". The owner-level
part is narrower: is `opencodeModel: beam-free` (`opencode-mem.jsonc:86`) still intended, given it produces a
`host_model_not_found` 404 whenever the host reaches for it, while the other attempts resolve a different summarizer
and succeed? A config answer, not a code question.

## Not in scope

- Changing the live config: an owner action, and this issue is filed from a read-only check.
- Issue 151/166 (adapter block lifecycle) — unrelated surface.
- Any engine restaging.
