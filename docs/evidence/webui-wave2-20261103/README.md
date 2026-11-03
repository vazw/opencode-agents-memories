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

Artifacts (outside this directory, `.tmp/artifacts/`; re-captured 2026-11-04 on the icon-only toggle build):

- memories-1024.png sha256 aaaa35902adc5108b83b791ac95a89f04f11fab3bc951b6d210d8cb79230be49
- memories-1280.png sha256 a13a5edff08f2175b509858de2a8f8cb63a1db4bc4373e2fad69e9f43801a1ae
- memories-1600.png sha256 d67887b83abf0744d6a51507ca782142f2d57d92ebd7ca6c70c46a847ef871d3
- memories-1920.png sha256 1e9f4620b16bd9d83335461e0d9bad14d208e1824011cbc6672b55cab70efcab
- tags-1280.png sha256 81df3ccd971a79428a2cc7e1fb327884abba81c7cbb6da995b8aec2fdf20aefa
- sessions-1280.png sha256 fd9637b45df2bdb9d4ab418fc8afffb63fba2eb83ecf3b693f19daf9e11d6ddb

(The hashes listed in the earlier run of this record are superseded; those files were overwritten.)

## Icon-only theme toggle + mutant run (MEASURED 2026-11-04, owner directive "remove switch theme text leave only icon")

Toggle: `<button class="theme-toggle" aria-label title>` + svg; `textContent === ""` on all 10 routes at 1024/1280/1600/1920; `aria-label` = `title` = "Switch to light theme"; box 28x28. Header padding trim (`.theme-toggle` 0.4rem -> 0.25rem) puts the header at its 56 px token (bar top 55.993 == header bottom 55.993, B4 delta 0; with the first 0.4rem button the header was 57.637 and B4 delta was 1.644, a render catch the code/test pass did not flag).

Mutants were applied IN THE PAGE (an injected `<style>` or a patched `window.fetch`, both removed afterwards; the served `dist` was not touched), not on a scratch copy of the build. Source-level mutants (Rust code) were NOT RUN: that needs a rebuild per mutant.

| id | mutant | positive | mutant result |
|---|---|---|---|
| B1 | `.memory-list{grid-template-columns:1fr}` | 2 tracks == expected 2 | 1 track: FAILS (killed) |
| B2 | 1fr + card/content max-width removed | content 503.476 | 1499.628: FAILS (killed) |
| B3 | `.tags-tag{white-space:nowrap;overflow-wrap:normal}` with a 128-char cell | wrapper delta 0, page delta 0 | wrapper delta 528: FAILS (killed). Second mutant (`.tags-scroll{overflow-x:visible}`): wrapper delta 528 (killed by the wrapper delta); page delta stays 0 because `.tags-card` has `overflow:hidden`, so the blueprint's "page delta > 1" wording does NOT hold |
| B4 | `.memories-filters{position:static}` | delta 0, top 55.993 | top -1863.66: FAILS (killed); restored passes |
| B5a | fetch patched to drop `tags` | +1 request, `tags=["docs"]`, 20/20 cards carry `docs` | `tags=[]`, cards without chip: FAILS (killed). Patched to send `tag=`: `tag=["docs"]`, 0 cards: FAILS (killed). NOT RUN: "set pill without `reload.run`" (source mutant) |
| B5b | `?tags=adr%2520placement` (what a no-decode `initial_tags` would produce) | request `["adr placement"]`, pill present, 1 card | request `["adr%20placement"]`, no pill, 0 cards: FAILS (killed). Not a source mutant. Pre-flight (two-sided, in page with the injected token header; plain curl gets 401): filtered 1 < unfiltered 3885: PASS |
| B6 | nav item label wrapped in `display:none` | every item has text + svg | FAILS (killed) |
| B9 | `.chip{outline:none}` | solid, 1.64384px == probe, colour == probe | style `none`: FAILS (killed) |
| B9 | `.chip{outline:2px solid var(--border)}` | colour rgb(122,162,247) | rgb(46,53,64): FAILS (killed) |
| B9 | `.chip:focus-visible{outline-width:1px}` | 1.64384px | 0.821918px < 1.14384: FAILS (killed) |
| B9 | `tabindex=-1` on `.memory-list .chip` | reached on Tab press 2 | not reached in 12 presses: FAILS (killed) |

B9 subject: fresh navigation with `location.search === ''`, focus `.memories-search`, Tab until `activeElement.classList.contains('chip') && closest('.memory-list')`.
B5a normalisation: chip text `trim().toLowerCase()`; the probe waits for the new request AND the filter pill before reading `data-tags`.
B7 on this build: 50 ranked rows (442, 338, ...), sorted, no `#tag-group-user`, heading "Memory tags (50 of 4533)". Heading text for the roster group is "Project (26)" (blueprint said "Projects (N)").
B8: select 12 options == `/api/projects` 12; selecting the repo path lists 50 session rows.
B10: current release bundle `web-ui-8b0751abba839df6_bg.wasm` is 1059275 B. The +9413 B icon delta was measured at Step 4 only; it was NOT re-measured at the toggle commit.
Console: only the Trunk preload `integrity` warning; no errors. Network on /sessions: 200s and 304s, no failures.
