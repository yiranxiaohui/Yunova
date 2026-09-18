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
    /// Directory the agent may work in when a task names none. Never defaults
    /// to `$HOME`: a task driven from a phone should not be able to touch
    /// everything the user owns because nobody set a narrower scope.
    pub workspace: PathBuf,
    /// Every directory a task may name, `workspace` included.
    ///
    /// A task carries the directory the user picked for it, so this is what
    /// decides whether that pick is honoured. It lives here, on the machine at
    /// risk, because a server that could add to it would own the boundary the
    /// device target exists to keep local.
    pub workspace_roots: Vec<PathBuf>,
    /// Where per-session runtime config is written.
    pub state_dir: PathBuf,
    /// The `pi` executable.
    pub program: String,
    /// When true, every shell and write is auto-approved.
    ///
    /// Off by default. On a personal machine the safe default is to ask, and a
    /// user who wants to walk away opts in explicitly.
    pub auto_approve: bool,
    /// Where to send runtime diagnostics a person should see.
    ///
    /// A runtime's stderr is the only place that says *why* it could not
    /// start, and it used to go to the process's own stderr — invisible in a
    /// GUI app. Routing it to the host's log is what makes the app's own
    /// 「运行日志」 enough to diagnose a failure, instead of needing the user to
    /// relaunch the app from a terminal.
    pub reporter: Option<Reporter>,
}

/// Sink for human-readable runtime diagnostics.
pub type Reporter = Arc<dyn Fn(String) + Send + Sync>;

impl Default for Config {
    fn default() -> Self {
        Self {
            workspace: PathBuf::from("."),
            workspace_roots: Vec::new(),
            state_dir: PathBuf::from("."),
            program: "pi".into(),
            auto_approve: false,
            reporter: None,
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
    ///
    /// `workspace` is the directory the task was created with, as forwarded by
    /// the server. It is checked against the authorized roots here and the
    /// start fails if it is outside them: refusing is the only safe answer,
    /// because silently falling back to the default would run a task somewhere
    /// other than where its author is watching.
    pub async fn start(
        &self,
        session_id: i64,
        models_json: &Value,
        workspace: Option<&str>,
        to_server: mpsc::Sender<ToServer>,
    ) -> Result<(), String> {
        // A second runtime for one session would fork the transcript.
        if self.inner.lock().await.contains_key(&session_id) {
            return Ok(());
        }

        let cwd = resolve_workspace(
            &self.config.workspace,
            &self.config.workspace_roots,
            workspace,
        )?;

        let agent_dir = self.config.state_dir.join(format!("s{session_id}"));
        write_runtime_config(&agent_dir, models_json, self.config.auto_approve).await?;

        tokio::fs::create_dir_all(&cwd)
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
            .current_dir(&cwd)
            // pi is a JS entry point with a `node` shebang, so it is the
            // *child's* `PATH` that decides whether it can start at all. A GUI
            // app inherits almost nothing, so without this the process dies at
            // exec with `env: 'node': No such file or directory` and the task
            // just never produces a frame.
            .env("PATH", crate::runtime_install::augmented_path())
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

        // Diagnostics from stderr, kept for the failure report below.
        //
        // The reason a runtime could not start is written here and nowhere
        // else, so discarding it is what turns a one-line fix ("node is not on
        // the app's PATH") into an unexplained "运行时已退出" that repeats
        // forever. Bounded because a chatty runtime must not grow this without
        // limit over a long session.
        let diagnostics = Arc::new(Mutex::new(Vec::<String>::new()));

        // Draining stderr is not optional: a full pipe blocks the child, which
        // looks like an agent that silently stopped responding.
        if let Some(stderr) = stderr {
            let diagnostics = Arc::clone(&diagnostics);
            let reporter = self.config.reporter.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    eprintln!("[runtime {session_id}] {line}");
                    if let Some(reporter) = reporter.as_ref() {
                        reporter(format!("任务 {session_id} 运行时: {line}"));
                    }
                    let mut held = diagnostics.lock().await;
                    if held.len() == STDERR_KEPT_LINES {
                        held.remove(0);
                    }
                    held.push(line);
                }
            });
        }

