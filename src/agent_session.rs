//! Target-agnostic agent session runtime.
//!
//! A "target" is only a transport. Both the cloud sandbox and a user's own
//! machine run `pi --mode rpc` and speak identical JSONL, so the parts that
//! actually carry product behaviour — event fan-out to every connected
//! client, session mirroring, approval routing, idle tracking — live here once
//! and work for any driver.
//!
//! The fan-out is what makes the three-client story work: a prompt sent from a
//! phone and one sent from the desktop enter the same session, and every
//! connected client sees the same stream. Approvals are broadcast the same
//! way, so whichever device is at hand can answer.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::sync::{RwLock, broadcast, mpsc, oneshot};

use crate::agent_rpc::{self, Inbound};
use crate::db::{self, DbKind, Pool};

/// Where a session's tools execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A sandbox container allocated by this server.
    Cloud,
    /// The user's own machine, reached through the desktop client's socket.
    Device,
}

impl Target {
    pub fn as_str(self) -> &'static str {
        match self {
            Target::Cloud => "cloud",
            Target::Device => "device",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "cloud" => Some(Target::Cloud),
            "device" => Some(Target::Device),
            _ => None,
        }
    }
}

/// A transport to one live `pi --mode rpc` instance.
///
/// Implementors only move bytes. `Send + Sync` because a session is driven
/// from a spawned task while HTTP handlers submit commands concurrently.
#[async_trait::async_trait]
pub trait AgentTransport: Send + Sync {
    /// Write one already-framed JSONL record to the runtime's stdin.
    async fn send(&self, line: String) -> Result<(), String>;
    /// Stop the runtime and release its resources.
    async fn shutdown(&self);
}

/// Broadcast capacity per session. Sized so a slow client (a phone on a bad
/// link) can fall behind a burst of streaming deltas without the producer
/// blocking; a client that overruns it gets `Lagged` and resyncs from the
/// mirror rather than stalling the agent.
const EVENT_BUFFER: usize = 512;

/// What a subscribed client receives.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// A runtime frame to render, forwarded verbatim.
    Frame(Value),
    /// A blocking dialog was answered, by this client or another one. Lets
    /// every subscriber retire the request instead of showing a stale card.
    ApprovalResolved { request_id: String },
    /// The agent fully settled (no retry or queued work remains).
    Settled,
    /// The transport died; the session is no longer live.
    Closed { reason: String },
}

/// A pending blocking dialog awaiting a human answer.
struct PendingApproval {
    request: Value,
}

/// One live session: its transport, subscribers, and pending approvals.
pub struct LiveSession {
    pub session_id: i64,
    pub user_id: i64,
    pub target: Target,
    /// Plaintext gateway credential minted for this runtime, retired when the
    /// session ends.
    ///
    /// Held here rather than only at the launch site because a session can end
    /// in several ways — the user stops it, the runtime exits, the device
    /// disconnects, the sandbox hits its lifetime ceiling — and every one of
    /// them must kill the credential. Anchoring it to the session object means
    /// teardown cannot forget a path.
    session_token: Option<String>,
    transport: Arc<dyn AgentTransport>,
    events: broadcast::Sender<SessionEvent>,
    /// Blocking `extension_ui_request`s keyed by request id. A request left
    /// here unanswered keeps the runtime blocked until its own timeout.
    approvals: RwLock<HashMap<String, PendingApproval>>,
    /// Outstanding command responses keyed by our correlation id.
    inflight: RwLock<HashMap<String, oneshot::Sender<Inbound>>>,
    /// Serializes mirror passes. Mirroring runs off the frame pump (see
    /// `attach`), so two settles in quick succession could otherwise issue
    /// overlapping `get_entries` calls and advance the cursor out of order.
    mirror_lock: tokio::sync::Mutex<()>,
    /// Monotonic source for correlation ids, unique within this session.
    next_id: std::sync::atomic::AtomicU64,
}

