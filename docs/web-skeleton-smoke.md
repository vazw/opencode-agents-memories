# Web skeleton smoke recipe (W6.1 / issue #23 — extended by W6.2 / issue #24 and W6.3 / issue #25)

Manual acceptance recipe for the `mem-web` axum skeleton: the health
endpoint, the CORS gate, the auth gates, the static shell and the D-8
Host-header allowlist, plus the W6.2 REST surface (memories, tags, stats,
search, bulk operations, profile, skills + the OpenCode catalog, maintenance)
and the W6.3 session-history surface.

The server is started by the composition root (W5.x wires `mem_web::server::start`
into the plugin). Until then the recipe can be reproduced against any host
that exposes the same routes.

## Defaults

| Setting | Default | Meaning |
|---|---|---|
| `webServerEnabled` | `true` | the server may start at all |
| `webServerHost` | `127.0.0.1` | the bind host (loopback by default) |
| `webServerPort` | `4747` | the bind port |
| `webServerAuthUsername` | *(OS user)* | the Basic-auth username |
| `webServerAuthPassword` | *(unset)* | the Basic-auth password; unset ⇒ Basic auth is off |
| `webServerApiToken` | *(unset)* | the API token; unset ⇒ the local token is the only way into `/api/*` |
| `OPENCODE_MEM_WEB_DIST` / `--web-dist` | first READY exe-relative candidate (`<exe_dir>/web-ui/dist`, then `<exe_dir>/../../web-ui/dist`) | the built web UI (`web-ui/dist`) that mem-web serves. **Env/CLI only** — there is no `opencode-mem.json` key; the CLI flag wins over the env var (W6.8) |

## Smoke transcript

**Prerequisite (W6.8):** a dist must resolve — build it (`web-ui/dist`, e.g.
`trunk build`) or pass `--web-dist <dir>` / set `OPENCODE_MEM_WEB_DIST` — or
the web server **refuses to launch**, and every step below is refused
(connection refused, plus `web.dist_missing` + `web.launch_failed` on stderr).
Steps 1–5 are `curl: (7) Failed to connect` without a resolvable dist.

```bash
# 1. The health endpoint — always reachable, no credentials.
curl -i http://127.0.0.1:4747/api/health
# HTTP/1.1 200 OK
# content-type: application/json
# {"success":true,"status":"ok","authEnabled":false}

# 2. The API surface needs the local token (or the configured API token).
curl -i http://127.0.0.1:4747/api/stats
# HTTP/1.1 401 Unauthorized
# {"success":false,"error":"Unauthorized"}
curl -i -H "x-opencode-mem-token: $(cat ~/.opencode-mem/.auth-token)" http://127.0.0.1:4747/api/stats
# HTTP/1.1 200 OK (once W6.2 adds the route)

# 3. With `webServerAuthPassword` set, the shell and the API surface need
#    Basic credentials.
curl -i http://127.0.0.1:4747/api/stats
# HTTP/1.1 401 Unauthorized
# www-authenticate: Basic realm="OpenCode Memory Explorer", charset="UTF-8"
curl -u <user>:<pw> -i http://127.0.0.1:4747/api/health
# HTTP/1.1 200 OK

# 4. The CORS gate — a foreign origin is refused while Basic auth is off.
curl -i -H 'Origin: https://example.com' http://127.0.0.1:4747/api/health
# HTTP/1.1 403 Forbidden
# {"success":false,"error":"Cross-origin requests are restricted to loopback origins."}

# 5. D-8 — a foreign Host is refused at the gate, before any response can
#    carry the local token.
curl -i -H 'Host: evil.example:4747' http://127.0.0.1:4747/api/health
# HTTP/1.1 403 Forbidden
# {"success":false,"error":"Host header is not allowed."}
curl -i -H 'Host: evil.example:4747' http://127.0.0.1:4747/
# HTTP/1.1 403 Forbidden — the body never contains the token or the
# __OPENCODE_MEM_TOKEN__ marker.

# 6. The static shell — 200 with the built shell once the dist is resolved
#    (`web-ui/dist`, `--web-dist` or `OPENCODE_MEM_WEB_DIST`); the composition
#    root refuses the whole web launch when it is not (W6.8).
curl -i http://127.0.0.1:4747/
# HTTP/1.1 200 OK
# content-type: text/html
# <!doctype html>…<title>OpenCode Memory Explorer</title>…
# <script>window.__OPENCODE_MEM_TOKEN__="…"</script>
```

