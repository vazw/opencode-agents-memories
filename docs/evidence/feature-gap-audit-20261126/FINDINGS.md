# Feature-gap audit — what exists upstream (or in an accepted ADR) and was never filed

- **Date:** 2026-11-26
- **Scope:** read-only audit of this repository. No source edited, no issue filed, no network. The only
  write is this file.
- **Question:** which user-visible features exist in the vendored upstream clone (or are mandated by an
  accepted ADR) but have **no issue** in `.scratch/rust-port/issues/`?
- **Evidence labels:** `MEASURED` (a command run here, output quoted), `READ` (a file/line read),
  `INFERRED` (derived from the two, with the closing observation named), `REPORTED` (quoted from a doc).

---

## 0. Snapshot (every number re-derived)

| Quantity | Value | Command |
|---|---|---|
| Upstream clone HEAD | `9086c65` "release v2.39.0", commit date 2026-10-31 | `git -C example/opencode-mem log -1 --format='%h %ci'` |
| Upstream version | `2.39.0` | `grep -m1 '"version"' example/opencode-mem/package.json` |
| Upstream test files | **140** | `ls example/opencode-mem/tests/*.test.ts \| wc -l` |
| Parity pin | `906038e`, **110** rows | `git -C example/opencode-mem log --oneline -1 906038e`; `wc -l docs/agents/upstream-tests-906038e.txt` |
| Commits since pin | **50** (30 of them touch `tests/`) | `git -C example/opencode-mem log --oneline 906038e..HEAD \| wc -l` |
| Registry dispositions | port **93**, covered-by **9**, N-A **8**; 0 missing/unknown | `bash scripts/check-parity-matrix.sh` |
| Issue files | **170**; first status token: `done` 155, `landed` 2, `fixed` 3, `wontfix` 4, `ready-for-human` 1, 5 bolded/other | `grep -h '^Status:' .scratch/rust-port/issues/*.md \| sed 's/^Status: *//' \| awk '{print $1}' \| sort \| uniq -c` |
| Open work queue | **none** — issue 28 is the only non-terminal item and it is `ready-for-human` (owner's) | same command + `grep -rl 'ready-for-agent' .scratch/rust-port/issues` → 0 hits |

All three statements the audit was asked to test are therefore live: the matrix is pinned 30 test files
behind the clone, the tracker has no open work, and the clone has been re-pinned only in prose.

**Staleness of the pin and whether it can hide unported tests — MEASURED.** Yes, definitionally: the
matrix's own maintenance rule (`docs/agents/testing.md:225`) requires one disposition per upstream test,
and the pin covers 110 of the 140 files now in the clone. The 30 files listed in §1 are outside the
matrix, so `check-parity-matrix.sh` exits 0 while none of them has a disposition. Nothing else catches
it: no script diffs `ls tests/*.test.ts` against the pinned list.

---

## 0b. Owner ruling, 2026-11-26 — what happens to these gaps

Decided after reading this report; recorded here so the findings and their dispositions do not drift apart.

- **Scope:** upstream 2.33→2.39 features are **out of scope**, except where an accepted ADR already
  mandates the work. The non-goal list, each entry with its re-open trigger, lives in `OVERVIEW.md`
  §"Scope ruling 2026-11-26". G1, G2, G3, G5, G6, G7, G8, G9 and G10 therefore become `N-A`-by-ruling
  dispositions in the parity matrix, **not** issues.
- **In scope, filed:** G4 →
  `.scratch/rust-port/issues/173-purge-report-does-not-name-the-vault-tier.md`.
- **G11 withdrawn** as a false positive (§5): upstream does not exempt pinned rows from the age cutoff
  either. The audit's only `partial` row is gone; nothing else changed rank.
- **Re-pin** (`docs/agents/testing.md` + `docs/agents/upstream-tests-*.txt` to `9086c65`/140) happens
  **after** this ruling, writing one disposition per upstream file, with `N-A` rows citing the OVERVIEW
  ruling rather than "unspecified".

## 1. Upstream coverage — the 30 files with no disposition

Derivation (MEASURED):

```sh
ls example/opencode-mem/tests/*.test.ts | xargs -n1 basename | sort > /tmp/up140.txt
comm -23 /tmp/up140.txt <(awk -F'|' '/^\| `tests\//{gsub(/`| /,"",$2); print $2}' docs/agents/testing.md | sed 's|^tests/||' | sort)
# → 30 lines
```

| # | Upstream test (post-pin) | Rust counterpart found | Status |
|---|---|---|---|
| 1 | `markdown-config.test.ts` | none (`git grep -l markdown-config` → 0) | **gap** — markdown backend absent |
| 2 | `markdown-document.test.ts` | none | **gap** — same feature |
| 3 | `markdown-vault.test.ts` | none | **gap** — same feature |
| 4 | `markdown-watcher.test.ts` | none | **gap** — same feature |
| 5 | `markdown-write-through.test.ts` | none | **gap** — same feature |
| 6 | `dreaming-grouping.test.ts` | none (`git grep -il dreaming` → 0 repo-wide) | **gap** — dreaming absent |
| 7 | `dreaming-prompt.test.ts` | none | **gap** — same feature |
| 8 | `memory-trash.test.ts` | none; delete is hard (`crates/mem-store/src/vector.rs:911` `delete_memories`) | **gap** — soft-delete trash + restore absent |
| 9 | `update-on-capture.test.ts` | none (`git grep -i updateOnCapture` → 0) | **gap** — add/update/supersede on capture absent |
| 10 | `embedding-idle-dispose.test.ts` | none (`git grep -n idle crates/mem-embed/src` → 0) | **gap** — model never released |
| 11 | `memory-provenance.test.ts` | `crates/mem-bridge/src/inject.rs:1277` emits only `relevance="N%"` | **gap** — id/created/project attributes absent |
| 12 | `memory-injection-eligibility.test.ts` | `crates/mem-bridge/src/inject.rs:472–502` (age cutoff, session exclusion); `crates/mem-bridge/tests/inject_chat_gates.rs:67`, `:597` | **covered-by — audit error, corrected 2026-11-26 (see §5)**. The first draft claimed upstream exempts pinned rows from the age cutoff and the port does not. Upstream's own oracle refutes that: `tests/fixtures/memory-injection-eligibility.mjs:189` adds `too-old-pinned` with `pinned: true`, and `result.ageFiltered` (`:192`, asserted at test `:66-69`) is `["recent"]` — the pinned-but-too-old row is **excluded** when the cutoff is on, and only reappears when it is disabled. The port behaves identically |
| 13 | `api-route-provider.test.ts` | none; `ProviderKind` has 8 kinds (`crates/mem-ai/src/provider/registry.rs:55`) | **gap** — provider absent |
| 14 | `cheaper-inference-provider.test.ts` | none | **gap** — provider absent |
| 15 | `runcrate-provider.test.ts` | none | **gap** — provider absent |
| 16 | `auto-capture-config-toast.test.ts` | engine publishes `auto_capture_provider_status` (`crates/mem-core/src/config/schema.rs:479`); wrapper has no readiness toast (`grep provider plugin/src/plugin.js` → 0) | **gap** — wrapper surface absent (dead-letter is the substitute, issue 172) |
| 17 | `auto-capture-provider-order.test.ts` | none; `capture_provider.rs` resolves one provider, no ordered fallback | **gap** — opencode-first/external-first fallback absent |
| 18 | `cleanup-ai-sessions.test.ts` | none — the port has no host session manager | **N-A candidate** — no in-process session store to expire |
| 19 | `turso-wal-index-recovery.test.ts` | none; `bak_reaping.rs` reaps sidecars but never quarantines `-shm`/`-tshm` and retries an open | **gap** — resilience absent |
| 20 | `turso-close-checkpoint.test.ts` | `grep checkpoint crates/mem-store/src` → 0; release is `Drop`-based | **uncertain** — depends on turso crate drop semantics |
| 21 | `turso-operation-lock.test.ts` | `crates/mem-store/src/operation_lock.rs:177` + `tests/turso_operation_lock_xproc.rs` | **covered-by** — but `docs/agents/testing.md:231` still says "upstream has no `operation-lock.test.ts`", true only at the pin |
| 22 | `stats-service.test.ts` | `crates/mem-web/src/api/stats.rs`, `tests/stats_contract.rs` | **covered-by** — post-pin file, row absent from matrix |
| 23 | `stats-log-parsing.test.ts` | `crates/mem-web/src/api/stats.rs:256` "Parsers + daily series (this wave; upstream `stats-service.ts`)" | **covered-by** — row absent |
| 24 | `transformers-resolve.test.ts` | none — no JS CJS resolver; `ort` loads directly (`crates/mem-embed/src/local/`) | **N-A** — Bun/node packaging subject |
| 25 | `plugin-turso-refcount.test.ts` | none — no JS Turso pool; engine lock is `mem-store/src/engine_lock.rs:125` | **N-A** — JS handle-pool subject |
| 26 | `serialize-error.test.ts` | logging only (`crates/mem-core/src/logging`) | **N-A/low** — diagnostics, no user surface |
| 27 | `add-memory-phase-logging.test.ts` | capture emits `auto_capture.memory_persisted` etc.; no per-phase error fields | **low** — diagnostics |
| 28 | `tag-migration-opencode-provider.test.ts` | tag migration routes exist (`crates/mem-web/src/api/maintenance.rs:141–230`) and use `generate_tags`; provider selection is config-driven (`opencode_provider`, `schema.rs:320`) | **covered-by (different design)** — needs a parity measurement, not a feature |
| 29 | `memory-recall.test.ts` | recall gates live in `crates/mem-bridge/src/inject.rs` + `plugin/src/plugin.js:1034–1047`; owned by issues 63/98 (both `done`) | **covered-by, no row** |
| 30 | `profile-inject-empty-memories.test.ts` | `crates/mem-bridge/tests/inject_profile.rs:582` (profile-only block) | **covered-by** — row absent |

Count after the §5 correction: **4 N-A**, **7 covered-by / covered-by-different-design**, **1 uncertain**,
**1 low**, **17 gaps** (nine feature groups). `READ` for the counterpart cells; each counterpart path was
opened or grepped, not assumed — the one cell that was *not* opened (row 12) was wrong, which is the
argument for the rule.

**Why the pin could not have caught these — INFERRED.** Re-pinning the matrix (issue 71's territory;
issue 28 is the owner's rehearsal) only *creates the rows*. A row that is written honestly for #1–#17
and #19 says "no counterpart", so the pin alone does not close any gap — it makes the gaps first-class.

---

## 2. OVERVIEW.md user stories — where each criterion lives

`OVERVIEW.md` was written against upstream ~2.31 and is the scope authority. Mapping is `READ` unless
marked.

| Epic / US | AC | State | Evidence |
|---|---|---|---|
| 1.1 capture | scope triple + agent session type | implemented | `crates/mem-episodic/src/record.rs` (`SessionMetadata`), `crates/mem-store/src/episodic_sql.rs:47` |
| 1.1 capture | structured summary: completed / files / decisions / threads | implemented | `crates/mem-episodic/src/payload.rs:36` (`WorkItem`, `Decision`) |
| 1.1 capture | metadata + embedding stored | implemented | `crates/mem-episodic/src/embedding.rs`; `crates/mem-bridge/src/capture.rs` |
| 1.2 recall/inject | N recent + semantic, dedicated block, budgets, blocker-first | implemented | `crates/mem-bridge/src/inject.rs:84` (`HISTORY_BLOCK_OPEN`), `:1046` (one writer of `<memory_context>`) |
| 1.3 attribution | agent_role/agent_id + parent link; compaction preservation | implemented | `crates/mem-episodic/src/attribution.rs:66`; `crates/mem-episodic/tests/compaction_agent_preservation.rs` |
| 1.4 timeline tool | `recall_project_sessions` with time/semantic/file/agent filters | implemented | `crates/mem-episodic/src/tool_schema.rs:21`; `plugin/src/tools.js:41` (registered) |
| 1.4 orphan linkage | clean parent links | implemented | `crates/mem-episodic/tests/orphan_linkage.rs` |
| 1.5 consolidation | merge >30d episodic into milestones; promote decisions | implemented | `crates/mem-episodic/src/consolidation.rs:237`, `promotion.rs`; cleanup `crates/mem-web/src/api/cleanup_sweep.rs` |
| 2.1 scopes/tags | strict validation, traversal refusal | implemented | `crates/mem-core/src/scope.rs:111`, `crates/mem-core/src/tags/` |
| 2.1 connection mgmt | readiness gates, deterministic release, async mutex | implemented | `crates/mem-store/src/connection.rs`, `ready.rs`, `operation_lock.rs:177` |
| 2.1 shards | rotation, inventory, path migration | implemented | `crates/mem-store/src/shard_manager.rs:142`, `shard_path_migration.rs` |
| 2.2 vector search | search + exact fallback + re-embed migrations | implemented | `crates/mem-store/src/vector.rs:621`, `engine_migrator.rs` |
| 2.3 embeddings | local ONNX, presets, dtypes, prefixes, remote retry | implemented | `crates/mem-embed/src/embedder.rs:53`; `remote/retry.rs`; `CONTEXT.md` |
| 2.4 encryption | key, migrator, operation lock, multiprocess | implemented | `crates/mem-store/src/encryption_key.rs`, `encryption_migrator.rs`, `db.rs:462` |
| 3.1 profile | cold buffer, learning lock, language detection | implemented | `crates/mem-profile/src/cold_buffer.rs`, `learning_lock.rs:613`, `language_detector.rs:126` |
| 3.2 decay + AI cleanup | confidence/decay, validator-driven merge | implemented | `crates/mem-profile/src/decay.rs:53`, `ai_cleanup.rs`, `validation.rs` |
| 4.1 providers | the **seven named** providers + host | implemented | `crates/mem-ai/src/provider/registry.rs:55` lists exactly the OVERVIEW set |
| 4.1 loop prevention | internal capture sessions isolated | **partial** | `crates/mem-ai/src/privacy/injection.rs:131` (`is_internal_prompt`) — an injection blocklist exemption, not a capture-session registry; see §4.3 |
| 4.2 privacy | `<private>` strip, injected-prompt filter, secret resolver perms | implemented | `crates/mem-ai/src/privacy/tags.rs:97`, `injection.rs`, `provider/secret_resolver.rs:22` |
| 4.3 portability | export/import with serde validation | implemented | `crates/mem-portability/src/service.rs:243`, `:388` |
| 4.3 skills | manager + bridge | implemented | `crates/mem-skills/src/manager.rs:160` |
| 5.1 REST | upstream contract to axum; sessions GET/DELETE | implemented | `crates/mem-web/src/api/mod.rs:96` (43-row `ROUTE_TABLE`), `:365`, `:372` |
| 5.2 plugin bridge | V1/V2 adapter, toasts, shutdown | implemented | `crates/mem-bridge/src/engine.rs:249`, `jsonrpc.rs`; `plugin/` |

No OVERVIEW criterion is "not implemented and no issue". The gaps in §1 sit **beyond** OVERVIEW's
frozen list (upstream 2.33→2.39 features) — OVERVIEW does not mention markdown mode, dreaming, trash,
update-on-capture, idle dispose, provenance attributes, or the three new providers. That is itself a
finding: the scope authority is 30 test files behind the behavioural contract it points at.

---

## 3. Docs-promised surfaces the code does not provide

1. **`CONTEXT-MAP.md` → `mem-web`: "upstream REST contract".** MEASURED: the Rust router answers 43
   routes; upstream `src/services/web-server.ts` answers six more that the port lacks —
   `GET /api/memories/trash` (:540), `POST /api/memories/{id}/restore` (:546), `GET /api/markdown/status`
   (:608), `POST /api/markdown/reindex` (:613), `GET /api/dreaming/status` (:652),
   `POST /api/dreaming/run` (:657). The Leptos UI has no Markdown or Dreaming page either.
2. **ADR-0019 D-19-9 (Accepted 2026-10-29) vs `crates/mem-bridge/src/purge.rs:126`.** D-19-9 mandates
   `NOT_ERASED` grow to `memories-shard` **and `vault`**, each with `erased: Yes|No|Unknown`; the code
   ships two entries (`memories-shard`, `milestones-recap`), no `vault`, no `erased` field. Accepted
   decision, unimplemented, no work item (see §4.2).
3. **`crates/mem-vault/CONTEXT.md:95`** is honest that the purge leg (W8b) is unshipped — but ships no
   pointer to an issue, and `crates/mem-bridge/tests/vault_source_ref.rs:5` carries the only code TODO
   (`TODO(W8b): equality half … deferred`).
4. **`docs/agents/testing.md:3`** states "every upstream test receives exactly one of three dispositions",
   then the same paragraph discloses the 30-file exception (pin `906038e`/110 vs clone `9086c65`/140). So
   the blanket rule and its known exception sit in one sentence — accurate, but the *rule* is unenforced,
   which is why `scripts/check-parity-matrix.sh` exits 0 (`MEASURED`, exit 0 at 110/140). `:231` says
   "upstream has no `operation-lock.test.ts`"; the clone ships `tests/turso-operation-lock.test.ts`
   (differently prefixed) and that subject **is** Rust-covered by `turso_operation_lock_xproc.rs`. The
   sentence is a naming artefact, not a false claim — low severity, worth re-wording at the re-pin.
5. **`docs/agents/testing.md:205`** records that closing the `v2-memory-context` internal-prompt leg
   "needs a capture-side suppression rule **no issue owns**". That is an explicit, written-down,
   still-untracked gap (issue 52 is `done`).
6. **`OVERVIEW.md` §4.1 provider list** is now a subset of upstream (`api-route`, `runcrate`,
   `cheaper-inference` added upstream in the 30-file window). Not a broken promise — a stale scope list.

---

## 4. Deferred / unowned work with no tracked record

### 4.1 `shortcut:` debt — the two known ones, and no others (MEASURED)

`git grep -n 'shortcut:' -- . ':!example'` returns exactly two lines, both prose inside `done` issues:

- `.scratch/rust-port/issues/166-adapter-reactivation-wipes-live-blocks.md:155`
- `.scratch/rust-port/issues/171-session-block-silences-and-undetectable-dead-code.md:245`

No third marker exists in tracked code. Both concern the same thing (the plugin `dist/**` write contract
documented only in gitignored `.tmp/`).

### 4.2 Unimplemented accepted decision — the purge report omits the vault

- `docs/adr/0019-erasure-posture.md:257` — **D-19-9**, status Accepted, mandates the `vault` tri-state.
- `crates/mem-bridge/src/purge.rs:126` — `NOT_ERASED: [NotErasedTier; 2]`, neither entry is the vault.
- `crates/mem-vault/src/lib.rs:45-47` — "`purge.completed`'s `not_erased` is **no longer** complete or
  true without naming the vault … a purge run today erases no vault rows and does not say so."
- It **is** written down twice (ADR-0019 G5/D-19-9; issue 43's comment at `:290` as a "medium finding"),
  but issue 43 is `done` and no issue, PRD item, or status line executes it.

Consequence, as stated by the crate itself: an operator purging a session reads a success report that
over-claims completeness while encrypted vault ciphertext survives. Severity is disclosure, not data
escape — the vault is encrypted per scope (ADR-0012 D-14).

### 4.3 The capture-side suppression rule (no issue owns it)

`docs/agents/testing.md:205` states the v2 memory-context `:70` leg is not closable by issue 52 and
requires "a capture-side suppression rule no issue owns". Current code filters internal prompts out of
*injection* (`crates/mem-ai/src/privacy/injection.rs:131`) but there is no capture-side equivalent
(`git grep internal crates/mem-bridge/src/capture.rs` → 0). This is the same risk class as upstream's
infinite-capture-loop guard (`src/services/ai/internal-capture-sessions.ts`), which also has no Rust
counterpart (test #18 above).

### 4.4 Other markers

`git grep -nE '\b(TODO|FIXME|XXX|HACK)\b' -- . ':!example'` → **one** code TODO
(`crates/mem-bridge/tests/vault_source_ref.rs:5`, W8b) plus three hits inside
`scripts/check-parity-matrix.sh` (gnarly-prose rejection strings, not debt). No `unimplemented!`/`todo!`
anywhere.

---

## 5. Ranked gaps

Rank = user-visible value. Each carries the observation that would close it as *not a gap*.

### G1 — Deleting a memory is irreversible; upstream keeps a 30-day trash
- **Missing (MEASURED):** `GET /api/memories/trash`, `POST /api/memories/{id}/restore`; a
  `dreaming.trashRetentionDays` purge; the soft-delete column for the `memories` shard. Upstream test:
  `memory-trash.test.ts:79` (soft-deleted row hidden from list, restored from trash). Rust deletes hard
  (`crates/mem-store/src/vector.rs:911`).
- **Inverted world:** show that an erased memory is recoverable some other way (export snapshot, undo log)
  and the safety value collapses — `git grep -n 'undo\|recover' crates/mem-store/src` → nothing today.
- **Disposition:** **new issue**. Small–medium; one column + migration + 2 routes + trash UI row.

### G2 — Markdown vault backend (`memoryBackend: "markdown"`) does not exist
- **Missing (READ, upstream README:238-259):** an entire storage mode — `.md` files under `markdownPath`
  as source of truth, frontmatter pack/parse, watcher, write-through on add/delete, foreign-page
  reconciliation, `GET /api/markdown/status`, `POST /api/markdown/reindex`, a Web UI Markdown page.
  Five upstream test files; `git grep -i memoryBackend` → 0; `git grep -i markdown` in `crates/` only
  hits *formatting* helpers (`crates/mem-ai/src/privacy/injection.rs`, `crates/mem-bridge/src/capture*.rs`).
- Config consequence: `FileConfig` tolerates unknown keys by design (`crates/mem-core/src/config/schema.rs:6`),
  so an existing upstream config with `"memoryBackend": "markdown"` loads **silently** and runs in database
  mode — the user believes the folder is the store.
- **Inverted world:** a `docs/` ruling that markdown storage is an explicit port non-goal (like i18n/auto-update
  in issue 27/D8), or evidence that no user of this port can reach the key.
- **Disposition:** **new issue** — likely two (backend + UI routes/page). Largest gap here.

### G3 — "Dreaming" background consolidation does not exist
- **Missing (MEASURED):** `git grep -il dreaming` → 0 repo-wide. Upstream: opt-in interval pass grouping
  *related* memories (not only near-dupes), LLM merge, soft-delete superseded rows into trash, optional
  cross-project grouping, a process lock, `GET /api/dreaming/status`, `POST /api/dreaming/run`, and the
  `dreaming.*` config block (`example/opencode-mem/src/config.ts:247-258`). The port has synchronous
  dedup (`crates/mem-web/src/api/deduplication/`, issue 40/54) and retention cleanup — different
  mechanics, no scheduler, no trash, no cross-project grouping.
- **Inverted world:** show the existing dedup endpoint + cleanup scheduler can be driven on an interval
  with the same grouping semantics (it cannot: no related-threshold, no trash, no LLM merge) — i.e. a
  mapping doc that claims equivalence.
- **Disposition:** **new issue**; depends on G1 (trash) for its soft-delete half.

### G4 — Purge success report does not name the vault (accepted ADR unimplemented)
- See §4.2. Privacy disclosure defect, owner already accepted the remedy in ADR-0019 D-19-9.
- **Inverted world:** D-19-9 is superseded by a newer ADR or owner ruling that removes the vault tier
  from `NOT_ERASED` (search `docs/adr` → none).
- **Disposition:** **new issue** (the two-entry `NOT_ERASED` + `erased` tri-state + a test). Small, and it
  closes an audit finding that has been open since 2026-10-25.

### G5 — Injected memories carry no provenance; the model cannot cite or date them
- **Missing (MEASURED):** `crates/mem-bridge/src/inject.rs:1277` renders `<memory relevance="N%">`; upstream
  adds `id`, `created`, `project` attributes (`src/services/memory-provenance.ts:87`, used at
  `src/services/context.ts:38`) and a header sentence telling the model it may cite by id. The port's own
  header (`inject.rs` frame) does not.
- **Inverted world:** a measurement showing the host model never uses ids in practice, or a ruling that
  attribute rendering is noise against the token budget.
- **Disposition:** **new issue**, small; a strong candidate to attach to the existing injection work
  (149/161/171) instead of standing alone.

### G6 — Three upstream AI providers have no Rust client
- **Missing (MEASURED):** upstream `src/services/ai/providers/` has `api-route.ts`, `cheaper-inference.ts`,
  `runcrate.ts`; `ProviderKind` (`crates/mem-ai/src/provider/registry.rs:55`) has 8 kinds and none of
  these. Config keys are absent, so a user who configured them upstream gets "no provider" (dead-letter,
  issue 172) with no explanation.
- **Inverted world:** an owner ruling that these three are provider-additions out of port scope, or a
  measurement that the host already supplies them through `opencode` provider ids.
- **Disposition:** **new issue** (one, with three sub-items). Also an `OVERVIEW.md` §4.1 amendment.

### G7 — `updateOnCapture` (add / update / supersede on capture) does not exist
- **Missing (MEASURED):** `git grep -i updateOnCapture` → 0; upstream test asserts add/update/supersede
  and rollback-on-failed-supersede (`tests/update-on-capture.test.ts:86-125`). Consequence: capture is
  add-only, so repeated facts accumulate as duplicates — the exact problem `dreaming`/dedup then clean up.
- **Inverted world:** evidence that deduplication already absorbs the duplicate volume well enough that
  the owner rules the config N-A.
- **Disposition:** **new issue**, medium; config keys + decision hook in `capture.rs`.

### G8 — No WAL-index recovery; a corrupt `-shm` is not quarantined and retried
- **Missing (MEASURED):** `git grep -n 'quarantin' crates/` → only `*.quarantine-*` naming inside
  `crates/mem-bridge/tests/sweep.rs`; upstream `tests/turso-wal-index-recovery.test.ts:84` quarantines
  only index sidecars, retries the open exactly once, refuses while another process holds the recovery
  lock, and never moves a file when `integrity_check` fails. `crates/mem-store/src/db.rs:876` has
  `integrity_check` (verify) but no retry-on-invalid-page-type path.
- **Inverted world:** a measurement reproducing a torn `-shm` on this stack and showing the engine opens
  anyway (turso 0.8's own recovery), or a ruling that recovery belongs to the host.
- **Disposition:** **new issue**, resilience; rank below the user-facing features.

### G9 — `embeddingIdleTimeoutMs` does not exist; the ONNX model is never released
- **Missing (MEASURED):** key absent from config schema and example; `git grep -n idle crates/mem-embed/src` → 0.
  Upstream defaults to `0` (disabled), so default-behaviour parity holds; only operators who set it lose
  the RAM saving.
- **Inverted world:** the key is absent from the frozen `FileConfig` by decision (search issues/config-surface
  → nothing), or a measurement that RSS return via `ort` drop is negligible.
- **Disposition:** **new issue**, small; could be deferred with a `wontfix` ruling instead.

### G10 — Wrapper has no capture-provider readiness surface and no provider fallback order
- **Missing (MEASURED):** `grep provider plugin/src/plugin.js` → 0; upstream toasts a config reminder when
  auto-capture is on without a ready provider (`tests/auto-capture-config-toast.test.ts:101`) and tries
  opencode-first/external-first (`auto-capture-provider-order.test.ts`). The Rust engine publishes
  `auto_capture_provider_status` in `hello` (`crates/mem-core/src/config/schema.rs:479`), so the state is
  available; the surface is not.
- **Inverted world:** issue 172's dead-letter line is ruled a sufficient user-visible substitute.
- **Disposition:** **addition to issue 172** (or 63) rather than a new issue.

### G11 — withdrawn: the audit's claim was wrong (corrected 2026-11-26)

The first draft filed "pinned memories are not exempt from the injection age cutoff" as a partial gap,
claiming upstream keeps them. **MEASURED, and false.** Upstream's fixture
(`example/opencode-mem/tests/fixtures/memory-injection-eligibility.mjs:189`) adds `too-old-pinned` with
`pinned: true`; `ageFiltered` (`:192`) is asserted equal to `["recent"]` at test `:66-69` — the pinned row is
filtered out while the cutoff is enabled and returns only when it is disabled (`ageDisabled`). The Rust port
applies the same cutoff (`crates/mem-bridge/src/inject.rs:478-502`) and pins it with
`test_assemble_drops_rows_older_than_max_age_days` (`crates/mem-bridge/tests/inject_chat_gates.rs:67`) and
`test_max_age_days_drops_old_tier1_rows` (`:597`). Behaviour matches; row 12 of §1 is `covered-by`, not a gap.

Lesson kept deliberately: this was the report's only `partial` cell and it was the only cell whose
counterpart was inferred from a test *name* rather than opened. A gap claim is a claim about the oracle, not
about our code.

### G12 — Matrix and docs corrections (no code)
- Re-pin `docs/agents/testing.md` + `docs/agents/upstream-tests-*.txt` to `9086c65`/140 (30 new rows; the
  owner's call — issue 71 territory, issue 28 is the rehearsal).
- Fix `testing.md:231`'s stale "upstream has no `operation-lock.test.ts`".
- Record N-A rulings for #18/#24/#25/#26 (`cleanup-ai-sessions`, `transformers-resolve`,
  `plugin-turso-refcount`, `serialize-error`) so they stop appearing as gaps.
- **Disposition:** **doc correction**, folded into issue 71/28.

---

## 6. Deliberately not re-filed (checked so this report does not relitigate)

- Issue 27's N-A rulings stand: auto-update, i18n, runtime-require (`.scratch/rust-port/issues/27-deferred-behavior-sweep.md`).
- Issue 51's `opencodeModel: beam-free` and the Latin-script store census: `wontfix` by owner.
- Session-history UI absence: issue 145 closed *as a measurement* (by design).
- `turso-operation-lock`, `stats-*`, `profile-inject-empty-memories`, `memory-recall` are covered — only
  their matrix rows are missing.
- `memory-provenance`'s suffix form (`buildMemoryProvenanceSuffix`, used by `src/index.ts`) has no obvious
  Rust subject; only the attribute form (G5) is user-visible.

---

## 7. Open questions for the owner

1. **Scope ruling:** are upstream 2.33→2.39 features (markdown backend, dreaming, trash, update-on-capture,
   idle dispose, three providers) in scope for this port, or declared non-goals like i18n? This decides
   whether G1–G3, G6–G7, G9 become issues or `wontfix` rows.
2. **Re-pin now or after the rulings?** Re-pinning first makes all 30 dispositions explicit (and
   `check-parity-matrix.sh` will then demand 140/140 cells).
3. **D-19-9:** still as designed (vault tri-state), or superseded? It is the only accepted decision in the
   ADR set found to have no implementing code.
4. **Trash retention:** adopt upstream's `dreaming.trashRetentionDays` default 30, or tie purge to the
   existing retention sweep?
5. **G5 placement:** standalone small issue, or an AC on the live injection wave?

---

## 8. Reproduce this audit

```sh
git -C example/opencode-mem log -1 --format='%h %ci'
grep -m1 '"version"' example/opencode-mem/package.json
ls example/opencode-mem/tests/*.test.ts | wc -l
bash scripts/check-parity-matrix.sh
comm -23 <(ls example/opencode-mem/tests/*.test.ts | xargs -n1 basename | sort) \
         <(awk -F'|' '/^\| `tests\//{gsub(/`| /,"",$2); print $2}' docs/agents/testing.md | sed 's|^tests/||' | sort)
git grep -n 'shortcut:' -- . ':!example'
git grep -nE '\b(TODO|FIXME|XXX|HACK)\b' -- . ':!example'
git grep -il 'dreaming\|memoryBackend\|updateOnCapture\|embeddingIdleTimeout'
grep -n 'NOT_ERASED\|vault' crates/mem-bridge/src/purge.rs crates/mem-vault/src/lib.rs
```
