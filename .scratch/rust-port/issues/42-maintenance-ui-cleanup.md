# 42 — W8.4 · Maintenance section in Settings (run cleanup from the UI)

Status: done — 2026-11-04 (SUPERSEDED — frontend landed e996a63; backend residual owned by issue 39)
and the report/refused/unavailable/error rendering merged as **`e996a63`** (`feat(web-ui): add a
confirmed Maintenance section that runs cleanup (issue 42)`) in `web-ui/src/core_screens.rs`, with its
wasm tests (`web-ui/tests/core_screens.rs`) and styles. **Residual:** the `200 success:true` five-count
path cannot be exercised against a real server until `POST /api/cleanup` is real — issue 39, currently
**PAUSED** — and the acceptance boxes below stay open because this docs wave runs no cargo: they are
unverified here, not failed.

## What to build

The Settings screen (`web-ui/src/core_screens.rs`, `SettingsPanel`) reads **only** `/api/health` and
`/api/stats`. Its own doc comment says so, with a reason that has now expired:

> The rest of the Settings path group (`/api/cleanup`, `/api/deduplicate`, `/api/migration/*`) is
> maintenance-backed and answers the documented stub in this build, so calling it would show a
> permanent error instead of data (Decision 3).

Issue 39 makes `/api/cleanup` real. This issue adds a **Maintenance** section to Settings that can run
cleanup and display its report — with the stub state still handled correctly, because the UI must work
against a server where the service is absent (degrade path) as well as one where it is present.

## Contract (from issue 39 — do not invent fields)

`POST /api/cleanup` answers, on success, the 5-field report:

```
{ success: true, data: { deletedCount, userCount, projectCount, promptsDeleted,
                         protectedMemoriesSkipped } }
```

Three other outcomes must be rendered distinctly:

| server answer | meaning | UI |
|---|---|---|
| `200 {success:true, data:{…}}` | cleanup ran | show the five counts |
| `200 {success:false, error:"…"}` | business refusal | show the message, not a crash |
| `501` | service absent in this build | show "not available in this build" — **the current state today** |
| transport/auth failure | token or network | the existing failure path used by health/stats |

**Do not treat 501 as an error state.** It is the documented contract for a service that is not wired,
and the UI must render it as a normal, explicable outcome — this is what "Decision 3" was protecting,
and the protection must survive the endpoint becoming real.

## Behaviour requirements

1. **Never run cleanup automatically.** No call on mount, no call on reload, no polling. A destructive
   server operation happens only on an explicit user action.
2. **Confirm before deleting.** A first click arms, a second confirms (or an explicit confirm control),
   and the armed state is visible. Deleting memories is not undoable from the UI.
3. **Show the report after a successful run** — all five fields, labelled. `protectedMemoriesSkipped`
   needs a human label explaining it means "spared because a linked prompt expired recently".
4. **Disable the action while a run is in flight** and re-enable afterwards, so double-submit is
   impossible client-side (the server also guards it, per issue 39 — but the UI must not rely on that).
5. **Existing screens unaffected.** Settings keeps reading health + stats; this is an addition to the
   screen, not a rewrite.

## Measured surface (2026-10-25, HEAD `04df266`)

| piece | state |
|---|---|
| `SettingsPanel` | `web-ui/src/core_screens.rs`, `SettingsScreen` state enum (`Loading`, view, failure) |
| endpoint spec | `/api/cleanup` already present in `web-ui/src/paths.rs` (`EndpointId::Cleanup`) — **no path table change needed** |
| auth | local token in `x-opencode-mem-token`; `AuthContext` + `api_request::request` are the existing seams |
| tests | `web-ui` tests are wasm-only (`#![cfg(target_arch = "wasm32")]`, `wasm-bindgen-test`) under `web-ui/tests/`; baseline **121 passed / 0 failed** at `065ffdb` |
| server for E2E | none required: the wasm suite cannot reach a server, so prove rendering from constructed values |

## Acceptance criteria

- [ ] Settings renders a Maintenance section; no request is issued until the user acts (proven by a
      test that mounts the screen and asserts zero requests).
- [ ] Confirmation flow: a single action does not fire the request; the confirmed action does.
- [ ] A `200 success:true` response renders all five counts with human labels.
- [ ] A `501` renders the "not available in this build" state, visually distinct from a failure.
- [ ] A `200 success:false` renders the server's message without panicking or showing zeros.
- [ ] The action is disabled while in flight and re-enabled after success **and** after failure.
- [ ] `protectedMemoriesSkipped` carries an explanatory label, not a bare number.
- [ ] No `unwrap`/`expect` on the render path (ADR-0013); existing wasm suite green with the new tests
      added (baseline 121 → expected 121 + N).
- [ ] No new dependencies; `web-ui/Cargo.toml` and `web-ui/Cargo.lock` byte-identical.

## Out of scope

- Deduplicate and migration actions — issues 40 and 39 carry their own endpoints; this section can
  grow, but only `/api/cleanup` is wired here.
- Any scheduling or automatic triggering.
- Server-side changes of any kind. If the frontend needs a contract change, STOP and report — issue 39
  owns the contract and is landing in parallel.

## Comments

- Paired with issue 39 (backend `POST /api/cleanup`) and developed in a separate git worktree so the
  two waves cannot contaminate each other's pre-commit hook runs; the file sets are disjoint
  (`web-ui/` vs `crates/` + `src/`), so the merge is trivial.
- The 501 requirement is the load-bearing one: without it, this wave would regress the exact decision
  (`Decision 3`) that kept a permanent error off the Settings screen.