## REST surface smoke (W6.2 / issue #24)

Every API request below carries the local token (the same seam as block 2):

```bash
TOKEN=$(cat ~/.opencode-mem/.auth-token)
AUTH="x-opencode-mem-token: $TOKEN"
```

Each group shows the **unauthorized** case (no token → `401`) and its
**rejection** case (a traversal-shaped container tag → `400
{"success":false,"error":"Invalid container tag"}`, or the group's own
business rejection where it takes no tag). The canonical seeds of the
hermetic suite (`api_route_auth.rs`) are used: the memory `mem_probe_1`, the
prompt `prompt_probe_1`, the skill `ship-it`.

### A. Memories / tags / stats / search

```bash
# Unauthorized — the whole /api/* surface is token-gated.
curl -i http://127.0.0.1:4747/api/memories
# HTTP/1.1 401 Unauthorized
# {"success":false,"error":"Unauthorized"}

curl -i -H "$AUTH" http://127.0.0.1:4747/api/tags
# HTTP/1.1 200 OK
# {"success":true,"data":{"project":[{"tag":"mem_project_c1c2c3d4e5f67890",…}],"user":[…]}}

curl -i -H "$AUTH" 'http://127.0.0.1:4747/api/memories?tag=mem_project_c1c2c3d4e5f67890'
# HTTP/1.1 200 OK
# {"success":true,"data":{"memories":[…],"total":N,"page":1,"pageSize":20,"totalPages":M}}

curl -i -H "$AUTH" -H 'content-type: application/json' \
  -d '{"content":"smoke probe","containerTag":"mem_project_c1c2c3d4e5f67890"}' \
  http://127.0.0.1:4747/api/memories
# HTTP/1.1 200 OK
# {"success":true,"data":{"id":"mem_…"}}

curl -i -H "$AUTH" 'http://127.0.0.1:4747/api/search?q=probe'
# HTTP/1.1 200 OK
# {"success":true,"data":{…}}

curl -i -H "$AUTH" http://127.0.0.1:4747/api/stats
# HTTP/1.1 200 OK
# {"success":true,"data":{"total":N,"byScope":{"project":P,"user":U},"byType":{…}}}

# Rejection (D-10): the scope segment is `x`, so the guard refuses the tag
# with 400 — never a sanitised or widened listing, never an echo of the tag.
curl -i -H "$AUTH" 'http://127.0.0.1:4747/api/memories?tag=project_x_../../../../Temp/pwn'
# HTTP/1.1 400 Bad Request
# {"success":false,"error":"Invalid container tag"}
```

### B. Bulk operations

```bash
curl -i -H "$AUTH" -H 'content-type: application/json' \
  -d '{"ids":["mem_probe_1"]}' http://127.0.0.1:4747/api/memories/bulk-delete
# HTTP/1.1 200 OK
# {"success":true,"data":{"deleted":1}}

curl -i -H "$AUTH" -H 'content-type: application/json' \
  -d '{"ids":["prompt_probe_1"]}' http://127.0.0.1:4747/api/prompts/bulk-delete
# HTTP/1.1 200 OK
# {"success":true,"data":{"deleted":1}}

# B2: the static `bulk-delete` segment shadows `{id}` FOR EVERY METHOD, so the
# route layer answers upstream's `Invalid ID` — not the plain-text 404.
curl -i -X DELETE -H "$AUTH" http://127.0.0.1:4747/api/memories/bulk-delete
# HTTP/1.1 200 OK
# {"success":false,"error":"Invalid ID"}

# Rejection: the body's ids are required (upstream's business error at 200).
curl -i -H "$AUTH" -H 'content-type: application/json' -d '{"ids":[]}' \
  http://127.0.0.1:4747/api/memories/bulk-delete
# HTTP/1.1 200 OK
# {"success":false,"error":"ids array is required"}

# Unauthorized — no token.
curl -i -X DELETE http://127.0.0.1:4747/api/memories/bulk-delete
# HTTP/1.1 401 Unauthorized
# {"success":false,"error":"Unauthorized"}
```

### C. Profile

