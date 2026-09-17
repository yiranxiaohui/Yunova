//! The window shell.
//!
//! What makes this an *app* rather than a program the user babysits in a
//! terminal: it shows the site in a webview and keeps the connector running
//! behind it, so signing in, starting a task and watching it run all happen in
//! one window — the same shape as any other desktop chat client.
//!
//! The split is worth stating, because it is the whole design. The webview
//! renders the server's own React bundle, so the desktop app never reimplements
//! the product and never lags a release behind it. Around that sits the part a
//! browser genuinely cannot do: hold a connector open, supervise local
//! runtimes, notify when an agent is blocked, and stay alive in the tray after
//! the window is closed.
//!
//! Two constraints shape the code:
//!
//! * The remote page is **not** trusted with local control. It is loaded in
//!   its own webview with no Tauri IPC, so a compromised or hostile server
//!   cannot call into this process to widen the workspace or switch approval
//!   off. Local control lives in a separate, locally-served panel.
//! * Closing the window does not stop the connector, because a task may be
//!   running on this machine. The tray reflects that, and quitting is explicit.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::Mutex;

use crate::connector::{Connector, ConnectorConfig, Host, Status, bound_account};
use crate::endpoint::{SESSION_COOKIE, site_origin};
use crate::identity::Login;
use crate::runtime_install::{self, RuntimeStatus};
use crate::settings::{Settings, settings_path};
use crate::updates::{self, UpdateState, Updates};

/// Label of the webview showing the server's own UI.
const SITE_WINDOW: &str = "site";
/// Label of the local control panel.
const PANEL_WINDOW: &str = "panel";
/// Marker injected into the site window so the page can tell it is running
/// inside this client.
///
/// The page needs to know, or it keeps advertising the download of an app the
/// user is already looking at. It cannot ask: the site webview is named in no
/// capability, so every IPC call from it is refused — which is the point. A
/// plain global is the right shape for that boundary, because it carries one
/// bit of information and grants nothing.
///
/// Kept as a named constant because `web/src/lib/platform.ts` reads the same
/// name, and a test asserts the two agree.
const DESKTOP_MARKER: &str = "__YUNOVA_DESKTOP__";
/// How many log lines the panel can show. Bounded because this process may run
/// for weeks in the tray and an unbounded log is a slow memory leak.
const LOG_LIMIT: usize = 400;

/// Shared state behind the Tauri commands.
pub struct AppState {
    connector: Arc<Connector>,
    settings: Mutex<Settings>,
    log: Arc<Mutex<Vec<String>>>,
    status: Arc<Mutex<Status>>,
    /// Where the site webview points, so the cookie lookup does not have to
    /// take the settings lock from a synchronous trait method.
    site: Arc<std::sync::RwLock<String>>,
    /// Set when the user disconnected on purpose. Without it the watcher that
    /// makes attaching automatic would immediately undo an explicit
    /// "断开", which is the sort of fight the user always loses.
    paused: Arc<AtomicBool>,
}

/// What the panel renders in one request, so it never has to stitch together
/// several calls to answer "what is going on".
#[derive(Debug, Serialize)]
pub struct Snapshot {
    status: Status,
    settings: Settings,
    /// Account this machine is already bound as, when it has a stored token.
    bound_as: Option<String>,
    active_sessions: usize,
    log: Vec<String>,
    site_url: String,
    /// Whether the agent runtime is present, and where. The app can execute
    /// tasks locally only if this exists, so it belongs in the same snapshot
    /// as the connection state rather than behind a second call the panel
    /// might forget to make.
    runtime: RuntimeStatus,
    /// Update availability and progress, so the panel can offer the button
    /// without a second round trip.
    updates: UpdateState,
}

/// The [`Host`] implementation that turns connector events into window events,
/// tray state and OS notifications.
struct WindowHost {
    app: AppHandle,
    log: Arc<Mutex<Vec<String>>>,
    status: Arc<Mutex<Status>>,
    site: Arc<std::sync::RwLock<String>>,
    paused: Arc<AtomicBool>,
}

