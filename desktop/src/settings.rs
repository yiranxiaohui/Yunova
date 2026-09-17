//! Persisted window-shell settings.
//!
//! The headless client is configured by environment variables, which is right
//! for a service unit and wrong for an app: a user who double-clicks an icon
//! has no environment to set. So the window keeps its own settings file and
//! still honours the environment when it is present, which keeps one binary
//! usable both ways.
//!
//! What is *not* stored here is the credential. That lives in its own
//! per-server file with owner-only permissions, because a settings file is
//! something a user copies between machines and a device token must not travel
//! with it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::endpoint::{default_site_url, hostname};
use crate::runtime_env;

/// User-visible configuration of the local execution target.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    /// Site address, e.g. `https://chat.yunnet.top`. Defaults to the address
    /// this client was built for, so a fresh install connects on its own.
    #[serde(default = "default_site")]
    pub site_url: String,
    /// Name shown in the device list.
    #[serde(default = "hostname")]
    pub name: String,
    /// The directory the agent may work in.
    #[serde(default)]
    pub workspace: String,
    /// Whether tool calls run without asking.
    #[serde(default)]
    pub auto_approve: bool,
    /// Whether the connector starts as soon as the app does.
    #[serde(default = "yes")]
    pub connect_on_launch: bool,
    /// The `pi` executable.
    #[serde(default = "default_program")]
    pub program: String,
}

fn yes() -> bool {
    true
}

fn default_site() -> String {
    default_site_url().to_string()
}

fn default_program() -> String {
    "pi".to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            // Not empty: an app that has to be told its own address cannot
            // connect by itself, and "本地电脑未在线" with a blank settings
            // form is the worst possible first run.
            site_url: default_site(),
            name: hostname(),
            // Never `$HOME`: an unconfigured app must not hand an agent
            // everything the user owns just because nobody narrowed it.
            workspace: default_workspace().to_string_lossy().into_owned(),
            auto_approve: false,
            connect_on_launch: true,
            program: default_program(),
        }
    }
}

/// A conservative default the user is expected to change.
///
/// `~/Yunova` rather than the home directory itself: the point of the
/// workspace is to bound the damage a prompt from a phone can do, and a
/// default that bounds nothing would make the setting decorative.
pub fn default_workspace() -> PathBuf {
    home()
        .map(|h| h.join("Yunova"))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn home() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

impl Settings {
    /// Load settings, letting the environment win where it is set.
    ///
    /// The precedence is deliberate: an administrator who ships the app with
    /// `YUNOVA_DEVICE_URL` preset should not have that silently replaced by a
    /// stale file, and a user running the same binary from a terminal should
    /// get the behaviour the old CLI had.
    pub fn load(path: &Path) -> Self {
        let mut settings = std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Settings>(&raw).ok())
            .unwrap_or_default();

        if let Ok(v) = runtime_env::var("YUNOVA_DEVICE_URL")
            && !v.trim().is_empty()
        {
            settings.site_url = v.trim().to_string();
        }
        if let Ok(v) = runtime_env::var("YUNOVA_DEVICE_NAME")
            && !v.trim().is_empty()
        {
            settings.name = v.trim().to_string();
        }
        if let Ok(v) = runtime_env::var("YUNOVA_DEVICE_WORKSPACE")
            && !v.trim().is_empty()
        {
            settings.workspace = v.trim().to_string();
        }
        if let Ok(v) = runtime_env::var("YUNOVA_PI_BIN")
            && !v.trim().is_empty()
        {
            settings.program = v.trim().to_string();
        }
        if let Ok(v) = runtime_env::var("YUNOVA_DEVICE_AUTO_APPROVE") {
            settings.auto_approve = matches!(v.as_str(), "1" | "true" | "yes" | "on");
        }
        if settings.name.trim().is_empty() {
            settings.name = hostname();
        }
        // A settings file written before the address had a default, or one a
        // user cleared, must not leave the app unable to reach its own site.
        if settings.site_url.trim().is_empty() {
            settings.site_url = default_site();
        }
        if settings.workspace.trim().is_empty() {
            settings.workspace = default_workspace().to_string_lossy().into_owned();
        }
        settings
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("无法创建配置目录 {}: {e}", parent.display()))?;
        }
        // Clearing the address field is read as "go back to the built-in
        // site", not as "disconnect this app permanently": the latter has a
        // switch of its own, and a blank required field is not a setting.
        let mut out = self.clone();
        if out.site_url.trim().is_empty() {
            out.site_url = default_site();
        }
        let body = serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?;
        std::fs::write(path, body).map_err(|e| format!("无法写入 {}: {e}", path.display()))
    }

    /// Whether there is enough here to attempt a connection.
    pub fn is_configured(&self) -> bool {
        !self.site_url.trim().is_empty()
    }

    pub fn workspace_path(&self) -> PathBuf {
        PathBuf::from(shellexpand(&self.workspace))
    }

    /// Where per-session runtime config goes.
    ///
    /// Under the workspace, but in a dot-directory the agent has no reason to
    /// touch, matching the CLI's layout so a user switching between them keeps
    /// the same on-disk shape.
    pub fn state_dir(&self) -> PathBuf {
        runtime_env::var("YUNOVA_DEVICE_STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| self.workspace_path().join(".yunova-agent"))
    }
}

