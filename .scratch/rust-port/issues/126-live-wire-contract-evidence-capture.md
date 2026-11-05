# 126 — live host wire-contract evidence capture (issue 88 residual)

Status: done — 2026-11-04

## Problem Statement

The OpenCode host wire contract (2.2.5) drifted from what the capture port expected: create responses wrap in `{"data":…}` while the port read a bare `{id}`; the prompt call POSTed to the GET-only `/session/{id}/message` instead of the documented `/api/experimental/generate/text`; and wire-shape mismatches were flattened to `summarizer_failed`, hiding the real failure class. Code changes already landed: `crates/mem-ai/src/provider/opencode_host.rs` makes one stateless POST `{base}/experimental/generate/text` with `{"prompt","model"}` decoding `{"data":{"text"}}`; `crates/mem-bridge/src/capture_prompt.rs` classifies `ProviderErrorKind::WireShape → host_wire_shape_mismatch` (never `summarizer_failed`); the `MEASURED_2_2_5_CREATE_BODY` constant was removed.

What remained was the opsec-audited residual: independently confirm the wire drift with live exchanges. No live request/response had ever been captured. Every shape in `docs/evidence/issue-88-host-wire-contract-20261101/` was READ (documented, schema-derived samples) or INFERRED; the drift claim stayed NEEDS_VALIDATION. One captured live create exchange and one captured live generate exchange were required to promote evidence levels READ→MEASURED and close the residual.

## Solution

Run (operator-executed, owner-gated — it POSTs to the serving host) `.tmp/scratch/capture-live-88.sh`, which:

1. Reads `url` + `password` from `~/.local/state/opencode/service.json` (host 2.2.5 confirmed).
2. POSTs create → `{origin}/api/session` with HTTP Basic Auth `opencode:$PASS` and body `{"agent":"opencode-mem","model":{"id":"beam-free","providerID":"opencode-go"}}` (Model.Ref object, matching `OpenCodeHostProvider::model_ref`).
3. POSTs generate → `{origin}/api/experimental/generate/text` with body `{"prompt":"user: fix the retry storm in the login handler","model":{…Model.Ref}}`.
4. Issues a compensating DELETE on the created session (no orphaned sessions on the host).
5. Writes `create.exchange.json` and `generate.exchange.json` with headers REDACTED and bodies verbatim.
6. Validates JSON + envelope presence (`create`: `.data`; `generate`: `.data.text`).

Then promote README §3 evidence levels READ→MEASURED for the captured create/generate/DELETE steps and drop `NEVER CAPTURED` for those steps only; mark §7 COMPLETE. Run full workspace regression (`cargo test --workspace` green) and clippy clean. No production code changes — issue 88 code already landed; docs/fixture portion is TDD-exempt.

## User Stories

1. As an operator, I want to capture a live create exchange against the OpenCode 2.2.5 host so the create envelope shape is MEASURED, not just READ.
2. As an operator, I want to capture a live generate exchange against the documented `/api/experimental/generate/text` route so the prompt path is verified at runtime.
3. As an operator, I want captured bodies verbatim with headers redacted so no secrets leak into committed evidence.
4. As an operator, I want a compensating DELETE on any created session so no orphaned sessions remain on the serving host.
5. As a reviewer, I want README evidence levels promoted READ→MEASURED for captured steps so the drift claim graduates from NEEDS_VALIDATION.
6. As a reviewer, I want the full workspace regression green after evidence capture so no existing behaviour regressed.

## Implementation Decisions

- Auth: HTTP Basic `opencode:$PASS` from `service.json`, matching `crates/mem-ai/src/provider/host_auth.rs` (`HostAuthorization`).
- Create body model is a `Model.Ref` object `{id, providerID}`, not a bare string (host rejects bare string with `400 Expected Model.Ref | null`).
- Routes: `{origin}/api/session` (create) and `{origin}/api/experimental/generate/text` (generate); no `/session/{id}/message` POST.
- Evidence files: JSON-wrapped `{request, response, captured_at}`, headers `REDACTED`, bodies verbatim, under `docs/evidence/issue-88-host-wire-contract-20261101/`.
- Compensating DELETE after create; treat host as read-only except the one create/delete pair.
- No new Rust crates, no workspace dependency changes, no production code, no test harness added.

## Testing Decisions

- Preconditions (agent-executable): `cargo test -p mem-bridge --test host_capture_e2e` green; `cargo test -p mem-ai --lib` green; `service.json` resolves `hostVersion 2.2.5`; no uncommitted issue-88 Rust files.
- Validation (agent-executable once files exist): `jq empty` on both files; create body has top-level `data`; generate body has `data.text`; neither file claims MEASURED unless captured.
- Regression (agent-executable): `cargo test --workspace` green; `cargo clippy` clean for touched files.
- Pinned existing rows (already green, no new tests): `test_sampled_2_2_5_prompt_posts_to_generate_with_only_a_prompt`, `test_sampled_2_2_5_wire_persists_a_capture`, `test_capture_names_a_wire_shape_mismatch`, `test_wire_error_table_covers_every_declared_generate_failure`.

## Out of Scope

- Restarting or signalling the host (pid 13853) — owner-gated; agent cannot.
- Writing to live store beyond the single create/delete pair.
- New Rust code, new test files, or new crate dependencies (issue 88 code already landed).
- Pushing commits; commits stay local; vault/README updates stay uncommitted.

## Further Notes

- Captured 2026-11-04T11:44Z against `http://127.0.0.1:49374` (version 2.2.5).
- Create: 200, `data` envelope, session `ses_e59a04f07ffeElHt9jTxpGsGP7`, compensating DELETE issued.
- Generate: 200, `data.text` envelope.
- Driver script: `.tmp/scratch/capture-live-88.sh` (owner runs it; agent cannot touch the host).
- README `§3` and `§7` updated; `§7` marked COMPLETE.
- `cargo test --workspace`: all suites green, zero failures.
- Git state (uncommitted, no push): `docs/evidence/issue-88-host-wire-contract-20261101/README.md` modified; `create.exchange.json` + `generate.exchange.json` new; debris `crates/mem-bridge/.tmp/` and `plugin/undefined.lifecycle` removed.

## Comments

- 2026-11-04: live capture executed by operator; both exchanges captured (create 200 `data` envelope, generate 200 `data.text` envelope), compensating DELETE issued, README evidence levels promoted READ→MEASURED, workspace regression green. Issue 88 residual closed.
- Earlier in this session: flipped `Status: ready-for-agent → done — 2026-11-04` on issues 86, 87, 88, 89, 90, 91, 97, 99 (all were GATE APPROVED + coder closed in the prior wave; `Status:` lines were never flipped — this closes that tracker defect class covered by issue 89).
- 2026-11-04 (issue-126-wire-contract-closure blueprint verification): read-verified `create.exchange.json` (200 `data` envelope, `data.id == ses_e59a04f07ffeElHt9jTxpGsGP7`, headers `REDACTED`) and `generate.exchange.json` (200 `data.text` string envelope, headers `REDACTED`); README §3 `MEASURED (observed)` + §7 `COMPLETE` with compensating DELETE documented; `cargo test -p mem-bridge --test host_capture_e2e` (3 passed), `cargo test -p mem-ai --lib` (71 passed), `cargo test --workspace` green (296 suites, 0 failed; transient flake `test_probe_refuses_git_executable_inside_the_untrusted_project_root` passes solo), `cargo clippy --workspace --all-targets` exit 0 (24 pre-existing warnings, 0 new). No prod code, no push.