impl LiveSession {
    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.events.subscribe()
    }

    fn new_request_id(&self) -> String {
        let n = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("s{}-{n}", self.session_id)
    }

    /// Send a command and wait for its correlated response.
    ///
    /// pi answers a command as soon as it is *accepted*, not when the work
    /// finishes, so this must not be used to wait for a turn to complete —
    /// that is what `SessionEvent::Settled` is for.
    pub async fn request(&self, mut command: Value) -> Result<Inbound, String> {
        let id = self.new_request_id();
        command["id"] = json!(id);
        let (tx, rx) = oneshot::channel();
        self.inflight.write().await.insert(id.clone(), tx);

        if let Err(e) = self.transport.send(agent_rpc::encode(&command)).await {
            self.inflight.write().await.remove(&id);
            return Err(e);
        }
        rx.await.map_err(|_| "运行时已断开".to_string())
    }

    /// Fire a command without waiting (used for UI dialog answers, which get
    /// no response frame of their own).
    pub async fn notify(&self, command: Value) -> Result<(), String> {
        self.transport.send(agent_rpc::encode(&command)).await
    }

    /// Answer a pending blocking dialog. Removing it first makes the operation
    /// idempotent: with several clients subscribed, two devices can race to
    /// approve the same request and only the first must reach the runtime.
    pub async fn resolve_approval(&self, request_id: &str, answer: Value) -> Result<(), String> {
        let existed = self.approvals.write().await.remove(request_id).is_some();
        if !existed {
            return Err("该审批请求已处理或已过期".into());
        }
        self.notify(answer).await?;
        // Tell every other client the request is settled. The runtime emits no
        // event of its own when a dialog is answered, so without this a second
        // device keeps showing a card that can no longer be acted on until it
        // reloads — which defeats answering from whichever device is at hand.
        let _ = self.events.send(SessionEvent::ApprovalResolved {
            request_id: request_id.to_string(),
        });
        Ok(())
    }

    pub async fn pending_approvals(&self) -> Vec<Value> {
        self.approvals
            .read()
            .await
            .values()
            .map(|p| p.request.clone())
            .collect()
    }

    pub async fn shutdown(&self) {
        self.transport.shutdown().await;
    }

    /// Retire this session's gateway credential.
    ///
    /// Idempotent by construction: revoking an already-revoked hash is a
    /// no-op update, so every teardown path may call it without coordinating.
    pub async fn revoke_token(&self, pool: &Pool, kind: DbKind) {
        if let Some(token) = &self.session_token {
            crate::agent_token::revoke_by_plaintext(pool, kind, token).await;
        }
    }
}

/// All live sessions in this process, keyed by session id.
///
/// In-memory by design: a live session is bound to a running child process or
/// an open socket, neither of which survives a restart. Persistent state lives
/// in `agent_sessions`/`agent_entries`, which is why boot resets `running`
/// rows back to `idle`.
#[derive(Clone, Default)]
pub struct SessionRegistry {
    inner: Arc<RwLock<HashMap<i64, Arc<LiveSession>>>>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get(&self, session_id: i64) -> Option<Arc<LiveSession>> {
        self.inner.read().await.get(&session_id).cloned()
    }

    pub async fn is_live(&self, session_id: i64) -> bool {
        self.inner.read().await.contains_key(&session_id)
    }

    pub async fn remove(&self, session_id: i64) -> Option<Arc<LiveSession>> {
        self.inner.write().await.remove(&session_id)
    }

    pub async fn live_ids(&self) -> Vec<i64> {
        self.inner.read().await.keys().copied().collect()
    }

