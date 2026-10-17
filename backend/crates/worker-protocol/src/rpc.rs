//! The JSON-RPC 2.0 envelope, and its framing: one message per line, as MCP's stdio transport
//! does (worker-interface.md §10). A line that does not parse leaves the stream usable: the next
//! line is the next message.

use std::io;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

use crate::Method;

const VERSION: &str = "2.0";

/// A request's id: the caller's, echoed in the response.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Number(i64),
    String(String),
}

/// Any message on the wire. Tried in this order: a request has an id and a method, a response an
/// id and no method, a notification a method and no id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    Request(Request),
    Response(Response),
    Notification(Notification),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: Id,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Id,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// A JSON-RPC error. The protocol's own refusals are [`RpcError::rejected`]: code
/// [`REJECTED`], with a reason a program can match in `data.reason`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
/// The call was understood and refused: its preconditions do not hold, such as an unknown task,
/// an invalid spec or an id reused for something else.
pub const REJECTED: i64 = -32000;

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    pub fn rejected(reason: &str, message: impl Into<String>) -> Self {
        Self { code: REJECTED, message: message.into(), data: Some(serde_json::json!({ "reason": reason })) }
    }

    /// The reason of a refusal; `None` for other errors.
    pub fn reason(&self) -> Option<&str> {
        (self.code == REJECTED).then(|| self.data.as_ref()?.get("reason")?.as_str()).flatten()
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.reason() {
            Some(reason) => write!(f, "{reason}: {}", self.message),
            None => write!(f, "{} ({})", self.message, self.code),
        }
    }
}

impl std::error::Error for RpcError {}

impl Request {
    pub fn new<M: Method>(id: Id, params: &M::Params) -> Self {
        let params = serde_json::to_value(params).expect("protocol params are JSON serializable");
        Self { jsonrpc: VERSION.into(), id, method: M::NAME.into(), params }
    }

    /// The params, if this is a call of `M` and they parse.
    pub fn params<M: Method>(&self) -> Result<M::Params, RpcError> {
        if self.method != M::NAME {
            return Err(RpcError::new(METHOD_NOT_FOUND, format!("not {}: {}", M::NAME, self.method)));
        }
        serde_json::from_value(self.params.clone()).map_err(|e| RpcError::new(INVALID_PARAMS, e.to_string()))
    }
}

impl Response {
    pub fn ok<M: Method>(id: Id, result: &M::Result) -> Self {
        let result = serde_json::to_value(result).expect("protocol results are JSON serializable");
        Self { jsonrpc: VERSION.into(), id, result: Some(result), error: None }
    }

    pub fn err(id: Id, error: RpcError) -> Self {
        Self { jsonrpc: VERSION.into(), id, result: None, error: Some(error) }
    }

    /// The result as `M`'s, or the error the other side sent.
    pub fn into_result<M: Method>(self) -> Result<M::Result, RpcError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        serde_json::from_value(self.result.unwrap_or(Value::Null))
            .map_err(|e| RpcError::new(INVALID_REQUEST, format!("the result of {} does not parse: {e}", M::NAME)))
    }
}

impl Notification {
    pub fn new(method: &str, params: &impl Serialize) -> Self {
        let params = serde_json::to_value(params).expect("protocol notifications are JSON serializable");
        Self { jsonrpc: VERSION.into(), method: method.into(), params }
    }
}

/// Reads the next message. `Ok(None)` at the end of the stream. A line that is not a message is
/// an [`io::ErrorKind::InvalidData`] error; reading again goes on with the next line.
pub async fn read<R: AsyncBufRead + Unpin>(reader: &mut R) -> io::Result<Option<Message>> {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        if !line.trim().is_empty() {
            break;
        }
    }
    serde_json::from_str(&line).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Writes one message and flushes it.
pub async fn write<W: AsyncWrite + Unpin>(writer: &mut W, message: &Message) -> io::Result<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await
}
