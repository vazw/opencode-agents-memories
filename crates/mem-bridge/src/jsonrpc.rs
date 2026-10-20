//! JSON-RPC 2.0 NDJSON codec (ADR-0007 D-42, `docs/bridge-schema.md` §1/§4).
//!
//! One JSON object per line. Lexicographic key order at every level is the
//! canonical wire form: `serde_json`'s default map is a `BTreeMap`, so a
//! `serde_json::Value` round trip is already canonical — both `Request` and
//! `Response` build their output through `serde_json::Value`, which makes
//! byte-equality fixtures checkable.
//!
//! `serde_json::to_string` never emits a trailing `.0` for integral floats,
//! and `RequestId::Number` carries `i64`, so the id round trip is Loss-less.

use serde::Deserialize;
use serde_json::{Map, Value};

/// Correlation id: string, number, or null (a parse error's answered id).
#[derive(Debug, Clone, PartialEq)]
pub enum RequestId {
    Null,
    String(String),
    Number(i64),
}

impl RequestId {
    /// The string form or `None`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Null => Some(Self::Null),
            Value::String(text) => Some(Self::String(text.clone())),
            Value::Number(number) => number.as_i64().map(Self::Number),
            _ => None,
        }
    }

    fn to_value(&self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::String(text) => text.as_str().into(),
            Self::Number(number) => (*number).into(),
        }
    }
}

impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Self::from_value(&value)
            .ok_or_else(|| serde::de::Error::custom("id must be a string, number or null"))
    }
}

/// A parsed JSON-RPC request: method, params and the correlation id. v1 has
/// no notifications, so a request without `id` is invalid.
#[derive(Debug, Clone, PartialEq)]
/// A missing `params` renders as [`Value::Null`]; encoding drops a null
/// params member, so a request without params byte-round-trips.
pub struct Request {
    pub id: RequestId,
    pub method: String,
    pub params: Value,
}

impl Request {
    /// Reads one named parameter of the `params` object (`None` when params
    /// are absent or the key is missing).
    pub fn param(&self, name: &str) -> Option<&Value> {
        self.params.get(name)
    }
}

impl From<&Request> for Value {
    fn from(request: &Request) -> Self {
        let mut map = Map::new();
        map.insert("id".to_owned(), request.id.to_value());
        map.insert("jsonrpc".to_owned(), "2.0".into());
        map.insert("method".to_owned(), request.method.as_str().into());
        if !request.params.is_null() {
            map.insert("params".to_owned(), request.params.clone());
        }
        Value::Object(map)
    }
}

/// A parse-line failure: parse error (`-32700`) or invalid request
/// (`-32600`, including a batched array and a missing id/method).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseLineError {
    Parse,
    InvalidRequest,
}

impl ParseLineError {
    /// The JSON-RPC error code this failure answers with.
    pub fn code(self) -> i32 {
        match self {
            Self::Parse => -32700,
            Self::InvalidRequest => -32600,
        }
    }

    /// The JSON-RPC message text.
    pub fn message(self) -> &'static str {
        match self {
            Self::Parse => "Parse error",
            Self::InvalidRequest => "Invalid Request",
        }
    }
}

impl From<ParseLineError> for i32 {
    fn from(error: ParseLineError) -> Self {
        error.code()
    }
}

impl From<ParseLineError> for &'static str {
    fn from(error: ParseLineError) -> Self {
        error.message()
    }
}

/// `encode_request`'s output plus newline; used by the stdio loop.
fn to_line(value: &Value) -> String {
    let mut line = serde_json::to_string(value).expect("Value serializes");
    line.push('\n');
    line
}

/// Parses one NDJSON line into a [`Request`].
///
/// A line may carry the trailing `\n`. A JSON array — a batch — is not a v1
/// request and is rejected with `InvalidRequest`; so is a non-object, a
/// missing id or a missing method.
pub fn parse_line(line: &str) -> Result<Request, ParseLineError> {
    let value = serde_json::from_str::<Value>(line.trim_end_matches('\n'))
        .map_err(|_error| ParseLineError::Parse)?;
    let object = match &value {
        Value::Object(map) => map,
        _ => return Err(ParseLineError::InvalidRequest),
    };
    let jsonrpc = object.get("jsonrpc").and_then(Value::as_str);
    if jsonrpc != Some("2.0") {
        return Err(ParseLineError::InvalidRequest);
    }
    let id = object
        .get("id")
        .and_then(RequestId::from_value)
        .ok_or(ParseLineError::InvalidRequest)?;
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .ok_or(ParseLineError::InvalidRequest)?
        .to_owned();
    let params = match object.get("params") {
        None => Value::Null,
        Some(Value::Object(params)) => Value::Object(params.clone()),
        Some(_other) => return Err(ParseLineError::InvalidRequest),
    };
    Ok(Request { id, method, params })
}

/// Serializes a request to one canonically-encoded NDJSON line.
pub fn encode_request(request: &Request) -> String {
    to_line(&request.into())
}

/// The `error` envelope member: code, message, optional advisory `data`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TransportError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl TransportError {
    fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("code".to_owned(), self.code.into());
        map.insert("message".to_owned(), self.message.as_str().into());
        if let Some(data) = &self.data {
            map.insert("data".to_owned(), data.clone());
        }
        Value::Object(map)
    }
}

/// A JSON-RPC response: result or error, never both and never neither on the
/// v1 surface (every request is answered).
#[derive(Debug, Clone, Deserialize)]
pub struct Response {
    pub id: RequestId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<TransportError>,
}

impl Response {
    /// A success response.
    pub fn result(id: RequestId, result: Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    /// An error response without advisory data.
    pub fn error(id: RequestId, code: i32, message: impl Into<String>) -> Self {
        Self::error_with_data(id, code, message, Value::Null)
    }

    /// An error response with optional advisory data; null data is dropped.
    pub fn error_with_data(
        id: RequestId,
        code: i32,
        message: impl Into<String>,
        data: Value,
    ) -> Self {
        let data = match data {
            Value::Null => None,
            other => Some(other),
        };
        Self {
            id,
            result: None,
            error: Some(TransportError {
                code,
                message: message.into(),
                data,
            }),
        }
    }

    /// The canonical `-32700` envelope: the id is not recoverable, so it is
    /// null (JSON-RPC 2.0, `tests/fixtures/bridge/error-32700.json`).
    pub fn parse_error() -> Self {
        Self::error(
            RequestId::Null,
            ParseLineError::Parse.into(),
            ParseLineError::Parse.message(),
        )
    }

    /// The canonical `-32600` envelope with a null id.
    pub fn invalid_request() -> Self {
        Self::error(
            RequestId::Null,
            ParseLineError::InvalidRequest.into(),
            ParseLineError::InvalidRequest.message(),
        )
    }

    /// The canonical envelope for a rejected line, with a recoverable id
    /// where the failure kind allows one.
    pub fn from_parse_line_error(id: RequestId, error: ParseLineError) -> Self {
        Self::error(id, error.into(), error.message())
    }

    fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("id".to_owned(), self.id.to_value());
        map.insert("jsonrpc".to_owned(), "2.0".into());
        if let Some(result) = &self.result {
            map.insert("result".to_owned(), result.clone());
        }
        if let Some(error) = &self.error {
            map.insert("error".to_owned(), error.to_value());
        }
        Value::Object(map)
    }
}

impl serde::Serialize for Response {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serde_json::Value::serialize(&self.to_value(), serializer)
    }
}

/// Serializes a response to one canonically-encoded NDJSON line.
pub fn encode_response(response: &Response) -> String {
    to_line(&response.to_value())
}