impl Host for WindowHost {
    fn status(&self, status: Status) {
        let app = self.app.clone();
        let slot = Arc::clone(&self.status);
        let paused = Arc::clone(&self.paused);
        tauri::async_runtime::spawn(async move {
            *slot.lock().await = status.clone();
            // Emitted rather than polled so the panel reflects a dropped
            // connection immediately instead of on its next timer tick.
            let _ = app.emit("connector://status", &status);

            // A sign-in is the one state the user must act on — and the place
            // to do it is the site itself, because signing in there is what
            // binds this machine now. Opening the local panel instead would
            // ask for the same password a second time in a window that looks
            // like a settings dialog.
            // Not while paused, though: "解除绑定" and "断开" both end in this
            // state on purpose, and yanking the user out of the panel they
            // just clicked in would read as the app fighting them.
            if let Status::NeedsLogin { .. } = status
                && !paused.load(Ordering::SeqCst)
            {
                let settings = app.state::<AppState>().settings.lock().await.clone();
                let _ = open_site(&app, &settings);
            }
        });
    }

    fn log(&self, line: String) {
        // Still on stderr: when something goes wrong badly enough that the
        // window will not open, the terminal is the only place left to look.
        eprintln!("[device] {line}");
        let stamped = format!("{} {line}", now_hhmmss());
        let app = self.app.clone();
        let log = Arc::clone(&self.log);
        tauri::async_runtime::spawn(async move {
            {
                let mut guard = log.lock().await;
                guard.push(stamped.clone());
                let overflow = guard.len().saturating_sub(LOG_LIMIT);
                if overflow > 0 {
                    guard.drain(..overflow);
                }
            }
            let _ = app.emit("connector://log", &stamped);
        });
    }

    /// The site's session cookie, read out of the app's own site webview.
    ///
    /// This is what makes the client attach by itself: the user signs in to
    /// the page once — which they have to do anyway to use the product — and
    /// the connector proves the machine with that session instead of asking
    /// for the password again in a second window. The remote page never gets
    /// to *send* anything here; the app reads the cookie from its own webview,
    /// so this adds no IPC surface for a server to reach.
    fn session_token(&self) -> Option<String> {
        let origin = self.site.read().ok()?.clone();
        site_session(&self.app, &origin)
    }

    fn attention(&self, title: String, body: String) {
        // The reason this app exists as a window rather than a daemon: an
        // agent blocked on approval has to be able to reach a user who is
        // doing something else.
        let _ = self
            .app
            .notification()
            .builder()
            .title(&title)
            .body(&body)
            .show();
        let _ = self.app.emit("connector://attention", &body);
        if let Some(win) = self.app.get_webview_window(SITE_WINDOW) {
            // Not focused: stealing focus from whatever the user is typing in
            // is worse than a badge they notice a second later.
            let _ = win.set_focus();
        }
    }
}

fn now_hhmmss() -> String {
    // A wall clock without pulling in a date library: the log only needs to
    // answer "how long ago", and the process may run for weeks so a
    // monotonic-since-start counter would read as nonsense.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let day = secs % 86_400;
    format!("{:02}:{:02}:{:02}", day / 3600, (day % 3600) / 60, day % 60)
}

// ---------------------------------------------------------------------------
// commands
// ---------------------------------------------------------------------------

#[tauri::command]
async fn snapshot(
    state: State<'_, AppState>,
    updates: State<'_, Arc<Updates>>,
) -> Result<Snapshot, String> {
    let settings = state.settings.lock().await.clone();
    Ok(Snapshot {
        status: state.status.lock().await.clone(),
        bound_as: bound_account(&config_of(&settings)).map(|c| c.username),
        active_sessions: state.connector.active_sessions().await,
        log: state.log.lock().await.clone(),
        site_url: site_origin(&settings.site_url),
        runtime: runtime_install::status(&settings.program).await,
        updates: updates.state().await,
        settings,
    })
}

/// Install or update the agent runtime with the user's chosen package manager.
///
/// The app can do this because it is a desktop app: the alternative is telling
/// someone who installed a GUI application to open a terminal and run an npm
/// command, which is where most people stop.
#[tauri::command]
async fn install_runtime(
    app: AppHandle,
    state: State<'_, AppState>,
    manager: String,
) -> Result<RuntimeStatus, String> {
    let _ = app.emit("connector://log", &format!("正在用 {manager} 安装运行时…"));
    let output = runtime_install::install(&manager).await?;
    if !output.is_empty() {
        // The installer's own output is the only useful record of what it did,
        // and it is what makes a later failure diagnosable.
        for line in output
            .lines()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            let _ = app.emit("connector://log", &line.to_string());
        }
    }
    let settings = state.settings.lock().await.clone();
    let status = runtime_install::status(&settings.program).await;
    let _ = app.emit(
        "connector://log",
        &match status.version.as_deref() {
            Some(v) => format!("运行时已就绪：{v}"),
            None => "安装已结束，但仍未找到可用的运行时".to_string(),
        },
    );
    Ok(status)
}