```bash
curl -i -H "$AUTH" http://127.0.0.1:4747/api/user-profile
# HTTP/1.1 200 OK
# {"success":true,"data":{"profileData":{…}}}  # centroid / anchor are stripped

curl -i -H "$AUTH" 'http://127.0.0.1:4747/api/user-profile/changelog?profileId=probe'
# HTTP/1.1 200 OK
# {"success":true,"data":{"changelog":[…]}}

curl -i -H "$AUTH" 'http://127.0.0.1:4747/api/user-profile/snapshot?chlogId=probe'
# HTTP/1.1 200 OK
# {"success":true,"data":{…}}  # the parameter is `chlogId` (upstream's typo)

curl -i -H "$AUTH" -H 'content-type: application/json' \
  -d '{"type":"preferences","index":0,"action":"edit"}' \
  -X PATCH http://127.0.0.1:4747/api/user-profile/item
# HTTP/1.1 200 OK
# {"success":true}

# Rejection: an out-of-range index keeps upstream's business error at 200.
curl -i -H "$AUTH" -H 'content-type: application/json' \
  -d '{"type":"preferences","index":99,"action":"edit"}' \
  -X PATCH http://127.0.0.1:4747/api/user-profile/item
# HTTP/1.1 200 OK
# {"success":false,"error":"index out of range"}

# Unauthorized — no token.
curl -i http://127.0.0.1:4747/api/user-profile
# HTTP/1.1 401 Unauthorized
# {"success":false,"error":"Unauthorized"}
```

### D. Skills and the OpenCode catalog

**W6.4 note:** the production wiring (`ApiServices::production`) leaves the
`SkillPort` at `unavailable()` until W6.4 lands the skills manager. Until
then the skills routes answer `500 {"success":false,"error":"Internal Server
Error"}` in production — the contract below is what the handlers answer once
the port is injected (`ApiServices::with_skills`), and it is exercised today
by the fake-backed `skills_api_handlers.rs` suite. The guard rejections (400)
happen **before** the port and therefore already work.

```bash
curl -i -H "$AUTH" http://127.0.0.1:4747/api/skills
# HTTP/1.1 200 OK
# {"success":true,"data":{"skills":[{"id":"ship-it",…}]}}

curl -i -H "$AUTH" -H 'content-type: application/json' \
  -d '{"id":"ship-it","name":"Ship It","content":"## Ship"}' \
  http://127.0.0.1:4747/api/skills
# HTTP/1.1 200 OK — D-16/D-18: the body containerTag (if present) goes
# through the same guard as the query filter.

curl -i -H "$AUTH" http://127.0.0.1:4747/api/skills/ship-it
# HTTP/1.1 200 OK
# {"success":true,"data":{"skill":{…}}}

curl -i -X DELETE -H "$AUTH" http://127.0.0.1:4747/api/skills/ship-it
# HTTP/1.1 200 OK
# {"success":true}

# Rejection (D-16): the tag filter goes through the guard.
curl -i -H "$AUTH" 'http://127.0.0.1:4747/api/skills?tag=mem_project_foo/bar'
# HTTP/1.1 400 Bad Request
# {"success":false,"error":"Invalid container tag"}

# D-20: a percent-encoded separator decodes to `/` and is refused as an id.
curl -i -H "$AUTH" http://127.0.0.1:4747/api/skills/foo%2Fbar
# HTTP/1.1 400 Bad Request
# {"success":false,"error":"Invalid skill id"}

# The two catalog paths answer OUTSIDE the envelope (not `{success,…}`).
# On a loopback bind they are PUBLIC (D-11) — OpenCode's skill-catalog fetch
# sends no token.
curl -i http://127.0.0.1:4747/skills/index.json
# HTTP/1.1 200 OK
# {"skills":[…]}
curl -i http://127.0.0.1:4747/skills/ship-it/ship-it.md
# HTTP/1.1 200 OK
# text/markdown; the raw `## Ship` file

# On a non-loopback bind the SAME paths require the token (D-11).
curl -i http://<lan-ip>:4747/skills/index.json
# HTTP/1.1 401 Unauthorized
# {"success":false,"error":"Unauthorized"}

# Unauthorized — no token (the /api/skills route).
curl -i http://127.0.0.1:4747/api/skills
# HTTP/1.1 401 Unauthorized
# {"success":false,"error":"Unauthorized"}
```

### E. Maintenance

```bash
curl -i -H "$AUTH" http://127.0.0.1:4747/api/migration/detect
# HTTP/1.1 200 OK
# {"success":true,"data":{"needsMigration":false,…}}

curl -i -H "$AUTH" http://127.0.0.1:4747/api/migration/tags/detect
# HTTP/1.1 200 OK
# {"success":true,"data":{…}}

