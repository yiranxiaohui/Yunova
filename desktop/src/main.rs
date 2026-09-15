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
//! This is the headless core. A GUI shell can embed it unchanged; the
//! connector, policy and supervision do not depend on having a window.

mod proto;
mod runtime;
#[path = "../../src/runtime_env.rs"]
mod runtime_env;

use std::path::PathBuf;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
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
                 需要环境变量 YUNOVA_DEVICE_TOKEN（网页「本地电脑」页生成的配对码）\n\
                 可选 YUNOVA_DEVICE_WORKSPACE（Agent 可操作的目录，默认当前目录）\n\
                 可选 YUNOVA_DEVICE_NAME、YUNOVA_PI_BIN\n\
                 可选 YUNOVA_DEVICE_AUTO_APPROVE=1（放开审批，谨慎使用）"
            );
            std::process::exit(2);
        }
    };
    let url = normalize_url(&raw);
    let token = match runtime_env::var("YUNOVA_DEVICE_TOKEN") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("需要环境变量 YUNOVA_DEVICE_TOKEN（配对码）");
            std::process::exit(2);
        }
    };
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

    let mut backoff = 1u64;
    loop {
        match run_once(&url, &token, &name, &manager).await {
            Ok(()) => {
                eprintln!("[device] 连接已关闭，准备重连");
                backoff = 1;
            }
            Err(e) => eprintln!("[device] 连接错误: {e}，{backoff} 秒后重连"),
        }
        // A dropped socket leaves local runtimes unreachable, so retire them
        // instead of leaving processes nobody can talk to.
        manager.stop_all().await;
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(30);
    }
}

async fn run_once(
    url: &str,
    token: &str,
    name: &str,
    manager: &RuntimeManager,
) -> Result<(), String> {
    let (ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| e.to_string())?;
    let (mut sink, mut stream) = ws.split();

    let hello = serde_json::to_string(&ToServer::Hello {
        token: token.to_string(),
        name: name.to_string(),
        platform: Some(platform().to_string()),
    })
    .map_err(|e| e.to_string())?;
    sink.send(Message::Text(hello))
        .await
        .map_err(|e| e.to_string())?;

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

    while let Some(msg) = stream.next().await {
        let msg = msg.map_err(|e| e.to_string())?;
        let Message::Text(text) = msg else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<FromServer>(&text) else {
            continue;
        };
        match parsed {
            FromServer::HelloOk { device_id } => {
                println!("已连接，设备 ID {device_id}");
            }
            FromServer::Error { message } => {
                eprintln!("[device] 服务器拒绝: {message}");
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
    Ok(())
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
        // the unsafe direction would send a pairing code in the clear.
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
