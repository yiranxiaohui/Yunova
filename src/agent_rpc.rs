//! pi RPC protocol: framing, commands, and the events clients care about.
//!
//! `pi --mode rpc` speaks JSONL over stdin/stdout. Both execution targets use
//! the identical protocol, so everything above the transport — session
//! mirroring, event fan-out, approval routing — is written once here and
//! reused by every driver.
//!
//! Only the subset Yunova actually drives is modelled. Events are kept as
//! opaque `serde_json::Value` after their `type` is read, because the client
//! renders far more of the payload than the server ever inspects, and pinning
//! every field would break on a pi upgrade that adds one.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Strict JSONL framing, per pi's RPC contract: LF is the *only* record
/// delimiter, and a trailing CR is stripped.
///
/// This is deliberately not a generic line reader. `U+2028`/`U+2029` are valid
/// inside JSON strings, and splitting on them — which several stdlib line
/// readers do — corrupts records. The protocol docs call this out explicitly.
#[derive(Default)]
pub struct JsonlDecoder {
    buffer: String,
}

impl JsonlDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk and return every complete record it completed.
    ///
    /// Unparseable lines are skipped rather than failing the stream: one bad
    /// frame must not tear down a live agent session. A partial trailing line
    /// stays buffered for the next chunk.
    pub fn push(&mut self, chunk: &str) -> Vec<Value> {
        self.buffer.push_str(chunk);
        let mut out = Vec::new();
        while let Some(idx) = self.buffer.find('\n') {
            let mut line = self.buffer[..idx].to_string();
            self.buffer.drain(..=idx);
            if line.ends_with('\r') {
                line.pop();
            }
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(&line) {
                Ok(v) => out.push(v),
                Err(e) => eprintln!("[agent-rpc] dropping unparseable frame: {e}"),
            }
        }
        out
    }
}

/// Encode a command as one JSONL record, newline included.
pub fn encode(command: &Value) -> String {
    let mut s = serde_json::to_string(command).unwrap_or_else(|_| "{}".to_string());
    s.push('\n');
    s
}

// ---------------------------------------------------------------------------
// commands
// ---------------------------------------------------------------------------

/// Send a prompt. `streaming_behavior` must be set when the agent is already
/// streaming, otherwise pi rejects the command outright.
pub fn cmd_prompt(id: &str, message: &str, streaming_behavior: Option<&str>) -> Value {
    let mut v = json!({ "id": id, "type": "prompt", "message": message });
    if let Some(b) = streaming_behavior {
        v["streamingBehavior"] = json!(b);
    }
    v
}

pub fn cmd_abort(id: &str) -> Value {
    json!({ "id": id, "type": "abort" })
}

/// Incremental history fetch. `since` is an entry id already mirrored; pi
/// returns only entries strictly after it. Entry ids are stable, so this works
/// as a durable cursor even across a client or server restart.
pub fn cmd_get_entries(id: &str, since: Option<&str>) -> Value {
    let mut v = json!({ "id": id, "type": "get_entries" });
    if let Some(s) = since {
        v["since"] = json!(s);
    }
    v
}

/// Answer a blocking extension UI dialog. `id` must match the request's id or
/// the runtime stays blocked until its timeout.
pub fn extension_ui_value(id: &str, value: &str) -> Value {
    json!({ "type": "extension_ui_response", "id": id, "value": value })
}

pub fn extension_ui_confirm(id: &str, confirmed: bool) -> Value {
    json!({ "type": "extension_ui_response", "id": id, "confirmed": confirmed })
}

pub fn extension_ui_cancel(id: &str) -> Value {
    json!({ "type": "extension_ui_response", "id": id, "cancelled": true })
}

// ---------------------------------------------------------------------------
// inbound frames
// ---------------------------------------------------------------------------

