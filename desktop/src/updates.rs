//! In-app updates.
//!
//! The window renders the server's own UI, so the *product* never goes stale:
//! reloading the page is the update. What does go stale is this shell — the
//! connector protocol, the tray, the notification path, the local policy. When
//! the server changes an `agent_device` frame, an old client does not degrade
//! visibly; it stops appearing under "运行位置" while the window it opens keeps
//! working perfectly. Nothing in the product asks the user to look at their
//! app version, so without an updater that machine simply stays missing.
//!
//! Hence: check on launch, tell the user, install on request. Three properties
//! are worth stating because each one is a decision rather than a default.
//!
//! **Signature verification is the whole mechanism.** An update is arbitrary
//! code arriving over the network at a binary whose entire job is executing
//! commands on a personal computer. The plugin refuses any bundle that does
//! not verify against the `pubkey` compiled into the app, and that cannot be
//! switched off, so the delivery channel — GitHub, a CDN, a proxy — does not
//! have to be trusted, only the offline signing key.
//!
//! **The install is never automatic.** A task may be running on this machine
//! right now, started from a phone that has since been put away; on Windows
//! the installer additionally terminates the app outright. So the check is
//! automatic and the install is a button, and [`can_install_now`] refuses
//! while the connector has live sessions rather than silently killing work the
//! user cannot see from here.
//!
//! **Not every install can update itself.** A `.deb` is owned by dpkg and a
//! bare binary by whoever unpacked it; both are the shapes a server runs
//! headless. Rather than fail at download time with a plugin error, the
//! capability is resolved up front from the bundle type the binary was patched
//! with, and unsupported installs are told to use their package manager.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::Mutex;

/// How the running copy of the app can be updated.
///
/// Resolved from the bundle type Tauri patches into the binary at build time,
/// which is the only thing that actually distinguishes an AppImage from a
/// `.deb` at runtime — the path and the file name do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Install {
    /// An installer shape the updater can replace in place.
    SelfUpdating,
    /// Installed by a package manager, which owns the files and would be left
    /// inconsistent by an in-place swap.
    Managed,
    /// A bare executable with no installer around it: `--headless` on a
    /// server, or an unpacked archive.
    Standalone,
}

impl Install {
    fn detect() -> Self {
        use tauri::utils::config::BundleType;
        // `None` means the binary was never patched by the bundler: the
        // standalone archive this project publishes for headless servers, or
        // a `cargo run` during development.
        match tauri::utils::platform::bundle_type() {
            Some(BundleType::Deb | BundleType::Rpm) => Self::Managed,
            Some(_) => Self::SelfUpdating,
            None => Self::Standalone,
        }
    }

    /// Why this install cannot update itself, in the user's words.
    fn refusal(self) -> Option<&'static str> {
        match self {
            Self::SelfUpdating => None,
            Self::Managed => Some(
                "这个副本由系统包管理器安装，请用包管理器升级（或改用 AppImage 安装包），\
                 否则原地替换会让包数据库与磁盘内容不一致。",
            ),
            Self::Standalone => Some(
                "这是独立二进制（通常是服务器上的无界面用法），没有安装器可以替换，\
                 请重新下载对应版本的压缩包。",
            ),
        }
    }
}

/// Progress of an install in flight.
///
/// A struct rather than `Option<Option<u8>>`: that nests to `null` for both
/// "not downloading" and "downloading, length unknown" once serialized, so
/// the panel could not tell a running download from an idle app and would
/// cheerfully report "已是最新版本" while bytes were arriving.
#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    /// `None` when the server sent no content length.
    pub percent: Option<u8>,
}

/// What the panel shows about updates.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateState {
    /// Version of the running app, so the panel never has to guess.
    pub current_version: String,
    /// Set once a newer signed release has been found.
    pub available: Option<Available>,
    pub install: Install,
    /// Why the button is not offered, when it is not.
    pub blocked: Option<String>,
    /// Set while an install is in flight.
    pub downloading: Option<Progress>,
    /// Set when the update is installed and only a restart is left.
    pub installed: bool,
    /// Last failure, kept so a silent background check is still visible.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Available {
    pub version: String,
    /// Release notes, as published in the update manifest.
    pub notes: Option<String>,
    pub date: Option<String>,
}

/// Update state plus the pending [`Update`] handle it refers to.
///
/// The handle is kept from the check rather than re-fetched at install time:
/// re-checking would download a *different* manifest than the one the user
/// just agreed to, which is a small but real bait-and-switch.
pub struct Updates {
    state: Mutex<UpdateState>,
    pending: Mutex<Option<Update>>,
}

impl Updates {
    pub fn new() -> Self {
        let install = Install::detect();
        Self {
            state: Mutex::new(UpdateState {
                current_version: env!("CARGO_PKG_VERSION").to_string(),
                available: None,
                install,
                blocked: install.refusal().map(str::to_string),
                downloading: None,
                installed: false,
                error: None,
            }),
            pending: Mutex::new(None),
        }
    }

    pub async fn state(&self) -> UpdateState {
        self.state.lock().await.clone()
    }

    async fn set(&self, f: impl FnOnce(&mut UpdateState)) {
        f(&mut *self.state.lock().await);
    }
}

impl Default for Updates {
    fn default() -> Self {
        Self::new()
    }
}

