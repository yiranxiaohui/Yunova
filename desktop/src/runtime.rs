//! Local runtime supervision.
//!
//! This is where the desktop client differs from the cloud sandbox in kind,
//! not degree. A sandbox is disposable and isolated, so it can run an agent
//! with automatic approval. A personal machine is neither, and the prompts
//! driving it may arrive from a phone or be influenced by web content the
//! agent read. So the policy that constrains the agent lives *here*, on the
//! machine at risk, and the server is never asked to be trusted with it:
//!
//! * the runtime is confined to a user-chosen workspace, not `$HOME`;
//! * an approval extension is installed by this client and mounted where the
//!   agent cannot edit it away;
//! * the generated `models.json` never contains an upstream provider key.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, mpsc};

use crate::proto::ToServer;

/// One running local runtime.
struct Runtime {
    stdin: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
}

/// All runtimes this client is hosting, keyed by server session id.
#[derive(Clone, Default)]
pub struct RuntimeManager {
    inner: Arc<Mutex<HashMap<i64, Arc<Runtime>>>>,
    config: Arc<Config>,
}

/// Local execution policy. Everything here is the user's decision, not the
/// server's.
pub struct Config {
    /// Directory the agent may work in. Never defaults to `$HOME`: a task
    /// driven from a phone should not be able to touch everything the user
    /// owns because nobody set a narrower scope.
    pub workspace: PathBuf,
    /// Where per-session runtime config is written.
    pub state_dir: PathBuf,
    /// The `pi` executable.
    pub program: String,
    /// When true, every shell and write is auto-approved.
    ///
    /// Off by default. On a personal machine the safe default is to ask, and a
    /// user who wants to walk away opts in explicitly.
    pub auto_approve: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            workspace: PathBuf::from("."),
            state_dir: PathBuf::from("."),
            program: "pi".into(),
            auto_approve: false,
        }
    }
}

