# Web skeleton smoke recipe (W6.1 / issue #23)

Manual acceptance recipe for the `mem-web` axum skeleton: the health
endpoint, the CORS gate, the auth gates, the static shell and the D-8
Host-header allowlist.

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

## Smoke transcript

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

# 6. The static shell — 404 until W6.5 produces `web-ui/dist` (D-7).
curl -i http://127.0.0.1:4747/
# HTTP/1.1 404 Not Found
# File not found
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
| **D-7** | Upstream answers `File not found` (404) when no web directory exists. | The server starts and answers 404 for the static paths until W6.5 produces `web-ui/dist`. A missing `dist` is never a startup error and never a panic. | The skeleton must boot before the UI bundle exists. |
| **D-8** | Upstream has no Host check: after a DNS rebind the attacker's page is *same-origin* with the local server, so the CORS gate cannot stop it, and the local token injected into the shell could be exfiltrated. | **Host allowlist** at the gate: `localhost`, `127.0.0.1`, `::1` and the configured bind host (each with an optional `:port`); everything else is refused with `403` **before** the token can be embedded in any response. A missing or empty `Host` is refused too. A wildcard bind (`0.0.0.0` / `::`) disables the allowlist. | The user chose the mitigation over recording the risk as accepted. |

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
