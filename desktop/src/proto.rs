//! Protocol shared with the server's `agent_device` module.
//!
//! Kept as a separate, hand-written mirror rather than a shared crate so the
//! client stays a standalone binary a user can drop onto a machine. The
//! variants must stay in step with `src/agent_device.rs`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Frames this client sends to the server.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToServer {
    Hello {
        token: String,
        name: String,
        platform: Option<String>,
    },
    Heartbeat,
    /// One JSONL record from a local runtime's stdout, verbatim.
    Frame {
        session_id: i64,
        frame: Value,
    },
    RuntimeClosed {
        session_id: i64,
        reason: Option<String>,
    },
    RuntimeError {
        session_id: i64,
        message: String,
    },
}

/// Frames the server sends to this client.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromServer {
    HelloOk {
        device_id: i64,
    },
    Error {
        message: String,
    },
    StartRuntime {
        session_id: i64,
        models_json: Value,
    },
    /// One JSONL record for a local runtime's stdin, verbatim.
    Frame {
        session_id: i64,
        frame: Value,
    },
    StopRuntime {
        session_id: i64,
    },
}