impl RuntimeManager {
    pub fn new(config: Config) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(config),
        }
    }

    /// How many runtimes are live.
    ///
    /// The window shows this because closing it does not stop them: a user who
    /// cannot see that work is still running on their machine cannot make an
    /// informed decision about quitting.
    pub async fn active(&self) -> usize {
        self.inner.lock().await.len()
    }

    /// Start a runtime for `session_id`.
    ///
    /// `to_server` receives every JSONL record the runtime emits, tagged with
    /// the session, so the caller can relay it without knowing how the process
    /// is managed.
    pub async fn start(
        &self,
        session_id: i64,
        models_json: &Value,
        to_server: mpsc::Sender<ToServer>,
    ) -> Result<(), String> {
        // A second runtime for one session would fork the transcript.
        if self.inner.lock().await.contains_key(&session_id) {
            return Ok(());
        }

        let agent_dir = self.config.state_dir.join(format!("s{session_id}"));
        write_runtime_config(&agent_dir, models_json, self.config.auto_approve).await?;

        tokio::fs::create_dir_all(&self.config.workspace)
            .await
            .map_err(|e| format!("无法创建工作目录: {e}"))?;

        // Resolved rather than spawned by name: a GUI app inherits a minimal
        // `PATH`, so a runtime the user installed in their terminal is
        // invisible here unless the known install prefixes are searched too.
        // Without this the app reports "启动 pi 失败" on a machine where `pi`
        // works perfectly in a shell.
        let program = crate::runtime_install::resolve(&self.config.program).ok_or_else(|| {
            format!(
                "未找到运行时 {}，请在「本机设置」里安装 pi 后重试",
                self.config.program
            )
        })?;
        let mut cmd = Command::new(&program);
        cmd.arg("--mode")
            .arg("rpc")
            .arg("--no-session")
            .current_dir(&self.config.workspace)
            .env("PI_CODING_AGENT_DIR", &agent_dir)
            .env("PI_OFFLINE", "1")
            .env("PI_SKIP_VERSION_CHECK", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("启动 {} 失败: {e}", self.config.program))?;
        let stdin = child.stdin.take().ok_or("无法获取运行时标准输入")?;
        let stdout = child.stdout.take().ok_or("无法获取运行时标准输出")?;
        let stderr = child.stderr.take();

        let runtime = Arc::new(Runtime {
            stdin: Mutex::new(Some(stdin)),
            child: Mutex::new(Some(child)),
        });
        self.inner.lock().await.insert(session_id, runtime);

        // stdout -> server. Strict LF framing, matching the RPC contract: a
        // generic line reader would also split on U+2028/U+2029, which are
        // legal inside JSON strings.
        let tx = to_server.clone();
        let manager = self.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf).await {
                    Ok(0) => break,
                    Ok(_) => {
                        let line = String::from_utf8_lossy(&buf);
                        let trimmed = line.trim_end_matches(['\n', '\r']);
                        if trimmed.trim().is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<Value>(trimmed) {
                            Ok(frame) => {
                                if tx
                                    .send(ToServer::Frame { session_id, frame })
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            // One corrupt frame must not tear down the session.
                            Err(e) => eprintln!("[runtime {session_id}] 丢弃无法解析的帧: {e}"),
                        }
                    }
                    Err(e) => {
                        eprintln!("[runtime {session_id}] 读取失败: {e}");
                        break;
                    }
                }
            }
            manager.forget(session_id).await;
            let _ = tx
                .send(ToServer::RuntimeClosed {
                    session_id,
                    reason: Some("运行时已退出".into()),
                })
                .await;
        });

        // Draining stderr is not optional: a full pipe blocks the child, which
        // looks like an agent that silently stopped responding.
        if let Some(stderr) = stderr {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    eprintln!("[runtime {session_id}] {line}");
                }
            });
        }

        Ok(())
    }

    /// Write one JSONL record to a runtime's stdin.
    pub async fn send(&self, session_id: i64, frame: &Value) -> Result<(), String> {
        let runtime = self
            .inner
            .lock()
            .await
            .get(&session_id)
            .cloned()
            .ok_or("该任务没有正在运行的运行时")?;
        let mut guard = runtime.stdin.lock().await;
        let stdin = guard.as_mut().ok_or("运行时标准输入已关闭")?;
        let mut line = serde_json::to_string(frame).map_err(|e| e.to_string())?;
        line.push('\n');
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

    /// Stop a runtime, letting it exit cleanly if it will.
    pub async fn stop(&self, session_id: i64) {
        let Some(runtime) = self.inner.lock().await.remove(&session_id) else {
            return;
        };
        // Close stdin first: pi exits on EOF, which lets it finish writing
        // rather than being killed mid-write.
        runtime.stdin.lock().await.take();
        let mut guard = runtime.child.lock().await;
        if let Some(mut child) = guard.take()
            && tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
                .await
                .is_err()
        {
            let _ = child.kill().await;
        }
    }

    pub async fn stop_all(&self) {
        let ids: Vec<i64> = self.inner.lock().await.keys().copied().collect();
        for id in ids {
            self.stop(id).await;
        }
    }

    async fn forget(&self, session_id: i64) {
        self.inner.lock().await.remove(&session_id);
    }
}

/// The approval gate installed on the user's machine.
///
/// Written by this client rather than sent by the server: the policy that
/// protects the machine must not be something a server can switch off. The
/// runtime discovers it because it sits under the session's config dir.
const APPROVAL_EXTENSION: &str = r#"// Installed by yunova-desktop. Gates tools that can change this machine.
export default function (pi) {
  const GUARDED = new Set(["bash", "powershell", "write", "edit"]);
  pi.on("tool_call", async (event, ctx) => {
    if (!GUARDED.has(event.toolName)) return;
    const detail =
      event.args?.command ?? event.args?.path ?? JSON.stringify(event.args ?? {});
    const ok = await ctx.ui.confirm({
      title: `允许在本机执行 ${event.toolName}？`,
      message: String(detail).slice(0, 500),
    });
    if (!ok) return { block: true, reason: "用户拒绝了该操作" };
  });
}
"#;

