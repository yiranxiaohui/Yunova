//! The connector: this machine as an execution target, independent of any UI.
//!
//! It dials the server and holds a WebSocket open — a personal computer
//! usually has no reachable address, so an outbound connection is the only
//! thing that works without port forwarding. Its job is deliberately small:
//! run `pi --mode rpc` locally and pipe its stdio over the relay. The thinking
//! loop is the runtime's, the session and billing are the server's, and the
//! local execution policy (workspace scope, approval gate) is this client's.
//!
//! Everything a *person* would see is pushed through [`Host`] rather than
//! printed. That is what lets the window shell and the headless CLI share one
//! connection loop instead of two that drift apart: the window renders status,
//! the terminal prints it, and neither owns the protocol.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::endpoint::{blocking_prompt, device_endpoint, is_loopback, platform};
use crate::identity::{
    Credential, Login, credential_path, fingerprint, forget_credential, load_credential,
    save_credential,
};
use crate::proto::{FromServer, ToServer};
use crate::runtime::{Config, RuntimeManager};

/// Everything the connector needs to know about the user's choices.
///
/// The local execution policy lives here rather than on the server: a sandbox
/// is disposable, a personal machine is not, so what the agent may touch has
/// to be the user's decision and not something a server can widen.
#[derive(Debug, Clone)]
pub struct ConnectorConfig {
    /// Site address as the user typed it, e.g. `https://yunnet.top`.
    pub site_url: String,
    /// Name shown in the device list.
    pub name: String,
    /// Directory a task gets when it names none.
    pub workspace: PathBuf,
    /// Every directory a task may name, `workspace` included.
    ///
    /// Advertised to the server so the web UI can offer a picker, and
    /// re-checked locally on every start. A server can therefore suggest a
    /// directory but never add one.
    pub workspace_roots: Vec<PathBuf>,
    /// Where per-session runtime config is written.
    pub state_dir: PathBuf,
    /// The `pi` executable.
    pub program: String,
    /// Whether tool calls run without asking.
    pub auto_approve: bool,
    /// Override for where the device token is stored; used by packaging and
    /// by tests.
    pub config_dir: Option<PathBuf>,
}

/// What the user is told, in terms a UI can render directly.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    /// Not connected and not trying to be.
    Offline,
    Connecting,
    Online {
        device_id: i64,
        username: String,
    },
    /// No usable credential: the user must sign in before anything else can
    /// happen. Kept distinct from `Error` because the remedy differs — waiting
    /// does not fix it, and a retry loop must not hide it.
    NeedsLogin {
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Transient trouble; the loop is still retrying.
    Error {
        message: String,
    },
}

/// The surface a person watches: status, a log, and the moments that need them.
pub trait Host: Send + Sync + 'static {
    fn status(&self, status: Status);
    fn log(&self, line: String);
    /// An agent on this machine is blocked waiting for a human. Worth
    /// interrupting for: a task nobody knows is waiting never finishes.
    fn attention(&self, title: String, body: String);
    /// Obtain the site's own session token, when the host has a signed-in
    /// view of the site to read one from.
    ///
    /// This is what lets the desktop app bind this machine without a second
    /// sign-in: the user already authenticated in the window, so asking for
    /// the password again is a step that exists only because the connector
    /// could not see it. A headless host has no window and returns `None`.
    fn session_token(&self) -> Option<String> {
        None
    }

    /// Obtain account credentials. A terminal can ask; a window cannot ask
    /// from inside the connection loop, so it declines and lets its own UI
    /// drive the sign-in instead.
    ///
    /// The error is what the UI shows as the reason, so a host with nothing to
    /// add should return an empty string rather than restating the state.
    fn login(&self) -> Result<Login, String> {
        Err(String::new())
    }
}

/// How this connection intends to authenticate.
enum Attach {
    Token(String),
    /// The site's browser session, read from the app's own signed-in window.
    Session(String),
    Login(Login),
}