    /// Register a transport and start pumping its frames.
    ///
    /// `frames` yields decoded JSONL records from the runtime. The pump owns
    /// correlation, approval bookkeeping, mirroring and fan-out, so a driver
    /// never has to reimplement protocol behaviour.
    #[allow(clippy::too_many_arguments)]
    pub async fn attach(
        &self,
        pool: Pool,
        kind: DbKind,
        session_id: i64,
        user_id: i64,
        target: Target,
        transport: Arc<dyn AgentTransport>,
        mut frames: mpsc::Receiver<Value>,
        session_token: Option<String>,
    ) -> Arc<LiveSession> {
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let live = Arc::new(LiveSession {
            session_id,
            user_id,
            target,
            session_token,
            transport,
            events,
            approvals: RwLock::new(HashMap::new()),
            inflight: RwLock::new(HashMap::new()),
            mirror_lock: tokio::sync::Mutex::new(()),
            next_id: std::sync::atomic::AtomicU64::new(1),
        });
        self.inner.write().await.insert(session_id, live.clone());

        let registry = self.clone();
        let session = live.clone();
        tokio::spawn(async move {
            while let Some(frame) = frames.recv().await {
                match agent_rpc::classify(frame) {
                    Inbound::Response {
                        id,
                        command,
                        success,
                        error,
                        data,
                    } => {
                        // Hand the reply to whoever is waiting on this id. An
                        // unmatched response is normal: fire-and-forget
                        // commands and pi-initiated replies have no waiter.
                        let waiter = match &id {
                            Some(id) => session.inflight.write().await.remove(id),
                            None => None,
                        };
                        if let Some(w) = waiter {
                            let _ = w.send(Inbound::Response {
                                id,
                                command,
                                success,
                                error,
                                data,
                            });
                        }
                    }
                    Inbound::UiRequest {
                        id,
                        method,
                        blocking,
                        raw,
                    } => {
                        // Only dialogs are tracked as pending. Recording the
                        // fire-and-forget methods would leak one entry per
                        // notification and never be answered.
                        if blocking {
                            session
                                .approvals
                                .write()
                                .await
                                .insert(id.clone(), PendingApproval { request: raw.clone() });
                        }
                        let _ = (id, method);
                        let _ = session.events.send(SessionEvent::Frame(raw));
                    }
                    Inbound::Event { kind: ev, raw } => {
                        let settled = agent_rpc::is_settled(&ev);
                        let _ = session.events.send(SessionEvent::Frame(raw));
                        if settled {
                            // Mirror off this loop. `mirror_entries` issues a
                            // `get_entries` command and awaits its response,
                            // but only this loop delivers responses, so
                            // awaiting it inline would deadlock the session.
                            let pool = pool.clone();
                            let session = session.clone();
                            tokio::spawn(async move {
                                let session_id = session.session_id;
                                if let Err(e) = mirror_entries(&pool, kind, &session).await {
                                    eprintln!(
                                        "[agent-session {session_id}] mirror failed: {e}"
                                    );
                                }
                                set_status(&pool, kind, session_id, "idle").await;
                                let _ = session.events.send(SessionEvent::Settled);
                            });
                        }
                    }
                }
            }

            // The runtime's stdout closed, so its stdin is gone too and no
            // further `get_entries` could ever be answered. Mirroring already
            // ran at each settle, which is the last point the transcript could
            // be read, so there is nothing left to persist here.
            //
            // This is the one exit every target shares — a sandbox exiting, a
            // device reporting its runtime closed, a dropped socket — so it is
            // where the credential dies. Waiting for an explicit stop would
            // leave a usable token behind whenever the runtime ended on its
            // own, and on the device target that token is in the user's hands.
            set_status(&pool, kind, session_id, "idle").await;
            session.revoke_token(&pool, kind).await;
            let _ = session.events.send(SessionEvent::Closed {
                reason: "运行时已退出".into(),
            });
            registry.remove(session_id).await;
        });

        live
    }
}

// ---------------------------------------------------------------------------
// persistence
// ---------------------------------------------------------------------------

async fn set_status(pool: &Pool, kind: DbKind, session_id: i64, status: &str) {
    let sql = db::q(
        kind,
        "UPDATE agent_sessions SET status = ?, updated_at = ? WHERE id = ?",
    );
    let _ = sqlx::query(&sql)
        .bind(status)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(session_id)
        .execute(pool)
        .await;
}

