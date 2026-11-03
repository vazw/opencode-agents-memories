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
