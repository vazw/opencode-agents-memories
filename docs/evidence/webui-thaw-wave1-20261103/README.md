# web-ui Thaw wave 1 — render evidence (text only)

Issue 118; blueprint `webui-thaw-wave1-shell-r6` (sha256 header `6d54b8762eae`). Binary artifacts live in
`/home/vaz/Code/Personal/Rust/opencode-agents-memory/.tmp/scratch/webui-wave1-20261103T120833Z/` (gitignored); this file carries paths and hashes only.

- build: `cd web-ui && trunk build` → exit 0; bundle `web-ui-321d4859fd9a772f_bg.wasm`, `styles-51057aba76b0c15d.css`
- before method: served app at `http://127.0.0.1:3742` (bundle `52c1081f…`), chrome-devtools MCP, isolated context, 1280×800, `probe.js` unchanged → `before.json`
- after method: own build served by `web-ui/scripts/serve-dist.mjs --port 8123 --root web-ui/dist`, same probe/window → `after-dark.json`, `after-light.json`; no API, so data screens render their error branch (structural run; not comparable to before.json heights)
- screenshots: 20 PNGs (10 routes × dark/light), viewport 1280×800 captured full-page (all routes short in the preview; no capture timed out)
- console: only the pre-existing `integrity` preload warning; 0 errors on all routes, both themes
- network: no failed requests recorded other than the expected API errors of the API-less preview

## sha256
```
6fa506b5d58c2622f4e1c2efa1795339c06f216babcfb35d3c15db53ffa087d9  after-dark-agents.png
df657857ee4c828f95ceed88f24b0045991346b60536496d88e34f8819576531  after-dark-export-import.png
c46e42d99b5d651609aa6a33f51668a9587789b534e1d5526c30b350eac739d6  after-dark-home.png
a4ae9e2d4f2804ba6493dd2368ddb818ea7ea6b4c220d5661464f8b6947a4eaf  after-dark-project-memories.png
836c421d3f57240d750d67ac9dd42000467006b8687f2a6629d17c1a48c83268  after-dark-sessions.png
31b7c8fb27c79a2075ae887ba64c84961b8a7b57a0e92d10ba1abd3390f9b7a1  after-dark-settings.png
5a47316fc99685b401ae1a3acffca93562d72f47491a83a04d993b10e75dde72  after-dark-skill-library.png
24782b4cba4ea5f19b01cf6457cbb1081a6c24bd183e30852da263631e82a3e0  after-dark-stats.png
cbe8e3f9a5df0ab23cf6e706bebc2f617f3c0e34fbe6ae1b9a1a8198bf729741  after-dark-tags.png
81ee22521ad0cdaa9949f933418f0381d2de7a9035b646145d48da7054676aab  after-dark-user-profile.png
b5f3c1f54d37b2bfce11647a31e7c78aeeb06eaa032de5bb67fae4f0301c7fb1  after-light-agents.png
eb79fbf369fd3f3891ec0266b807680250e34ad546005960d071a84c64e965ec  after-light-export-import.png
b03d74432199a3b8b732150706a07d6d064c75154aea85dd2c9846b4cb6e025e  after-light-home.png
8566eeaab468c6a463d286c75c36869ad1f0af61351520ae8a739b7dfb467af3  after-light-project-memories.png
a991a9f0aaef8af069ad697b7f13a8e2a1fd4f3bcc557b35ff96c18cb3c4adc6  after-light-sessions.png
35af80a4c0d2e5cb38cd476685eb7e5a3bb211fb1e360026be7e92b4558582f9  after-light-settings.png
7694251519a0e95a338ab2b2dd46a3fff1e9e5b79a7002905d36ea927a5d4c4c  after-light-skill-library.png
38d1930450d141bba628735a5060e6f9a5678f19e110a84bc5b01be486ade0a5  after-light-stats.png
dd459924c13471b75ebe03c1a15c17055e898c21a16c9835c6de179427fa9f42  after-light-tags.png
70a85714ebd49c8c7f2d35c0b3e4767edaec1a17a7190d1f33ac072820cd34e9  after-light-user-profile.png
44ef5398094f0e24f8de7b3840217b6e0f98df9b44fea9843088565addc4d76b  dist/index.html
db3b952edcd17dc06a9af848383a6eb8d145002f1159800ac98663f5faf099a6  dist/styles-51057aba76b0c15d.css
118f82664ad340d210b161d93e27655382afc7b0bf40dbe15ae9a15d00160c99  dist/web-ui-321d4859fd9a772f_bg.wasm
e8a69a4a9684b53fae4f742bbdd25b426f1bda923fdf95b0222f039462731deb  dist/web-ui-321d4859fd9a772f.js
df6b877ea28056e914ff8186cd848d46aaff1248f9d3e2048f1405e8bed5c984  before.json
a67fec86979ff5e3e96b29165978e60a741678d51553f633cc07761b80da0743  after-dark.json
186993a5a498c0499bc2da0ad9a2808e0f23fcf2dfa5f203ff5a8d4734832382  after-light.json
2e4fc6b6902a95ef9affe434eb98ffff0fa7334a9859611c09b423259e1d687f  probe.js
```

## Outcome-changing catches (not flagged by the code/test pass)

1. Brand header rendered flush against the top-left edge (no padding; Thaw's drawer header supplies none) — green build, green tests, clean console; found in the first screenshot, fixed with `.app-brand` padding, re-rendered.
2. The `Step 6` gate `grep -c thaw styles.css` read 1 — my own explanatory comment contained the word; reworded (source-gate catch, not a render catch).
3. Before-state: the nav has never had its landmark (`attr:aria-label` rendered as a literal attribute name) — a defect no test saw; fixed and measured 0 → 1.
4. Probe discipline: a shared Chromium window was resized to 1578×833 by another session mid-run on two routes; `innerW/innerH` recorded in the JSON exposed it and both were re-read at 1280×800.

## Not judged here (operator, Step 8, live engine)
±2 % `docH` pair vs `before.json`, `slack > 0` on `/project-memories` `/stats` `/user-profile` `/tags`, R11 (`profile-identity-select`, `agents-project-select` native `<select>`), R13 `/stats` ×3.
