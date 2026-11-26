# Issue 145 — R2/R3 against a real subagent, 2026-11-26

One measured child, whole loop automated: R2 setup by message, compaction by overflow, R3 by follow-up.
The result is a **delivery success and an instrument failure** — the runbook's Q6 question, exactly as
written, cannot be trusted.

## Setup

- Child: `ses_de871b890ffeb7fc6GwwcmlSll`, spawned as a `syseng` subagent of `ses_e9d5dd698ffe5lUV7W7s2KDpbh`,
  directory this repository.
- R2 action: one named background shell (`sleep 900`, host id `sh_2pqfsnjl`), then one known action — `read`
  of `/etc/hostname` (content `Gigabyte`).
- Host `opencode v2.4.2` pid 15805; R1 satisfied, `inject.received` carries `"host_contract":"setup"` (232
  lines at 2026-11-26T11:11:03Z). Resolved `chatMessage.injectOn`: absent from config → default `first`.

## How compaction was triggered, and what could not be used

| route | outcome |
|---|---|
| `POST /api/session/:id/compact` (`packages/protocol/src/groups/session.ts:513`) | exists, but the host answers **401**; auth is HTTP Basic with a server password (`packages/server/src/middleware/authorization.ts:44`) |
| `opencode api …` — the CLI path that carries that credential | **denied by the shell permission layer** (`permission.rejected`); not worked around |
| `opencode run -s <child> "/compact"` | sent as literal text — the child answered "Nothing to compact on my side"; slash commands are expanded only on `session.command` (`packages/server/src/handlers/session.ts`) |
| overflow auto-compaction | **used**: a 620 025-byte locally generated pad, attached with `opencode run --file` |

## Timeline (UTC)

| time | event |
|---|---|
| 11:12:10 | turn 0 inject: `status:"ok"`, `block_bytes:2932`, `turn_index:0` |
| 11:13:37.066 | boundary 1 — `session_message` seq 43, `type='compaction'` (oversized turn) |
| 11:13:54 | child answers the oversized turn: Q6 `NONE`, Q7 correct (`read /etc/hostname`) — **no inject after the boundary** |
| 11:14:33.991/34.390 | turn 3 inject: `status:"ok"`, `block_bytes:3595` — the post-compaction refresh |
| 11:14:47 | child answers: Q6 `NONE` (nonce B was a bare 22-char fact), Q7 correct |
| 11:15:24 | nonce C written **after** the refresh → never injectable (cache live in `first` mode) |
| 11:15:55.329 | boundary 2 — seq 94 (second oversized turn) |
| 11:16:21 | nonce D `PROBE-NONCE-9399b46e` written **after** the boundary |
| 11:16:30.226/30.628 | turn 7 inject: `status:"ok"`, `block_bytes:2914`, and the block **contains** `## Project facts\n<memory relevance="70%">PROBE-NONCE-9399b46e — issue 145 deliver…` |
| 11:16:5x | child answers the refresh turn: Q6 `NONE` — a false negative |
| 11:17:2x | directed read ("quote the first entry under `## Project facts`") → verbatim `PROBE-NONCE-9399b46e — issue 145 delivery probe run token, c…` |

Final state: `session_message` types `assistant 11, compaction 2, idle 9, user 9`; three inject pairs total
(turns 0, 3, 7); the last inject is turn 7 and no inject followed it, so the directed read quoted the same
block the `NONE` answers were given against.

## Reading

**Delivery: present (arm b).** The engine built a block for the child, the host logged it with
`status:"ok"` and 2914 bytes against that child's session id, the child's prompt carried it, and the child
reproduced a 60-character string from it verbatim. `Q9` is positive at the boundary-crossing turn; `Q6`
is positive when asked correctly.

**Instrument: the runbook's Q6 is invalid.** Four free-recall questions ("quote any string matching
`PROBE-NONCE-<8 hex>`") produced four `NONE` answers, one of them against a context that demonstrably
contained the token. A directed question ("first entry under `## Project facts`") returned it first try.
The failure is a model-attention failure on a pattern-search question over a synthetic block, not a
delivery failure. Therefore **`Q6 = 0` cannot be read as `miss-host`** — the runbook's first-match rule
(`miss-host`: engine `ok`, `block_bytes > 0`, `Q6 = 0`) would have declared a delivery failure here that
did not happen.

## Two protocol defects this run exposed, now corrected

1. **The compaction turn never injects.** Auto-compaction runs before the model call inside the same turn;
   the rebuilt request re-runs no `context` hook (`inject.requested` absent for that turn). Its question can
   only be answered from the pre-compaction block, so it must not carry the probe. Question turns must come
   **after** the boundary.
2. **`first` mode caches between refreshes** (`plugin/src/v2/adapter.js:86-92`). A nonce written after a
   refresh is never injected until the next compaction or retry. The nonce must therefore be written in the
   window **boundary → first prompt**, and that first prompt is the question turn.

Corrected per-child shape: pad turn → verify boundary in `session_message` → write the nonce as a
semantically rich memory (a bare token ranks low and may not be selected) → ask a **directed** question.

## Second batch — four more children, and a model-free way to read the refresh

Children 2–5 (`ses_de7eedf22ffe…`, `…21ffe…`, `…12ffe…`, `…11ffe…`), same protocol, action tokens
`/proc/version`, `/etc/os-release`, `/etc/shells`, `/proc/cmdline`. Each got one 620 025-byte pad turn and then
one prompt after the boundary.

| child | boundary (UTC) | turn-0 block | post-boundary block | sections |
|---|---|---|---|---|
| c2 | 13:35:25.072 | 3598 B | **3231 B** | `Project facts, Recent sessions` → `+ Recent actions` |
| c3 | 13:35:25.188 | 3598 B | **3702 B** | same gain |
| c4 | 13:35:17.695 | 3598 B | **3139 B** | same gain |
| c5 | 13:35:25.127 | 3598 B | **3245 B** | same gain |

All four post-boundary injects are `status:"ok"`, all at `turn_index:2`. **6/6 boundaries across five children
produced a post-boundary inject** when a prompt followed, and every one of them produced a block that the turn-0
block could not have been: the `Recent actions` section is new, and the byte counts move. That is a model-free
reading — it needs no question answered.

Delivery-to-model is **not** re-established for these four. A nonce was written for c4 only; it does not appear in
that child's truncated `block_prefix`, and the child answered `NONE` to a directed question and to a lexical one
("an entry that mentions a probe run for child 4"). Two explanations remain and this run does not separate them:
selection ranking left the entry out of the block, or the entry was in the block beyond the truncation and the
model did not report it. So c4's `NONE` is **inconclusive, not a miss** — the same trap the pilot exposed.

### What a future run needs

The instrument that worked is the one with a log-side witness: write the nonce, then **require the token to appear
in `inject.received`'s `block_prefix` before asking the child anything**. If it is not there, the probe is
inconclusive and no reading may be taken from the answer. That check only works when the entry lands early in the
block, so a reliable run also needs a way to raise its rank — or an engine-side debug surface that returns the
whole built block, which does not exist today.
