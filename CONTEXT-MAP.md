# CONTEXT-MAP

This repository is multi-context: each crate is its own bounded context with its own ubiquitous language (convention: `docs/agents/domain.md`). Root `CONTEXT.md` does not exist because the root package is the composition root, not a context.

| Context (crate) | Purpose | Glossary | May depend on |
|---|---|---|---|
| `mem-core` | shared kernel: memory scopes, tags, config loading, typed errors, shared utilities | `crates/mem-core/CONTEXT.md` | — |
| `mem-store` | storage engine on the `turso` crate: shards, connection lifecycle, migrations, vector search, encryption | `crates/mem-store/CONTEXT.md` | `mem-core` |
| `mem-embed` | embedding pipeline: local ONNX (`ort`) + remote providers behind the `Embedder` seam producing `Vec<f32>` | `crates/mem-embed/CONTEXT.md` | `mem-core` |
| `mem-ai` | AI provider seam: `AiProvider` trait + provider clients + privacy filtering before anything is persisted or sent | `crates/mem-ai/CONTEXT.md` | `mem-core`, `mem-embed` |
| `mem-profile` | passive user-profile learning: prompt cold buffer, extraction, confidence/decay, AI cleanup | `crates/mem-profile/CONTEXT.md` | `mem-core`, `mem-store`, `mem-ai` |
| `mem-episodic` | per-project agent session (episodic) memory: session summaries, timelines, consolidation | `crates/mem-episodic/CONTEXT.md` | `mem-core`, `mem-store`, `mem-embed`, `mem-ai` |
| `mem-facts` | the durable five-tier facts DB (`facts.db`): atomic facts, per-project/tier dedup, per-session provenance, explicit purge | `crates/mem-facts/CONTEXT.md` | `mem-core`, `mem-store` |
| `mem-vault` | the verbatim, encrypted, per-project vault tier: one key per scope hash, one encrypted `vault.db` per scope, keyed content ids, no content write path | `crates/mem-vault/CONTEXT.md` | `mem-core`, `mem-store` |
| `mem-skills` | the skills manager: the `skills.db` table, the per-skill `SKILL.md` content tree and the change-notification seam | `crates/mem-skills/CONTEXT.md` | `mem-core`, `mem-store` |
| `mem-portability` | the portable, versioned memory bundle: the durable tiers, the opt-in staging tier and the export/import round trip | `crates/mem-portability/CONTEXT.md` | `mem-core`, `mem-store`, `mem-embed`, `mem-profile`, `mem-episodic` |
| `mem-web` | axum web explorer backend: upstream REST contract + new session-history endpoints | `crates/mem-web/CONTEXT.md` | `mem-core`, `mem-store`, `mem-embed`, `mem-ai`, `mem-profile`, `mem-episodic`, `mem-skills`, `mem-portability` |
| `mem-bridge` | OpenCode host integration: V1/V2 plugin bridge, stdio server, TUI companion | `crates/mem-bridge/CONTEXT.md` | `mem-core`, `mem-store`, `mem-embed`, `mem-ai`, `mem-profile`, `mem-episodic`, `mem-facts` |

Context dependency rules are recorded in `docs/adr/0001-workspace-crate-decomposition.md` and enforced by `tests/workspace_deps.rs`. The root package `opencode-agents-memory` is the composition root binary — it depends on all twelve contexts but is not itself a context.