/// Re-check the runtime without installing anything.
///
/// Needed because the user may install it themselves in a terminal while the
/// app is open, and an app that only looks once would keep claiming it is
/// missing.
#[tauri::command]
async fn check_runtime(state: State<'_, AppState>) -> Result<RuntimeStatus, String> {
    let settings = state.settings.lock().await.clone();
    Ok(runtime_install::status(&settings.program).await)
}

/// Look for a newer signed release, on request.
///
/// Separate from the launch check so the user has a way to ask again after
/// fixing whatever made the first attempt fail.
#[tauri::command]
async fn check_update(app: AppHandle) -> Result<UpdateState, String> {
    let result = updates::check(&app).await;
    let state = app.state::<Arc<Updates>>().inner().clone().state().await;
    match result {
        Ok(_) => Ok(state),
        Err(e) => Err(e),
    }
}

/// Download and install the pending update.
///
/// Refuses while the connector has live sessions: the point of this app is
/// that a task started on a phone runs here, and replacing the binary
/// underneath one would lose work nobody is watching.
#[tauri::command]
async fn install_update(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if let Some(busy) = updates::can_install_now(state.connector.active_sessions().await) {
        return Err(busy);
    }
    updates::install(&app).await
}

/// Restart into the version that was just installed.
#[tauri::command]
async fn restart_app(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if let Some(busy) = updates::can_install_now(state.connector.active_sessions().await) {
        return Err(busy);
    }
    // Stop local runtimes first: `restart` replaces this process, and the
    // `RunEvent::Exit` handler that normally cleans them up does not run.
    state.connector.stop().await;
    app.restart();
}

/// Persist settings and, when already connected, reconnect so the change is
/// real rather than merely recorded.
#[tauri::command]
async fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    updates: State<'_, Arc<Updates>>,
    next: Settings,
) -> Result<Snapshot, String> {
    let path = settings_path(None);
    next.save(&path)?;
    // `save` fills a cleared address back in, so read back what was stored
    // rather than trusting the form: otherwise the panel and the connector
    // would disagree about which site this is.
    let next = Settings::load(&path);
    let reconnect = state.connector.is_running().await;
    *state.settings.lock().await = next.clone();
    if let Ok(mut slot) = state.site.write() {
        *slot = site_origin(&next.site_url);
    }

    if reconnect && next.is_configured() {
        state.connector.start(config_of(&next), None).await;
    }
    // The site webview points at whatever server was configured when it was
    // created, so a changed address has to be reloaded or the user keeps
    // looking at the old one. The URL is JSON-encoded rather than
    // interpolated: it is a value the user typed into a text field, and a
    // stray quote would otherwise end the string literal and run as script in
    // the window that is deliberately denied IPC.
    if let Some(win) = app.get_webview_window(SITE_WINDOW) {
        let target = site_origin(&next.site_url);
        let encoded = serde_json::to_string(&target).unwrap_or_else(|_| "\"\"".into());
        let _ = win.eval(format!("window.location.replace({encoded})"));
    }
    snapshot(state, updates).await
}

#[tauri::command]
async fn connect(state: State<'_, AppState>) -> Result<(), String> {
    let settings = state.settings.lock().await.clone();
    if !settings.is_configured() {
        return Err("请先填写站点地址".into());
    }
    state.paused.store(false, Ordering::SeqCst);
    state.connector.start(config_of(&settings), None).await;
    Ok(())
}

#[tauri::command]
async fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
    // Remembered, so the watcher that attaches automatically does not
    // reconnect a second later and make the button look broken.
    state.paused.store(true, Ordering::SeqCst);
    state.connector.stop().await;
    Ok(())
}

