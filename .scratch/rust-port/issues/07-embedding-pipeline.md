# 07 — W1.4 · Embedding pipeline — local ORT + remote (US 2.3)

Status: ready-for-agent

## What to build

`mem-embed` — `ort = "=2.0.0-rc.14"` (exact pin) + HF `tokenizers`: ONNX model resolution/loading, presets, dtypes (fp32/fp16/q8/int8 via weight-quantized models), task prefixes; **mean-pooling + L2-norm post-processing — the real parity surface** (implement the fastembed-rs recipe for the default `Xenova/nomic-embed-text-v1`); remote provider embeddings with exponential backoff. ort is sync: inference via `spawn_blocking`, `with_intra_threads(n)` for pool control (roadmap §0b D4). Wave detail: `.scratch/rust-port/roadmap.md` §W1.4.

## Acceptance criteria

- [ ] Behaviors ported: `embedding-model-presets`, `embedding-onnx-dtype`, `embedding-remote-retry`, `embedding-task-prefixes`, `onnxruntime-resolve`, `memory-tool-warmup` (warmup behavior).
- [ ] Golden-vector tests: recorded TS-side embeddings for fixed inputs reproduced within a small numeric tolerance (same ONNX engine, ort 1.31 vs upstream 1.30).
- [ ] Deterministic tests via a tiny bundled ONNX fixture (no network needed).
- [ ] q8 path uses weight-quantized models (int64 inputs are dtype-independent).

## Blocked by

- 02 · W0.2 (can run concurrently with 05–06)
