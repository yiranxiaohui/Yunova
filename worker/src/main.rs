mod proto;
mod exec;
#[path = "../../src/runtime_env.rs"]
mod runtime_env;

use futures_util::{SinkExt, StreamExt};
use proto::{ToServer, ToWorker};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() {
    // 安装 rustls 加密后端（ring）。连 wss:// 前必须装好进程级 CryptoProvider，
    // 否则 rustls 0.23 无法自动确定后端会直接 panic。
    let _ = rustls::crypto::ring::default_provider().install_default();

    let raw = crate::runtime_env::var("YUNOVA_WORKER_URL")
        .expect("需要环境变量 YUNOVA_WORKER_URL（如 https://chat.yunnet.top）");
    let url = normalize_url(&raw);
    eprintln!("[worker] 连接地址: {url}");
    let token = crate::runtime_env::var("YUNOVA_WORKER_TOKEN")
        .expect("需要环境变量 YUNOVA_WORKER_TOKEN（配对码）");
    let name = crate::runtime_env::var("YUNOVA_WORKER_NAME")
        .unwrap_or_else(|_| hostname());

    let mut backoff = 1u64;
    loop {
        match run_once(&url, &token, &name).await {
            Ok(_) => {
                eprintln!("[worker] 连接正常关闭，3 秒后重连");
                backoff = 1;
            }
            Err(e) => {
                eprintln!("[worker] 连接错误: {e}，{backoff} 秒后重连");
            }
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(30);
    }
}

/// 把用户给的地址规整成 WebSocket 端点。
/// 接受站点根地址（推荐），自动转协议并补上 `/api/worker/connect`：
///   https://chat.yunnet.top        -> wss://chat.yunnet.top/api/worker/connect
///   http://10.0.0.1:3000           -> ws://10.0.0.1:3000/api/worker/connect
///   chat.yunnet.top                -> wss://chat.yunnet.top/api/worker/connect
/// 也兼容已经写全的旧式地址（含 ws/wss 协议或带 /api/worker/connect 后缀）。
fn normalize_url(raw: &str) -> String {
    let raw = raw.trim();

    // 拆出协议与主机部分。无协议时默认按 https（即 wss）处理。
    let (scheme, rest) = match raw.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        None => ("https".to_string(), raw),
    };
    let ws_scheme = match scheme.as_str() {
        "http" | "ws" => "ws",
        // https / wss / 其它一律按加密处理
        _ => "wss",
    };

    // 去掉末尾斜杠，再去掉用户可能已经写上的 /api/worker/connect 后缀。
    let rest = rest.trim_end_matches('/');
    let rest = rest
        .strip_suffix("/api/worker/connect")
        .unwrap_or(rest)
        .trim_end_matches('/');

    format!("{ws_scheme}://{rest}/api/worker/connect")
}

fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "worker".to_string())
}

async fn run_once(url: &str, token: &str, name: &str) -> Result<(), String> {
    let (ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| format!("WS 连接失败: {e}"))?;
    let (mut tx, mut rx) = ws.split();

    // 发 hello
    let hello = serde_json::to_string(&ToServer::Hello {
        token: token.to_string(),
        name: name.to_string(),
    })
    .unwrap();
    tx.send(Message::Text(hello)).await.map_err(|e| e.to_string())?;

    // 出站通道（心跳 + 工具结果都经它发，避免多任务争用 sink）
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Message>(32);

    // 心跳任务
    let hb_tx = out_tx.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(20)).await;
            let hb = serde_json::to_string(&ToServer::Heartbeat).unwrap();
            if hb_tx.send(Message::Text(hb)).await.is_err() {
                break;
            }
        }
    });

    loop {
        tokio::select! {
            Some(msg) = out_rx.recv() => {
                tx.send(msg).await.map_err(|e| e.to_string())?;
            }
            frame = rx.next() => {
                let frame = match frame {
                    Some(f) => f.map_err(|e| e.to_string())?,
                    None => return Ok(()),
                };
                let text = match frame {
                    Message::Text(t) => t,
                    Message::Close(_) => return Ok(()),
                    Message::Ping(p) => {
                        let _ = out_tx.send(Message::Pong(p)).await;
                        continue;
                    }
                    _ => continue,
                };
                let to_worker: ToWorker = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("[worker] 解析消息失败: {e}");
                        continue;
                    }
                };
                match to_worker {
                    ToWorker::HelloOk { worker_id } => {
                        eprintln!("[worker] 鉴权成功，worker_id={worker_id}");
                    }
                    ToWorker::Error { message } => {
                        return Err(format!("后端拒绝: {message}"));
                    }
                    ToWorker::Exec { call_id, tool, args } => {
                        let out_tx = out_tx.clone();
                        tokio::spawn(async move {
                            let (ok, output, truncated) = exec::run(&tool, &args).await;
                            let res = serde_json::to_string(&ToServer::ToolResult {
                                call_id,
                                ok,
                                output,
                                truncated,
                            })
                            .unwrap();
                            let _ = out_tx.send(Message::Text(res)).await;
                        });
                    }
                }
            }
        }
    }
}