/// Bind this machine by signing in.
///
/// The password is handed straight to the connector and never stored: what
/// lands on disk is a device-scoped token, so the credential on this computer
/// can be revoked to one machine instead of standing in for the account.
#[tauri::command]
async fn sign_in(
    state: State<'_, AppState>,
    username: String,
    password: String,
) -> Result<(), String> {
    if username.trim().is_empty() || password.is_empty() {
        return Err("账号和密码都不能为空".into());
    }
    let settings = state.settings.lock().await.clone();
    if !settings.is_configured() {
        return Err("请先填写站点地址".into());
    }
    state.paused.store(false, Ordering::SeqCst);
    state
        .connector
        .start(
            config_of(&settings),
            Some(Login {
                username: username.trim().to_string(),
                password,
            }),
        )
        .await;
    Ok(())
}

#[tauri::command]
async fn sign_out(state: State<'_, AppState>) -> Result<(), String> {
    let settings = state.settings.lock().await.clone();
    // Unbinding must stick: the site window is probably still signed in, and
    // re-attaching from that session would undo the click instantly.
    state.paused.store(true, Ordering::SeqCst);
    state.connector.sign_out(&config_of(&settings)).await;
    Ok(())
}

/// Native folder picker for the workspace.
///
/// Typed paths are error-prone and this one decides what an agent can reach,
/// so the app offers the OS dialog rather than leaving it to a text field.
#[tauri::command]
async fn pick_workspace(app: AppHandle) -> Result<Option<String>, String> {
    pick_folder(app, "选择 Agent 可操作的目录").await
}

/// Native folder picker for an additional allowed directory.
///
/// Separate command rather than a parameter, so the dialog can say what the
/// choice means: adding a root widens what tasks may pick, and that is a
/// different decision from changing the default.
#[tauri::command]
async fn pick_extra_workspace(app: AppHandle) -> Result<Option<String>, String> {
    pick_folder(app, "添加任务可选的项目目录").await
}

async fn pick_folder(app: AppHandle, title: &str) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .set_title(title)
        .pick_folder(move |picked| {
            let _ = tx.send(picked);
        });
    let picked = rx.recv().map_err(|e| e.to_string())?;
    Ok(picked.map(|p| p.to_string()))
}

/// Bring the site window forward, creating it if the user closed it.
#[tauri::command]
async fn show_site(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let settings = state.settings.lock().await.clone();
    open_site(&app, &settings).map_err(|e| e.to_string())
}

#[tauri::command]
async fn open_panel(app: AppHandle) -> Result<(), String> {
    show_panel(&app).map_err(|e| e.to_string())
}

fn config_of(settings: &Settings) -> ConnectorConfig {
    ConnectorConfig {
        site_url: settings.site_url.clone(),
        name: settings.name.clone(),
        workspace: settings.workspace_path(),
        workspace_roots: settings.workspace_roots(),
        state_dir: settings.state_dir(),
        program: settings.program.clone(),
        auto_approve: settings.auto_approve,
        config_dir: None,
    }
}

// ---------------------------------------------------------------------------
// windows
// ---------------------------------------------------------------------------

/// Show the server's own UI.
///
/// Loaded as a remote URL in a webview that is granted no Tauri IPC. That is
/// the one non-negotiable boundary here: the page comes from a server, and a
/// page from a server must not be able to reconfigure the machine it is
/// displayed on.
///
/// The only thing handed to it is [`DESKTOP_MARKER`], a read-only boolean that
/// lets the page drop the "install the desktop app" affordances. Read-only so
/// the page cannot unset it for its own scripts, and a boolean so it reveals
/// nothing about this machine.
fn open_site(app: &AppHandle, settings: &Settings) -> tauri::Result<()> {
    if let Some(win) = app.get_webview_window(SITE_WINDOW) {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        return Ok(());
    }
    let url = site_origin(&settings.site_url);
    let parsed = url.parse().map_err(|_| tauri::Error::WebviewNotFound)?;
    WebviewWindowBuilder::new(app, SITE_WINDOW, WebviewUrl::External(parsed))
        .title("Yunova")
        .inner_size(1180.0, 800.0)
        .min_inner_size(900.0, 600.0)
        .center()
        .initialization_script(format!(
            "Object.defineProperty(window, '{DESKTOP_MARKER}', {{ value: true }});"
        ))
        .build()?;
    Ok(())
}