/// Why a connection ended.
enum Outcome {
    /// The socket closed normally; reconnect with what we already have.
    Closed,
    /// A sign-in succeeded and returned a credential worth persisting.
    Attached(Credential),
}

enum Failure {
    /// The server declined the credential. Retrying it unchanged is pointless.
    Rejected(String),
    /// The client itself refuses to proceed — a wrong URL, not a wrong
    /// credential. No amount of retrying fixes it, so it must not be retried.
    Misconfigured(String),
    /// Network-level problem; the same credential will work once it clears.
    Transport(String),
}

/// A supervised connection, startable and stoppable while the process lives.
///
/// The window shell needs that: changing the workspace or the approval policy
/// must take effect without asking the user to quit the app, and every running
/// runtime belongs to the connection that spawned it.
pub struct Connector {
    host: Arc<dyn Host>,
    running: Mutex<Option<Running>>,
    status: Mutex<Status>,
}

struct Running {
    handle: JoinHandle<()>,
    manager: RuntimeManager,
}

impl Connector {
    pub fn new(host: Arc<dyn Host>) -> Arc<Self> {
        Arc::new(Self {
            host,
            running: Mutex::new(None),
            status: Mutex::new(Status::Offline),
        })
    }

    pub async fn status(&self) -> Status {
        self.status.lock().await.clone()
    }

    pub async fn is_running(&self) -> bool {
        match self.running.lock().await.as_ref() {
            // A finished task is not a running connection: `run` returns only
            // when it has given up for a reason retrying cannot fix, and a
            // headless caller waiting on this would otherwise wait forever.
            Some(r) => !r.handle.is_finished(),
            None => false,
        }
    }

    /// How many agent sessions are executing on this machine right now.
    pub async fn active_sessions(&self) -> usize {
        match self.running.lock().await.as_ref() {
            Some(r) => r.manager.active().await,
            None => 0,
        }
    }

    /// Start (or restart) the connection.
    ///
    /// `login` is present only when the user just typed credentials;
    /// otherwise the stored device token is used, which is the normal path and
    /// the one that never touches a password.
    pub async fn start(self: &Arc<Self>, config: ConnectorConfig, login: Option<Login>) {
        self.stop().await;

        let manager = RuntimeManager::new(Config {
            workspace: config.workspace.clone(),
            workspace_roots: config.workspace_roots.clone(),
            state_dir: config.state_dir.clone(),
            program: config.program.clone(),
            auto_approve: config.auto_approve,
        });

        let this = Arc::clone(self);
        let mgr = manager.clone();
        let handle = tokio::spawn(async move {
            this.run(config, login, mgr).await;
        });

        *self.running.lock().await = Some(Running { handle, manager });
    }

    /// Stop connecting and retire every local runtime.
    ///
    /// The runtimes go too: a dropped socket leaves them unreachable, and a
    /// runtime nobody can talk to is just a process holding the user's files
    /// open for no reason.
    pub async fn stop(&self) {
        let Some(running) = self.running.lock().await.take() else {
            return;
        };
        running.handle.abort();
        running.manager.stop_all().await;
        self.set_status(Status::Offline).await;
    }

    /// Forget the stored credential so the next start asks for an account.
    pub async fn sign_out(&self, config: &ConnectorConfig) {
        self.stop().await;
        forget_credential(&credential_path_for(config));
        self.set_status(Status::NeedsLogin { reason: None }).await;
    }

    async fn set_status(&self, status: Status) {
        *self.status.lock().await = status.clone();
        self.host.status(status);
    }