/// Pull everything after the stored cursor and persist it.
///
/// The runtime owns the authoritative session tree; this mirror is what every
/// client reads. Using the stored entry id as `since` means a reconnect
/// fetches only new entries, and the unique `(session_id, entry_id)` index
/// makes an overlapping replay harmless.
pub async fn mirror_entries(
    pool: &Pool,
    kind: DbKind,
    session: &Arc<LiveSession>,
) -> Result<usize, String> {
    // Serialize passes so overlapping settles cannot interleave their
    // `get_entries` ranges and move the cursor backwards.
    let _guard = session.mirror_lock.lock().await;

    let cursor: Option<String> = sqlx::query_scalar(&db::q(
        kind,
        "SELECT cursor FROM agent_sessions WHERE id = ?",
    ))
    .bind(session.session_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?
    .flatten();

    let reply = session
        .request(agent_rpc::cmd_get_entries("", cursor.as_deref()))
        .await?;

    let data = match reply {
        Inbound::Response {
            success: true,
            data: Some(data),
            ..
        } => data,
        // A stale cursor (its entry no longer exists) makes pi reject the
        // request. Fall back to a full resync rather than leaving the mirror
        // permanently stuck.
        Inbound::Response { success: false, .. } if cursor.is_some() => {
            match session.request(agent_rpc::cmd_get_entries("", None)).await? {
                Inbound::Response {
                    success: true,
                    data: Some(data),
                    ..
                } => data,
                _ => return Err("无法读取会话条目".into()),
            }
        }
        _ => return Err("无法读取会话条目".into()),
    };

    let entries = agent_rpc::parse_entries(&data);
    if entries.is_empty() {
        return Ok(0);
    }

    let insert = db::q(
        kind,
        "INSERT INTO agent_entries (session_id, entry_id, parent_id, kind, payload) \
         VALUES (?, ?, ?, ?, ?)",
    );
    let mut written = 0usize;
    for e in &entries {
        let payload = serde_json::to_string(&e.payload).unwrap_or_else(|_| "{}".into());
        // Conflicts are expected on replay and are not an error; skipping them
        // is exactly the idempotency the unique index is there to provide.
        let res = sqlx::query(&insert)
            .bind(session.session_id)
            .bind(&e.entry_id)
            .bind(e.parent_id.as_deref())
            .bind(&e.kind)
            .bind(&payload)
            .execute(pool)
            .await;
        if res.is_ok() {
            written += 1;
        }
    }

    // Advance the cursor to the last entry we saw so the next sync resumes
    // from here. `leafId` is not used: it can point into an abandoned branch,
    // while append order is what `since` is defined against.
    if let Some(last) = entries.last() {
        let _ = sqlx::query(&db::q(
            kind,
            "UPDATE agent_sessions SET cursor = ?, updated_at = ? WHERE id = ?",
        ))
        .bind(&last.entry_id)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(session.session_id)
        .execute(pool)
        .await;
    }

    Ok(written)
}

/// Clear `running` rows left behind by a restart, and retire the credentials
/// their runtimes were holding.
///
/// A live session is bound to a child process or an open socket, so nothing
/// survives a restart. Without this, a session interrupted mid-stream would
/// stay `running` forever and refuse new prompts.
///
/// The token half matters just as much: the in-memory `LiveSession` that owns
/// a credential is gone after a restart, so nothing else can ever revoke it.
/// Session tokens are named `session-<id>` by the launcher, which is what makes
/// them identifiable here without storing the plaintext.
pub async fn reset_running_sessions(pool: &Pool, kind: DbKind) {
    let sql = db::q(
        kind,
        "UPDATE agent_sessions SET status = 'idle' WHERE status = 'running'",
    );
    if let Err(e) = sqlx::query(&sql).execute(pool).await {
        eprintln!("[agent-session] resetting interrupted sessions failed: {e}");
    }

    let revoke = db::q(
        kind,
        &format!(
            "UPDATE agent_tokens SET revoked = {} WHERE name LIKE 'session-%' AND revoked = {}",
            "1",
            "0",
        ),
    );
    match sqlx::query(&revoke).execute(pool).await {
        Ok(r) if r.rows_affected() > 0 => eprintln!(
            "[agent-session] revoked {} orphaned session credential(s) from a previous run",
            r.rows_affected()
        ),
        Ok(_) => {}
        Err(e) => eprintln!("[agent-session] revoking orphaned session credentials failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_round_trip_through_their_stored_form() {
        for t in [Target::Cloud, Target::Device] {
            assert_eq!(Target::parse(t.as_str()), Some(t));
        }
        assert_eq!(Target::parse("local"), None);
        assert_eq!(Target::parse(""), None);
    }

    #[tokio::test]
    async fn a_restart_retires_the_credentials_it_can_no_longer_revoke() {
        // After a restart the LiveSession that owned a session credential is
        // gone, so nothing else would ever revoke it. Hand-managed tokens must
        // survive the same sweep.
        use crate::db::install_drivers;
        use sqlx::any::AnyPoolOptions;

        install_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::migrate(&pool, DbKind::Sqlite).await.unwrap();
        sqlx::query("INSERT INTO users (id, username, password_hash) VALUES (1, 'u1', 'x')")
            .execute(&pool)
            .await
            .unwrap();

        let orphan = crate::agent_token::mint(&pool, DbKind::Sqlite, 1, "session-7", Some(12))
            .await
            .unwrap();
        let manual = crate::agent_token::mint(&pool, DbKind::Sqlite, 1, "laptop", None)
            .await
            .unwrap();

        reset_running_sessions(&pool, DbKind::Sqlite).await;

        assert_eq!(
            crate::agent_token::user_for_token(&pool, DbKind::Sqlite, &orphan).await,
            None,
            "a session credential cannot outlive the process that owned it"
        );
        assert_eq!(
            crate::agent_token::user_for_token(&pool, DbKind::Sqlite, &manual).await,
            Some(1),
            "a token the user manages by hand must not be swept"
        );

        pool.close().await;
    }

    struct RecordingTransport {
        sent: Arc<RwLock<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl AgentTransport for RecordingTransport {
        async fn send(&self, line: String) -> Result<(), String> {
            self.sent.write().await.push(line);
            Ok(())
        }
        async fn shutdown(&self) {}
    }

    fn live_for_test(
        transport: Arc<dyn AgentTransport>,
    ) -> (Arc<LiveSession>, broadcast::Sender<SessionEvent>) {
        let (events, _) = broadcast::channel(16);
        let live = Arc::new(LiveSession {
            session_id: 7,
            user_id: 1,
            target: Target::Cloud,
            session_token: None,
            transport,
            events: events.clone(),
            approvals: RwLock::new(HashMap::new()),
            inflight: RwLock::new(HashMap::new()),
            mirror_lock: tokio::sync::Mutex::new(()),
            next_id: std::sync::atomic::AtomicU64::new(1),
        });
        (live, events)
    }

    #[tokio::test]
    async fn ending_a_session_retires_its_gateway_credential() {
        // The device target hands this plaintext to a machine the user owns,
        // so "the session ended" has to mean "the credential is dead".
        use crate::db::install_drivers;
        use sqlx::any::AnyPoolOptions;

        install_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::migrate(&pool, DbKind::Sqlite).await.unwrap();
        sqlx::query("INSERT INTO users (id, username, password_hash) VALUES (1, 'u1', 'x')")
            .execute(&pool)
            .await
            .unwrap();
        let token = crate::agent_token::mint(&pool, DbKind::Sqlite, 1, "session-7", Some(12))
            .await
            .unwrap();

        let (events, _) = broadcast::channel(16);
        let live = Arc::new(LiveSession {
            session_id: 7,
            user_id: 1,
            target: Target::Device,
            session_token: Some(token.clone()),
            transport: Arc::new(RecordingTransport {
                sent: Arc::new(RwLock::new(Vec::new())),
            }),
            events,
            approvals: RwLock::new(HashMap::new()),
            inflight: RwLock::new(HashMap::new()),
            mirror_lock: tokio::sync::Mutex::new(()),
            next_id: std::sync::atomic::AtomicU64::new(1),
        });

        assert_eq!(
            crate::agent_token::user_for_token(&pool, DbKind::Sqlite, &token).await,
            Some(1)
        );
        live.revoke_token(&pool, DbKind::Sqlite).await;
        assert_eq!(
            crate::agent_token::user_for_token(&pool, DbKind::Sqlite, &token).await,
            None,
            "the credential must not survive the session"
        );

        pool.close().await;
    }

    #[tokio::test]
    async fn every_subscriber_receives_the_same_frame() {
        // The three-client story depends on this: a phone and a desktop
        // watching one session must both see the full stream.
        let sent = Arc::new(RwLock::new(Vec::new()));
        let (live, events) = live_for_test(Arc::new(RecordingTransport { sent }));
        let mut a = live.subscribe();
        let mut b = live.subscribe();

        events.send(SessionEvent::Frame(json!({"type":"x"}))).unwrap();

        for rx in [&mut a, &mut b] {
            match rx.recv().await.unwrap() {
                SessionEvent::Frame(v) => assert_eq!(v["type"], "x"),
                other => panic!("expected frame, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn an_approval_can_only_be_answered_once() {
        // With several clients subscribed, two devices can race to approve the
        // same request; only the first may reach the runtime.
        let sent = Arc::new(RwLock::new(Vec::new()));
        let (live, _events) = live_for_test(Arc::new(RecordingTransport { sent: sent.clone() }));
        live.approvals.write().await.insert(
            "u1".into(),
            PendingApproval {
                request: json!({"id":"u1","method":"confirm"}),
            },
        );

        assert_eq!(live.pending_approvals().await.len(), 1);
        live.resolve_approval("u1", agent_rpc::extension_ui_confirm("u1", true))
            .await
            .expect("first answer wins");
        assert!(
            live.resolve_approval("u1", agent_rpc::extension_ui_confirm("u1", false))
                .await
                .is_err(),
            "a second answer must be refused"
        );

        // Exactly one answer reached the runtime.
        let lines = sent.read().await;
        assert_eq!(lines.len(), 1);
        let v: Value = serde_json::from_str(lines[0].trim()).unwrap();
        assert_eq!(v["confirmed"], true);
    }

    #[tokio::test]
    async fn resolving_an_approval_tells_every_other_client() {
        // The runtime emits nothing when a dialog is answered, so a second
        // device would keep showing an unactionable card without this event.
        let sent = Arc::new(RwLock::new(Vec::new()));
        let (live, _events) = live_for_test(Arc::new(RecordingTransport { sent }));
        live.approvals.write().await.insert(
            "u1".into(),
            PendingApproval {
                request: json!({"id":"u1","method":"confirm"}),
            },
        );
        let mut other = live.subscribe();

        live.resolve_approval("u1", agent_rpc::extension_ui_confirm("u1", true))
            .await
            .unwrap();

        match other.recv().await.unwrap() {
            SessionEvent::ApprovalResolved { request_id } => assert_eq!(request_id, "u1"),
            other => panic!("expected ApprovalResolved, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn answering_an_unknown_approval_is_refused() {
        let sent = Arc::new(RwLock::new(Vec::new()));
        let (live, _events) = live_for_test(Arc::new(RecordingTransport { sent: sent.clone() }));
        assert!(
            live.resolve_approval("nope", agent_rpc::extension_ui_cancel("nope"))
                .await
                .is_err()
        );
        assert!(sent.read().await.is_empty(), "nothing may be sent");
    }

    #[tokio::test]
    async fn requests_get_a_session_unique_correlation_id() {
        let sent = Arc::new(RwLock::new(Vec::new()));
        let (live, _events) = live_for_test(Arc::new(RecordingTransport { sent: sent.clone() }));

        // The waiter is never answered here; we only assert the wire format.
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            live.request(agent_rpc::cmd_prompt("", "hi", None)),
        )
        .await;
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            live.request(agent_rpc::cmd_prompt("", "again", None)),
        )
        .await;

        let lines = sent.read().await;
        assert_eq!(lines.len(), 2);
        let a: Value = serde_json::from_str(lines[0].trim()).unwrap();
        let b: Value = serde_json::from_str(lines[1].trim()).unwrap();
        assert_ne!(a["id"], b["id"], "ids must not collide within a session");
        assert_eq!(a["id"], "s7-1");
        assert_eq!(b["id"], "s7-2");
    }

    #[tokio::test]
    async fn a_dead_transport_fails_the_request_instead_of_hanging() {
        struct DeadTransport;
        #[async_trait::async_trait]
        impl AgentTransport for DeadTransport {
            async fn send(&self, _line: String) -> Result<(), String> {
                Err("broken pipe".into())
            }
            async fn shutdown(&self) {}
        }

        let (live, _events) = live_for_test(Arc::new(DeadTransport));
        let err = live
            .request(agent_rpc::cmd_prompt("", "hi", None))
            .await
            .expect_err("a failed write must surface");
        assert!(err.contains("broken pipe"));
        // The waiter must not be left behind, or the id would leak.
        assert!(live.inflight.read().await.is_empty());
    }
}
