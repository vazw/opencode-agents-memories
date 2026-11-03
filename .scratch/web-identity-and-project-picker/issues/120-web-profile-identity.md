# 120 — the web profile page reports "no profile" because the composition root hardcodes its identity

*(Renumbered from 115 on 2026-11-03. The number spaces of `.scratch/*/issues/` had collided: `.scratch/rust-port/`
already held a 115, a 116 and a 117, and the tracker's staleness block keys only on the bare number, so a
shared number lets one feature's commit contradict another feature's status line. Numbers are now unique
across every feature directory; commits that cite `(issue 115)` for this work predate the renumber.)*

Status: done — 2026-11-03 — landed in `73a964f` (resolver + composition-root wiring) and `b4f024b`
(the roster route, the picker and the honest missing message). Evidence: `tests/web_identity.rs` (6),
`crates/mem-web/tests/api_profiles_roster.rs` (6), `web-ui/tests/core_screens.rs` (identity-control
oracle). The live identity values are O1's proof, not this line's.

Defect A of `.scratch/web-identity-and-project-picker/PRD.md`.

## Symptom (MEASURED, 2026-11-02 ~16:50Z, live engine `127.0.0.1:3742`, read-only GETs)

    GET /api/user-profile                    → {"exists":false,"userId":"unknown",…}
    GET /api/user-profile?userId=vaz@siamstr.com → exists:true, version:169

Two real profiles exist (`vaz@siamstr.com`, 1430 analyzed prompts; `jakkaphat.j@bangmod.co.th`, 330).
The page was not wrong about the store — it was asking about an identity nothing had resolved.

## Cause

`src/main.rs` built the web `AppState` with a literal `user_id`, so every profile handler bound
`"unknown"`. The inject path meanwhile resolved a real email through
`mem_bridge::composition::EnvResolver` (precedence `config.user_email_override` → `git config
user.email` → `"unknown"`). Two identities for one install, one of them fabricated (D-1).

## Fix

- `src/web_identity.rs` — `resolve_user_id_with(config, resolver, directory)` and the production
  `resolve_user_id(config, directory)`: the SAME resolver and config fields the inject path uses, over
  the **launch cwd** (`getTags(process.cwd())`, D-2). Total: a resolver error, a blank directory and a
  non-UTF-8 path all yield `UNKNOWN_USER_ID`, never a panic (a panic here takes the web surface down
  with a stdio engine attached).
- `RootWebLauncher` carries `cwd: PathBuf`, resolved once in `main`; the `AppState` wiring line calls
  the resolver.
- `GET /api/user-profiles` (port-only, D-3): every stored identity, identity-level fields only,
  ordered `totalPromptsAnalyzed` desc then `userId` asc, bounded at 200 with a WARN, `isDefault`
  computed by identity (never by position), `501` when the port is unwired.
- The profile page renders a muted identity line plus a picker when the roster is not the single
  composed identity; it never auto-selects another identity. The select refetches
  `?userId=…` on change.
- `ProfileMissingDto.message` becomes owned: zero stored profiles keeps upstream's literal
  byte-for-byte (D-4); with alternatives stored it names the REQUESTED identity and a count, never
  another identity; a roster read error logs a warning without the identity and falls back to the
  literal rather than turning a working GET into a `500`.

## Residuals

- The `user_id` wiring line itself is not in-process testable (it sits in an async launch future and
  `SystemGitProbe` is constructed inside `EnvResolver::resolve`, so an extracted seam could not catch
  the interesting mutant). Closed by O1 against a live host: `pgrep -x opencode-agents` (the `comm`
  truncation makes `pgrep -x opencode-agents-memory` unable to match), `readlink /proc/<pid>/cwd`,
  then `git -C <that dir> config user.email` — the expected value is the command's output, never a
  literal, and `exists:true` holds only while the computed identity has a stored profile.
- The roster's WARN at the 200-row bound is not asserted in-process (the mem-web suite has no tracing
  subscriber); the bound itself is (`test_profile_roster_is_bounded`).