    /// The retry loop. Holds one credential at a time and reports every change
    /// of state, so a UI never has to infer what is happening.
    async fn run(
        &self,
        config: ConnectorConfig,
        first_login: Option<Login>,
        manager: RuntimeManager,
    ) {
        let url = device_endpoint(&config.site_url);
        let cred_path = credential_path_for(&config);
        let mut credential = load_credential(&cred_path);
        let mut pending_login = first_login;
        let mut backoff = 1u64;

        loop {
            let attach = match (pending_login.take(), &credential) {
                (Some(login), _) => Attach::Login(login),
                (None, Some(c)) => Attach::Token(c.token.clone()),
                // No device token yet. Before troubling the user, look for a
                // signed-in site window: that is the normal first run of the
                // desktop app, and it is the whole reason the machine appears
                // in the device list on its own.
                (None, None) => match self.host.session_token() {
                    Some(session) => Attach::Session(session),
                    None => match self.host.login() {
                        Ok(login) => Attach::Login(login),
                        Err(reason) => {
                            // Nothing to connect with, and guessing is not an
                            // option: stop rather than spin against a server
                            // that will refuse every attempt.
                            self.set_status(Status::NeedsLogin {
                                reason: (!reason.trim().is_empty()).then_some(reason),
                            })
                            .await;
                            return;
                        }
                    },
                },
            };
            let signing_in = matches!(attach, Attach::Login(_) | Attach::Session(_));

            self.set_status(Status::Connecting).await;
            match run_once(
                &url,
                attach,
                &config,
                &manager,
                &cred_path,
                Arc::clone(&self.host),
            )
            .await
            {
                Ok(Outcome::Closed) => {
                    self.host.log("连接已关闭，准备重连".into());
                    backoff = 1;
                }
                Ok(Outcome::Attached(fresh)) => {
                    credential = Some(fresh);
                    backoff = 1;
                }
                Err(Failure::Misconfigured(message)) => {
                    // Retrying a wrong address changes nothing and buries the
                    // one line the user actually needs to read.
                    self.set_status(Status::Error {
                        message: message.clone(),
                    })
                    .await;
                    self.host.log(message);
                    manager.stop_all().await;
                    return;
                }
                Err(Failure::Rejected(message)) => {
                    // The server refused this credential. Dropping it is what
                    // makes recovery possible: the next attempt asks the user
                    // to sign in instead of retrying a token that will never
                    // be accepted.
                    self.host.log(format!("服务器拒绝: {message}"));
                    let had_token = credential.is_some();
                    if had_token {
                        forget_credential(&cred_path);
                        credential = None;
                        self.host.log("已清除本地凭证，需要重新登录".into());
                    }
                    self.set_status(Status::NeedsLogin {
                        reason: Some(message),
                    })
                    .await;
                    manager.stop_all().await;
                    if signing_in && !had_token {
                        // A wrong password would otherwise re-prompt as fast
                        // as the host can answer.
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    continue;
                }
                Err(Failure::Transport(e)) => {
                    self.set_status(Status::Error {
                        message: format!("{e}（{backoff} 秒后重连）"),
                    })
                    .await;
                }
            }
            manager.stop_all().await;
            tokio::time::sleep(Duration::from_secs(backoff)).await;
            backoff = (backoff * 2).min(30);
        }
    }
}

/// Where this configuration's device token lives.
pub fn credential_path_for(config: &ConnectorConfig) -> PathBuf {
    credential_path(
        config.config_dir.as_deref(),
        &device_endpoint(&config.site_url),
    )
}

/// Which account, if any, this machine is already bound as.
pub fn bound_account(config: &ConnectorConfig) -> Option<Credential> {
    load_credential(&credential_path_for(config))
}

async fn run_once(
    url: &str,
    attach: Attach,
    config: &ConnectorConfig,
    manager: &RuntimeManager,
    cred_path: &Path,
    host: Arc<dyn Host>,
) -> Result<Outcome, Failure> {
    // Credentials cross this socket, so refuse to send a password in the
    // clear. Plain HTTP stays usable for local development and for an
    // already-issued token, but a password is not worth the same latitude.
    if matches!(attach, Attach::Login(_) | Attach::Session(_))
        && !url.starts_with("wss://")
        && !is_loopback(url)
    {
        return Err(Failure::Misconfigured(
            "拒绝在非加密连接上发送凭据，请将站点地址改为 https://".into(),
        ));
    }

    let (ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| Failure::Transport(e.to_string()))?;
    let (mut sink, mut stream) = ws.split();

    let mut signed_in_as = String::new();
    let hello = match attach {
        Attach::Token(token) => ToServer::Hello {
            token,
            name: config.name.clone(),
            platform: Some(platform().to_string()),
        },
        Attach::Session(session) => ToServer::Attach {
            session,
            name: config.name.clone(),
            platform: Some(platform().to_string()),
            fingerprint: Some(fingerprint(&config.name)),
        },
        Attach::Login(login) => {
            signed_in_as = login.username.clone();
            ToServer::Login {
                username: login.username,
                password: login.password,
                name: config.name.clone(),
                platform: Some(platform().to_string()),
                fingerprint: Some(fingerprint(&config.name)),
            }
        }
    };
    let hello = serde_json::to_string(&hello).map_err(|e| Failure::Transport(e.to_string()))?;
    sink.send(Message::Text(hello))
        .await
        .map_err(|e| Failure::Transport(e.to_string()))?;

    // Outbound queue: runtime output, heartbeats and lifecycle notices all
    // funnel through one writer so the socket has a single owner.
    let (tx, mut rx) = mpsc::channel::<ToServer>(256);

    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let Ok(text) = serde_json::to_string(&msg) else {
                continue;
            };
            if sink.send(Message::Text(text)).await.is_err() {
                break;
            }
        }
    });

    let beat = tx.clone();
    let heartbeat = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(25)).await;
            if beat.send(ToServer::Heartbeat).await.is_err() {
                break;
            }
        }
    });

    // Runtime output passes through this hop on its way to the server, which
    // is the only place that can see an agent blocked on a human. Without it
    // the user would have to already be watching the site to notice that a
    // task on their own machine is waiting for them.
    let (watch_tx, mut watch_rx) = mpsc::channel::<ToServer>(256);
    let relay = tx.clone();
    let notifier = Arc::clone(&host);
    let watcher = tokio::spawn(async move {
        while let Some(msg) = watch_rx.recv().await {
            if let ToServer::Frame { frame, .. } = &msg
                && let Some((title, body)) = blocking_prompt(frame)
            {
                notifier.attention(title, body);
            }
            if relay.send(msg).await.is_err() {
                break;
            }
        }
    });

    let mut outcome = Outcome::Closed;
    let mut rejection: Option<String> = None;

    while let Some(msg) = stream.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                heartbeat.abort();
                writer.abort();
                watcher.abort();
                return Err(Failure::Transport(e.to_string()));
            }
        };
        let Message::Text(text) = msg else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<FromServer>(&text) else {
            continue;
        };
        match parsed {
            FromServer::HelloOk {
                device_id,
                token,
                username: bound_as,
            } => {
                // A token arrives only right after a sign-in, and it is stored
                // immediately rather than when the socket closes: a client
                // killed while connected would otherwise lose the credential
                // it just obtained and ask for the password again.
                let username = match token {
                    Some(token) => {
                        let cred = Credential {
                            // The server's answer wins: attaching through the
                            // site's session never involved a username here,
                            // so this is the only place it can come from.
                            username: bound_as
                                .map(|u| u.trim().to_string())
                                .filter(|u| !u.is_empty())
                                .unwrap_or_else(|| signed_in_as.clone()),
                            token,
                            device_id,
                        };
                        match save_credential(cred_path, &cred) {
                            Ok(()) => {
                                host.log(format!("已绑定本机，凭证保存于 {}", cred_path.display()))
                            }
                            Err(e) => {
                                host.log(format!("无法保存设备凭证（下次启动仍需登录）: {e}"))
                            }
                        }
                        let name = cred.username.clone();
                        outcome = Outcome::Attached(cred);
                        name
                    }
                    None => load_credential(cred_path)
                        .map(|c| c.username)
                        .unwrap_or_default(),
                };
                host.status(Status::Online {
                    device_id,
                    username,
                });
                host.log(format!("已连接，设备 ID {device_id}"));
                // Advertise the directories tasks may run in, so the web UI
                // can offer them. Sent after the handshake rather than with
                // it: the handshake is about identity, and a rejected attach
                // must not have leaked a list of local paths.
                let _ = tx.send(workspaces_frame(config)).await;
            }
            FromServer::Error { message } => {
                rejection = Some(message);
                break;
            }
            FromServer::StartRuntime {
                session_id,
                models_json,
                workspace,
            } => {
                host.log(match workspace.as_deref() {
                    Some(dir) => format!("任务 {session_id}: 在 {dir} 启动本机运行时"),
                    None => format!("任务 {session_id}: 启动本机运行时"),
                });
                if let Err(e) = manager
                    .start(
                        session_id,
                        &models_json,
                        workspace.as_deref(),
                        watch_tx.clone(),
                    )
                    .await
                {
                    host.log(format!("任务 {session_id} 启动失败: {e}"));
                    // Report it so the session fails visibly instead of
                    // waiting forever for frames that will never arrive.
                    let _ = tx
                        .send(ToServer::RuntimeError {
                            session_id,
                            message: e,
                        })
                        .await;
                }
            }
            FromServer::Frame { session_id, frame } => {
                if let Err(e) = manager.send(session_id, &frame).await {
                    host.log(format!("任务 {session_id} 转发失败: {e}"));
                    let _ = tx
                        .send(ToServer::RuntimeError {
                            session_id,
                            message: e,
                        })
                        .await;
                }
            }
            FromServer::StopRuntime { session_id } => {
                host.log(format!("任务 {session_id}: 停止本机运行时"));
                manager.stop(session_id).await;
            }
            FromServer::ListDir { req_id, path } => {
                // Answered on a blocking thread: a home directory on a slow
                // disk can take long enough that reading it inline would
                // stall the frame loop, and the socket also carries the
                // transcript of whatever is running.
                let listing = tokio::task::spawn_blocking({
                    let default = config.workspace.clone();
                    let roots = config.workspace_roots.clone();
                    move || dir_listing(req_id, path, &default, &roots)
                })
                .await;
                let listing = listing.unwrap_or_else(|e| ToServer::DirListing {
                    req_id,
                    path: None,
                    parent: None,
                    entries: Vec::new(),
                    error: Some(format!("读取目录失败: {e}")),
                });
                if tx.send(listing).await.is_err() {
                    break;
                }
            }
        }
    }

    heartbeat.abort();
    writer.abort();
    watcher.abort();
    match rejection {
        // A rejection *after* a successful sign-in is about the device being
        // removed, not about the token just stored, so the fresh credential is
        // still reported: discarding it would force a needless re-login.
        Some(message) => match outcome {
            Outcome::Attached(cred) => {
                host.log(format!("服务器拒绝: {message}"));
                Ok(Outcome::Attached(cred))
            }
            Outcome::Closed => Err(Failure::Rejected(message)),
        },
        None => Ok(outcome),
    }
}

