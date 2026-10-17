//! The worker protocol between the orchestrator (Lobotomy's core and Angela) and a worker host
//! (worker-interface.md). JSON-RPC 2.0, one JSON message per line on the worker host's stdin and
//! stdout (§10).
//!
//! - [`ops`]: what the orchestrator asks of a worker host (§3), with each method's params and
//!   result.
//! - [`event`]: what a worker host reports (§4, §5), and the snapshot a subscription starts with
//!   (§9).
//! - [`rpc`]: the JSON-RPC envelope and its framing.
//!
//! Every id a caller generates is also its idempotency key: the same id again returns the first
//! result (§3, §9).

pub mod event;
pub mod ops;
pub mod rpc;

/// The protocol version this crate speaks. A worker host and an orchestrator agree on one in
/// `hello` (§8).
pub const PROTOCOL_VERSION: &str = "0.1";

/// A method of the protocol: its name on the wire, its params and its result.
pub trait Method {
    const NAME: &'static str;
    type Params: serde::Serialize + serde::de::DeserializeOwned;
    type Result: serde::Serialize + serde::de::DeserializeOwned;
}