/// Navigate the site window to a path on the configured origin.
///
/// How the app's own menu drives the product without reimplementing it: the
/// page is the server's, so "new chat" is a navigation rather than a feature
/// this client has to own and keep in step with releases.
///
/// The URL is JSON-encoded because it is built from a user-supplied origin; a
/// stray quote would otherwise terminate the string literal and execute as
/// script in the window that is deliberately denied IPC.
fn navigate_site(app: &AppHandle, path: &str) {
    let app = app.clone();
    let path = path.to_string();
    tauri::async_runtime::spawn(async move {
        let settings = app.state::<AppState>().settings.lock().await.clone();
        if open_site(&app, &settings).is_err() {
            return;
        }
        if let Some(win) = app.get_webview_window(SITE_WINDOW) {
            let target = format!("{}{path}", site_origin(&settings.site_url));
            let encoded = serde_json::to_string(&target).unwrap_or_else(|_| "\"\"".into());
            let _ = win.eval(format!("window.location.assign({encoded})"));
        }
    });
}

/// The application menu.
///
/// A desktop app is expected to have one: it is where the keyboard shortcuts
/// are discovered, and on macOS its absence is what makes a window feel like a
/// web page in a frame rather than an installed application. The entries stay
/// deliberately thin — they open windows and navigate the site — because the
/// product itself lives in the page and duplicating its features here would
/// mean maintaining two versions of each.
fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let new_chat = MenuItem::with_id(app, "new-chat", "新建对话", true, Some("CmdOrCtrl+N"))?;
    let new_task = MenuItem::with_id(
        app,
        "new-task",
        "新建工作任务",
        true,
        Some("CmdOrCtrl+Shift+N"),
    )?;
    let home = MenuItem::with_id(app, "home", "回到首页", true, Some("CmdOrCtrl+0"))?;
    let panel = MenuItem::with_id(app, "panel", "本机设置…", true, Some("CmdOrCtrl+,"))?;
    let reload = MenuItem::with_id(app, "reload", "重新载入", true, Some("CmdOrCtrl+R"))?;
    let quit = MenuItem::with_id(
        app,
        "quit",
        "退出（停止本机任务）",
        true,
        Some("CmdOrCtrl+Q"),
    )?;

    let file = Submenu::with_items(
        app,
        "Yunova",
        true,
        &[
            &new_chat,
            &new_task,
            &home,
            &PredefinedMenuItem::separator(app)?,
            &panel,
            &reload,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    // Copy/paste must be present as real menu items, not just as key handling:
    // on macOS the system only delivers the standard editing shortcuts to a
    // webview when the menu declares them, so without this block the user
    // cannot paste into the page at all.
    let edit = Submenu::with_items(
        app,
        "编辑",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    let window = Submenu::with_items(
        app,
        "窗口",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::fullscreen(app, None)?,
        ],
    )?;

    Menu::with_items(app, &[&file, &edit, &window])
}

/// Route a menu click. Shared with the tray so one id means one behaviour.
fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "new-chat" => navigate_site(app, "/"),
        "new-task" => navigate_site(app, "/t"),
        "home" => navigate_site(app, "/"),
        "panel" => {
            let _ = show_panel(app);
        }
        "reload" => {
            if let Some(win) = app.get_webview_window(SITE_WINDOW) {
                let _ = win.eval("window.location.reload()");
            }
        }
        "open" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let settings = app.state::<AppState>().settings.lock().await.clone();
                let _ = if settings.is_configured() {
                    open_site(&app, &settings)
                } else {
                    show_panel(&app)
                };
            });
        }
        "quit" => {
            // Explicit: the user is told this stops local tasks, so honour it
            // immediately rather than leaving runtimes behind.
            let state = app.state::<AppState>();
            let connector = Arc::clone(&state.connector);
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                connector.stop().await;
                app.exit(0);
            });
        }
        _ => {}
    }
}