curl -i -H "$AUTH" -H 'content-type: application/json' -d '{"strategy":"fresh-start"}' \
  http://127.0.0.1:4747/api/migration/run
# HTTP/1.1 200 OK
# {"success":true,"data":{…}}

curl -i -H "$AUTH" -H 'content-type: application/json' -d '{"batchSize":5}' \
  http://127.0.0.1:4747/api/migration/tags/run-batch
# HTTP/1.1 200 OK
# {"success":true,"data":{"processed":N,"errors":[]}}

curl -i -H "$AUTH" http://127.0.0.1:4747/api/migration/tags/progress
# HTTP/1.1 200 OK
# {"success":true,"data":{"processed":N,"cursor":C,"total":T,…}}

# Rejection: the strategy must be one of `fresh-start` / `re-embed`.
curl -i -H "$AUTH" -H 'content-type: application/json' -d '{"strategy":"yolo"}' \
  http://127.0.0.1:4747/api/migration/run
# HTTP/1.1 200 OK
# {"success":false,"error":"Invalid strategy"}

# D-15: no service exists in this build — the stubs answer 501, not a
# placeholder and not a 200 with invented data.
curl -i -X POST -H "$AUTH" http://127.0.0.1:4747/api/cleanup
# HTTP/1.1 501 Not Implemented
# {"success":false,"error":"cleanup is not available in this build"}
curl -i -X POST -H "$AUTH" http://127.0.0.1:4747/api/deduplicate
# HTTP/1.1 501 Not Implemented
# {"success":false,"error":"deduplicate is not available in this build"}

# Unauthorized — no token.
curl -i http://127.0.0.1:4747/api/migration/detect
# HTTP/1.1 401 Unauthorized
# {"success":false,"error":"Unauthorized"}
```

The migration routes read `ApiServices::maintenance`, which the production
wiring also leaves `unavailable()` until the composition root (W5.x) supplies
a `MaintenancePort`; the same 500/`Internal Server Error` note as the skills
block applies. The envelopes above are the handler contract, exercised by
`api_endpoint_contract.rs` over the fake port.

### F. Sessions (W6.3 — issue #25)

`ApiServices::production()` keeps the `SessionPort` on `UnavailablePort` until
the W5.x composition root calls `.with_sessions(episodic_session_port(store))`,
so the production wiring answers the documented `501` **by design** today. The
block below is the contract once that call exists — and it is exactly what the
dev harness serves, over a real `EpisodicStore` and the real
`EpisodicSessionPort`.

```bash
# The dev harness: a real episodic store seeded with a parent + child row, the
# real EpisodicSessionPort, the built web-ui bundle, bound to 127.0.0.1:0.
# `demo-token` is DEMO-ONLY, never a production credential: the harness binds
# loopback only, and production reads the per-installation FileLocalToken.
(cd web-ui && trunk build)
cargo run -p mem-web --example session_history_demo
# OpenCode Memory Explorer — W6.3 session-history demo
#   UI:            http://127.0.0.1:<port>/sessions
#   project id:    demo-project
#   parent id:     <parent-row>
#   child id:      <child-row>
#   demo token:    demo-token  (demo-only, loopback-only)
#   static dist:   …/web-ui/dist

# The project timeline, newest first. Two agents, one session id.
curl -i -H "x-opencode-mem-token: demo-token" \
  http://127.0.0.1:<port>/api/projects/demo-project/sessions
# HTTP/1.1 200 OK
# {"success":true,"data":[
#   {"id":"<child-row>","projectId":"demo-project","sessionId":"demo-session",
#    "agentId":"agent-child","agentName":"Implementer","summary":"…",
#    "parentSessionId":"<parent-row>","orphaned":false,
#    "filesTouched":[],"createdAt":…,"updatedAt":…},
#   {"id":"<parent-row>",…}]}

# Delete the PARENT (not the child): the same atomic write batch marks the
# children orphaned (D-12), so the child stays visible in the timeline, flagged.
curl -i -X DELETE -H "x-opencode-mem-token: demo-token" \
  http://127.0.0.1:<port>/api/projects/demo-project/sessions/<parent-row>
# HTTP/1.1 200 OK
# {"success":true}
curl -s -H "x-opencode-mem-token: demo-token" \
  http://127.0.0.1:<port>/api/projects/demo-project/sessions
# {"success":true,"data":[{"id":"<child-row>",…,"orphaned":true,
#   "parentSessionId":"<parent-row>"}]}

