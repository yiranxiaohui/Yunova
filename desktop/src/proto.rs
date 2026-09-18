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
    /// Reconnect with a stored device token.
    Hello {
        token: String,
        name: String,
        platform: Option<String>,
    },
    /// First run: sign in with the user's account, which binds this machine
    /// and returns a device token to store.
    Login {
        username: String,
        password: String,
        name: String,
        platform: Option<String>,
        /// Stable machine identifier, so re-running this client rebinds the
        /// same device row instead of registering a duplicate.
        fingerprint: Option<String>,
    },
    /// First run in the desktop app: the site window is already signed in, so
    /// its session token is proof enough to bind this machine. No password is
    /// asked for, because the user already gave one to the page.
    Attach {
        session: String,
        name: String,
        platform: Option<String>,
        fingerprint: Option<String>,
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
    /// The directories this machine lets tasks run in.
    ///
    /// Sent right after the handshake and again when the user edits them, so
    /// the web UI can offer a picker. Advertising them is not delegating the
    /// decision: the list is what this client will *accept*, and it re-checks
    /// every start against it.
    Workspaces {
        default: String,
        roots: Vec<WorkspaceRoot>,
        /// How this machine gates tool calls, for display only.
        ///
        /// Reported for the same reason the roots are: the web UI is where the
        /// user watches a task, and "why is nothing asking me" or "why is this
        /// asking about every file" is unanswerable from there otherwise. The
        /// server can render it and never set it — the gate is written by this
        /// client into a directory the server cannot reach.
        approval: String,
    },
    /// Answer to a `list_dir` request.
    DirListing {
        req_id: u64,
        /// The directory listed, or `None` when the roots themselves were.
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        /// Where "up" goes, stopping at a root so the picker cannot walk out
        /// of the authorized scope.
        #[serde(skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        entries: Vec<DirEntry>,
        /// Set instead of `entries` when the request was refused or failed.
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

/// One directory this machine authorizes.
#[derive(Debug, Serialize)]
pub struct WorkspaceRoot {
    pub path: String,
    pub label: String,
}

/// One child directory in a listing.
#[derive(Debug, Serialize)]
pub struct DirEntry {
    pub path: String,
    pub name: String,
    pub repo: bool,
    /// An authorized root that does not exist on disk right now.
    ///
    /// Only ever set on the root screen: children are read from the
    /// filesystem, so they exist by construction. The picker needs to know,
    /// because entering such a root fails — a directory cannot be canonicalized
    /// before it exists — and an entry that can only produce an error is worse
    /// than one shown as unavailable.
    pub missing: bool,
}

/// Frames the server sends to this client.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromServer {
    HelloOk {
        device_id: i64,
        /// Present only right after a sign-in: the device token to store, so
        /// the password is never kept on disk.
        #[serde(default)]
        token: Option<String>,
        /// The account this machine was just bound to. Needed because the
        /// automatic path never asks for a username, so the server is the
        /// only one that knows it.
        #[serde(default)]
        username: Option<String>,
    },
    Error {
        message: String,
    },
    StartRuntime {
        session_id: i64,
        models_json: Value,
        /// The directory this task was created with.
        ///
        /// Absent for a task that named none, which is every task created
        /// before tasks could choose. Present values are re-checked against
        /// the local roots before anything starts.
        #[serde(default)]
        workspace: Option<String>,
        /// How this task asked tool calls to be gated.
        ///
        /// Honoured only when it is *stricter* than this machine's own
        /// setting, which is what keeps the policy local: the server can ask
        /// for more confirmations, never for fewer. Absent means "this
        /// machine's own policy", which is what every earlier task meant.
        #[serde(default)]
        approval: Option<String>,
    },
    /// One JSONL record for a local runtime's stdin, verbatim.
    Frame {
        session_id: i64,
        frame: Value,
    },
    StopRuntime {
        session_id: i64,
    },
    /// List the directories under `path`, for the web UI's picker.
    ///
    /// `None` asks for the authorized roots. Anything else is answered only
    /// when it lies inside one of them.
    ListDir {
        req_id: u64,
        #[serde(default)]
        path: Option<String>,
    },
}
