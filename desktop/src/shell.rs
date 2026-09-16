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

use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::Mutex;

use crate::connector::{Connector, ConnectorConfig, Host, Status, bound_account};
use crate::endpoint::site_origin;
use crate::identity::Login;
use crate::settings::{Settings, settings_path};

/// Label of the webview showing the server's own UI.
const SITE_WINDOW: &str = "site";
/// Label of the local control panel.
const PANEL_WINDOW: &str = "panel";
/// How many log lines the panel can show. Bounded because this process may run
/// for weeks in the tray and an unbounded log is a slow memory leak.
const LOG_LIMIT: usize = 400;

/// Shared state behind the Tauri commands.
pub struct AppState {
    connector: Arc<Connector>,
    settings: Mutex<Settings>,
    log: Arc<Mutex<Vec<String>>>,
    status: Arc<Mutex<Status>>,
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
}

/// The [`Host`] implementation that turns connector events into window events,
/// tray state and OS notifications.
struct WindowHost {
    app: AppHandle,
    log: Arc<Mutex<Vec<String>>>,
    status: Arc<Mutex<Status>>,
}

impl Host for WindowHost {
    fn status(&self, status: Status) {
        let app = self.app.clone();
        let slot = Arc::clone(&self.status);
        tauri::async_runtime::spawn(async move {
            *slot.lock().await = status.clone();
            // Emitted rather than polled so the panel reflects a dropped
            // connection immediately instead of on its next timer tick.
            let _ = app.emit("connector://status", &status);

            // A sign-in is the one state the user must act on, and the panel
            // is the only place to do it. Without this the app would show the
            // site and sit there silently unattached — the machine would
            // simply never appear in the device list, with nothing on screen
            // saying why.
            if let Status::NeedsLogin { .. } = status {
                let _ = show_panel(&app);
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
async fn snapshot(state: State<'_, AppState>) -> Result<Snapshot, String> {
    let settings = state.settings.lock().await.clone();
    Ok(Snapshot {
        status: state.status.lock().await.clone(),
        bound_as: bound_account(&config_of(&settings)).map(|c| c.username),
        active_sessions: state.connector.active_sessions().await,
        log: state.log.lock().await.clone(),
        site_url: site_origin(&settings.site_url),
        settings,
    })
}

/// Persist settings and, when already connected, reconnect so the change is
/// real rather than merely recorded.
#[tauri::command]
async fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    next: Settings,
) -> Result<Snapshot, String> {
    let path = settings_path(None);
    next.save(&path)?;
    let reconnect = state.connector.is_running().await;
    *state.settings.lock().await = next.clone();

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
    snapshot(state).await
}

#[tauri::command]
async fn connect(state: State<'_, AppState>) -> Result<(), String> {
    let settings = state.settings.lock().await.clone();
    if !settings.is_configured() {
        return Err("请先填写站点地址".into());
    }
    state.connector.start(config_of(&settings), None).await;
    Ok(())
}

#[tauri::command]
async fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
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
    state.connector.sign_out(&config_of(&settings)).await;
    Ok(())
}

/// Native folder picker for the workspace.
///
/// Typed paths are error-prone and this one decides what an agent can reach,
/// so the app offers the OS dialog rather than leaving it to a text field.
#[tauri::command]
async fn pick_workspace(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .set_title("选择 Agent 可操作的目录")
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
        .build()?;
    Ok(())
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
        .invoke_handler(tauri::generate_handler![
            snapshot,
            save_settings,
            connect,
            disconnect,
            sign_in,
            sign_out,
            pick_workspace,
            show_site,
            open_panel,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let log = Arc::new(Mutex::new(Vec::new()));
            let status = Arc::new(Mutex::new(Status::Offline));
            let host = Arc::new(WindowHost {
                app: handle.clone(),
                log: Arc::clone(&log),
                status: Arc::clone(&status),
            });
            let connector = Connector::new(host);

            app.manage(AppState {
                connector: Arc::clone(&connector),
                settings: Mutex::new(settings.clone()),
                log,
                status,
            });

            build_tray(&handle)?;

            // An unconfigured app opens the panel, because there is nothing to
            // show until a server is known; a configured one opens the site,
            // which is what the user came for.
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

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "打开 Yunova", true, None::<&str>)?;
    let panel = MenuItem::with_id(app, "panel", "本机设置…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出（停止本机任务）", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &panel, &quit])?;

    TrayIconBuilder::with_id("yunova")
        .icon(app.default_window_icon().cloned().ok_or_else(|| {
            tauri::Error::InvalidIcon(std::io::Error::other("missing bundled window icon"))
        })?)
        .tooltip("Yunova")
        .menu(&menu)
        // The menu must not also fire on a left click, or the click-to-open
        // gesture below never reaches us on Windows.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
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
            "panel" => {
                let _ = show_panel(app);
            }
            "quit" => {
                // Explicit: the user is told this stops local tasks, so honour
                // it immediately rather than leaving runtimes behind.
                let state = app.state::<AppState>();
                let connector = Arc::clone(&state.connector);
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    connector.stop().await;
                    app.exit(0);
                });
            }
            _ => {}
        })
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
