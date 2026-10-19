// Records the nomic-embed-text-v1 golden vectors for `golden_vectors.rs` using
// the upstream JS stack (transformers.js + onnxruntime-node) — the exact
// runtime the behavioural contract is measured against.
//
// NOT run by cargo. Run once, deliberately, from a scratch dir OUTSIDE the
// repo (no node_modules may ever land in the tree). See README.md for the full
// recipe. Required env:
//   TS_GOLDEN_CACHE — transformers.js cache dir (the model lives here)
//   GOLDEN_OUT      — destination path for the JSON fixture
//
// Usage (from the scratch dir holding the npm install):
//   TS_GOLDEN_CACHE=/tmp/opencode/w14-model-cache \
//   GOLDEN_OUT=/path/to/crates/mem-embed/tests/fixtures/nomic_goldens.json \
//     node record_ts_goldens.mjs

import { writeFileSync } from "node:fs";
import { env, pipeline } from "@huggingface/transformers";

const cacheDir = process.env.TS_GOLDEN_CACHE;
const outPath = process.env.GOLDEN_OUT;
if (!cacheDir || !outPath) {
  throw new Error("TS_GOLDEN_CACHE and GOLDEN_OUT must both be set");
}

// Pin the same cache layout our Rust loader reads:
//   {cacheDir}/{model}/{config.json,tokenizer.json,onnx/model.onnx}
env.cacheDir = cacheDir;
env.allowLocalModels = true;
env.allowRemoteModels = true;

const model = "Xenova/nomic-embed-text-v1";
const dtype = "fp32";
const pooling = "mean";
const normalize = true;

const inputs = [
  "hello world",
  "The quick brown fox jumps over the lazy dog.",
  "search_query: what is a vector database?",
];

const extractor = await pipeline("feature-extraction", model, { dtype });

const embeddings = [];
for (const input of inputs) {
  const output = await extractor(input, { pooling, normalize });
  embeddings.push(Array.from(output.data));
}

const goldens = {
  model,
  pooling,
  normalize,
  dtype,
  dims: embeddings[0].length,
  inputs,
  embeddings,
};

writeFileSync(outPath, `${JSON.stringify(goldens)}\n`, "utf8");
console.log(`wrote ${embeddings.length} embeddings of dim ${goldens.dims} to ${outPath}`);
