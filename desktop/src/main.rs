//! Yunova desktop connector.
//!
//! Turns the user's own computer into an execution target. It dials the server
//! and holds a WebSocket open — a personal machine usually has no reachable
//! address, so an outbound connection is the only thing that works without
//! port forwarding.
//!
//! Its job is deliberately small: run `pi --mode rpc` locally and pipe its
//! stdio over the relay. The thinking loop is the runtime's, the session and
//! billing are the server's, and the local execution policy (workspace scope,
//! approval gate) is this client's. That split is what lets a task started on
//! a phone run here without the phone or the server being trusted with the
//! machine.
//!
//! Attaching is a sign-in, not a pairing step: the user gives their account
//! credentials once and the server binds this machine on the spot. What stays
//! on disk afterwards is a device-scoped token, so the credential this client
//! holds can be revoked to one computer instead of standing in for the
//! account.
//!
//! This is the headless core. A GUI shell can embed it unchanged; the
//! connector, policy and supervision do not depend on having a window.

mod identity;
mod proto;
mod runtime;
#[path = "../../src/runtime_env.rs"]
mod runtime_env;

use std::path::PathBuf;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use identity::{
    Credential, credential_path, fingerprint, forget_credential, load_credential, prompt_login,
    save_credential,
};
use proto::{FromServer, ToServer};
use runtime::{Config, RuntimeManager};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() {
    // rustls 0.23 cannot pick a backend on its own; without this, connecting
    // to wss:// panics.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let raw = match runtime_env::var("YUNOVA_DEVICE_URL") {
        Ok(v) => v,
        Err(_) => {
            eprintln!(
                "需要环境变量 YUNOVA_DEVICE_URL（站点地址，如 https://yunnet.top）\n\
                 首次运行会提示登录 Yunova 账号，登录后自动绑定本机\n\
                 可选 YUNOVA_USERNAME、YUNOVA_PASSWORD（无人值守启动，免交互登录）\n\
                 可选 YUNOVA_DEVICE_WORKSPACE（Agent 可操作的目录，默认当前目录）\n\
                 可选 YUNOVA_DEVICE_NAME、YUNOVA_PI_BIN\n\
                 可选 YUNOVA_DEVICE_AUTO_APPROVE=1（放开审批，谨慎使用）"
            );
            std::process::exit(2);
        }
    };
    let url = normalize_url(&raw);
    let name = runtime_env::var("YUNOVA_DEVICE_NAME").unwrap_or_else(|_| hostname());

    // The workspace bounds what the agent can reach. Defaulting to the current
    // directory rather than $HOME keeps an unconfigured run from exposing
    // everything the user owns.
    let workspace = runtime_env::var("YUNOVA_DEVICE_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let state_dir = runtime_env::var("YUNOVA_DEVICE_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| workspace.join(".yunova-agent"));
    let auto_approve = matches!(
        runtime_env::var("YUNOVA_DEVICE_AUTO_APPROVE")
            .unwrap_or_default()
            .as_str(),
        "1" | "true" | "yes" | "on"
    );

    let manager = RuntimeManager::new(Config {
        workspace: workspace.clone(),
        state_dir,
        program: runtime_env::var("YUNOVA_PI_BIN").unwrap_or_else(|_| "pi".into()),
        auto_approve,
    });

    // The credential lives outside the workspace: the workspace is precisely
    // what the agent may rewrite, and a token the agent can edit is a token it
    // can replace or leak. An explicit override exists for packaging.
    let cred_path = credential_path(
        runtime_env::var("YUNOVA_DEVICE_CONFIG_DIR")
            .ok()
            .map(PathBuf::from)
            .as_deref(),
        &url,
    );

    println!("Yunova 桌面客户端");
    println!("  服务器:   {url}");
    println!("  设备名:   {name}");
    println!("  工作目录: {}", manager.workspace().display());
    println!(
        "  审批:     {}",
        if auto_approve {
            "已放开（Agent 可直接执行命令）"
        } else {
            "逐条确认（在网页或手机上处理）"
        }
    );
    if auto_approve {
        println!("  ⚠ 自动批准下 Agent 可在本机任意执行命令，只在信任的环境中使用。");
    }

    // Stop local runtimes on Ctrl-C rather than orphaning them.
    let shutdown = manager.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        eprintln!("\n正在停止本机运行时…");
        shutdown.stop_all().await;
        std::process::exit(0);
    });

    let mut credential = load_credential(&cred_path);
    match &credential {
        Some(c) if !c.username.is_empty() => {
            println!("  账号:     {}（设备 {}）", c.username, c.device_id)
        }
        Some(_) => println!("  账号:     已保存设备凭证"),
        None => println!("  账号:     未登录，马上要求登录"),
    }

    let mut backoff = 1u64;
    loop {
        // No stored token means this is a first attach, or the server rejected
        // the old one. Ask for the account, then let the connection mint a
        // device token.
        let attach = match &credential {
            Some(c) => Attach::Token(c.token.clone()),
            None => match prompt_login(
                runtime_env::var("YUNOVA_USERNAME").ok(),
                runtime_env::var("YUNOVA_PASSWORD").ok(),
            ) {
                Ok(login) => Attach::Login(login),
                Err(e) => {
                    eprintln!("[device] {e}");
                    std::process::exit(2);
                }
            },
        };

        match run_once(&url, attach, &name, &manager, &cred_path).await {
            Ok(Outcome::Closed) => {
                eprintln!("[device] 连接已关闭，准备重连");
                backoff = 1;
            }
            Ok(Outcome::Attached(fresh)) => {
                credential = Some(fresh);
                backoff = 1;
            }
            Err(Failure::Misconfigured(message)) => {
                // Nothing about retrying changes a wrong URL, and looping on
                // it buries the one line the user actually needs to read.
                eprintln!("[device] {message}");
                manager.stop_all().await;
                std::process::exit(2);
            }
            Err(Failure::Rejected(message)) => {
                // The server refused this credential. Dropping it is what
                // makes recovery possible: the next iteration asks the user to
                // sign in instead of retrying a token that will never be
                // accepted.
                eprintln!("[device] 服务器拒绝: {message}");
                if credential.is_some() {
                    forget_credential(&cred_path);
                    credential = None;
                    eprintln!("[device] 已清除本地凭证，将重新登录");
                } else {
                    // A wrong password would otherwise spin as fast as the user
                    // can be re-prompted.
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                manager.stop_all().await;
                continue;
            }
            Err(Failure::Transport(e)) => {
                eprintln!("[device] 连接错误: {e}，{backoff} 秒后重连")
            }
        }
        // A dropped socket leaves local runtimes unreachable, so retire them
        // instead of leaving processes nobody can talk to.
        manager.stop_all().await;
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(30);
    }
}

