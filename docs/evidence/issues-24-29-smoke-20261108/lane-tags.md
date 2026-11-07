# TAGS lane — bundle 259e2ecdd2554e09 (sha256 38dedc58bcc80d62975be0fb270220899a526130c0f6f73e7e5bc6dfa6a5d455)
LANE_EXPECT=post

| row | kind | fixture | outcome | expected | detail |
| --- | --- | --- | --- | --- | --- |
| tag_link_color_not_browser_default | RED | tag anchors on /tags | PASS | PASS | color=rgb(122, 162, 247) (not browser default), underline=underline |
| tag_link_resolves_accent_token | RED | tag anchors on /tags | PASS | PASS | color=rgb(122, 162, 247) == probe-converted --accent=#7aa2f7 |
| tag_rule_declares_token_not_literal | RED | stylesheet text | PASS | PASS | :where(a) { color: var(--accent);   text-decoration-line: underline; } in bundle /home/vaz/Code/Personal/Rust/opencode-agents-memory/.tmp/worktrees/issues-24-29/web-ui/dist/styles-dfc26d953db5e467.css |
| tag_rows_render_and_href_preserved | CONTROL | 2 ranked memory tags | PASS | PASS | links=2 hrefs intact (first="rust") |
| nav_anchor_color_unchanged | CONTROL | nav anchor vs fg2 token | PASS | PASS | nav=rgb(232, 234, 237) == probe-converted --colorNeutralForeground2=#e8eaed |