/// A frame from the runtime, classified by what the server must *do* with it.
///
/// Everything a client merely renders stays in the `Event` catch-all: the
/// server forwards it verbatim and never has to be taught new event types.
#[derive(Debug)]
pub enum Inbound {
    /// Reply to a command we sent, correlated by `id`.
    Response {
        id: Option<String>,
        command: String,
        success: bool,
        error: Option<String>,
        data: Option<Value>,
    },
    /// A blocking dialog (`select`/`confirm`/`input`/`editor`) that must be
    /// answered, or fire-and-forget UI the server just forwards.
    UiRequest {
        id: String,
        method: String,
        blocking: bool,
        raw: Value,
    },
    /// Anything else: streaming deltas, tool execution, lifecycle.
    Event { kind: String, raw: Value },
}

/// Dialog methods block the runtime until answered. The fire-and-forget
/// methods (`notify`, `setStatus`, `setWidget`, ...) must NOT be answered —
/// tracking them as pending would leak an entry per event.
fn is_blocking_ui(method: &str) -> bool {
    matches!(method, "select" | "confirm" | "input" | "editor")
}

pub fn classify(frame: Value) -> Inbound {
    let ty = frame
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    match ty.as_str() {
        "response" => Inbound::Response {
            id: frame
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string),
            command: frame
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            success: frame
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            error: frame
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string),
            data: frame.get("data").cloned(),
        },
        "extension_ui_request" => {
            let method = frame
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Inbound::UiRequest {
                id: frame
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                blocking: is_blocking_ui(&method),
                method,
                raw: frame,
            }
        }
        _ => Inbound::Event {
            kind: ty,
            raw: frame,
        },
    }
}

/// Whether an event means the agent has fully settled.
///
/// `agent_settled` is the correct signal, not `agent_end`: the latter fires per
/// low-level run and is still followed by automatic retries, compaction
/// retries, and queued follow-ups. Marking a session idle on `agent_end` would
/// report completion while the agent is still working.
pub fn is_settled(kind: &str) -> bool {
    kind == "agent_settled"
}

/// Entries extracted from a `get_entries` response, ready to mirror.
#[derive(Debug, Serialize, Deserialize)]
pub struct MirroredEntry {
    pub entry_id: String,
    pub parent_id: Option<String>,
    pub kind: String,
    pub payload: Value,
}

