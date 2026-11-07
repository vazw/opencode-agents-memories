# DIALOG lane — bundle 259e2ecdd2554e09 (sha256 38dedc58bcc80d62975be0fb270220899a526130c0f6f73e7e5bc6dfa6a5d455)
LANE_EXPECT=post

| row | kind | fixture | outcome | expected | detail |
| --- | --- | --- | --- | --- | --- |
| dialog_role_and_name | RED | memory, session, skill rows | PASS | PASS | /project-memories:"Delete permanently? This cannot be undone." /sessions:"Delete permanently? This cannot be undone." /skill-library:"Delete permanently? This cannot be undone." |
| dialog_escape_disarms | RED | memory row, Escape | PASS | PASS | surface gone, Delete usable, no DELETE issued |
| dialog_focus_trapped_forward_and_reverse | RED | memory row, Tab and Shift-Tab | PASS | PASS | forward and reverse Tab stayed inside |
| dialog_focus_returns_to_invoker | RED | memory row, Cancel | PASS | PASS | focus returned to the same Delete node |
| confirm_removed_invoker_focus_fallback | RED | DELETE held, row removed | PASS | PASS | focus on [data-testid=memories-reload] |
| maintenance_confirm_in_dialog | RED | cleanup armed | PASS | PASS | dialogs=1 confirm=1 |
| maintenance_focus_returns | RED | cleanup armed, cancel then Escape | PASS | PASS | focus on maintenance-action (esc path: dialog present=false) |
| maintenance_inline_action_inert_while_armed | RED | cleanup armed | PASS | PASS | disabled while armed (Confirm cleanup) |
| dialog_cancel_disarms | CONTROL | memory row, Cancel | PASS | PASS | surface gone, no DELETE issued |
| dialog_mask_does_not_close | CONTROL | memory row, outside click | PASS | PASS | clicked 789.0410766601562,518.8131256103516 on DIV.thaw-dialog-surface__backdrop fade-in-transition-enter-from fade-in-transition-enter-active; surface kept |
| dialog_confirm_executes_exactly_once | CONTROL | DELETE held, Confirm twice | PASS | PASS | exactly one DELETE |
| confirm_navigation_does_not_steal_focus | CONTROL | focus moved before resolve | PASS | PASS | focus stayed on memories-reload |
| maintenance_dialog_preserves_notice | CONTROL | cleanup armed then cancelled | PASS | PASS | notice via [data-testid="maintenance-dialog-cancel"], no POST |
| maintenance_exactly_one_cleanup_post | CONTROL | every confirm path in turn | PASS | PASS | exactly one POST /api/cleanup (dialog confirm present=1, inline probe covered) |
