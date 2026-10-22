# ADR-0001: Workspace crate decomposition

- **Status:** Accepted
- **Date:** 2026-10-18
- **Context source:** roadmap Rev-2 §W0.1 + §0b (`.scratch/rust-port/roadmap.md`), issue 01 workspace skeleton (`.scratch/rust-port/issues/01-workspace-skeleton.md`). This is the roadmap's proposed ADR candidate 4.

## Context

The repository begins as a single root package (hello-world `src/main.rs`) and will be built out as a Rust port of `opencode-mem` across many waves. The crate layout must be fixed early because the port's wave plan, test seams, and multi-context domain docs (`CONTEXT-MAP.md` + per-crate `CONTEXT.md`) all reference crate boundaries. Deciding the decomposition now avoids repeated structural churn in later waves.

## Decision

The repository is a Cargo **workspace** of eleven library crates plus the existing root package kept as a thin composition binary (`opencode-agents-memory`, which owns no logic of its own):

| Crate | Purpose |
|---|---|
| `mem-core` | shared kernel: memory scopes, tags, config loading, typed errors, shared utilities |
| `mem-store` | storage engine on the `turso` crate: shards, connection lifecycle, migrations, vector search, encryption |
| `mem-embed` | embedding pipeline: local ONNX (`ort`) + remote providers behind the `Embedder` seam producing `Vec<f32>` |
| `mem-ai` | AI provider seam: `AiProvider` trait + provider clients + privacy filtering |
| `mem-profile` | passive user-profile learning: prompt cold buffer, extraction, confidence/decay, AI cleanup |
| `mem-episodic` | per-project agent session (episodic) memory: session summaries, timelines, consolidation |
| `mem-facts` | the durable five-tier facts DB (`facts.db`): atomic facts, per-project/tier dedup, per-session provenance, explicit purge |
| `mem-skills` | the skills manager: the `skills.db` table, the per-skill `SKILL.md` content tree, the change-notification seam |
| `mem-portability` | the portable, versioned memory bundle: the durable tiers, the opt-in staging tier, the export/import round trip |
| `mem-web` | axum web explorer backend: upstream REST contract + new session-history endpoints |
| `mem-bridge` | OpenCode host integration: V1/V2 plugin bridge, stdio server, TUI companion |

### Allowed dependency edges (frozen)

The table is a **ceiling**: a crate's actual `mem-*` dependencies (direct) must be a subset of its row, and every row entry listed is declared from day one so the frozen graph is real.

| Crate | May depend on |
|---|---|
| `mem-core` | — |
| `mem-store` | `mem-core` |
| `mem-embed` | `mem-core` |
| `mem-ai` | `mem-core`, `mem-embed` |
| `mem-profile` | `mem-core`, `mem-store`, `mem-ai` |
| `mem-episodic` | `mem-core`, `mem-store`, `mem-embed`, `mem-ai` |
| `mem-facts` | `mem-core`, `mem-store` |
| `mem-skills` | `mem-core`, `mem-store` |
| `mem-portability` | `mem-core`, `mem-store`, `mem-embed`, `mem-profile`, `mem-episodic` |
| `mem-web` | `mem-core`, `mem-store`, `mem-embed`, `mem-ai`, `mem-profile`, `mem-episodic`, `mem-skills`, `mem-portability` |
| `mem-bridge` | `mem-core`, `mem-store`, `mem-embed`, `mem-ai`, `mem-profile`, `mem-episodic`, `mem-facts` |

**Sibling rule:** `mem-web` and `mem-bridge` must **not** depend on each other — they are independent integration surfaces (HTTP explorer vs. OpenCode host plugin) composed only by the root binary. Reopening this rule requires amending this ADR.

**W6.4 amendment (two new crates).** `mem-skills` owns the skills table and its content tree; `mem-portability` owns the bundle and the export/import service. Two edges follow from the design and are recorded here:

