//! Deterministic tiny ONNX fixture builder (std-only protobuf encoder).
//!
//! No committed ONNX binary, no network: every test case generates the model
//! bytes it needs. The graph is the smallest one that exercises the real
//! pipeline shape — an int64 token-id input with a symbolic `seq` dimension, a
//! `Gather` lookup against a constant embedding table, and an f32
//! `[1, seq, hidden]` output — plus an optional `Identity` branch that
//! declares an `attention_mask` input (so the mask-feeding path is covered).
//!
//! Layout: vocab 16 × hidden 4, `row(token)[k] = (token + 1) * 0.25 *
//! (k + 1) * scale`. Note that a uniform `scale` cancels under L2
//! normalization, so tests use a *negative* scale when they need to prove
//! which cached weight file the loader actually read.
//!
//! Verified loading under ort 2.0.0-rc.14 (recon at `/tmp/opencode/w14-recon`).

use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// protobuf wire format primitives
// ---------------------------------------------------------------------------

fn varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out
}

fn tag(field: u32, wire_type: u32) -> Vec<u8> {
    varint(((field << 3) | wire_type) as u64)
}

fn field_varint(field: u32, value: u64) -> Vec<u8> {
    let mut out = tag(field, 0);
    out.extend(varint(value));
    out
}

fn field_bytes(field: u32, bytes: &[u8]) -> Vec<u8> {
    let mut out = tag(field, 2);
    out.extend(varint(bytes.len() as u64));
    out.extend_from_slice(bytes);
    out
}

fn field_str(field: u32, value: &str) -> Vec<u8> {
    field_bytes(field, value.as_bytes())
}

/// A nested message field (`field: bytes` holding a serialized message).
fn msg(field: u32, body: &[u8]) -> Vec<u8> {
    field_bytes(field, body)
}

// ---------------------------------------------------------------------------
// ONNX protobuf pieces
// ---------------------------------------------------------------------------

/// Embedding-table rows in the fixture model.
pub const FIXTURE_VOCAB: usize = 16;
/// Embedding width in the fixture model.
pub const FIXTURE_HIDDEN: usize = 4;

const IR_VERSION: u64 = 8;
const OPSET_VERSION: u64 = 13;
const DATA_TYPE_FLOAT: u64 = 1;
const DATA_TYPE_INT64: u64 = 7;
/// Symbolic (dynamic) sequence-length dimension name shared by input/output.
const SYMBOLIC_SEQ: &str = "seq";

/// The embedding row for a token id: `(token + 1) * 0.25 * (k + 1) * scale`.
pub fn fixture_row(token_id: i64, scale: f32) -> Vec<f32> {
    let base = (token_id + 1) as f32 * 0.25 * scale;
    (1..=FIXTURE_HIDDEN as i64)
        .map(|k| base * k as f32)
        .collect()
}

/// `TensorProto` for the constant embedding table (`embeddings`).
fn embeddings_initializer(scale: f32) -> Vec<u8> {
    let mut raw = Vec::new();
    for token in 0..FIXTURE_VOCAB as i64 {
        for value in fixture_row(token, scale) {
            raw.extend(value.to_le_bytes());
        }
    }
    let mut tensor = Vec::new();
    tensor.extend(field_varint(1, FIXTURE_VOCAB as u64)); // dims[0]
    tensor.extend(field_varint(1, FIXTURE_HIDDEN as u64)); // dims[1]
    tensor.extend(field_varint(2, DATA_TYPE_FLOAT)); // data_type
    tensor.extend(field_str(8, "embeddings")); // name
    tensor.extend(field_bytes(9, &raw)); // raw_data (little-endian f32)
    tensor
}

fn dimension_value(value: usize) -> Vec<u8> {
    msg(1, &field_varint(1, value as u64))
}

fn dimension_param(name: &str) -> Vec<u8> {
    msg(1, &field_str(2, name))
}

fn tensor_type(elem_type: u64, dims: &[Vec<u8>]) -> Vec<u8> {
    let mut shape = Vec::new();
    for dim in dims {
        shape.extend(dim);
    }
    let mut tensor_type = Vec::new();
    tensor_type.extend(field_varint(1, elem_type));
    tensor_type.extend(msg(2, &shape));
    msg(1, &tensor_type)
}

fn value_info(name: &str, elem_type: u64, dims: &[Vec<u8>]) -> Vec<u8> {
    let mut info = Vec::new();
    info.extend(field_str(1, name));
    info.extend(msg(2, &tensor_type(elem_type, dims)));
    info
}

fn node(inputs: &[&str], outputs: &[&str], name: &str, op_type: &str) -> Vec<u8> {
    let mut node = Vec::new();
    for input in inputs {
        node.extend(field_str(1, input));
    }
    for output in outputs {
        node.extend(field_str(2, output));
    }
    node.extend(field_str(3, name));
    node.extend(field_str(4, op_type));
    node
}