/// Parse a `get_entries` response payload into mirrorable rows.
///
/// Entries without an `id` are skipped: the id is the mirror's dedupe key and
/// its resume cursor, so a row without one could be inserted repeatedly.
pub fn parse_entries(data: &Value) -> Vec<MirroredEntry> {
    data.get("entries")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    let entry_id = e.get("id").and_then(Value::as_str)?.to_string();
                    Some(MirroredEntry {
                        entry_id,
                        parent_id: e
                            .get("parentId")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        kind: e
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("message")
                            .to_string(),
                        payload: e.clone(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_records_only_on_lf_and_tolerates_crlf() {
        let mut d = JsonlDecoder::new();
        let out = d.push("{\"type\":\"a\"}\r\n{\"type\":\"b\"}\n");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["type"], "a");
        assert_eq!(out[1]["type"], "b");
    }

    #[test]
    fn unicode_separators_inside_strings_do_not_split_records() {
        // U+2028/U+2029 are legal inside a JSON string. A generic line reader
        // would split here and corrupt both halves.
        let mut d = JsonlDecoder::new();
        let out = d.push("{\"type\":\"event\",\"text\":\"a\u{2028}b\u{2029}c\"}\n");
        assert_eq!(out.len(), 1, "record must stay intact");
        assert_eq!(out[0]["text"], "a\u{2028}b\u{2029}c");
    }

    #[test]
    fn partial_records_buffer_until_their_newline_arrives() {
        let mut d = JsonlDecoder::new();
        assert!(d.push("{\"type\":\"ev").is_empty());
        assert!(d.push("ent\",\"n\":1}").is_empty());
        let out = d.push("\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["n"], 1);
    }

    #[test]
    fn a_malformed_frame_is_dropped_without_losing_the_rest() {
        // A live agent session must survive one corrupt frame.
        let mut d = JsonlDecoder::new();
        let out = d.push("not json\n{\"type\":\"ok\"}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["type"], "ok");
    }

    #[test]
    fn encoded_commands_are_exactly_one_terminated_line() {
        let s = encode(&cmd_prompt("r1", "hi", None));
        assert!(s.ends_with('\n'));
        assert_eq!(s.matches('\n').count(), 1);
        let v: Value = serde_json::from_str(s.trim()).unwrap();
        assert_eq!(v["type"], "prompt");
        assert_eq!(v["id"], "r1");
        assert!(v.get("streamingBehavior").is_none());
    }

    #[test]
    fn steering_behavior_is_sent_when_requested() {
        let v = cmd_prompt("r1", "stop", Some("steer"));
        assert_eq!(v["streamingBehavior"], "steer");
    }

    #[test]
    fn get_entries_omits_the_cursor_on_a_first_sync() {
        assert!(cmd_get_entries("r1", None).get("since").is_none());
        assert_eq!(cmd_get_entries("r1", Some("abc"))["since"], "abc");
    }

    #[test]
    fn responses_are_classified_with_their_correlation_id() {
        let f = json!({"id":"r1","type":"response","command":"prompt","success":true});
        match classify(f) {
            Inbound::Response { id, command, success, .. } => {
                assert_eq!(id.as_deref(), Some("r1"));
                assert_eq!(command, "prompt");
                assert!(success);
            }
            other => panic!("expected response, got {other:?}"),
        }
    }

    #[test]
    fn only_dialog_ui_requests_are_marked_blocking() {
        for m in ["select", "confirm", "input", "editor"] {
            let f = json!({"type":"extension_ui_request","id":"u1","method":m});
            match classify(f) {
                Inbound::UiRequest { blocking, .. } => assert!(blocking, "{m} blocks"),
                other => panic!("expected ui request, got {other:?}"),
            }
        }
        // Answering these would be a protocol error, and tracking them as
        // pending would leak one entry per notification.
        for m in ["notify", "setStatus", "setWidget", "setTitle", "set_editor_text"] {
            let f = json!({"type":"extension_ui_request","id":"u1","method":m});
            match classify(f) {
                Inbound::UiRequest { blocking, .. } => assert!(!blocking, "{m} is fire-and-forget"),
                other => panic!("expected ui request, got {other:?}"),
            }
        }
    }

    #[test]
    fn unknown_event_types_pass_through_verbatim() {
        // Forward-compatibility: a pi upgrade that adds an event must not need
        // a server change to reach the client.
        let f = json!({"type":"some_future_event","payload":{"a":1}});
        match classify(f) {
            Inbound::Event { kind, raw } => {
                assert_eq!(kind, "some_future_event");
                assert_eq!(raw["payload"]["a"], 1);
            }
            other => panic!("expected event, got {other:?}"),
        }
    }

    #[test]
    fn only_agent_settled_marks_a_session_finished() {
        assert!(is_settled("agent_settled"));
        // agent_end is still followed by retries and queued follow-ups.
        assert!(!is_settled("agent_end"));
        assert!(!is_settled("turn_end"));
    }

    #[test]
    fn entries_are_parsed_and_rows_without_an_id_are_skipped() {
        let data = json!({
            "entries": [
                {"type":"message","id":"a1","parentId":null,"message":{"role":"user"}},
                {"type":"message","id":"b2","parentId":"a1","message":{"role":"assistant"}},
                {"type":"message","parentId":"b2"},
            ],
            "leafId": "b2"
        });
        let rows = parse_entries(&data);
        assert_eq!(rows.len(), 2, "the id-less entry has no dedupe key");
        assert_eq!(rows[0].entry_id, "a1");
        assert_eq!(rows[0].parent_id, None);
        assert_eq!(rows[1].parent_id.as_deref(), Some("a1"));
        assert_eq!(rows[1].kind, "message");
    }

    #[test]
    fn extension_ui_responses_carry_the_request_id() {
        assert_eq!(extension_ui_value("u1", "Allow")["value"], "Allow");
        assert_eq!(extension_ui_confirm("u1", true)["confirmed"], true);
        assert_eq!(extension_ui_cancel("u1")["cancelled"], true);
        for v in [
            extension_ui_value("u1", "x"),
            extension_ui_confirm("u1", false),
            extension_ui_cancel("u1"),
        ] {
            assert_eq!(v["id"], "u1");
            assert_eq!(v["type"], "extension_ui_response");
        }
    }
}
