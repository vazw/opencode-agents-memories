# 38 — Security · Upgrade tokio 1.54.0 in `web-ui` (RUSTSEC-2026-0384)

Status: **landed — AC evidence unrecorded** — 2026-10-25. The bump merged as **`4232934`**
(`fix(web-ui): bump tokio to 1.54.1 for RUSTSEC-2026-0384 (issue 38)`); `web-ui/Cargo.lock` pins `tokio
1.54.1`. **Residual:** the acceptance evidence — `cargo audit` = 0 vulnerabilities, the `cargo tree -i
tokio` re-measurement, the wasm suite at its baseline and both web-ui gates — is not recorded in this
file, and a docs-only wave cannot produce it, so the boxes below stay open. Closing needs one run with
cargo in `web-ui/`.

## What to build

Clear **RUSTSEC-2026-0384** from `web-ui/Cargo.lock`, which pins `tokio 1.54.0` — a version with a
race condition in `tokio::time::Interval`, fixed in **>= 1.54.1**.

Measured 2026-10-25 with `cargo audit` in `web-ui/`:

```
Crate:     tokio
Version:   1.54.0
Title:     Race condition in `tokio::time::Interval` on Tokio v1.54.0
Date:      2026-10-21
ID:        RUSTSEC-2026-0384
Solution:  Upgrade to >=1.54.1
error: 1 vulnerability found!
```

Three warnings accompany it, all informational and probably out of scope for the same wave:
`paste 1.0.15` (RUSTSEC-2024-0436, unmaintained), `proc-macro-error2 2.0.1` (RUSTSEC-2026-0173,
unmaintained), and `tokio 1.54.0` **yanked**. Decide explicitly whether each is fixed here or
recorded as accepted — do not leave them silently unaddressed.

## Measured surface (do not re-derive)

- **Who pulls it:** `wasm-bindgen-futures 0.4.79` declares the `tokio` dependency in
  `web-ui/Cargo.lock`.
- **Is it compiled?** `cargo tree -i tokio` prints *nothing to print* for **both** the host target and
  `wasm32-unknown-unknown`, so tokio is in the lock but resolves into neither built tree. Runtime
  exposure is therefore probably nil — but `cargo audit` reads the lock, and CI/audit gates will
  keep failing until the lock moves. State the conclusion you reach about exploitability; do not
  inherit this paragraph's "probably".
- **Root workspace is unaffected:** `Cargo.lock` pins `tokio 1.53.3`, and RUSTSEC-2026-0384 is
  version-specific to 1.54.0. Root `cargo audit`: **0 vulnerabilities across 495 deps**.
- **Pre-existing, not introduced by the web-ui repair:** `web-ui/Cargo.lock`'s sha256 was unchanged
  across commit `065ffdb` (that wave's blueprint forbade touching it), so this predates it.

## Constraints

- **`web-ui/Cargo.lock` is protected by the frozen UI stack:** leptos 0.7.x (floor `^0.7.7` via thaw)
  + thaw `^0.4.8`, per `docs/adr/0003-web-explorer-ui-stack.md`. A tokio **patch** bump is unrelated
  to that pairing, but the wave must prove the pairing did not move — record the leptos/thaw versions
  before and after.
- The pre-commit hook runs **both** web-ui gates on any staged `web-ui/` manifest change, so this
  wave's own commit pays ≈2.5 s warm on top of the root pair (ADR-0016).
- No new dependencies. This is a version resolution change, not a feature.
- `example/opencode-mem/` is read-only reference.

## Triage decisions (2026-10-25)

- **Fix:** patch bump only — `cd web-ui && cargo update -p tokio --precise 1.54.1` (or the newest
  1.54.x), then re-run `cargo audit`. No other crate moves.
- **`paste` and `proc-macro-error2` unmaintained warnings: accepted, not fixed.** Both arrive through
  the **frozen** leptos 0.7.x / thaw 0.4.8 pairing (ADR-0003). Clearing them means moving the UI
  stack, which is a separate decision with its own ADR amendment — do not attempt it here.
- **Yanked tokio: cleared by the same bump.** A yanked version cannot be newly resolved; pinning
  above it removes the warning.
- **Exposure:** `cargo tree -i tokio` returns nothing for host and `wasm32-unknown-unknown`, so the
  crate is not compiled into either built tree. **Re-measure and record the result** rather than
  repeating this sentence — if a future change makes tokio actually compile, the advisory becomes
  live and the wave's priority changes.

## Acceptance criteria

- [ ] `cd web-ui && cargo audit` reports **0 vulnerabilities**; the two unmaintained warnings remain,
      recorded above with their reason.
- [ ] `web-ui/Cargo.lock` pins `tokio >= 1.54.1`; `cargo tree -i tokio` result recorded before and
      after, so the "not compiled" claim is re-measured rather than assumed.
- [ ] leptos and thaw versions byte-identical before and after (frozen pairing, ADR-0003).
- [ ] Both web-ui gates green on the new lock: `cargo clippy --workspace --lib -- -A clippy::all -D
      clippy::unwrap_used -D clippy::expect_used` and `cargo clippy --workspace --all-targets -- -D
      warnings`.
- [ ] wasm suite green at its measured baseline (`cd web-ui && cargo test --target
      wasm32-unknown-unknown` → 121 passed / 0 failed as of `065ffdb`).
- [ ] Root workspace untouched: root `Cargo.lock` digest unchanged, root gates still rc 0.

## Comments

- Filed 2026-10-25 from the `065ffdb` review, which flagged it as pre-existing and out of scope. The
  finding was independently reproduced before filing — `cargo audit` in `web-ui/`, plus the
  `cargo tree -i tokio` empty result and the `wasm-bindgen-futures` reverse dependency.
- Not folded into the web-ui panic-repair wave: that blueprint explicitly required
  `web-ui/Cargo.lock` to stay byte-identical, and mixing a dependency bump into a panic-repair diff
  would make both harder to review.
