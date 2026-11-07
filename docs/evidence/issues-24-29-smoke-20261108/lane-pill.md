# PILL_CONTRACT lane — bundle 259e2ecdd2554e09 (sha256 38dedc58bcc80d62975be0fb270220899a526130c0f6f73e7e5bc6dfa6a5d455)
LANE_EXPECT=post

| row | kind | fixture | outcome | expected | detail |
| --- | --- | --- | --- | --- | --- |
| pill_is_thaw_button | RED | card + filter pills | PASS | PASS | card=5 filter=1 all thaw-button |
| pill_no_chip_class | RED | card + filter pills | PASS | PASS | no .chip element in the document |
| pill_no_chip_selector | RED | stylesheet text | PASS | PASS | grep -c '\.chip' prints 0 in bundle /home/vaz/Code/Personal/Rust/opencode-agents-memory/.tmp/worktrees/issues-24-29/web-ui/dist/styles-dfc26d953db5e467.css |
| pill_has_stable_testid | RED | card + filter pills | PASS | PASS | memory-tag-pill=5 memory-filter-pill=1 |
| pill_accessible_name_preserved | CONTROL | card + filter pills | PASS | PASS | card="Filter by tag: rust" filter="Remove tag: rust" |
| pill_click_filters_as_before | CONTROL | tagged + untagged records | PASS | PASS | rows=1 tagged requests=1 |
| pill_duplicate_filter_idempotent | CONTROL | rust pill clicked twice | PASS | PASS | filters=1 extra tagged requests=0 |
| pill_remove_filter_as_before | CONTROL | rust filter removed | PASS | PASS | filters=0 rows=3 unfiltered requests=1 |
| pill_overflow_survives | CONTROL | six-tag record | PASS | PASS | pills=4 overflow=<span> |
| pill_keyboard_activation | CONTROL | Enter on the focused rust pill | PASS | PASS | filters=1 tagged requests=1 |
| pill_type_button_no_submit | CONTROL | card pill type attribute | PASS | PASS | type=button on 5 pills, path unchanged |