// ---------------------------------------------------------------------------
// workspace advertising and browsing
// ---------------------------------------------------------------------------

/// The directories this machine will accept, as a frame.
fn workspaces_frame(config: &ConnectorConfig) -> ToServer {
    let mut roots = Vec::new();
    // The default first, so a picker can present it as what a task gets when
    // it asks for nothing.
    for path in std::iter::once(&config.workspace).chain(
        config
            .workspace_roots
            .iter()
            .filter(|p| *p != &config.workspace),
    ) {
        roots.push(crate::proto::WorkspaceRoot {
            path: path.to_string_lossy().into_owned(),
            label: root_label(path),
        });
    }
    ToServer::Workspaces {
        default: config.workspace.to_string_lossy().into_owned(),
        roots,
    }
}

/// A short name for a root, since the full path is often too long for a menu.
fn root_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        // A root like `/` or `C:\` has no file name; showing the path itself
        // is better than an empty entry.
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Answer one directory listing.
///
/// Every path is validated against the authorized roots before it is read, so
/// this is not a way for a server to enumerate the user's disk: with no path
/// it returns the roots, and with one it returns children only when that
/// directory lies inside a root.
fn dir_listing(req_id: u64, path: Option<String>, default: &Path, roots: &[PathBuf]) -> ToServer {
    let refuse = |error: String| ToServer::DirListing {
        req_id,
        path: None,
        parent: None,
        entries: Vec::new(),
        error: Some(error),
    };

    // No path: the roots themselves. Listed as entries so the picker's first
    // screen and its later screens have one shape.
    let Some(raw) = path.as_deref().map(str::trim).filter(|p| !p.is_empty()) else {
        let mut entries = Vec::new();
        for root in std::iter::once(default).chain(roots.iter().map(PathBuf::as_path)) {
            let as_string = root.to_string_lossy().into_owned();
            if entries
                .iter()
                .any(|e: &crate::proto::DirEntry| e.path == as_string)
            {
                continue;
            }
            entries.push(crate::proto::DirEntry {
                path: as_string,
                name: root_label(root),
                repo: root.join(".git").exists(),
            });
        }
        return ToServer::DirListing {
            req_id,
            path: None,
            parent: None,
            entries,
            error: None,
        };
    };

    let dir = match crate::runtime::resolve_workspace(default, roots, Some(raw)) {
        Ok(dir) => dir,
        Err(e) => return refuse(e),
    };
    let children = match crate::runtime::list_children(&dir) {
        Ok(children) => children,
        Err(e) => return refuse(e),
    };
    // "Up" is offered only while it stays inside the scope; at a root the
    // picker goes back to the root list instead, so browsing cannot walk out
    // of what the user authorized.
    let parent = dir
        .parent()
        .filter(|p| {
            crate::runtime::resolve_workspace(default, roots, Some(&p.to_string_lossy())).is_ok()
        })
        .map(|p| p.to_string_lossy().into_owned());

    ToServer::DirListing {
        req_id,
        path: Some(dir.to_string_lossy().into_owned()),
        parent,
        entries: children
            .into_iter()
            .map(|c| crate::proto::DirEntry {
                path: c.path,
                name: c.name,
                repo: c.repo,
            })
            .collect(),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::ToServer;

    #[test]
    fn a_status_serialises_for_a_ui() {
        let v = serde_json::to_value(Status::Online {
            device_id: 3,
            username: "orca".into(),
        })
        .unwrap();
        assert_eq!(v["state"], "online");
        assert_eq!(v["device_id"], 3);

        // The absent-reason case must not render `null`, which a UI would
        // happily print as text.
        let v = serde_json::to_value(Status::NeedsLogin { reason: None }).unwrap();
        assert_eq!(v["state"], "needs_login");
        assert!(v.get("reason").is_none());
    }

    #[tokio::test]
    async fn a_window_host_that_cannot_prompt_stops_instead_of_spinning() {
        // The GUI cannot ask for a password from inside the connection loop.
        // Without a stored token the loop must therefore park in NeedsLogin,
        // not hammer a server that will refuse every attempt — and it must not
        // invent a reason, or the panel shows "需要登录 · 需要登录".
        struct Silent {
            seen: std::sync::Mutex<Vec<Status>>,
        }
        impl Host for Silent {
            fn status(&self, status: Status) {
                self.seen.lock().unwrap().push(status);
            }
            fn log(&self, _line: String) {}
            fn attention(&self, _title: String, _body: String) {}
        }

        let host = Arc::new(Silent {
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let connector = Connector::new(Arc::clone(&host) as Arc<dyn Host>);
        let dir = std::env::temp_dir().join(format!("yunova-noauth-{}", std::process::id()));
        connector
            .start(
                ConnectorConfig {
                    // Never dialled: the loop gives up before it connects.
                    site_url: "https://example.invalid".into(),
                    name: "laptop".into(),
                    workspace: dir.clone(),
                    workspace_roots: vec![dir.clone()],
                    state_dir: dir.clone(),
                    program: "pi".into(),
                    auto_approve: false,
                    config_dir: Some(dir.clone()),
                },
                None,
            )
            .await;

        for _ in 0..40 {
            if !connector.is_running().await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(!connector.is_running().await, "must not retry forever");
        assert_eq!(
            connector.status().await,
            Status::NeedsLogin { reason: None },
            "an empty reason must not become a duplicated label"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_signed_in_window_attaches_without_asking_for_a_password() {
        // The point of the whole automatic path: with a session available the
        // connector must try it rather than parking in NeedsLogin, which is
        // what left a freshly installed app showing "本地电脑（离线）".
        struct Windowed {
            seen: std::sync::Mutex<Vec<Status>>,
            asked_for_password: std::sync::atomic::AtomicBool,
        }
        impl Host for Windowed {
            fn status(&self, status: Status) {
                self.seen.lock().unwrap().push(status);
            }
            fn log(&self, _line: String) {}
            fn attention(&self, _title: String, _body: String) {}
            fn session_token(&self) -> Option<String> {
                Some("nc_session_value".into())
            }
            fn login(&self) -> Result<Login, String> {
                self.asked_for_password
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                Err(String::new())
            }
        }

        let host = Arc::new(Windowed {
            seen: std::sync::Mutex::new(Vec::new()),
            asked_for_password: std::sync::atomic::AtomicBool::new(false),
        });
        let connector = Connector::new(Arc::clone(&host) as Arc<dyn Host>);
        let dir = std::env::temp_dir().join(format!("yunova-session-{}", std::process::id()));
        connector
            .start(
                ConnectorConfig {
                    // Unresolvable on purpose: the assertion is about which
                    // credential the loop reaches for, not about reaching a
                    // server.
                    site_url: "https://example.invalid".into(),
                    name: "laptop".into(),
                    workspace: dir.clone(),
                    workspace_roots: vec![dir.clone()],
                    state_dir: dir.clone(),
                    program: "pi".into(),
                    auto_approve: false,
                    config_dir: Some(dir.clone()),
                },
                None,
            )
            .await;

        // It must reach the transport rather than give up, so wait for a
        // Connecting/Error rather than for the task to end.
        let mut tried = false;
        for _ in 0..80 {
            if host
                .seen
                .lock()
                .unwrap()
                .iter()
                .any(|s| matches!(s, Status::Connecting))
            {
                tried = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        connector.stop().await;
        assert!(tried, "a session in the window must be tried");
        assert!(
            !host
                .asked_for_password
                .load(std::sync::atomic::Ordering::SeqCst),
            "the user already signed in on the site; asking again is the step this removes"
        );
        assert!(
            !host
                .seen
                .lock()
                .unwrap()
                .iter()
                .any(|s| matches!(s, Status::NeedsLogin { .. })),
            "a usable session must not be reported as a missing login"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_session_is_never_sent_over_plain_http() {
        // It buys the same access a password does, so it gets the same rule:
        // the check lives in `run_once`, and this asserts the classification
        // it depends on rather than reaching a socket.
        assert!(!is_loopback("ws://yunnet.top/api/agent/devices/connect"));
        assert!(is_loopback("ws://127.0.0.1:3000/api/agent/devices/connect"));
    }

    #[test]
    fn the_attach_frame_proves_the_machine_with_the_sites_session() {
        let text = serde_json::to_string(&ToServer::Attach {
            session: "nc_session_value".into(),
            name: "laptop".into(),
            platform: Some("linux".into()),
            fingerprint: Some("fp123".into()),
        })
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["type"], "attach");
        assert_eq!(v["session"], "nc_session_value");
        // Same rebinding rule as a password login: re-running the app must not
        // add a second entry to the user's device list.
        assert_eq!(v["fingerprint"], "fp123");
        assert!(
            v.get("password").is_none(),
            "this path exists precisely so no password is involved"
        );
    }

    #[test]
    fn the_hello_frame_matches_the_servers_wire_format() {
        let text = serde_json::to_string(&ToServer::Hello {
            token: "ynd_x".into(),
            name: "laptop".into(),
            platform: Some("linux".into()),
        })
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["type"], "hello");
        assert_eq!(v["token"], "ynd_x");
        assert_eq!(v["name"], "laptop");
        assert_eq!(v["platform"], "linux");
    }

    #[test]
    fn the_login_frame_carries_the_account_and_the_machine() {
        let text = serde_json::to_string(&ToServer::Login {
            username: "orca".into(),
            password: "secret".into(),
            name: "laptop".into(),
            platform: Some("linux".into()),
            fingerprint: Some("fp123".into()),
        })
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["type"], "login");
        assert_eq!(v["username"], "orca");
        // The fingerprint is what makes a re-login rebind this machine instead
        // of adding a second entry to the user's device list.
        assert_eq!(v["fingerprint"], "fp123");
    }

    #[test]
    fn a_hello_ok_without_a_token_is_a_plain_reconnect() {
        // Only a sign-in mints a token, so the field must be optional or an
        // ordinary reconnect would fail to parse.
        let ok: FromServer = serde_json::from_str(r#"{"type":"hello_ok","device_id":3}"#).unwrap();
        match ok {
            FromServer::HelloOk {
                device_id,
                token,
                username,
            } => {
                assert_eq!(device_id, 3);
                assert!(token.is_none());
                assert!(username.is_none());
            }
            other => panic!("expected hello_ok, got {other:?}"),
        }

        let fresh: FromServer = serde_json::from_str(
            r#"{"type":"hello_ok","device_id":3,"token":"ynd_new","username":"orca"}"#,
        )
        .unwrap();
        match fresh {
            FromServer::HelloOk {
                token, username, ..
            } => {
                assert_eq!(token.as_deref(), Some("ynd_new"));
                // The automatic attach never typed a username, so the panel
                // can only name the account if the server says it.
                assert_eq!(username.as_deref(), Some("orca"));
            }
            other => panic!("expected hello_ok, got {other:?}"),
        }
    }

    #[test]
    fn runtime_frames_are_tagged_with_their_session() {
        // One socket multiplexes every session on this machine, so an untagged
        // frame could be applied to the wrong transcript.
        let text = serde_json::to_string(&ToServer::Frame {
            session_id: 7,
            frame: serde_json::json!({"type":"agent_settled"}),
        })
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["type"], "frame");
        assert_eq!(v["session_id"], 7);
        assert_eq!(v["frame"]["type"], "agent_settled");
    }

    #[test]
    fn server_commands_parse_including_the_runtime_credential() {
        let start: FromServer = serde_json::from_str(
            r#"{"type":"start_runtime","session_id":4,"models_json":{"providers":{}}}"#,
        )
        .unwrap();
        assert!(matches!(
            start,
            FromServer::StartRuntime { session_id: 4, .. }
        ));

        let frame: FromServer =
            serde_json::from_str(r#"{"type":"frame","session_id":4,"frame":{"type":"prompt"}}"#)
                .unwrap();
        match frame {
            FromServer::Frame { session_id, frame } => {
                assert_eq!(session_id, 4);
                assert_eq!(frame["type"], "prompt");
            }
            other => panic!("expected frame, got {other:?}"),
        }

        let stop: FromServer =
            serde_json::from_str(r#"{"type":"stop_runtime","session_id":4}"#).unwrap();
        assert!(matches!(stop, FromServer::StopRuntime { session_id: 4 }));
    }
}
