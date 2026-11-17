# 140 — Thaw adoption: replace the ruled elements screen by screen

Status: ready-for-agent — will replace the `replace`-classified element kinds from issue 139 across `web-ui/src/` and `web-ui/public/styles.css`, screen by screen, with browser evidence per screen.

*(Parent: issue 118.)*

## Parent

118 — the web UI draws 1 of 58 Thaw components.

## What to build

Apply issue 139's ruling. For each screen, swap the `replace`-classified raw elements for their Thaw equivalents, leaving every `keep-native` and `no-equivalent` element untouched, and prove each screen still renders and behaves.

Work screen by screen so a bad swap is undone by staging the previous bundle back rather than by reverting a whole wave. Removed classes must lose their CSS rules in the same step — the class census fails otherwise — and any evidence lane that pins today's markup moves with the swap.

No dependency bump: the stack is frozen at Thaw 0.4.8, and `Icon` stays out because `icondata` is not a dependency.

## Acceptance criteria

- [ ] The raw-element census drops to exactly the floor the ruling names; every remaining raw element is `keep-native` or `no-equivalent`.
- [ ] Each screen reached by the change renders with no console error, no WASM panic and no failed request.
- [ ] Behavioural oracles remain pure functions the Node runner can execute without a DOM; rendering evidence is the browser lane.
- [ ] The class census moves only as declared, `dead=0`, `unresolved=0`, at or above the pre-commit floor.
- [ ] Both native `<select>` testid contracts still resolve to `<select>` elements.
- [ ] A screen whose swap cannot be made without losing semantics or a contract is left unchanged and recorded against the ruling.

## Blocked by

- 132, 133, 134, 135, 136, 137, 138, 139 — the defect fixes change the same markup this migration would touch, and 139 supplies the ruling it applies.