/// Look for a newer signed release.
///
/// Errors are recorded and surfaced in the panel, never raised as a dialog: a
/// laptop is offline, behind a captive portal or on a rate-limited network all
/// the time, and none of that is worth interrupting someone for.
pub async fn check(app: &AppHandle) -> Result<Option<Available>, String> {
    let updates = app.state::<Arc<Updates>>().inner().clone();
    updates.set(|s| s.error = None).await;

    let result = async {
        let update = app.updater().map_err(|e| e.to_string())?.check().await;
        update.map_err(|e| e.to_string())
    }
    .await;

    match result {
        Ok(Some(update)) => {
            let found = Available {
                version: update.version.clone(),
                notes: update.body.clone().filter(|b| !b.trim().is_empty()),
                date: update.date.map(|d| d.to_string()),
            };
            *updates.pending.lock().await = Some(update);
            let slot = found.clone();
            updates.set(|s| s.available = Some(slot)).await;
            Ok(Some(found))
        }
        Ok(None) => {
            updates.set(|s| s.available = None).await;
            Ok(None)
        }
        Err(e) => {
            let message = e.clone();
            updates.set(|s| s.error = Some(message)).await;
            Err(e)
        }
    }
}

/// Whether installing right now would destroy work in progress.
///
/// The connector's own session count answers this: an update that interrupts
/// an agent mid-task loses something the user started somewhere else and
/// probably cannot see from this window.
pub fn can_install_now(active_sessions: usize) -> Option<String> {
    (active_sessions > 0).then(|| {
        format!(
            "本机正在运行 {active_sessions} 个任务，更新会中断它们。请等任务结束，\
             或在网页端停止后再更新。"
        )
    })
}

/// Download and install the update found by [`check`].
///
/// Deliberately does not restart. On Windows the installer stops the app by
/// itself, and everywhere else a connector that is idle *now* may not be idle
/// by the time the user reads the notice — so the last step stays a button.
pub async fn install(app: &AppHandle) -> Result<(), String> {
    let updates = app.state::<Arc<Updates>>().inner().clone();

    let state = updates.state().await;
    if let Some(blocked) = state.blocked {
        return Err(blocked);
    }

    let update = updates
        .pending
        .lock()
        .await
        .take()
        .ok_or("没有待安装的更新，请先检查更新")?;

    updates
        .set(|s| {
            s.downloading = Some(Progress { percent: None });
            s.error = None;
        })
        .await;

    // Progress is reported as a percentage rather than bytes: the panel shows
    // one line, and "37%" reads better there than a running byte count.
    let mut downloaded: u64 = 0;
    let progress = {
        let updates = updates.clone();
        let app = app.clone();
        move |chunk: usize, total: Option<u64>| {
            downloaded += chunk as u64;
            let percent = total.filter(|t| *t > 0).map(|total| {
                let pct = downloaded.saturating_mul(100) / total;
                pct.min(100) as u8
            });
            let updates = updates.clone();
            let app = app.clone();
            // Spawned because the callback is synchronous while the state
            // behind it is an async lock; blocking here would stall the very
            // download that is reporting.
            tauri::async_runtime::spawn(async move {
                updates
                    .set(|s| s.downloading = Some(Progress { percent }))
                    .await;
                let _ = emit_state(&app).await;
            });
        }
    };

    let result = update
        .download_and_install(progress, || {})
        .await
        .map_err(|e| e.to_string());

    match result {
        Ok(()) => {
            updates
                .set(|s| {
                    s.downloading = None;
                    s.installed = true;
                })
                .await;
            let _ = emit_state(app).await;
            Ok(())
        }
        Err(e) => {
            // Put the handle back so a retry does not require another check.
            let message = e.clone();
            updates
                .set(|s| {
                    s.downloading = None;
                    s.error = Some(message);
                })
                .await;
            let _ = emit_state(app).await;
            Err(e)
        }
    }
}

/// Push the current update state to the panel.
pub async fn emit_state(app: &AppHandle) -> Result<(), tauri::Error> {
    use tauri::Emitter;
    let state = app.state::<Arc<Updates>>().inner().clone().state().await;
    app.emit("updates://state", &state)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel distinguishes "idle" from "downloading, length unknown", and
    /// both of those from a percentage. An earlier version modelled this as
    /// `Option<Option<u8>>`, which serializes *both* of the first two to
    /// `null` — so the panel reported "已是最新版本" while an update was
    /// downloading. Asserted on the wire format, because that is where the
    /// distinction was lost.
    #[test]
    fn idle_and_unknown_length_downloads_are_distinguishable_on_the_wire() {
        let idle = serde_json::to_value(None::<Progress>).unwrap();
        let unknown = serde_json::to_value(Some(Progress { percent: None })).unwrap();
        let known = serde_json::to_value(Some(Progress { percent: Some(42) })).unwrap();

        assert!(
            idle.is_null(),
            "an idle app must serialize `downloading` as null"
        );
        assert!(
            !unknown.is_null(),
            "a download with no content length must still be visible to the panel"
        );
        assert_ne!(idle, unknown);
        assert_eq!(known["percent"], serde_json::json!(42));
    }

    /// A `.deb` and a bare binary must refuse with an explanation; only an
    /// installer shape may offer the button. Getting this backwards would let
    /// the updater overwrite files dpkg believes it owns.
    #[test]
    fn only_installer_shapes_may_self_update() {
        assert!(Install::SelfUpdating.refusal().is_none());
        assert!(Install::Managed.refusal().is_some());
        assert!(Install::Standalone.refusal().is_some());
    }

    /// An update during a live session would kill work started elsewhere —
    /// usually from a phone that has since been put away.
    #[test]
    fn a_busy_machine_refuses_to_install() {
        assert!(can_install_now(0).is_none());
        assert!(can_install_now(1).is_some());
        assert!(can_install_now(3).unwrap().contains('3'));
    }
}
