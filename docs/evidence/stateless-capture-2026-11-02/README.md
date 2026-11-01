# Stateless capture — first live measurement (2026-11-02)

**Result: `Auto-capture memory persisted` × 4, zero failures.** Before this re-stage, the engine
had **290** capture attempts and **0** persisted memories, with the last success on
2026-10-31T16:04. This is the measurement the plan deferred as `testable: no` (blueprint step 7,
`sha256=7f17ee5ab687`).

## What changed

Issue 108 replaced the create-session → prompt → delete-session lifecycle with **one** stateless
call per attempt:

```
POST {base}/experimental/generate/text   {"prompt": "<schema + transcript>", "model": "<ref>"}
  → {"data": {"text": "<JSON>"}}
```

No session is created, no agent is named, no `permission` ruleset is sent. The JSON schema travels
inside the prompt text because that route has no `format` field, and `CAPTURE_SYSTEM_PROMPT` no
longer asks for markdown.

## The two states, measured

| | before (sha `4b0177178c4a`) | after (sha `543159561997`) |
|---|---|---|
| capture attempts today | 290 | 4 |
| persisted | **0** | **4** |
| `final error` after 5 attempts | 58 | 0 |
| failure signature | `{"errorKind":"http-status","reason":"summarizer_failed","status":503}` | none |
| last success | 2026-10-31T16:04:39+07:00 | 2026-11-02T02:59:21+07:00 |

Raw log lines: `post-restart-log.txt` (filtered to entries after the restart timestamp; nothing
retyped).

## The re-stage, as performed

1. `cargo build --release` at `4057162` → sha `543159561997` (36.49 s).
2. Rollback point taken **first** from the running binary:
   `.tmp/scratch/engine-rollback-20261101T195830Z/opencode-agents-memory.4b0177178c4a`, with the
   restore command recorded beside it.
3. New binary staged as `plugin/engine/linux-x64/.opencode-agents-memory.new`, `chmod +x`, sha
   verified, then **`mv`** into place — a rename, never a copy over the running inode (`stage-engine.sh`
   does an in-place `cp` and hits `ETXTBSY` against a live binary).
4. `SIGTERM` to the single engine process (pid 5150). The wrapper's `retireChild` saw a handled
   exit, `autoRestarts` went 0 → 1 (budget `MAX_AUTO_RESTARTS = 1`), state → `cold`.
5. One memory call triggered the lazy respawn — **the wrapper started it, no manual spawn**. New
   pid 14221, sha `543159561997`, parent `opencode` pid 4318.

## What this proves, and what it does not

**Proved:** the configured model returns parseable JSON for a real capture-sized prompt carrying a
schema and one JSON-only instruction; the reply decodes through the new `{"data":{"text":…}}` path;
the memory is embedded, stored, and **retrievable** (a search for rework-specific text returned
`auto-capture-e00d34911ac1…`, `source: auto-capture`, whose content summarises the rework session
itself); and no session debris is created (0 sessions titled `opencode-mem capture` in the newest
200).

**Not proved:** the two blind spots recorded in ADR-0029 D-29-9 and D-29-10 — the lenient parser can
still prefer a well-formed *example* in the model's reply over its answer, and a `>30 s` retry hint
still removes a capture from the retry path permanently (tracked as issue 110).

## Reproduce

```sh
grep -E 'Auto-capture (memory persisted|skipped|final error)' ~/.opencode-mem/opencode-mem.log | tail -1
```