/// Expand a leading `~`, which is what users type and what a file picker never
/// produces.
fn shellexpand(raw: &str) -> String {
    let raw = raw.trim();
    match raw.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') => {
            match home() {
                Some(h) => format!("{}{}", h.to_string_lossy(), rest),
                None => raw.to_string(),
            }
        }
        _ => raw.to_string(),
    }
}

/// Where the settings file lives: alongside the credential, under the OS
/// config directory.
pub fn settings_path(config_dir: Option<&Path>) -> PathBuf {
    match config_dir {
        Some(dir) => dir.join("settings.json"),
        None => crate::identity::config_root().join("settings.json"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "yunova-settings-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    #[test]
    fn a_fresh_install_already_knows_where_to_connect() {
        // The address is the app's own, not a fact the user is expected to
        // supply; without this the first run shows a settings form and the
        // device list stays empty.
        let s = Settings::default();
        assert_eq!(s.site_url, default_site_url());
        assert!(s.is_configured(), "a default install must be connectable");
        assert!(
            s.connect_on_launch,
            "attaching must not require pressing a button"
        );
    }

    #[test]
    fn an_unconfigured_app_does_not_expose_the_home_directory() {
        // The workspace is the only thing bounding what a prompt from a phone
        // can reach, so its default must actually bound something.
        let s = Settings::default();
        assert!(
            !s.auto_approve,
            "asking must be the default on a personal machine"
        );
        if let Some(home) = home() {
            assert_ne!(s.workspace_path(), home);
            assert!(s.workspace_path().starts_with(&home));
        }
    }

    #[test]
    fn settings_survive_a_round_trip() {
        let dir = temp("roundtrip");
        let path = dir.join("settings.json");
        let s = Settings {
            site_url: "https://self.hosted.example".into(),
            name: "laptop".into(),
            auto_approve: true,
            ..Default::default()
        };
        s.save(&path).unwrap();

        let back = Settings::load(&path);
        // A self-hosted address must survive: the built-in default is a
        // starting point, not a lock.
        if runtime_env::var("YUNOVA_DEVICE_URL").is_err() {
            assert_eq!(back.site_url, "https://self.hosted.example");
        }
        assert_eq!(back.name, "laptop");
        assert!(back.auto_approve);
        assert!(back.is_configured());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_cleared_address_falls_back_to_the_built_in_site() {
        // Otherwise emptying the field would leave an app that can never
        // reconnect, with no hint of how to get back.
        let dir = temp("cleared");
        let path = dir.join("settings.json");
        let s = Settings {
            site_url: "   ".into(),
            ..Default::default()
        };
        s.save(&path).unwrap();
        let back = Settings::load(&path);
        if runtime_env::var("YUNOVA_DEVICE_URL").is_err() {
            assert_eq!(back.site_url, default_site_url());
        }
        assert!(back.is_configured());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_settings_file_falls_back_instead_of_refusing_to_start() {
        // A window that will not open is worse than one that opens with
        // defaults and lets the user fix it.
        let dir = temp("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, "not json").unwrap();
        let s = Settings::load(&path);
        if runtime_env::var("YUNOVA_DEVICE_URL").is_err() {
            assert_eq!(s.site_url, default_site_url());
        }
        assert!(!s.auto_approve);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_tilde_workspace_is_expanded() {
        // Users type `~/code`; a runtime spawned in a literal "~" directory
        // would silently work somewhere nobody expects.
        let mut s = Settings::default();
        s.workspace = "~/code".into();
        if let Some(home) = home() {
            assert_eq!(s.workspace_path(), home.join("code"));
        }
        // A name that merely starts with a tilde is not a home reference.
        s.workspace = "~weird".into();
        assert_eq!(s.workspace_path(), PathBuf::from("~weird"));
    }

    #[test]
    fn the_state_directory_stays_out_of_the_agents_way() {
        let mut s = Settings::default();
        s.workspace = "/tmp/ws".into();
        // The runtime config holds a quota-spending token, so it must not sit
        // in the tree the agent is rewriting under an ordinary name.
        if runtime_env::var("YUNOVA_DEVICE_STATE_DIR").is_err() {
            assert_eq!(s.state_dir(), PathBuf::from("/tmp/ws/.yunova-agent"));
        }
    }
}