/// How this connection intends to authenticate.
enum Attach {
    Token(String),
    Login(identity::Login),
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

async fn run_once(
    url: &str,
    attach: Attach,
    name: &str,
    manager: &RuntimeManager,
    cred_path: &std::path::Path,
) -> Result<Outcome, Failure> {
    // Credentials cross this socket, so refuse to send a password in the
    // clear. Plain HTTP stays usable for local development and for an
    // already-issued token, but a password is not worth the same latitude.
    if matches!(attach, Attach::Login(_)) && !url.starts_with("wss://") && !is_loopback(url) {
        return Err(Failure::Misconfigured(
            "拒绝在非加密连接上发送密码，请将 YUNOVA_DEVICE_URL 改为 https://".into(),
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
            name: name.to_string(),
            platform: Some(platform().to_string()),
        },
        Attach::Login(login) => {
            signed_in_as = login.username.clone();
            ToServer::Login {
                username: login.username,
                password: login.password,
                name: name.to_string(),
                platform: Some(platform().to_string()),
                fingerprint: Some(fingerprint(name)),
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

    let mut outcome = Outcome::Closed;
    let mut rejection: Option<String> = None;

    while let Some(msg) = stream.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                heartbeat.abort();
                writer.abort();
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
            FromServer::HelloOk { device_id, token } => {
                println!("已连接，设备 ID {device_id}");
                // A token arrives only right after a sign-in, and it is stored
                // immediately rather than when the socket closes: a client
                // killed while connected would otherwise lose the credential
                // it just obtained and ask for the password again.
                if let Some(token) = token {
                    let cred = Credential {
                        token,
                        username: signed_in_as.clone(),
                        device_id,
                    };
                    match save_credential(cred_path, &cred) {
                        Ok(()) => println!(
                            "[device] 已绑定本机，凭证保存于 {}",
                            cred_path.display()
                        ),
                        Err(e) => eprintln!(
                            "[device] 无法保存设备凭证（下次启动仍需登录）: {e}"
                        ),
                    }
                    outcome = Outcome::Attached(cred);
                }
            }
            FromServer::Error { message } => {
                rejection = Some(message);
                break;
            }
            FromServer::StartRuntime {
                session_id,
                models_json,
            } => {
                println!("[device] 任务 {session_id}: 启动本机运行时");
                if let Err(e) = manager.start(session_id, &models_json, tx.clone()).await {
                    eprintln!("[device] 任务 {session_id} 启动失败: {e}");
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
                    eprintln!("[device] 任务 {session_id} 转发失败: {e}");
                    let _ = tx
                        .send(ToServer::RuntimeError {
                            session_id,
                            message: e,
                        })
                        .await;
                }
            }
            FromServer::StopRuntime { session_id } => {
                println!("[device] 任务 {session_id}: 停止本机运行时");
                manager.stop(session_id).await;
            }
        }
    }

    heartbeat.abort();
    writer.abort();
    match rejection {
        // A rejection *after* a successful sign-in is about the device being
        // removed, not about the token just stored, so the fresh credential is
        // still reported: discarding it would force a needless re-login.
        Some(message) => match outcome {
            Outcome::Attached(cred) => {
                eprintln!("[device] 服务器拒绝: {message}");
                Ok(Outcome::Attached(cred))
            }
            Outcome::Closed => Err(Failure::Rejected(message)),
        },
        None => Ok(outcome),
    }
}

/// Whether the endpoint is this machine, where plain HTTP is a development
/// setup rather than an exposed password.
fn is_loopback(url: &str) -> bool {
    let host = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url)
        .split(['/', '?'])
        .next()
        .unwrap_or("");
    let host = match host.strip_prefix('[') {
        // Bracketed IPv6: the colons inside belong to the address, not to a
        // port, so the generic split would truncate it.
        Some(rest) => rest.split_once(']').map(|(h, _)| h).unwrap_or(rest),
        None => host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host),
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

/// Accept a site address and derive the device WebSocket endpoint.
///
/// Users paste what they see in the browser, so the bare origin must work:
///   https://yunnet.top   -> wss://yunnet.top/api/agent/devices/connect
///   http://10.0.0.1:3000 -> ws://10.0.0.1:3000/api/agent/devices/connect
///   yunnet.top           -> wss://yunnet.top/api/agent/devices/connect
/// An already-complete endpoint is left alone.
fn normalize_url(raw: &str) -> String {
    const PATH: &str = "/api/agent/devices/connect";
    let raw = raw.trim();
    let (scheme, rest) = match raw.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        // No scheme: assume TLS rather than silently downgrading.
        None => ("https".to_string(), raw),
    };
    let ws_scheme = match scheme.as_str() {
        "http" | "ws" => "ws",
        _ => "wss",
    };
    let rest = rest.trim_end_matches('/');
    if rest.ends_with(PATH) {
        return format!("{ws_scheme}://{rest}");
    }
    format!("{ws_scheme}://{rest}{PATH}")
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "我的电脑".into())
}

fn platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_site_address_becomes_a_secure_endpoint() {
        // What the user copies out of the browser has to work.
        assert_eq!(
            normalize_url("https://yunnet.top"),
            "wss://yunnet.top/api/agent/devices/connect"
        );
        assert_eq!(
            normalize_url("yunnet.top"),
            "wss://yunnet.top/api/agent/devices/connect"
        );
        // A trailing slash must not produce a doubled path.
        assert_eq!(
            normalize_url("https://yunnet.top/"),
            "wss://yunnet.top/api/agent/devices/connect"
        );
    }