        // stdout -> server. Strict LF framing, matching the RPC contract: a
        // generic line reader would also split on U+2028/U+2029, which are
        // legal inside JSON strings.
        let tx = to_server.clone();
        let manager = self.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::new();
            let mut framed = false;
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
                                framed = true;
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
            let status = manager.reap(session_id).await;

            // A runtime that never emitted a single frame did not "exit" in
            // any sense the user can act on — it failed to start. Reporting
            // both the same way is what left the app retrying a broken install
            // with no clue why, so this path carries the exit status and the
            // tail of stderr instead.
            if framed {
                let _ = tx
                    .send(ToServer::RuntimeClosed {
                        session_id,
                        reason: Some("运行时已退出".into()),
                    })
                    .await;
            } else {
                let detail = diagnostics.lock().await.join("; ");
                let _ = tx
                    .send(ToServer::RuntimeError {
                        session_id,
                        message: startup_failure(&manager.config.program, status, &detail),
                    })
                    .await;
            }
        });

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

    /// Drop a runtime and collect the exit status of its process.
    ///
    /// Waiting matters even though stdout already closed: the status is what
    /// distinguishes an exec failure (127) from a runtime that ran and chose
    /// to stop, and it is the only part of the story stderr does not tell.
    async fn reap(&self, session_id: i64) -> Option<std::process::ExitStatus> {
        let runtime = self.inner.lock().await.remove(&session_id)?;
        let mut guard = runtime.child.lock().await;
        let mut child = guard.take()?;
        match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
            Ok(Ok(status)) => Some(status),
            // Still alive with a closed stdout is not a state to leave behind.
            Err(_) => {
                let _ = child.kill().await;
                None
            }
            Ok(Err(_)) => None,
        }
    }
}

/// How many stderr lines to keep for a failure report.
const STDERR_KEPT_LINES: usize = 20;

/// Explain a runtime that exited before producing any output.
///
/// The wording points at the fix rather than the symptom. A missing `node` is
/// by far the most common cause — pi runs on it, and a GUI app does not
/// inherit the user's shell `PATH` — and telling someone "运行时已退出"
/// gives them nothing to act on.
fn startup_failure(
    program: &str,
    status: Option<std::process::ExitStatus>,
    stderr_tail: &str,
) -> String {
    let mut message = format!("运行时 {program} 启动后立即退出");
    if let Some(status) = status {
        match status.code() {
            Some(code) => message.push_str(&format!("（退出码 {code}）")),
            None => message.push_str("（被信号终止）"),
        }
    }
    if !stderr_tail.is_empty() {
        message.push_str(&format!("：{stderr_tail}"));
    }
    if stderr_tail.contains("node") || status.and_then(|s| s.code()) == Some(127) {
        message.push_str("。请确认本机已安装 Node.js，或在「本机设置」里重新安装 pi");
    }
    message
}

// ---------------------------------------------------------------------------
// workspace scope
// ---------------------------------------------------------------------------

/// Decide which directory a task may run in.
///
/// The one place the local boundary is enforced. A task carries the directory
/// the user picked for it, but that pick arrives over a socket from a server,
/// so it is treated as a request: honoured when it resolves inside a root the
/// user authorized on this machine, refused otherwise. `None` means the task
/// named nothing and gets `default`, which is what every task did before they
/// could choose.
///
/// Refusing rather than falling back matters. A task silently redirected to
/// another directory would report success while editing files nobody asked it
/// to touch.
pub fn resolve_workspace(
    default: &std::path::Path,
    roots: &[PathBuf],
    requested: Option<&str>,
) -> Result<PathBuf, String> {
    let Some(raw) = requested.map(str::trim).filter(|r| !r.is_empty()) else {
        return Ok(default.to_path_buf());
    };
    let asked = PathBuf::from(crate::settings::shellexpand(raw));
    // Checked before canonicalization, which resolves a relative path against
    // this process's current directory and would quietly accept one sent from
    // the network.
    if !asked.is_absolute() {
        return Err(format!("工作目录必须是绝对路径: {raw}"));
    }
    // Compared after resolving symlinks and `..`, so neither a crafted path
    // nor a link planted inside a root can aim the runtime outside it.
    let target = std::fs::canonicalize(&asked).map_err(|e| {
        // A missing directory is the common case and has a specific remedy,
        // so it does not get the raw OS wording: the app's own default
        // workspace does not exist until a task has run there, and "No such
        // file or directory (os error 2)" read like a bug rather than like
        // something the user could act on.
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("目录不存在: {}", asked.display())
        } else {
            format!("无法访问目录 {}: {e}", asked.display())
        }
    })?;
    if !target.is_dir() {
        return Err(format!("{} 不是目录", asked.display()));
    }
    let mut allowed: Vec<&PathBuf> = roots.iter().collect();
    let default = default.to_path_buf();
    if !allowed.contains(&&default) {
        allowed.push(&default);
    }
    for root in allowed {
        // A root that does not exist yet cannot contain anything, so it is
        // skipped rather than treated as a matching prefix.
        let Ok(root) = std::fs::canonicalize(root) else {
            continue;
        };
        if target == root || target.starts_with(&root) {
            return Ok(target);
        }
    }
    Err(format!(
        "{} 不在本机已授权的目录范围内，请先在「本机设置」里添加",
        asked.display()
    ))
}