/// Show the local control panel: the part that is genuinely this app's, and
/// the only surface allowed to touch local settings.
fn show_panel(app: &AppHandle) -> tauri::Result<()> {
    if let Some(win) = app.get_webview_window(PANEL_WINDOW) {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        return Ok(());
    }
    WebviewWindowBuilder::new(app, PANEL_WINDOW, WebviewUrl::App("index.html".into()))
        .title("Yunova 本机设置")
        .inner_size(560.0, 720.0)
        .min_inner_size(460.0, 560.0)
        .resizable(true)
        .build()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// entry point
// ---------------------------------------------------------------------------

pub fn run() {
    let settings = Settings::load(&settings_path(None));

    let mut builder = tauri::Builder::default();

    // Only when there is a session bus to claim a name on. The plugin panics
    // on a machine without one (a login over SSH, a container, a minimal
    // desktop), and refusing to start at all is a far worse outcome than the
    // small risk this guards against — the app is still usable, it just
    // cannot detect a second copy of itself.
    if has_single_instance_support() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // A second connector for the same machine would fight the first
            // over session ids, so the running instance is surfaced instead.
            let app = app.clone();
            let _ = show_panel(&app);
            if let Some(win) = app.get_webview_window(SITE_WINDOW) {
                let _ = win.set_focus();
            }
        }));
    }

    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            snapshot,
            save_settings,
            connect,
            disconnect,
            sign_in,
            sign_out,
            pick_workspace,
            pick_extra_workspace,
            show_site,
            open_panel,
            install_runtime,
            check_runtime,
            check_update,
            install_update,
            restart_app,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let log = Arc::new(Mutex::new(Vec::new()));
            let status = Arc::new(Mutex::new(Status::Offline));
            let site = Arc::new(std::sync::RwLock::new(site_origin(&settings.site_url)));
            let paused = Arc::new(AtomicBool::new(false));
            let host = Arc::new(WindowHost {
                app: handle.clone(),
                log: Arc::clone(&log),
                status: Arc::clone(&status),
                site: Arc::clone(&site),
                paused: Arc::clone(&paused),
            });
            let connector = Connector::new(host);

            app.manage(AppState {
                connector: Arc::clone(&connector),
                settings: Mutex::new(settings.clone()),
                log,
                status,
                site,
                paused: Arc::clone(&paused),
            });
            app.manage(Arc::new(Updates::new()));

            build_tray(&handle)?;

            // The menu is what makes this an application rather than a page in
            // a frame: it is where the shortcuts are discoverable, and on
            // macOS the standard editing shortcuts only reach the webview when
            // a menu declares them.
            let menu = build_menu(&handle)?;
            app.set_menu(menu)?;
            app.on_menu_event(|app, event| on_menu(app, event.id().as_ref()));

            // The site opens first and the connector follows, with no address
            // to type and no button to press: the app knows its own site, and
            // signing in to that page is what binds this machine. Only a
            // build with no address at all (a self-hosted one that cleared
            // the default) has anything to ask, and only then is the panel the
            // first thing the user sees.
            if settings.is_configured() {
                open_site(&handle, &settings)?;
                if settings.connect_on_launch {
                    let cfg = config_of(&settings);
                    tauri::async_runtime::spawn(async move {
                        connector.start(cfg, None).await;
                    });
                }
            } else {
                show_panel(&handle)?;
            }

            // Runs regardless of how the first connection went, because the
            // credential it is waiting for does not exist yet on a first run:
            // the user is about to create it by signing in to the page.
            let watcher = handle.clone();
            tauri::async_runtime::spawn(watch_site_login(watcher, paused));

            // Checked once at launch, never installed on its own. An outdated
            // shell fails silently — the window keeps working while this
            // machine stops showing up as a run target — so something has to
            // ask; but a client that replaced its own binary while an agent
            // was working would be worse than a stale one.
            let updater = handle.clone();
            tauri::async_runtime::spawn(async move {
                // A moment after launch, so the first connection attempt and
                // the window get the network to themselves.
                tokio::time::sleep(Duration::from_secs(5)).await;
                match updates::check(&updater).await {
                    Ok(Some(found)) => {
                        eprintln!("[update] {} is available", found.version);
                    }
                    Ok(None) => {}
                    // Offline, captive portal, rate limit: recorded in the
                    // panel, not raised at the user.
                    Err(e) => eprintln!("[update] check failed: {e}"),
                }
                let _ = updates::emit_state(&updater).await;
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the Yunova desktop app")
        .run(|app, event| match event {
            // Closing the last window must not quit: a task may still be
            // running on this machine, and killing it because a window was
            // closed would lose work the user started elsewhere.
            RunEvent::ExitRequested { api, code, .. } if code.is_none() => {
                api.prevent_exit();
            }
            RunEvent::Exit => {
                // Stop local runtimes rather than orphaning them.
                let state = app.state::<AppState>();
                let connector = Arc::clone(&state.connector);
                tauri::async_runtime::block_on(async move {
                    connector.stop().await;
                });
            }
            _ => {}
        });
}

/// Whether a session bus exists for the single-instance guard to use.
///
/// Checked rather than assumed because the plugin unwraps its connection: on a
/// machine with no session bus it aborts the process at startup, so the app
/// would fail to open on exactly the setups where a user is least able to
/// diagnose it.
#[cfg(target_os = "linux")]
fn has_single_instance_support() -> bool {
    match std::env::var("DBUS_SESSION_BUS_ADDRESS") {
        Ok(v) => !v.trim().is_empty() && v != "disabled:" && !v.starts_with("disabled"),
        // Unset is normal on a desktop session; the bus is then discovered
        // through the usual socket, so this is not evidence of absence.
        Err(_) => std::path::Path::new("/run/user")
            .join(current_uid().to_string())
            .join("bus")
            .exists(),
    }
}

#[cfg(target_os = "linux")]
fn current_uid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}

