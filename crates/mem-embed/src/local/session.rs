//! Committed ONNX session wrapper: one loaded session behind a `std::sync`
//! mutex, feeding only the inputs the model declares.
//!
//! ort's `Session::run` takes `&mut self`, and the tests use current-thread
//! runtimes, so the session lives in a `std::sync::Mutex` (never a `tokio`
//! one). A poisoned lock is recovered via `into_inner` rather than panicking.
//!
//! Feeding rules (transformers.js parity):
//! - `input_ids` is always fed as an int64 `[1, seq]` tensor;
//! - when the model declares `attention_mask`, the encoded mask is fed, or an
//!   all-ones mask for the unpadded single sequence;
//! - when the model declares `token_type_ids` (the BERT-family name) or
//!   `type_ids`, the encoded type ids are fed, or zeros;
//! - inputs the model does not declare are omitted entirely.

use std::sync::Mutex;

use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;

use crate::error::EmbedError;

/// A loaded ONNX session plus the names it declares.
pub struct LoadedSession {
    session: Mutex<Session>,
    input_names: Vec<String>,
    output_name: String,
}

impl LoadedSession {
    /// Load a graph from memory into the process-wide environment.
    ///
    /// `intra_threads` maps to ort's `with_intra_threads` pool control.
    pub fn load(
        model: &str,
        bytes: &[u8],
        intra_threads: Option<usize>,
    ) -> Result<Self, EmbedError> {
        let environment = crate::runtime::ort_environment()?;
        let mut builder =
            Session::builder(&environment).map_err(|error| model_load(model, error))?;
        if let Some(threads) = intra_threads {
            builder = builder
                .with_intra_threads(threads)
                .map_err(|error| model_load(model, error))?;
        }
        let session = builder
            .commit_from_memory(bytes)
            .map_err(|error| model_load(model, error))?;

        let input_names = session
            .inputs()
            .iter()
            .map(|input| input.name().to_string())
            .collect();
        let output_name = session
            .outputs()
            .first()
            .map(|output| output.name().to_string())
            .ok_or_else(|| EmbedError::Inference {
                message: "the ONNX session declares no outputs".to_string(),
            })?;

        Ok(Self {
            session: Mutex::new(session),
            input_names,
            output_name,
        })
    }

    /// Declared input names, in declaration order.
    pub fn input_names(&self) -> &[String] {
        &self.input_names
    }

    /// Name of the first declared graph output.
    pub fn output_name(&self) -> &str {
        &self.output_name
    }

    /// Whether the model declares an input with this name.
    pub fn declares(&self, name: &str) -> bool {
        self.input_names.iter().any(|declared| declared == name)
    }

    /// Run inference for a single sequence.
    ///
    /// Returns `(output shape, f32 data)` of the first declared output.
    pub fn run(
        &self,
        ids: &[i64],
        attention_mask: Option<&[i64]>,
        type_ids: Option<&[i64]>,
    ) -> Result<(Vec<i64>, Vec<f32>), EmbedError> {
        let sequence_length = ids.len();
        let mut feed: Vec<(String, SessionInputValue<'_>)> = Vec::with_capacity(3);
        feed.push(("input_ids".to_string(), i64_tensor(ids)?.into()));
        if self.declares("attention_mask") {
            let mask = match attention_mask {
                Some(mask) => mask.to_vec(),
                None => vec![1i64; sequence_length],
            };
            feed.push(("attention_mask".to_string(), i64_tensor(&mask)?.into()));
        }
        // BERT-family graphs (nomic-embed-text-v1 included) name this input
        // `token_type_ids`; some graphs emit the shorter `type_ids`.
        if let Some(name) = ["token_type_ids", "type_ids"]
            .into_iter()
            .find(|name| self.declares(name))
        {
            let types = match type_ids {
                Some(types) => types.to_vec(),
                None => vec![0i64; sequence_length],
            };
            feed.push((name.to_string(), i64_tensor(&types)?.into()));
        }

        let mut session = self
            .session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let outputs = session.run(feed).map_err(|error| EmbedError::Inference {
            message: error.to_string(),
        })?;
        let output =
            outputs
                .get(self.output_name.as_str())
                .ok_or_else(|| EmbedError::Inference {
                    message: format!(
                        "session output `{}` is missing from the run",
                        self.output_name
                    ),
                })?;
        let (shape, data) =
            output
                .try_extract_tensor::<f32>()
                .map_err(|error| EmbedError::TensorShape {
                    message: error.to_string(),
                })?;

        Ok((shape.iter().copied().collect(), data.to_vec()))
    }
}

/// An int64 `[1, len]` input tensor.
fn i64_tensor(values: &[i64]) -> Result<Tensor<i64>, EmbedError> {
    Tensor::from_array(([1usize, values.len()], values.to_vec())).map_err(|error| {
        EmbedError::TensorShape {
            message: error.to_string(),
        }
    })
}

fn model_load(model: &str, error: impl std::fmt::Display) -> EmbedError {
    EmbedError::ModelLoad {
        model: model.to_string(),
        message: error.to_string(),
    }
}