# Unknown, wrong-project and already-deleted are ONE answer — no existence
# leak, and the id is never echoed.
curl -i -X DELETE -H "x-opencode-mem-token: demo-token" \
  http://127.0.0.1:<port>/api/projects/demo-project/sessions/<parent-row>
# HTTP/1.1 404 Not Found
# {"success":false,"error":"Session not found"}

# The ids are data, never paths: a traversal-shaped project id passes the id
# rule, reaches the store as a bound value and matches nothing.
curl -i -H "x-opencode-mem-token: demo-token" \
  'http://127.0.0.1:<port>/api/projects/..%2F..%2Fetc/sessions'
# HTTP/1.1 200 OK
# {"success":true,"data":[]}
curl -i -X DELETE -H "x-opencode-mem-token: demo-token" \
  'http://127.0.0.1:<port>/api/projects/..%2F..%2Fetc/sessions/<child-row>'
# HTTP/1.1 404 Not Found
# {"success":false,"error":"Session not found"}

# A percent-encoded whitespace id is rejected (reject, never coerce) …
curl -i -H "x-opencode-mem-token: demo-token" \
  'http://127.0.0.1:<port>/api/projects/%20/sessions'
# HTTP/1.1 400 Bad Request
# {"success":false,"error":"invalid project_id: must not have leading or trailing whitespace"}

# … while a truly empty segment addresses no project at all: 404, not 400 (D-25).
curl -i -H "x-opencode-mem-token: demo-token" \
  'http://127.0.0.1:<port>/api/projects//sessions'
# HTTP/1.1 404 Not Found
# {"success":false,"error":"Session not found"}

