## Run — pre-change baseline (RED/CONTROL gate)

> **REGENERATED 2026-11-06 (rework pass, review-1 HIGH finding).** The table below replaces the
> original run's table: the shipped `maintenance_exactly_one_cleanup_post` row had hardcoded
> post-tree probe semantics, so the original pre-mode was NOT re-derivable (the reviewer measured
> `mismatches=1` re-running it). The row is now branched on `LANE_EXPECT` and the pre table was
> regenerated from the preserved bundle — same bundle id and sha256 as the original run, rows=14
> (the original prose said 13; the shipped script counts 14). Raw: `.tmp/progress/r9b-rework/dialog-pre-regen.{log,md}`.

Served bundle: `web-ui-72f5eaea5edb020c_bg.wasm`, sha256 `1cb2c66974848c285279e82b2d9631ebc1fe476a9a36d1df2e7fb7d1de25142a` — the wave-0 build preserved under `.tmp/scratch/issue129-shardB/dist` and served by its own engine (`--web-port 49392` via `--web-dist`), because `trunk build` overwrites `web-ui/dist` in place and the served bundle must never be swapped under a lane.

```
$ TEST_BASE_URL=http://127.0.0.1:49392 WEBUI_DIST=$PWD/.tmp/scratch/issue129-shardB/dist \
  LANE_EXPECT=pre LANE_TABLE=$PWD/.tmp/progress/r9b-rework/dialog-pre-regen.md DIALOG_ONLY=1 \
  node web-ui/scripts/capture-evidence.cjs
rc=0  rows=14 mismatches=0 stopped=0
```

Every row starts with the armed-surface guard (`[data-testid="confirm-gate"],[data-testid="confirm-dialog"]` must match exactly one element, `confirm-gate-confirm` and `confirm-gate-cancel` exactly once each). A guard miss is recorded as `STOP` — a fixture failure, never a RED. Supplies are real: `Input.dispatchMouseEvent` at the element centre for every click (never `element.click()`), `Input.dispatchKeyEvent` with the browser default action live for Tab/Shift-Tab/Enter/Escape, and fetch fixtures built with `new Response`.

# DIALOG lane — bundle 72f5eaea5edb020c (sha256 1cb2c66974848c285279e82b2d9631ebc1fe476a9a36d1df2e7fb7d1de25142a)
LANE_EXPECT=pre

| row | kind | fixture | outcome | expected | detail |
| --- | --- | --- | --- | --- | --- |
| dialog_role_and_name | RED | memory, session, skill rows | FAIL | FAIL | /project-memories: 0 [role="dialog"] element(s) |
| dialog_escape_disarms | RED | memory row, Escape | FAIL | FAIL | the armed gate/dialog remains after Escape |
| dialog_focus_trapped_forward_and_reverse | RED | memory row, Tab and Shift-Tab | FAIL | FAIL | Tab from the last action left the armed surface |
| dialog_focus_returns_to_invoker | RED | memory row, Cancel | FAIL | FAIL | focus went to BODY , not the original Delete node |
| confirm_removed_invoker_focus_fallback | RED | DELETE held, row removed | FAIL | FAIL | focus stayed on BODY, not the memories Reload fallback |
| maintenance_confirm_in_dialog | RED | cleanup armed | FAIL | FAIL | no dialog holds the final confirm (dialogs=0 confirm=0) |
| maintenance_focus_returns | RED | cleanup armed, cancel then Escape | FAIL | FAIL | focus stayed on BODY, not the maintenance action |
| maintenance_inline_action_inert_while_armed | RED | cleanup armed | FAIL | FAIL | the inline action is still enabled while armed (Confirm cleanup) |
| dialog_cancel_disarms | CONTROL | memory row, Cancel | PASS | PASS | surface gone, no DELETE issued |
| dialog_mask_does_not_close | CONTROL | memory row, outside click | PASS | PASS | clicked 685.2996368408203,628.8184585571289 on DIV.memories-pager; surface kept |
| dialog_confirm_executes_exactly_once | CONTROL | DELETE held, Confirm twice | PASS | PASS | exactly one DELETE |
| confirm_navigation_does_not_steal_focus | CONTROL | focus moved before resolve | PASS | PASS | focus stayed on memories-reload |
| maintenance_dialog_preserves_notice | CONTROL | cleanup armed then cancelled | PASS | PASS | notice via [data-testid="maintenance-cancel"], no POST |
| maintenance_exactly_one_cleanup_post | CONTROL | every confirm path in turn | PASS | PASS | exactly one POST /api/cleanup via the inline action (pre: no dialog) |

## Focus-return targets (declared before the run)

| site | invoker | fallback when the row is gone |
| --- | --- | --- |
| memory row | `memory-delete` | `memories-reload` |
| session row | `delete-session` | `load-sessions` |
| skill row | `skill-delete` | `skill-create` |
| maintenance | `maintenance-action` | the same button (it stays mounted; `disabled` is the site's own state) |

The lane asserts the memory pair (`dialog_focus_returns_to_invoker`, `confirm_removed_invoker_focus_fallback`); the other three are the same helper call with a site-chosen fallback, and the maintenance pair is asserted by `maintenance_focus_returns`.

## Pre-change behaviour observed by the RED rows

- `ConfirmGate` (`confirm.rs`, pre-change) is an inline `span` with no `role="dialog"`, no Esc handler, no mask and no Tab wrap; arming replaces the row's Delete button, so focus can only return to `<body>`.
- Maintenance keeps an inline `maintenance-cancel` and a still-enabled `maintenance-action` ("Confirm cleanup") while armed, so the final confirm is a second click on an always-visible button rather than a modal decision.
- `maintenance_exactly_one_cleanup_post` records `dialog confirm present=0` pre-change: the same row becomes the proof that the *dialog* path issues exactly one POST once the site moves. (Regenerated run wording: "exactly one POST /api/cleanup via the inline action (pre: no dialog)" — same fact, after the row's pre branch was restored per review-1 HIGH.)
