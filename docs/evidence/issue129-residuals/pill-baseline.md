## Run — pre-change baseline (RED/CONTROL gate)

Served bundle: `web-ui-72f5eaea5edb020c_bg.wasm`, sha256 `1cb2c66974848c285279e82b2d9631ebc1fe476a9a36d1df2e7fb7d1de25142a` (wave-0 build, unmodified).

```
$ TEST_BASE_URL=http://127.0.0.1:3799 WEBUI_DIST=$PWD/web-ui/dist LANE_EXPECT=pre \
  LANE_TABLE=$PWD/.tmp/progress/r9b/pills-pre.md PILL_CONTRACT_ONLY=1 \
  node web-ui/scripts/capture-evidence.cjs
rc=0  mismatches=0 stopped=0
```

Gate: every RED row must FAIL and every CONTROL row must PASS on the pre-change tree. `STOP` would mean the fixture never reached the actor (a lane failure, not a RED); `mismatches` counts deviations from the declared classification and exits 1.

# PILL_CONTRACT lane — bundle 72f5eaea5edb020c (sha256 1cb2c66974848c285279e82b2d9631ebc1fe476a9a36d1df2e7fb7d1de25142a)
LANE_EXPECT=pre

> Scope note (rework 2026-11-06, review-1 MEDIUM): `pill_no_chip_selector` reads the stylesheet of
> the BUNDLE under test (`WEBUI_DIST`), falling back to the source file only when the dist carries
> no styles entry — the detail line names which was read. This row IS re-derivable from a preserved
> bundle; the original run's `styles.css prints 1` wording predates that fix (the original run
> predated the bundle-scoped read and used the same source file, which at that time still held the
> `.chip` rule). Regenerated pre run: `.tmp/progress/r9b-rework/pill-pre-regen.{log,md}` —
> `FAIL … prints 1 in bundle …/dist/styles-e9d653cd395631f3.css`, 11 rows, 0 mismatches, rc=0.

| row | kind | fixture | outcome | expected | detail |
| --- | --- | --- | --- | --- | --- |
| pill_is_thaw_button | RED | card + filter pills | FAIL | FAIL | 5 card pill(s) of 5 and 1 filter pill(s) are not thaw-button |
| pill_no_chip_class | RED | card + filter pills | FAIL | FAIL | 2 element(s) still carry class "chip" |
| pill_no_chip_selector | RED | stylesheet text | FAIL | FAIL | grep -c '\.chip' styles.css prints 1 |
| pill_has_stable_testid | RED | card + filter pills | FAIL | FAIL | memory-tag-pill=0 of 5 card pills, memory-filter-pill=0 of 1 filter pill |
| pill_accessible_name_preserved | CONTROL | card + filter pills | PASS | PASS | card="Filter by tag: rust" filter="Remove tag: rust" |
| pill_click_filters_as_before | CONTROL | tagged + untagged records | PASS | PASS | rows=1 tagged requests=1 |
| pill_duplicate_filter_idempotent | CONTROL | rust pill clicked twice | PASS | PASS | filters=1 extra tagged requests=0 |
| pill_remove_filter_as_before | CONTROL | rust filter removed | PASS | PASS | filters=0 rows=3 unfiltered requests=1 |
| pill_overflow_survives | CONTROL | six-tag record | PASS | PASS | pills=4 overflow=<span> |
| pill_keyboard_activation | CONTROL | Enter on the focused rust pill | PASS | PASS | filters=1 tagged requests=1 |
| pill_type_button_no_submit | CONTROL | card pill type attribute | PASS | PASS | type=button on 5 pills, path unchanged |