# Unauthorized — no token.
curl -i http://127.0.0.1:<port>/api/projects/demo-project/sessions
# HTTP/1.1 401 Unauthorized
# {"success":false,"error":"Unauthorized"}
```

## Recorded divergences

| ID | Upstream | The port | Why |
|---|---|---|---|
| **D-1** | `WebServer` takes over a busy port, hops to the next free one and keeps a health-check loop alive (`checkServerAvailable`, `attemptTakeover`, `nextFallbackPort`, port exhaustion). | **Fail-fast bind**: a port conflict is `WebError::AddrInUse`; there is no port hopping, no takeover and no background retry loop. The health-probe *rule* is kept as a crate-root-reachable constant + predicate (`is_valid_health_probe_envelope`) for a later host lifecycle. | The recorded grill decision is "fail-fast on port conflict"; a single-process model does not need a takeover protocol. |
| **D-2** | CORS headers are added only to preflight responses. | Mirrored as-is: a non-preflight response carries no `Access-Control-*` header. | Upstream parity. Browser same-origin use of the UI is unaffected. |
| **D-3** | A panicking handler answers `500` with `String(error)` in the body. | `500` with the fixed envelope `{"success":false,"error":"Internal Server Error"}`. | No internal detail leaks to the client. |
| **D-4** | `Buffer.from(payload, "base64")` decodes leniently. | A Basic payload that does not decode, or that decodes to non-UTF-8 bytes, is refused (401). | Only malformed credentials are affected — they fail either way. |
| **D-5** | The upstream suite drives the Bun server. | The tests drive the port through two seams: the **router seam** (`build_router(..)` + `tower::ServiceExt::oneshot`) and the **socket seam** (raw HTTP/1.1 over `std::net::TcpStream` against a real server on `127.0.0.1:0`). No dev-dependencies. | `docs/agents/testing.md` forbids dev-dependencies; the seams keep the tests hermetic and drive the public interface. |
| **D-6** | The local token file lives at `~/.opencode-mem/.auth-token`. | `FileLocalToken` takes an explicit path; the composition root supplies the upstream location. | mem-web tests must never touch the user home. |
| **D-7** | Upstream answers `File not found` (404) when no web directory exists. | **Two layers (W6.8).** The composition root REFUSES the web launch when no dist resolves: `web.dist_missing` + `web.launch_failed` on stderr and **no socket bound**, so `GET /` is connection-refused, never a quiet 404. mem-web itself still answers 404 `File not found` when driven directly — its router-seam test `test_missing_dist_dir_returns_404_and_api_still_healthy` stays untouched and green. | AC3 (loud, diagnosable degradation) wins at the composition layer; D-7 yields there only. **Packaging follow-up: issue 36** stages `web-ui/dist` beside the engine binary and forwards `OPENCODE_MEM_WEB_DIST` / `--web-dist` from the wrapper. **Until issue 36 lands**, a plugin-launched engine resolves no dist, so the **ENTIRE axum surface — the REST API AND `/api/health`, not merely the UI — is unavailable** for plugin users; the W6.7 browser smoke and the W7.2 "web UI functional" rehearsal cannot pass before issue 36 closes. |
| **D-8** | Upstream has no Host check: after a DNS rebind the attacker's page is *same-origin* with the local server, so the CORS gate cannot stop it, and the local token injected into the shell could be exfiltrated. | **Host allowlist** at the gate: `localhost`, `127.0.0.1`, `::1` and the configured bind host (each with an optional `:port`); everything else is refused with `403` **before** the token can be embedded in any response. A missing or empty `Host` is refused too. A wildcard bind (`0.0.0.0` / `::`) disables the allowlist. | The user chose the mitigation over recording the risk as accepted. |
| **D-9** | A store failure answers `200` with the raw error string in the body. | `500` + the fixed `{"success":false,"error":"Internal Server Error"}`; the detail goes to the log. | An internal error string must never reach a client (the D-3 philosophy). The UI reads `success`, so client behaviour is unchanged. |
| **D-10** | The container-tag rejection echoes the offending tag in the message and answers `200` (`success:false`). | The fixed `"Invalid container tag"` with **400**; the log records the reason and the tag's length, never the tag text. | Untrusted input is never echoed into a response or a log (reflection / log injection), and a client error is a client error. |
| **D-11** | The two OpenCode catalog paths are **always public** (upstream's token gate is `path.startsWith("/api/")`, so `/skills/*` was never token-gated). | **Conditional**: public when the server binds a loopback address (the upstream-parity default), token-gated when the bind host is not loopback (the house `is_loopback_host` set, read from `AppState.bind_host`). The rule sits inside the token step; the D-8 Host check remains check 0. | The user's ruling (2026-10-20): **public on loopback, token elsewhere**. Upstream's own comment recommends exactly this posture. |
| **D-12** | `parseInt` on a bad `page` yields `NaN` → an empty page and `totalPages: null`. | `page ≥ 1` and `1 ≤ pageSize ≤ 200`; a violation is `400` `{"success":false,"error":"Invalid pagination parameters"}`. | `NaN` pagination and an unbounded `pageSize` are a DoS and a contract hazard; the UI only sends valid numbers. |
| **D-13** | (no upstream counterpart) | The session-history endpoints are registered from W6.2 on and answer the documented `501` (`"Session history endpoints land with W6.3"`) until the composition root injects the port. W6.3 supplies `EpisodicSessionPort`; `ApiServices::production()` still keeps `UnavailablePort` until the W5.x composition root calls `.with_sessions(episodic_session_port(store))`, so the production wiring answers the `501` **by design** until then. | The seam and the route shapes landed in W6.2; the W6.3 adapter makes the fill a one-builder change, and the 501 stays truthful until the composition root wires it. |
| **D-14** | The stored `metadata` object keeps its insertion order. | A `serde_json::Value` object is key-sorted. | Free-form metadata; the UI reads by key. |
| **D-15** | `POST /api/cleanup` and `POST /api/deduplicate` exist and answer real data (upstream ships `cleanup-service.ts` and `deduplication-service.ts`). | Both answer **501** with a fixed message (`"… is not available in this build"`). | The port has neither service; the `MaintenancePort` seam is pre-wired for the fill. (`mem_core::dedup` is the profile-conflict kernel, not upstream's memory-dedup service.) |
| **D-16** | `GET /api/skills?tag=<malformed>` applies no validation — the malformed tag simply matches nothing and the answer is an empty list. | The `tag` filter goes through the container-tag guard; a malformed tag is rejected with `400` `"Invalid container tag"`. | A traversal attempt must be rejected at **every** scoped endpoint; an empty list would silently answer a hostile tag. |
| **D-17** | `POST /api/memories` computes the embedding **before** the scope extraction, so an invalid container tag still burns an embedding. | The container tag is resolved first; an invalid tag is rejected before any embedding is computed (and before the port is called). | A rejected request must not consume an embedding, and the port must not do work for a request it refuses. |
| **D-18** | The **body** `containerTag` of `POST /api/skills` and `PUT /api/skills/{id}` is stored verbatim; upstream applies no validation. | The body's `containerTag` goes through the **same** guard as the query filter; a malformed body tag is rejected with `400` and never reaches the `SkillPort`. | The traversal requirement covers the body paths too, not only the query filters. |
| **D-19** | `GET /api/skills/{id}`, `DELETE /api/skills/{id}` and the catalog file answer ``Skill not found: {id}`` — the untrusted id is echoed. | The fixed `"Skill not found"`; the id is never echoed. | D-10's rule: untrusted input is never echoed into a response. |
| **D-20** | A skill id containing a **literal** `/` is refused with **400** `"Invalid skill id"` (a `path.startsWith("/api/skills/")` prefix match). | A literal `/` never matches `/api/skills/{id}` (matchit needs exactly one segment) → the router answers the plain-text **404**; the 400 is reachable through the **percent-encoded** `%2F` (axum's `Path` percent-decodes the segment) or an id that is empty after trimming. | The route shape cannot express "a segment containing a slash"; the divergence is a consequence of the router, and the port's answer for the reachable shape is upstream's 400. |
| **D-21** | The upstream server applies **no** request-body size limit. | The API router is layered with `DefaultBodyLimit::max(1 MiB)`; a larger body is refused with `413` before it is parsed. | A bounded body is a DoS control; the W6.1 skeleton has no body cap to preserve (`TimeoutLayer` bounds the *time* only). |
| **D-22** | `POST /api/migration/tags/run-batch` interleaves the tag-generation calls and the store writes row by row (the provider call for row *k* happens after row *k-1* is written). | The batch window's tag-generation calls are issued **before** the window is written (the sync `TagGeneration` seam is fed by a pre-computed map); the store writes and the embeddings stay inside the migrator's loop, in `spawn_blocking`. | The written set, the `processed`/`errors` counts and the cursor advance are identical; only the interleaving differs. The mechanism is forced by the frozen sync `TagGeneration`/`MigrationSink` traits. |
| **D-23** | `POST /api/migration/tags/run-batch` calls `provider.executeToolCall("You are a technical tagger.", …)` and reads `result.data.tags` — a **tool-call** wire mechanism with a session id. | The port calls the repo's sealed `AiProviderStructured::complete_structured` with a `{"tags":["…"]}` JSON schema — a **structured-completion** mechanism with no tool-call or session concept. | Mechanism change, recorded. Upstream's prompt text is reproduced verbatim (including the 2–4-tag range); only the wire mechanism changes. `mem-core` already flags this seam as W6.2's. |
| **D-24** | A route registered with `get(...)` answers `HEAD` with the `GET` handler's status (upstream answers **404** for `HEAD` — its branches match the method string). | axum's `MethodRouter` maps `GET` to `HEAD` automatically, so every `GET /api/*` route also answers `HEAD`. The W6.1 shell route already pins `.head(not_found_handler)` for `/`; the API routes inherit axum's `GET`→`HEAD`. | Removing the inherited `HEAD` would mean registering a `HEAD` method router on all 20-odd `GET` paths only to reproduce a 404 body no client reads; the status divergence is recorded instead. |
| **D-25** | (no upstream counterpart) | `matchit` 0.8.4 matches an **empty** segment as a `{project_id}` / `{session_id}` value, so `/api/projects//sessions` reaches the handler with `""` instead of falling through to a router 404. W6.3 pins that shape to **404** `{"success":false,"error":"Session not found"}` by rejecting an empty id in `EpisodicSessionPort`; a *non-empty* malformed id still gets the store's reject-never-coerce `400`. | An empty id addresses no project, so a `404` (not a validation `400`) is the honest answer; doing it in the adapter keeps the frozen W6.2 route table and handlers untouched. |

### D-8 residual risk (recorded, not eliminated)

- The control is **name-based, not address-based**: a client whose resolver maps
  a loopback *name* to a **non-loopback address** (a poisoned `/etc/hosts`
  entry, a split-horizon DNS view) is indistinguishable from a legitimate
  loopback client by any `Host` check.
- A **wildcard bind** (`0.0.0.0` / `::`) disables the allowlist entirely. The
  operator has then explicitly opted into network exposure, where
  `assert_network_auth` already demands an API token or a Basic password.
- This server is plaintext HTTP/1.1 only. HTTP/2 carries the authority in
  `:authority`, which hyper does not surface as a `Host` header — if TLS/h2
  is added later, the guard must fall back to `req.uri().authority()` or it
  will refuse every h2 request.
