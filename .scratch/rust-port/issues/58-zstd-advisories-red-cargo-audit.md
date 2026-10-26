# 58 — Supply chain · `cargo audit` is red on `main`: one pinned tantivy line yields 3 live vulnerabilities and 10 warnings

Status: needs-triage — 2026-10-26. **`cargo audit` exits 1 on `main` at `c4cd25b` and at the
issue-55 merge `6ad24a4`, with three unaccepted vulnerabilities in `zstd` / `zstd-safe` and ten
further warnings in the same family — all reachable through a single dependency pin
(`turso_core 0.8.3 → tantivy 0.26.2 → tantivy-sstable 0.7.0 → zstd ^0.13`).** The lock is
byte-identical to before the wave, so this is pre-existing and not attributable to issue 55; it is
filed because a repo gate is red and nothing tracked it.

## PREMISE — measured, not inherited

`cargo audit` from the repo root (`cargo-audit` reads `.cargo/audit.toml` only from the invocation
directory, per that file's own header), MEASURED 2026-10-26:

```
exit 1
error: 3 vulnerabilities found!
warning: 10 allowed warnings found
```

The three **vulnerabilities** (`cargo audit --json`, `vulnerabilities.list`):

| ID | crate | version | title |
|---|---|---|---|
| `RUSTSEC-2026-0391` | `zstd` | 0.13.3 | Data from an abandoned stream leaks into the next stream on a reused context |
| `RUSTSEC-2026-0398` | `zstd-safe` | 7.3.0 | `Seekable::decompress` sets a `Vec` length beyond its capacity for offsets past the end |
| `RUSTSEC-2026-0403` | `zstd-safe` | 7.3.0 | Uninitialized memory exposed when streaming decompression of a legacy frame fails |

The ten warnings are the same two crates (`RUSTSEC-2026-0395`, `-0396`, `-0397`, `-0399`, `-0400`,
`-0401`, `-0402`, `-0404`, `-0405`, `-0406`), all dated 2026-10-16. **Thirteen findings, one
dependency.**

## The single pin that produces all thirteen

```
zstd 0.13.3  ←  tantivy-sstable 0.7.0   (declares zstd = "0.13", features = ["experimental"])
             ←  tantivy-columnar 0.7.0
             ←  tantivy 0.26.2
             ←  turso_core 0.8.3
zstd-safe 7.3.0, zstd-sys 2.1.2+zstd.1.5.7   (transitive of zstd)
```

Measured with `cargo tree -i zstd --depth 3` and by reading
`~/.cargo/registry/src/*/tantivy-sstable-0.7.0/Cargo.toml`. `tantivy-sstable` is versioned in
lockstep with `tantivy`; the newest published is `0.27.0`, while `0.26.2` is what the lock holds.

## Why this is the same unblocking event as RUSTSEC-2026-0253

`.cargo/audit.toml` already records `RUSTSEC-2026-0253` (`lru 0.16.4`, unsound) as **blocked
upstream**, with the measured note that *"turso_core pins tantivy ^0.26.2 in BOTH 0.8.3 and
0.9.0-pre.1"*, and that `tantivy 0.27` is the line that carries `lru 0.18.2`. These three
vulnerabilities sit on the **same** pin: rising to `tantivy ^0.27` is what moves `zstd` off 0.13.

So one upstream event — `turso_core` admitting `tantivy ^0.27` — would close `RUSTSEC-2026-0253`
and this issue together. That is the useful shape of this finding: not three problems, one.

## What must NOT happen

- **Do not add these three IDs to `.cargo/audit.toml` to make the gate green.** That file's own
  header states the trap: an `ignore` entry silences an ID *regardless of class*, so accepting
  `unsound`-class precedent for `vulnerability`-class findings would hide a future reclassification.
  It also mandates the config-less re-enumeration drift check before any entry is added
  (copy `Cargo.lock` to a scratch dir with no `.cargo/audit.toml`, run `cargo audit -n`).
- **Do not bump `zstd` directly.** `zstd = "0.14"` alongside a `tantivy-sstable` that requires
  `0.13` adds a second copy; it does not remove `0.13.3` — the same reasoning the `lru` entry
  already records for `lru 0.18`.

## What is NOT established

- **Reachability in this codebase is unmeasured.** Whether any of the three is exercisable depends
  on whether `turso_core`'s FTS/sstable path ever calls `zstd` decompression on untrusted input; I
  have not traced it. That trace, not a severity score, is what would decide urgency.
- ~~**I did not read `tantivy-sstable 0.27.0`'s manifest**, so "0.27 moves zstd to 0.14" is
  `INFERRED`~~ — **measured 2026-10-27, see the comment below: `tantivy-sstable 0.27.0` declares
  `zstd = "^0.14"`. Upgraded `INFERRED` → `MEASURED`.**
- ~~Whether `turso` has published a release admitting `tantivy ^0.27` since `0.9.0-pre.1`~~
  — **checked 2026-10-27, see the comment: it has not.**

## Gate

All figures above are `MEASURED` on 2026-10-26 (`cargo audit`, `cargo audit --json`,
`cargo tree -i zstd`). The pre-existing claim is `READ`: `git diff --stat c4cd25b..6ad24a4 -- Cargo.lock
Cargo.toml` prints nothing. The repo's own gate list names `cargo audit` as a gate, so a red run is a
real finding rather than an optional check — but it is **outside the scope of any code wave** and
needs an owner decision (accept with a documented class-checked entry, or hold for the upstream bump).

## Comments

- **2026-10-27 — the escape route is measured closed, and `zstd` is confirmed compiled.** Three
  measurements, each answering one of the gaps above. Sources: crates.io API (`/crates/<name>`,
  `/crates/<name>/<ver>/dependencies`), `cargo tree -e features -i zstd`, `cargo update -p zstd --dry-run`.
  Advisory DB at this run: 1,378 advisories, last-updated `2026-10-26T14:45:01+01:00`. The 3 vuln IDs and
  10 warnings re-enumerate exactly as recorded above — no drift.

  **1. The crate is compiled, not feature-eliminated** (`cargo tree -e features -i zstd`):

  ```
  zstd v0.13.3  ├── zstd feature "arrays"
                │   └── zstd feature "default"
                │       └── tantivy-sstable v0.7.0 ─ default ─ tantivy-columnar v0.7.0
                │           ─ default ─ tantivy v0.26.2
                │               ├── tantivy feature "columnar-zstd-compression"
                │               │   └── tantivy feature "default"
                │               │       └── turso_core v0.8.3 ─ default ─ turso v0.8.3 ─ mem-store
  ```

  So the finding is not dismissible as "optional dep never enabled". (`turso_core` is given as the
  negative control in the same probe: it appears with the same shape, and it is certainly compiled.)

  **2. No in-range fix exists.** `tantivy-sstable 0.7.0` declares `zstd = "^0.13"`; the 0.13 line ends at
  **0.13.3 (2025-02-20)** — there is no 0.13.4, and every fixed release is 0.14.x or later
  (0.14.0 → 0.14.3). `cargo update -p zstd --dry-run` → **"Locking 0 packages"**, which is the lockfile
  agreeing. And no patch release inside turso's own range exists to carry the bump: `tantivy` publishes
  nothing between `0.26.2` and `0.27.0`, and `tantivy-sstable` nothing between `0.7.0` and `0.27.0`.

  **3. The upgrade hop is now measured, and upstream has not taken it.** The chain is
  `tantivy 0.27.0 → tantivy-sstable ^0.27 → zstd ^0.14` (`tantivy-sstable 0.27.0`'s manifest declares
  `zstd = "^0.14"`; `tantivy 0.27.0` depends on `tantivy-columnar ^0.27` and `tantivy-sstable ^0.27`).
  `turso_core`'s newest published release is **`0.9.0-pre.1` (2026-10-15)** and its dependency list still
  reads `tantivy ^0.26.2` — as does every release back through `0.8.0`. `^0.26.2` allows
  `>=0.26.2, <0.27.0`, so it can never select 0.27. The unblocking event has **not** happened.

  **Consequence for triage.** There is no action available on this tree that removes the findings: not
  `cargo update` (no in-range version), not a direct `zstd = "0.14"` (adds a second copy, as the issue
  already says), not a `[patch]` (a patch cannot satisfy a `^0.13` requirement with a 0.14 crate). The
  remaining choice is therefore genuinely binary and belongs to the owner:
  **(a) accept the three IDs** in `.cargo/audit.toml` — which requires the armed drift check its header
  mandates, an explicit note that these are `vulnerability` class (the file's existing entries are all
  `unmaintained`/`unsound`, so this would be the first vulnerability-class acceptance), and a revisit
  trigger tied to `turso_core` admitting `tantivy ^0.27`; **or (b) hold the gate red** until that release.
  What would still sharpen (a): a trace of whether `turso_core`'s sstable/FTS path feeds untrusted bytes
  to `zstd` decompression. Not measured here — the two `zstd-safe` vulns (`Seekable::decompress` length
  overflow, streaming legacy-frame failure) are decompression-side, so the trace decides exploitability,
  not fixability.
