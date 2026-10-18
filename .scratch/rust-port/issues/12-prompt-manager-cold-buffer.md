# 12 — W3.1 · Prompt manager & cross-process cold buffer (US 3.1)

Status: needs-triage

## What to build

`mem-profile::prompt` — user-prompt buffer claim/processing, cross-process file/DB locking, short-write hardening, idle scheduling. Port the worker fixture as a Rust subprocess fixture. Privacy filter (W2.3) is applied to stored prompts; lock primitives come from W1.5. Wave detail: `.scratch/rust-port/roadmap.md` §W3.1.

## Acceptance criteria

- [ ] Behaviors ported: `user-prompt-manager-claim`, `user-prompt-path-normalization`, `profile-cold-buffer-cross-process`, `user-profile-cold-buffer-isolation`, `profile-learning-lock`, `profile-learning-service-lock`, `profile-learning-storage-error`, `profile-learning-idle`, `profile-write`.
- [ ] Cross-process claim protocol stable under repeat loops (any flake = blocking defect).
- [ ] Short-write scenario from the upstream fixture is hardened and tested.

## Blocked by

- 08 · W1.5
- 11 · W2.3