/// Write a session's runtime config: models, and the approval gate unless the
/// user opted out.
pub async fn write_runtime_config(
    agent_dir: &Path,
    models_json: &Value,
    auto_approve: bool,
) -> Result<(), String> {
    tokio::fs::create_dir_all(agent_dir)
        .await
        .map_err(|e| format!("无法创建运行时配置目录: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(agent_dir, std::fs::Permissions::from_mode(0o700)).await;
    }

    let models_path = agent_dir.join("models.json");
    let body = serde_json::to_string_pretty(models_json)
        .map_err(|e| format!("序列化运行时配置失败: {e}"))?;
    tokio::fs::write(&models_path, body)
        .await
        .map_err(|e| format!("写入运行时配置失败: {e}"))?;

    // The config carries a gateway credential, so keep it out of reach of
    // other local users on a shared machine.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ =
            tokio::fs::set_permissions(&models_path, std::fs::Permissions::from_mode(0o600)).await;
    }

    let ext_dir = agent_dir.join("extensions");
    let ext_path = ext_dir.join("yunova-approval.js");
    if auto_approve {
        // Explicitly remove it: a stale gate from a previous run would
        // silently contradict the user's current choice.
        let _ = tokio::fs::remove_file(&ext_path).await;
    } else {
        tokio::fs::create_dir_all(&ext_dir)
            .await
            .map_err(|e| format!("无法创建扩展目录: {e}"))?;
        tokio::fs::write(&ext_path, APPROVAL_EXTENSION)
            .await
            .map_err(|e| format!("写入审批扩展失败: {e}"))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "yunova-desktop-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    #[tokio::test]
    async fn the_approval_gate_is_installed_by_default() {
        // On a personal machine, asking is the safe default: prompts may come
        // from a phone or be influenced by content the agent read.
        let dir = temp_dir("gate");
        write_runtime_config(&dir, &json!({"providers":{}}), false)
            .await
            .unwrap();

        let ext = dir.join("extensions").join("yunova-approval.js");
        let body = tokio::fs::read_to_string(&ext).await.unwrap();
        assert!(body.contains("tool_call"));
        for tool in ["bash", "powershell", "write", "edit"] {
            assert!(body.contains(tool), "{tool} must be gated");
        }
        assert!(body.contains("block: true"));

        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

    #[tokio::test]
    async fn opting_into_auto_approve_removes_a_stale_gate() {
        // Leaving a previous run's gate in place would silently contradict the
        // user's current choice.
        let dir = temp_dir("auto");
        write_runtime_config(&dir, &json!({"providers":{}}), false)
            .await
            .unwrap();
        let ext = dir.join("extensions").join("yunova-approval.js");
        assert!(ext.exists());

        write_runtime_config(&dir, &json!({"providers":{}}), true)
            .await
            .unwrap();
        assert!(!ext.exists(), "auto-approve must not leave a gate behind");

        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

    #[tokio::test]
    async fn the_gateway_credential_is_written_private() {
        let dir = temp_dir("perm");
        write_runtime_config(
            &dir,
            &json!({"providers":{"yunova-claude":{"apiKey":"yna_tok"}}}),
            false,
        )
        .await
        .unwrap();

        let path = dir.join("models.json");
        let body = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(body.contains("yna_tok"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = tokio::fs::metadata(&path)
                .await
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "a quota-spending token must not be readable by other local users"
            );
        }

        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

    #[tokio::test]
    async fn sending_to_an_unknown_session_fails_rather_than_silently_dropping() {
        let m = RuntimeManager::new(Config::default());
        let err = m
            .send(1, &json!({"type":"abort"}))
            .await
            .expect_err("no runtime exists for this session");
        assert!(err.contains("没有正在运行"));
    }

    #[tokio::test]
    async fn starting_a_missing_program_reports_a_usable_error() {
        let dir = temp_dir("missing");
        let m = RuntimeManager::new(Config {
            workspace: dir.clone(),
            state_dir: dir.clone(),
            program: "yunova-no-such-runtime".into(),
            auto_approve: true,
        });
        let (tx, _rx) = mpsc::channel(4);
        let err = m
            .start(1, &json!({"providers":{}}), tx)
            .await
            .expect_err("a missing runtime must fail");
        assert!(err.contains("yunova-no-such-runtime"), "got: {err}");
        // And it must say what to do about it: "failed to spawn" sends the
        // user to a terminal, while naming the install step keeps the fix
        // inside the app.
        assert!(err.contains("安装"), "got: {err}");

        tokio::fs::remove_dir_all(&dir).await.ok();
    }

    #[tokio::test]
    async fn stopping_an_unknown_session_is_a_no_op() {
        // Teardown races with a runtime exiting on its own, so this must not
        // panic or block.
        let m = RuntimeManager::new(Config::default());
        m.stop(123).await;
        m.stop_all().await;
    }
}
