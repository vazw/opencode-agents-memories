# mem-embed golden-vector fixtures

`nomic_goldens.json` holds the upstream-parity embeddings for
`Xenova/nomic-embed-text-v1`. It is consumed by `tests/golden_vectors.rs`:
`test_nomic_goldens_metadata_and_shapes` (always-on) pins the fixture's
metadata/shape contract, and the `#[ignore]`d
`test_nomic_embeddings_match_recorded_ts_goldens` compares our Rust pipeline
against the recorded vectors within tolerance.

## Provenance

The numbers are recorded from the same JS stack the upstream tests run:

- `@huggingface/transformers@4.3.1`
- `onnxruntime-node@1.30.0`
- model `Xenova/nomic-embed-text-v1`, `dtype: "fp32"`
- extraction options `{ pooling: "mean", normalize: true }`

The pinned inputs (verbatim, in order):

1. `hello world`
2. `The quick brown fox jumps over the lazy dog.`
3. `search_query: what is a vector database?`

## Cache layout

transformers.js writes the model into `TS_GOLDEN_CACHE` exactly where our Rust
loader reads it:

```
{cache}/{model}/config.json
{cache}/{model}/tokenizer.json
{cache}/{model}/onnx/model.onnx
```

So `MEM_EMBED_GOLDEN_CACHE` (Rust) and `TS_GOLDEN_CACHE` (JS) point at the same
directory: `/tmp/opencode/w14-model-cache`.

## Re-record instructions

Nothing here runs under `cargo test` — the recording is a deliberate, networked,
wave-time action. **Never install node_modules inside the repo.**

```bash
REPO=/home/vaz/Code/Personal/Rust/opencode-agents-memory
mkdir -p /tmp/opencode/w14-ts-goldens && cd /tmp/opencode/w14-ts-goldens
npm init -y
npm i @huggingface/transformers@4.3.1 onnxruntime-node@1.30.0
# onnxruntime-node ships a native binary via a blocked-by-default postinstall
# under npm's allow-scripts policy; run it explicitly:
(cd node_modules/onnxruntime-node && node ./script/install)

# Node resolves bare imports from the *script's* directory, so copy the
# recording script next to node_modules and run it there.
cp "$REPO/crates/mem-embed/tests/fixtures/record_ts_goldens.mjs" .
TS_GOLDEN_CACHE=/tmp/opencode/w14-model-cache \
GOLDEN_OUT="$REPO/crates/mem-embed/tests/fixtures/nomic_goldens.json" \
  node record_ts_goldens.mjs
```

### Canonicalize the cached tokenizer (required for the Rust check)

transformers.js happily reads HF's legacy `tokenizer.json` (`"version": "1.0"`),
but the `tokenizers` `1.0.0-rc.2` reader our port uses accepts **only** the
canonical v2.0 form. The recording step above therefore leaves the cache in a
state the Rust side cannot read; upgrade the cached file once with the rc.2
`canonicalize_file` pass. From any scratch dir (offline; uses the crate already
in the cargo registry cache):

```bash
mkdir -p /tmp/opencode/w14-canon/src && cd /tmp/opencode/w14-canon
cat > Cargo.toml <<'TOML'
[package]
name = "w14-canon"
version = "0.0.0"
edition = "2021"

[dependencies]
tokenizers = { version = "=1.0.0-rc.2", default-features = false }
TOML
cat > src/main.rs <<'RS'
fn main() {
    let path = std::env::args().nth(1).expect("usage: w14-canon <tokenizer.json>");
    let canonical = tokenizers::canonicalize_file(&path).expect("canonicalize_file");
    std::fs::write(&path, canonical).expect("write");
    tokenizers::from_json_file(&path).expect("rc.2 reader accepts canonical file");
    println!("canonicalized {path}");
}
RS
cargo run --release --offline -- \
  /tmp/opencode/w14-model-cache/Xenova/nomic-embed-text-v1/tokenizer.json
```

The file must then start with `"version": "2.0"`. This upgrade is a pure format
change — it does not alter token ids, so it does not affect the recorded
goldens.

Then re-run the tolerance check (see also the top of `golden_vectors.rs`):

```bash
MEM_EMBED_GOLDEN_CACHE=/tmp/opencode/w14-model-cache \
  cargo test -p mem-embed --test golden_vectors -- --ignored --nocapture
```

## Tolerance rationale

Our inference runtime is `ort` 2.0.0-rc.14; the goldens come from
onnxruntime-node 1.30.0. Same graph, different operator-kernel versions, so
exact bit equality is not expected. The gate is `cos >= 0.9999` and
`max_abs <= 1e-3` per input — tight enough to catch a real pipeline break
(wrong pooling strategy, missing L2 normalization, a different dtype weight
file) while absorbing fp32 kernel noise.
