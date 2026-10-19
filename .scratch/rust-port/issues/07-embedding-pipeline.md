# 07 — W1.4 · Embedding pipeline — local ORT + remote (US 2.3)

Status: done — 2026-10-18

## What to build

`mem-embed` — `ort = "=2.0.0-rc.14"` (exact pin) + HF `tokenizers`: ONNX model resolution/loading, presets, dtypes (fp32/fp16/q8/int8 via weight-quantized models), task prefixes; **mean-pooling + L2-norm post-processing — the real parity surface** (implement the fastembed-rs recipe for the default `Xenova/nomic-embed-text-v1`); remote provider embeddings with exponential backoff. ort is sync: inference via `spawn_blocking`, `with_intra_threads(n)` for pool control (roadmap §0b D4). Wave detail: `.scratch/rust-port/roadmap.md` §W1.4.

## Acceptance criteria

- [ ] Behaviors ported: `embedding-model-presets`, `embedding-onnx-dtype`, `embedding-remote-retry`, `embedding-task-prefixes`, `onnxruntime-resolve`, `memory-tool-warmup` (warmup behavior).
- [ ] Golden-vector tests: recorded TS-side embeddings for fixed inputs reproduced within a small numeric tolerance (same ONNX engine, ort 1.31 vs upstream 1.30).
- [ ] Deterministic tests via a tiny bundled ONNX fixture (no network needed).
- [ ] q8 path uses weight-quantized models (int64 inputs are dtype-independent).

## Blocked by

- 02 · W0.2 (can run concurrently with 05–06)

## Comments

- **2026-10-18:** Closed. Reviewer APPROVED (session `ses_ead65a2bfffeWplMJyfVdst6pk`). Test evidence: 419 passed / 0 failed / 2 ignored (baseline 330 + 89 new). Fan-out wave: s1 (steps 3-6, 32 tests), s2 (steps 7-8, 14 tests), s3 (steps 9-12, 37 tests), captain (steps 1-2, 13-14). Golden-vector: cosine 1.000000, max-abs 1.416e-7. L1-L6 ledger battery green over all four ledgers. One attributed foreign clippy failure (octal_escapes in s3's embedding_service.rs:289, left untouched per directive). Cross-shard integration fix: token_type_ids/type_ids in s1's session.rs. Next: W1.5 r1 coder (encryption + operation lock, approved `b667e70e972a`).