    #[test]
    fn plain_http_is_preserved_for_local_development() {
        assert_eq!(
            normalize_url("http://127.0.0.1:3000"),
            "ws://127.0.0.1:3000/api/agent/devices/connect"
        );
        assert_eq!(
            normalize_url("ws://127.0.0.1:3000"),
            "ws://127.0.0.1:3000/api/agent/devices/connect"
        );
    }

    #[test]
    fn an_unknown_scheme_is_treated_as_encrypted() {
        // Guessing wrong in the safe direction fails loudly; guessing wrong in
        // the unsafe direction would send a password in the clear.
        assert!(normalize_url("gopher://example.com").starts_with("wss://"));
    }

    #[test]
    fn a_complete_endpoint_is_left_alone() {
        assert_eq!(
            normalize_url("wss://yunnet.top/api/agent/devices/connect"),
            "wss://yunnet.top/api/agent/devices/connect"
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
        assert_eq!(v["password"], "secret");
        // The fingerprint is what makes a re-login rebind this machine instead
        // of adding a second entry to the user's device list.
        assert_eq!(v["fingerprint"], "fp123");
    }

    #[test]
    fn a_hello_ok_without_a_token_is_a_plain_reconnect() {
        // Only a sign-in mints a token, so the field must be optional or an
        // ordinary reconnect would fail to parse.
        let ok: FromServer =
            serde_json::from_str(r#"{"type":"hello_ok","device_id":3}"#).unwrap();
        match ok {
            FromServer::HelloOk { device_id, token } => {
                assert_eq!(device_id, 3);
                assert!(token.is_none());
            }
            other => panic!("expected hello_ok, got {other:?}"),
        }

        let fresh: FromServer =
            serde_json::from_str(r#"{"type":"hello_ok","device_id":3,"token":"ynd_new"}"#)
                .unwrap();
        match fresh {
            FromServer::HelloOk { token, .. } => assert_eq!(token.as_deref(), Some("ynd_new")),
            other => panic!("expected hello_ok, got {other:?}"),
        }
    }

    #[test]
    fn only_this_machine_counts_as_loopback() {
        // The password is allowed over plain HTTP exactly where the traffic
        // never leaves the machine.
        assert!(is_loopback("ws://127.0.0.1:3000/api/agent/devices/connect"));
        assert!(is_loopback("ws://localhost:3000/api/agent/devices/connect"));
        assert!(is_loopback("ws://[::1]:3000/api/agent/devices/connect"));
        assert!(!is_loopback("ws://10.1.51.1:4300/api/agent/devices/connect"));
        assert!(!is_loopback("ws://yunnet.top/api/agent/devices/connect"));
        // A hostname that merely starts with a loopback label is not loopback.
        assert!(!is_loopback("ws://localhost.evil.com/api"));
        assert!(!is_loopback("ws://127.0.0.1.evil.com/api"));
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
        assert!(matches!(start, FromServer::StartRuntime { session_id: 4, .. }));

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

        let stop: FromServer = serde_json::from_str(r#"{"type":"stop_runtime","session_id":4}"#)
            .unwrap();
        assert!(matches!(stop, FromServer::StopRuntime { session_id: 4 }));
    }
}
