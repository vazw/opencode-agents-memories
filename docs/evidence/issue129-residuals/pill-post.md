## Run — post-change RED→GREEN (same lane, `LANE_EXPECT=post`)

Served bundle: `web-ui-a8f2e4f0c4ebeba8_bg.wasm`, sha256 `88c338f653658823872cee2c63a297afbbbe4e5b7c10929f0511776bd24b5dae` (Step 5 tree, `trunk build --release`), served from a copy under `.tmp/scratch/r9b/dist-post-1` — the pre-change dist was never overwritten.

```
$ TEST_BASE_URL=http://127.0.0.1:3801 WEBUI_DIST=$PWD/.tmp/scratch/r9b/dist-post-1 LANE_EXPECT=post \
  LANE_TABLE=$PWD/.tmp/progress/r9b/pills-post.md PILL_CONTRACT_ONLY=1 \
  node web-ui/scripts/capture-evidence.cjs
rc=0  rows=11 mismatches=0 stopped=0
```

# PILL_CONTRACT lane — bundle a8f2e4f0c4ebeba8 (sha256 88c338f653658823872cee2c63a297afbbbe4e5b7c10929f0511776bd24b5dae)
LANE_EXPECT=post

| row | kind | fixture | outcome | expected | detail |
| --- | --- | --- | --- | --- | --- |
| pill_is_thaw_button | RED | card + filter pills | PASS | PASS | card=5 filter=1 all thaw-button |
| pill_no_chip_class | RED | card + filter pills | PASS | PASS | no .chip element in the document |
| pill_no_chip_selector | RED | stylesheet text | PASS | PASS | grep -c '\.chip' prints 0 |
| pill_has_stable_testid | RED | card + filter pills | PASS | PASS | memory-tag-pill=5 memory-filter-pill=1 |
| pill_accessible_name_preserved | CONTROL | card + filter pills | PASS | PASS | card="Filter by tag: rust" filter="Remove tag: rust" |
| pill_click_filters_as_before | CONTROL | tagged + untagged records | PASS | PASS | rows=1 tagged requests=1 |
| pill_duplicate_filter_idempotent | CONTROL | rust pill clicked twice | PASS | PASS | filters=1 extra tagged requests=0 |
| pill_remove_filter_as_before | CONTROL | rust filter removed | PASS | PASS | filters=0 rows=3 unfiltered requests=1 |
| pill_overflow_survives | CONTROL | six-tag record | PASS | PASS | pills=4 overflow=<span> |
| pill_keyboard_activation | CONTROL | Enter on the focused rust pill | PASS | PASS | filters=1 tagged requests=1 |
| pill_type_button_no_submit | CONTROL | card pill type attribute | PASS | PASS | type=button on 5 pills, path unchanged |

## Step 5 change

- `web-ui/src/core_screens.rs`: the card tag pill and the active-tag filter pill are now Thaw `Button` — `ButtonAppearance::Subtle`, `ButtonSize::Small`, `button_type=ButtonType::Button`, `attr:aria-label` on the button element itself, `attr:data-testid` `memory-tag-pill` / `memory-filter-pill`. The `+N more` overflow stays a `span` (asserted by `pill_overflow_survives`).
- `web-ui/public/styles.css`: the bespoke `.chip` rule is retired (line 798 pre-change). With the class dropped from both sides, the class census stays balanced: `emitted=178 hooks=178 missing=0 dead=0 unresolved=0` (a `.chip` rule left behind would print `dead=1`).
- `.githooks/pre-commit`: `--min-emitted` ratchet re-derived by measurement 179 → 178 (the one retired class token), both the check and its reproduce hint.

## Rendered shape check

`pill-post.png` — `/project-memories` on bundle `a8f2e4f0c4ebeba8` with the three-record fixture. Each pill measures `thaw-button thaw-button--small thaw-button--subtle thaw-button--rounded`, `type="button"`, 40 px tall, labels intact; the six-tag card shows four pills plus the `+2 more` span. The Thaw `Button` shape is visually correct, so the PRD's `InteractionTag` fallback is not taken.
