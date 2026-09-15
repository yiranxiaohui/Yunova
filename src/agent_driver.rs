//! Subprocess driver: `pi --mode rpc` as a child process.
//!
//! This is the first concrete [`AgentTransport`]. It runs the runtime on this
//! host, which is what the cloud target will use in step 3 — at that point the
//! same code runs *inside* a per-user container instead of on the host, so the
//! container boundary is a deployment change, not a protocol change.
//!
//! The spawned runtime is deliberately starved of credentials: its model
//! config points at Yunova's own gateway with an agent token, so a compromised
//! or prompt-injected agent still cannot reach a provider directly, and every
//! call is priced and metered. See `agent_token::runtime_models_json`.

use std::process::Stdio;
use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, mpsc};

use crate::agent_rpc::JsonlDecoder;
use crate::agent_session::AgentTransport;

/// How a runtime process is launched.
pub struct SpawnConfig {
    /// Executable, default `pi`. Overridable for a pinned or bundled build.
    pub program: String,
    /// Working directory the agent's tools operate in.
    pub cwd: std::path::PathBuf,
    /// pi config dir holding the generated `models.json`. Per-session so one
    /// user's gateway token is never visible to another's runtime.
    pub agent_dir: std::path::PathBuf,
    /// Session display name, surfaced in pi's own session listing.
    pub name: Option<String>,
}

/// A live child process speaking RPC over its stdio.
pub struct SubprocessTransport {
    stdin: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
}

#[async_trait::async_trait]
impl AgentTransport for SubprocessTransport {
    async fn send(&self, line: String) -> Result<(), String> {
        let mut guard = self.stdin.lock().await;
        let stdin = guard.as_mut().ok_or("运行时标准输入已关闭")?;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("写入运行时失败: {e}"))?;
        // Without an explicit flush a command can sit in the pipe buffer and
        // the agent appears to hang on an already-sent prompt.
        stdin
            .flush()
            .await
            .map_err(|e| format!("刷新运行时失败: {e}"))
    }

    async fn shutdown(&self) {
        // Close stdin first: pi exits cleanly on EOF, which lets it finish
        // writing its session file instead of being killed mid-write.
        self.stdin.lock().await.take();
        let mut guard = self.child.lock().await;
        if let Some(mut child) = guard.take() {
            let graceful = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                child.wait(),
            )
            .await;
            if graceful.is_err() {
                let _ = child.kill().await;
            }
        }
    }
}

/// Spawn a runtime and return its transport plus a stream of decoded frames.
///
/// stdout is decoded with the strict JSONL decoder; stderr is drained to the
/// server log. Draining stderr is not optional: a full stderr pipe blocks the
/// child, which would look like an agent that silently stopped responding.
pub async fn spawn(
    config: SpawnConfig,
) -> Result<(Arc<SubprocessTransport>, mpsc::Receiver<Value>), String> {
    let mut cmd = Command::new(&config.program);
    cmd.arg("--mode")
        .arg("rpc")
        // Session persistence is pi's own; Yunova mirrors entries itself and
        // is the source of truth for clients.
        .arg("--no-session")
        .current_dir(&config.cwd)
        .env("PI_CODING_AGENT_DIR", &config.agent_dir)
        // No update checks or telemetry from a server-side runtime.
        .env("PI_OFFLINE", "1")
        .env("PI_SKIP_VERSION_CHECK", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(name) = &config.name {
        cmd.arg("--name").arg(name);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动 Agent 运行时失败（{}）: {e}", config.program))?;

    let stdin = child.stdin.take().ok_or("无法获取运行时标准输入")?;
    let stdout = child.stdout.take().ok_or("无法获取运行时标准输出")?;
    let stderr = child.stderr.take();

    let (tx, rx) = mpsc::channel::<Value>(256);

    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        let mut decoder = JsonlDecoder::new();
        let mut buf = Vec::new();
        loop {
            buf.clear();
            // read_until on the LF byte keeps framing identical to the
            // protocol's definition; a lossy UTF-8 conversion prevents one bad
            // byte from ending the stream.
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) => break,
                Ok(_) => {
                    let chunk = String::from_utf8_lossy(&buf);
                    for frame in decoder.push(&chunk) {
                        if tx.send(frame).await.is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[agent-subprocess] stdout read failed: {e}");
                    break;
                }
            }
        }
    });

    if let Some(stderr) = stderr {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("[agent-runtime] {line}");
            }
        });
    }

    Ok((
        Arc::new(SubprocessTransport {
            stdin: Mutex::new(Some(stdin)),
            child: Mutex::new(Some(child)),
        }),
        rx,
    ))
}

/// Write the per-session pi config directory.
///
/// The generated `models.json` is the security boundary: it contains only the
/// gateway URL and an agent token, never an upstream provider key, and only
/// models the admin whitelisted. `0o700` keeps the token out of reach of other
/// local users on a shared host.
pub async fn write_agent_dir(
    agent_dir: &std::path::Path,
    models_json: &Value,
) -> Result<(), String> {
    tokio::fs::create_dir_all(agent_dir)
        .await
        .map_err(|e| format!("创建运行时配置目录失败: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(
            agent_dir,
            std::fs::Permissions::from_mode(0o700),
        )
        .await;
    }

    let path = agent_dir.join("models.json");
    let body = serde_json::to_string_pretty(models_json)
        .map_err(|e| format!("序列化运行时配置失败: {e}"))?;
    tokio::fs::write(&path, body)
        .await
        .map_err(|e| format!("写入运行时配置失败: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn spawning_a_missing_program_reports_a_usable_error() {
        let dir = std::env::temp_dir().join(format!("yunova-spawn-{}", std::process::id()));
        let res = spawn(SpawnConfig {
            program: "yunova-no-such-runtime".into(),
            cwd: dir.clone(),
            agent_dir: dir,
            name: None,
        })
        .await;
        let err = res.err().expect("a missing runtime must fail");
        assert!(
            err.contains("yunova-no-such-runtime"),
            "the error should name the program: {err}"
        );
    }

    #[tokio::test]
    async fn the_generated_config_is_written_private_and_without_upstream_keys() {
        let dir = std::env::temp_dir().join(format!(
            "yunova-agentdir-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let cfg = json!({
            "providers": {
                "yunova-claude": {
                    "baseUrl": "https://yunnet.top/api/proxy/claude",
                    "apiKey": "yna_token",
                }
            }
        });
        write_agent_dir(&dir, &cfg).await.unwrap();

        let body = tokio::fs::read_to_string(dir.join("models.json")).await.unwrap();
        assert!(body.contains("yna_token"));
        assert!(
            body.contains("/api/proxy/claude"),
            "the runtime must be pointed at the gateway"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = tokio::fs::metadata(dir.join("models.json"))
                .await
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "the gateway token must not be world-readable");
        }

        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }
}