#[cfg(not(target_os = "linux"))]
fn has_single_instance_support() -> bool {
    // Windows uses a named mutex and macOS an Apple event; neither can fail
    // the way the D-Bus path does.
    true
}

/// The site's session cookie, as held by the app's own site webview.
///
/// Reading it here rather than accepting it from the page keeps the boundary
/// intact: the remote document still has no IPC and cannot hand anything to
/// this process. What it can do — be signed in — is exactly what the server
/// will accept as proof that this machine belongs to that account.
///
/// Blocks the calling thread until the webview thread answers, which is why
/// every caller here is a spawned task: from the main thread, or from inside a
/// command or event handler, that wait is the deadlock Tauri documents.
fn site_session(app: &AppHandle, origin: &str) -> Option<String> {
    let win = app.get_webview_window(SITE_WINDOW)?;
    // The cookie store is keyed by URL, so ask for the configured origin
    // rather than wherever the page happens to have navigated.
    let url = origin.parse().ok()?;
    let cookies = win.cookies_for_url(url).ok()?;
    cookies
        .into_iter()
        .find(|c| c.name() == SESSION_COOKIE)
        .map(|c| c.value().trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Re-attach once the user signs in to the site window.
///
/// The connector gives up when it has no credential to offer, which on a first
/// run is every moment before the user finishes signing in to the page. Left
/// there, the app would show a perfectly working site while the web UI
/// reported "本地电脑（离线）" until something restarted it — the exact symptom
/// this watcher exists to remove. It only ever *starts* a connection that
/// parked for lack of a credential, so an explicit 断开 stays honoured.
async fn watch_site_login(app: AppHandle, paused: Arc<AtomicBool>) {
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let state = app.state::<AppState>();
        if paused.load(Ordering::SeqCst) || state.connector.is_running().await {
            continue;
        }
        if !matches!(state.connector.status().await, Status::NeedsLogin { .. }) {
            continue;
        }
        // Only retry when there is now something new to try with: a stored
        // token, or a session in the window that was not there before.
        let settings = state.settings.lock().await.clone();
        if !settings.is_configured() {
            continue;
        }
        let cfg = config_of(&settings);
        let origin = match state.site.read() {
            Ok(slot) => slot.clone(),
            Err(_) => continue,
        };
        if bound_account(&cfg).is_some() || site_session(&app, &origin).is_some() {
            state.connector.start(cfg, None).await;
        }
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "打开 Yunova", true, None::<&str>)?;
    let new_task = MenuItem::with_id(app, "new-task", "新建工作任务", true, None::<&str>)?;
    let panel = MenuItem::with_id(app, "panel", "本机设置…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出（停止本机任务）", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &new_task, &panel, &quit])?;

    TrayIconBuilder::with_id("yunova")
        .icon(app.default_window_icon().cloned().ok_or_else(|| {
            tauri::Error::InvalidIcon(std::io::Error::other("missing bundled window icon"))
        })?)
        .tooltip("Yunova")
        .menu(&menu)
        // The menu must not also fire on a left click, or the click-to-open
        // gesture below never reaches us on Windows.
        .show_menu_on_left_click(false)
        // Same handler as the application menu, so one id cannot come to mean
        // two different things depending on where it was clicked.
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button, .. } = event
                && button == tauri::tray::MouseButton::Left
            {
                let app = tray.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    let settings = app.state::<AppState>().settings.lock().await.clone();
                    let _ = if settings.is_configured() {
                        open_site(&app, &settings)
                    } else {
                        show_panel(&app)
                    };
                });
            }
        })
        .build(app)?;
    Ok(())
}
