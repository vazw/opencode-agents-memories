# 11 — W2.3 · Privacy: private-tag strip, prompt filter, secret resolution (US 4.2)

Status: needs-triage

## What to build

`mem-ai::privacy` — `<private>` tag stripping (including malformed tags), injected-prompt filtering, and secret resolution with strict file-permission checks. Nothing persisted or sent to embedding/LLM APIs may pass unfiltered. Wave detail: `.scratch/rust-port/roadmap.md` §W2.3.

## Acceptance criteria

- [ ] Behaviors ported: `privacy`, `privacy-malformed-tags`, `injected-prompt-filter`, `secret-resolver-permissions`.
- [ ] Malformed-tag handling matches upstream (no partial leakage).
- [ ] Secret resolver rejects files with permissive permissions (strict checks).

## Blocked by

- 02 · W0.2 (can run concurrently with 09–10)