- `mem-web → mem-skills`: the REST surface's `SkillPort` is implemented by the production adapter over `mem-skills`' manager. The `mem-web → mem-portability` edge exists for the same reason — the `mem_portability::facts::FactStore` implementation (`ProductionFacts`) lives in `mem-web`'s `store_adapter`, because the durable-fact reads and writes need the shard manager and the vector primitives.
- `mem-portability` never depends on `mem-web`; the edge is one-way. The `FactStore` seam is defined in `mem-portability` precisely so the dependency does not have to point the other way.

**W7.1 amendment (one crate, one edge).** `mem-facts` owns the durable five-tier facts DB (`facts.db`), and the bridge's production cross-tier task plus the purge orchestrator both live in `mem-bridge`, so one new edge follows: `mem-bridge → mem-facts`. The rule for future phases is normative here: **a row lists only an edge the crate declares today; a ceiling for a future phase is recorded in prose here and becomes a row in the phase that first declares it** — the `mem-facts → mem-embed` ceiling (Phase 2 fact embeddings) is prose, not a row. `mem-vault` and its per-project key scheme land in W7.2; no row is added here because no crate declares it in this wave.

**Front-end package exception (Rev: web stack — see ADR-0003):** the web explorer UI is a standalone package at `web-ui/` (leptos CSR, Trunk-built; §0b D5). It is **not** a workspace member and **not** a `mem-*` crate: it contains no `mem-*` code and nothing may depend on it — its only contract with the port is HTTP (the ADR-0003 REST surface). The workspace root Cargo.toml gains `exclude = ["web-ui"]` because a nested package directory would otherwise be workspace-resolved from the root. The crates-exactness assertion (`crates/` contains exactly the `mem-*` packages the table lists) is unchanged in kind; W7.1 raises the count to eleven.

External (non-`mem-`/non-workspace) dependencies are outside this rule; they are governed per-wave by the §0b stack commitments. Dev-dependency edges **are** within this rule (see Enforcement).

## Enforcement

- Cargo itself rejects cyclic manifests at build time, so the graph is acyclic by construction.
- `tests/workspace_deps.rs` asserts, for each of the eleven crates, that the actual direct `mem-*` dependency set (via `cargo tree -e normal,dev`, which includes dev edges cargo otherwise permits to cycle) is a subset of the allowed table — and that `crates/` contains exactly those eleven packages (a missing or unlisted twelfth crate fails).
- It is the **executable copy** of this ADR: the `ALLOWED` constant in the test and this ADR must be amended together. Allowed sets are ceilings — tightening is automatic from the tables; widening requires an ADR amendment. A new crate needs both a table row here and a test row.

## Consequences

- Adding an edge or a crate now carries deliberate friction (test + ADR edit in one change), which keeps boundaries honest while the port is young.
- Independent integration surfaces (`mem-web`, `mem-bridge`) cannot couple to each other; shared needs flow down into `mem-core` or `mem-store`.
- The root binary stays trivial (today: prints the eleven `CRATE_NAME`s), which keeps composition-level integration tests cheap.
- The UI lives outside this ADR's graph régime by design: wasm-target crates cannot compile on the host target, so admitting the UI into the workspace would break the workspace's own `cargo build --workspace` battery. HTTP is the seam prose; the table stays frozen at eleven (the W6.4 and W7.1 amendments add library crates, never the UI).

## Rejected alternatives

- **Single crate:** no enforced boundaries; wave conflicts and review scope would blur across the whole codebase.
- **Lib+bin-only split:** one giant library has the same problem with none of the payoff.
- **Finer-grained crates:** more churn than value at scaffolding time; the eleven-way split mirrors the port's service-tier structure.
- **Per-crate versioning:** the workspace is a single moving product; one version in `[workspace.package]` is simpler.
- **Permitting dev-dependency cycles:** cargo permits them; they would still create hidden coupling, so `dev` edge kind is included in the enforcement test's graph traversal.
