# web UI wave 2 (issue 123) — evidence record

Text-only. Screenshots live outside this directory (`.tmp/artifacts/`); paths and sha256 are appended in Step 8.

## Owner ruling 2026-11-03 (verbatim)

> "text sider + icon would be better."

The 64px icon rail is REJECTED (no `SiderMode`, `RAIL_MIN_VIEWPORT`, `--app-rail-w`). Design Fork 1 is deleted.

## Width table (MEASURED unless marked arithmetic)

| viewport | canvas | sider | body (arith) | panel inner (arith) | cards/row | card | prose | slack/side (arith) |
|---|---|---|---|---|---|---|---|---|
| 1024x800 | 1009 | 260 | 701 | 653 | 1 | 551.5 | 503.5 | 50.8 |
| 1280x800 | 1266 | 260 | 958 | 910 | 1 | 551.5 | 503.5 | 179.3 |
| 1600x800 | 1585 | 260 | 1277 | 1229 | 2 | 551.5 | 503.5 | 51.0 |
| 1920x800 | 1905 | 260 | 1597 | 1549 | 2 | 551.5 | 503.5 | 211.0 |

Source: blueprint webui-wave2-dashboard-r4 §2 (stamp 37f169b334a4).

## Rendered evidence (MEASURED 2026-11-04, lane engine 127.0.0.1:49380 over a copy of the store)

Browser: chrome-devtools-mcp on CDP 127.0.0.1:9222, own page (page 3); the live 3742 page was never navigated. dpr 0.99999998.

| id | result |
|---|---|
| B1 tracks | 1 / 1 / 2 / 2 / 3 at 1024 / 1280 / 1600 / 1920 / 2560 (list width 652 / 908 / 1228 / 1548 / 2188); equals `columns_for(list, 551.47, 24)` |
| B2 measure | list font 14px, card 551.47, content box 503.476, gap 24px at every width |
| B3 overflow | page delta 0 at 639/1024/1280/1600/1920/2560 on /project-memories and /tags; /tags wrapper delta 0 with a 128-char tag cell |
| B4 sticky | bar top 55.99 vs header bottom 56.44 after scrollTo(0,2000) (delta 0.45) — passes only after the Layout fix below |
| B5a chip click | first chip `docs`: +1 `/api/memories` request, `tags=docs`, no `tag=`, 20 cards all carrying it in `data-tags`, unfiltered page had cards without it, range "1-20 of 56" |
| B5b deep link | pre-flight filtered 1 < unfiltered 3885 (OK); `?tags=adr%20placement` -> request `tags=["adr placement"]`, pill present, 1 card whose `data-tags` has it |
| B6 routes | 10 routes: 1 Primary nav, 1 aria-current, 10 nav items each with text and svg, no colon attributes, bg rgb(20,23,28) |
| B7 /tags | 50 ranked rows (rust 442, architecture 338, ...) sorted by count, no `#tag-group-user`, "Memory tags (50 of 4533)" |
| B8 /sessions | no `#session-project-id`; select has 12 options = 12 roster rows; repo option lists 50 sessions; `?project=%2Fhome%2F...` deep link selects the repo |
| B9 focus ring | Tab from `.memories-search` reaches a `.memory-list .chip` on press 2; style solid, 1.64384px == probe 1.64384px, colour rgb(122,162,247) == probe |
| B10 wasm budget | step 4 release bundle 969101 -> 978514 B (+9413 B < 12288); icondata_mdi kept |

Console: one warning on every route (`integrity` attribute ignored for preload — Trunk's own preload link, pre-existing); no errors. Network: no failed requests on /project-memories.

Catch the code/test pass did not flag: `thaw::Layout` nests the page in `.thaw-scrollbar__container { overflow: scroll }` that never scrolls, so `position: sticky` on the header and filter bar was inert (B4 failed first: bar top -1863 vs header bottom -1943). Fixed by rendering a plain `div` wrapper instead of `Layout` (`web-ui/src/shell.rs`).

Blueprint discrepancy: its `range_text` example `(191, 20, 3827) -> "3801-3827 of 3827"` is wrong (page 191 ends at 3820; 3821-3827 is page 192). The test asserts both corrected values.

Artifacts (outside this directory, `.tmp/artifacts/`):

- memories-1920.png sha256 e8360b882bf96fde68bcd1832341a4fa2f22d996088268c262c9b0237d3f294a
- memories-1024.png sha256 ca113d6ec84d45d16526affc54067ffa675f9f27fd053aa68381134204c4aa67
- tags-1280.png sha256 8be4eb78b15ecf4f70af874e78bc6377f732fca606f3ab4360b8b812b4ac2b52
- sessions-1280.png sha256 bfa69d51563ea88417a0f3e8990aa3d268587e90cddbf4a1e6c3a2a45489c800

Not run: the per-criterion mutants (browser and unit). Positive criteria only.
