# 106 — the capture session's permission lockdown is sent under an undeclared key with an undeclared shape, so the deny-all ruleset may never apply

Status: needs-triage — 2026-11-01. Evidence levels are separate and stated per claim: the **spec** is READ,
the **tolerance** is MEASURED, the **consequence** is INFERRED and unmeasured. Security-relevant: the
thing that may not be applied is the rule that denies an internal session every permission.

## What is wrong

The port creates its internal capture session with a permission lockdown:

```rust
// crates/mem-ai/src/provider/opencode_host.rs, session_create_body()
"permission": [
    { "permission": "*",                "pattern": "*", "action": "deny"  },
    { "permission": "StructuredOutput", "pattern": "*", "action": "allow" },
],
```

The host's spec declares something structurally different:

```
POST /api/session body key      permissions            (plural)
                                Permission.Ruleset     = array of Permission.Rule
Permission.Rule                 { action, resource, effect }   all three REQUIRED,
                                additionalProperties: false
Permission.Effect               enum "allow" | "deny" | "ask"
```

So the key is wrong (`permission` vs `permissions`) **and** the shape is wrong
(`{permission, pattern, action}` vs `{action, resource, effect}`). This is a rebuild, not a rename —
which is why the approved wire-contract wave (issue 88) explicitly did **not** fold it in, and why it is
filed here instead of being "fixed" inside a wave that was already reviewed.

## Evidence levels

- **READ — the spec.** As above, from `.tmp/openapi.json` (§ `Session.Info`, `Permission.Rule`,
  `Permission.Ruleset`, `Permission.Effect`). Provenance of that file is unknown and is issue 88's
  evidence problem, not repeated here.
- **MEASURED — tolerance.** A live create returned a 2xx for a body that carries the undeclared
  `permission` key (the live capture failure was classified `protocol`, which `decode_json` maps from a
  2xx body it could not parse, never from a 4xx — so the host did not reject the body). `additionalProperties: false`
  in the spec is therefore documentation, not runtime enforcement.
- **INFERRED — the consequence.** If the host tolerates but ignores an undeclared key, the deny-all
  ruleset was never applied, and the internal capture session runs with whatever permissions a session
  gets by default. **Not measured.** Nothing here claims the engine does anything harmful; it claims a
  guard may be inert.

## What would settle it

`Session.Info` declares `permissions: Permission.Ruleset`, and the host serves
`GET /api/session/{sessionID}`. So: create a capture session, then read it back and inspect
`permissions` —

- an empty or absent ruleset, or one that does not contain the deny-all entry ⇒ the lockdown never
  applied, and this is a live security control that does not exist;
- the expected rules ⇒ the host normalises the legacy shape, and this issue closes as `wontfix` with that
  measurement recorded.

That is one request pair. It is the observation that closes this issue; everything else is inference.

## What lands (once triaged)

Whatever the measurement shows:

- If the ruleset is inert: send the declared shape (`permissions: [{action, resource, effect}, …]`),
  choosing the correct `resource` values rather than translating `permission`/`pattern` mechanically —
  the two shapes do not correspond one-to-one and `effect` has a third value (`ask`) the port never uses.
- If the host normalises it: record the measurement, close, and note in the code comment that the legacy
  shape is deliberately tolerated by the host rather than accidentally accepted.
- Either way, pin it: a row asserting the exact body sent to `POST /api/session`, so the next shape drift
  is a failing test rather than a silent no-op.

## Acceptance criteria

- [ ] The created session's `permissions` is read back from `GET /api/session/{sessionID}` and the result
      is recorded as evidence with the request body that produced it.
- [ ] The body sent contains the declared key and the declared rule shape, or the issue is closed as
      `wontfix` citing the read-back showing the host applies the legacy shape.
- [ ] A test pins the exact create body, so an undeclared key cannot be reintroduced silently.
- [ ] The code comment states which of the two outcomes is true — it currently says the shape is
      "tolerated in practice", which is accurate about acceptance and silent about effect.

## Parent

No slice parent. Surfaced while implementing issue 88 (the wire-contract wave), which moved `model` and
`agent` into the create body — both of which **are** declared fields — and deliberately left this one
alone because fixing it is a ruleset rebuild rather than a field move.

## Comments

**Why it was not fixed in place.** The wave that touched this exact function had to move two other fields
into it anyway, which makes "we'll do it later" look weak. The reason is stated plainly: the declared
shape is a different structure with different required members and a third effect value, so translating
it is a design decision about which resources a capture session should be allowed to touch — not a
rename. Making that call inside a reviewed wire repair would have been scope creep with a security
consequence and no measurement behind it.

**The general lesson, again.** The port was written against a client that carried this shape internally;
the host spec documents a different one; the runtime accepted both. Three sources, two shapes, one
silently inert guard. This is the same pattern as the wire-contract wave: behaviour observed in one
client is not a contract, and a 2xx is not agreement.