/// One child directory, as offered to the web picker.
pub struct ChildDir {
    pub path: String,
    pub name: String,
    /// Whether it looks like a project, so the picker can hint at the
    /// directory the user probably meant.
    pub repo: bool,
}

/// What lives under `path`, for the task's directory picker.
///
/// Only ever called with a path the caller already ran through
/// [`resolve_workspace`], so this cannot be used to read outside the
/// authorized roots. Hidden entries and files are left out: the picker chooses
/// a working directory, and `.git` or `node_modules` is never that choice.
pub fn list_children(path: &std::path::Path) -> Result<Vec<ChildDir>, String> {
    let mut out = Vec::new();
    let reader =
        std::fs::read_dir(path).map_err(|e| format!("无法读取目录 {}: {e}", path.display()))?;
    for entry in reader.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        // `file_type` rather than `metadata`, then a follow-up check: a
        // symlinked project directory is a normal way to organise code and
        // should still be offered.
        let is_dir = match entry.file_type() {
            Ok(t) if t.is_dir() => true,
            Ok(t) if t.is_symlink() => entry.path().is_dir(),
            _ => false,
        };
        if !is_dir {
            continue;
        }
        let child = entry.path();
        let repo = child.join(".git").exists();
        out.push(ChildDir {
            path: child.to_string_lossy().into_owned(),
            name,
            repo,
        });
    }
    // Sorted here rather than in the browser: the listing is capped below, so
    // the order decides what survives the cap.
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    // A home directory with thousands of entries would make the picker
    // unusable and the response large; the user can still type a path.
    out.truncate(500);
    Ok(out)
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
            workspace_roots: vec![dir.clone()],
            state_dir: dir.clone(),
            program: "yunova-no-such-runtime".into(),
            auto_approve: true,
            reporter: None,
        });
        let (tx, _rx) = mpsc::channel(4);
        let err = m
            .start(1, &json!({"providers":{}}), None, tx)
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
    async fn a_task_outside_the_authorized_roots_is_refused_not_redirected() {
        // The whole point of per-task directories: the task carries a path
        // that arrived over a socket, so honouring it unchecked would hand the
        // choice of what the agent can rewrite to whoever can reach the
        // server. Refusing is also the only safe failure — silently running in
        // the default directory would report success while editing files
        // nobody asked it to touch.
        let base = temp_dir("scope");
        let allowed = base.join("allowed");
        let outside = base.join("outside");
        tokio::fs::create_dir_all(allowed.join("project"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(&outside).await.unwrap();

        let roots = vec![allowed.clone()];

        // Inside a root: honoured, and canonicalized so the runtime is given
        // a real directory rather than whatever the caller typed.
        let picked = resolve_workspace(
            &allowed,
            &roots,
            Some(&allowed.join("project").to_string_lossy()),
        )
        .expect("a directory inside a root must be allowed");
        assert_eq!(
            picked,
            std::fs::canonicalize(allowed.join("project")).unwrap()
        );

        // Outside every root: refused, with the reason the user can act on.
        let err = resolve_workspace(&allowed, &roots, Some(&outside.to_string_lossy()))
            .expect_err("a directory outside the roots must be refused");
        assert!(err.contains("本机已授权"), "got: {err}");

        // `..` must not walk out of a root: the check runs on the resolved
        // path, so a traversal cannot dress an outside directory up as an
        // inside one.
        let traversal = allowed
            .join("project")
            .join("..")
            .join("..")
            .join("outside");
        assert!(
            resolve_workspace(&allowed, &roots, Some(&traversal.to_string_lossy())).is_err(),
            "`..` must not escape the authorized roots"
        );

        // A relative path is refused rather than resolved against whatever
        // directory this process happens to be in.
        assert!(resolve_workspace(&allowed, &roots, Some("project")).is_err());

        // Naming nothing still means the default, which is what every task
        // created before this feature existed sends.
        assert_eq!(
            resolve_workspace(&allowed, &roots, None).unwrap(),
            allowed.clone()
        );

        tokio::fs::remove_dir_all(&base).await.ok();
    }

    #[tokio::test]
    async fn a_symlink_planted_in_a_root_cannot_aim_the_agent_outside_it() {
        // Prefix-matching the path as written would accept this: the link
        // lives under an authorized root, so only resolving it first reveals
        // that the target does not.
        let base = temp_dir("symlink");
        let allowed = base.join("allowed");
        let secret = base.join("secret");
        tokio::fs::create_dir_all(&allowed).await.unwrap();
        tokio::fs::create_dir_all(&secret).await.unwrap();

        #[cfg(unix)]
        {
            let link = allowed.join("escape");
            std::os::unix::fs::symlink(&secret, &link).unwrap();
            let err =
                resolve_workspace(&allowed, &[allowed.clone()], Some(&link.to_string_lossy()))
                    .expect_err("a symlink out of a root must be refused");
            assert!(err.contains("本机已授权"), "got: {err}");
        }

        tokio::fs::remove_dir_all(&base).await.ok();
    }

    #[tokio::test]
    async fn browsing_lists_only_directories_worth_picking() {
        // The picker chooses a working directory, so files and `.git` are
        // noise; a Git repository is flagged because it is almost always the
        // directory the user actually meant.
        let base = temp_dir("browse");
        tokio::fs::create_dir_all(base.join("repo").join(".git"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(base.join("plain")).await.unwrap();
        tokio::fs::create_dir_all(base.join(".hidden"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(base.join("node_modules"))
            .await
            .unwrap();
        tokio::fs::write(base.join("file.txt"), "x").await.unwrap();

        let children = list_children(&base).unwrap();
        let names: Vec<&str> = children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["plain", "repo"], "got: {names:?}");
        assert!(
            children.iter().find(|c| c.name == "repo").unwrap().repo,
            "a Git checkout must be marked so the picker can hint at it"
        );

        tokio::fs::remove_dir_all(&base).await.ok();
    }

    #[tokio::test]
    async fn a_runtime_that_never_starts_reports_why_instead_of_just_exiting() {
        // The failure this covers: pi runs through a `node` shebang, so on a
        // machine where the app cannot find node the process dies at exec.
        // Reporting that as `RuntimeClosed{运行时已退出}` is what left the UI
        // retrying forever with nothing to act on — a runtime that never
        // emitted a frame did not exit, it failed to start, and the exit code
        // plus stderr are the only evidence of that.
        let dir = temp_dir("earlyexit");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let fake = dir.join("fake-runtime");
        tokio::fs::write(
            &fake,
            "#!/bin/sh\necho 'env: node: No such file' >&2\nexit 127\n",
        )
        .await
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
                .await
                .unwrap();
        }

        let m = RuntimeManager::new(Config {
            workspace: dir.clone(),
            workspace_roots: vec![dir.clone()],
            state_dir: dir.clone(),
            program: fake.to_string_lossy().into_owned(),
            auto_approve: true,
            reporter: None,
        });
        let (tx, mut rx) = mpsc::channel(4);
        m.start(1, &json!({"providers":{}}), None, tx)
            .await
            .expect("spawning succeeds; the process fails afterwards");

        let msg = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
            .await
            .expect("a failing runtime must report, not hang")
            .expect("a message is sent");
        match msg {
            ToServer::RuntimeError {
                session_id,
                message,
            } => {
                assert_eq!(session_id, 1);
                assert!(message.contains("127"), "exit code is evidence: {message}");
                assert!(message.contains("node"), "stderr must survive: {message}");
                // And it must name the fix, not just the symptom.
                assert!(message.contains("Node.js"), "got: {message}");
            }
            other => panic!("expected a startup failure, got {other:?}"),
        }

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