/// `[batch = 1, seq]` int64 — the shape shared by the token-id inputs.
fn token_input_dims() -> Vec<Vec<u8>> {
    vec![dimension_value(1), dimension_param(SYMBOLIC_SEQ)]
}

/// Serialize the whole fixture model.
///
/// `scale` multiplies every embedding component; `with_attention_mask` adds an
/// `attention_mask` int64 `[1, seq]` input plus an `Identity` node that echoes
/// it as a second graph output, so a session must feed the mask to run.
pub fn tiny_model_bytes(scale: f32, with_attention_mask: bool) -> Vec<u8> {
    build_model(scale, with_attention_mask, false)
}

/// Like [`tiny_model_bytes`] but the graph additionally declares a
/// `token_type_ids` int64 `[1, seq]` input (the BERT-family name the real
/// nomic model uses) echoed by an `Identity` node — so a session must feed the
/// declared type-id input to run. Regression coverage for the real-model
/// `token_type_ids` feed path.
pub fn tiny_model_bytes_with_token_type_ids(scale: f32) -> Vec<u8> {
    build_model(scale, false, true)
}

fn build_model(scale: f32, with_attention_mask: bool, with_token_type_ids: bool) -> Vec<u8> {
    let mut graph = Vec::new();
    // 1 = node
    graph.extend(msg(
        1,
        &node(
            &["embeddings", "input_ids"],
            &["token_embeddings"],
            "gather",
            "Gather",
        ),
    ));
    if with_attention_mask {
        graph.extend(msg(
            1,
            &node(&["attention_mask"], &["mask_echo"], "identity", "Identity"),
        ));
    }
    if with_token_type_ids {
        graph.extend(msg(
            1,
            &node(
                &["token_type_ids"],
                &["type_echo"],
                "type-identity",
                "Identity",
            ),
        ));
    }
    // 2 = name
    graph.extend(field_str(2, "tiny-embed-fixture"));
    // 5 = initializer
    graph.extend(msg(5, &embeddings_initializer(scale)));
    // 11 = input
    graph.extend(msg(
        11,
        &value_info("input_ids", DATA_TYPE_INT64, &token_input_dims()),
    ));
    if with_attention_mask {
        graph.extend(msg(
            11,
            &value_info("attention_mask", DATA_TYPE_INT64, &token_input_dims()),
        ));
    }
    if with_token_type_ids {
        graph.extend(msg(
            11,
            &value_info("token_type_ids", DATA_TYPE_INT64, &token_input_dims()),
        ));
    }
    // 12 = output
    graph.extend(msg(
        12,
        &value_info(
            "token_embeddings",
            DATA_TYPE_FLOAT,
            &[
                dimension_value(1),
                dimension_param(SYMBOLIC_SEQ),
                dimension_value(FIXTURE_HIDDEN),
            ],
        ),
    ));
    if with_attention_mask {
        graph.extend(msg(
            12,
            &value_info("mask_echo", DATA_TYPE_INT64, &token_input_dims()),
        ));
    }
    if with_token_type_ids {
        graph.extend(msg(
            12,
            &value_info("type_echo", DATA_TYPE_INT64, &token_input_dims()),
        ));
    }

    let mut model = Vec::new();
    model.extend(field_varint(1, IR_VERSION)); // ir_version
    model.extend(field_str(2, "mem-embed-fixture")); // producer_name
    model.extend(msg(7, &graph)); // graph
    model.extend(msg(8, &field_varint(2, OPSET_VERSION))); // opset_import
    model
}

/// Write fixture bytes under `dir/file_name` (parents created), e.g.
/// `write_tiny_model(model_dir, "onnx/model.onnx", 1.0, false)`.
pub fn write_tiny_model(
    dir: &Path,
    file_name: &str,
    scale: f32,
    with_attention_mask: bool,
) -> PathBuf {
    super::write_bytes(
        dir,
        file_name,
        &tiny_model_bytes(scale, with_attention_mask),
    )
}

/// Write a `token_type_ids`-declaring fixture under `dir/file_name`.
pub fn write_tiny_model_with_token_type_ids(dir: &Path, file_name: &str, scale: f32) -> PathBuf {
    super::write_bytes(dir, file_name, &tiny_model_bytes_with_token_type_ids(scale))
}

/// Path of the committed canonical `tokenizer.json` fixture.
pub fn tiny_tokenizer_path() -> PathBuf {
    super::fixture_path("tiny_tokenizer.json")
}

/// Bytes of the committed tokenizer fixture (for fake hub transports).
pub fn tiny_tokenizer_bytes() -> Vec<u8> {
    std::fs::read(tiny_tokenizer_path()).expect("read committed tokenizer fixture")
}
