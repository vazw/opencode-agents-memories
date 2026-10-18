# 13 — W3.2 · Learning service, profile manager & language detection (US 3.1)

Status: needs-triage

## What to build

Background extraction of preferences/languages/conventions from prompts via the `AiProvider` into the profile store; the pure language detector runs alongside. Wave detail: `.scratch/rust-port/roadmap.md` §W3.2.

## Acceptance criteria

- [ ] Behaviors ported: `user-profile-learning-language`, `user-profile-learning-prompt-mode`, `user-profile-learning-error`, `profile-tool-runtime`.
- [ ] Language detector is pure and independently tested.
- [ ] Learning loop survives provider errors without losing buffered prompts.

## Blocked by

- 09 · W2.1
- 12 · W3.1
