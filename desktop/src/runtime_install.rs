//! Finding and installing the agent runtime.
//!
//! The client runs `pi` to execute a task, and until now it simply assumed the
//! binary was on `PATH`. That assumption is wrong for the people this app is
//! built for: someone installs a desktop application by double-clicking it,
//! they do not have a Node toolchain, and "启动 pi 失败" tells them nothing
//! about what to do next. A desktop client that can execute tasks locally has
//! to be able to acquire the thing that executes them.
//!
//! Two problems are separable, and both matter:
//!
//! * **Finding it.** A GUI app on macOS is launched by the window server, not
//!   by a shell, so it inherits a minimal `PATH` that contains none of the
//!   usual install locations. A runtime the user installed by hand is
//!   therefore invisible to the app while being perfectly visible in their
//!   terminal — the confusing half of this problem, and the reason the search
//!   below looks in known locations rather than trusting `PATH` alone.
//! * **Installing it.** When it genuinely is absent, the app can install it
//!   with whichever Node package manager exists, into the user's own prefix.
//!
//! Installing deliberately shells out to a package manager instead of
//! downloading a binary: pi is published on npm, npm is where its updates come
//! from, and reimplementing package resolution here would produce an install
//! the user cannot later update with the tool they expect.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Serialize;
use tokio::process::Command;

/// The npm package that provides the runtime.
const RUNTIME_PACKAGE: &str = "@earendil-works/pi-coding-agent";

/// What the app knows about the runtime on this machine.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RuntimeStatus {
    /// Whether a usable runtime was found.
    pub installed: bool,
    /// Absolute path to the executable, when one was found. Shown so a user
    /// with several installs can see which one the app will actually run.
    pub path: Option<String>,
    /// Version string as reported by the runtime itself.
    pub version: Option<String>,
    /// Package managers available to install with, most preferred first. Empty
    /// means the app cannot install it and the user needs Node first.
    pub installers: Vec<String>,
}

/// Package managers that can install the runtime, in preference order.
///
/// `bun` first because it is fastest and is what the project itself uses;
/// `npm` last because it is the one that is always there when Node is.
const INSTALLERS: [&str; 3] = ["bun", "pnpm", "npm"];

/// Locate an executable without a shell.
///
/// `which` is not used: it may not exist on Windows, and spawning a shell to
/// answer a question this simple would inherit whatever startup files the
/// user's shell runs.
fn find_in_path(program: &str) -> Option<PathBuf> {
    // An explicit path is honoured as given, so a user who points the setting
    // at one specific build gets that build.
    let candidate = Path::new(program);
    if candidate.is_absolute() {
        return is_executable(candidate).then(|| candidate.to_path_buf());
    }

    let exts: &[&str] = if cfg!(windows) {
        &[".cmd", ".exe", ".bat", ""]
    } else {
        &[""]
    };

    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        for ext in exts {
            let full = dir.join(format!("{program}{ext}"));
            if is_executable(&full) {
                return Some(full);
            }
        }
    }
    None
}

/// Directories a Node package manager installs global binaries into.
///
/// Searched because a GUI app does not inherit the shell's `PATH`: on macOS an
/// app launched from Finder sees roughly `/usr/bin:/bin`, so a runtime the
/// user installed minutes earlier in their terminal would otherwise be
/// reported as missing.
fn well_known_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);

    if let Some(home) = home.as_ref() {
        dirs.push(home.join(".bun/bin"));
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".npm-global/bin"));
        dirs.push(home.join("node_modules/.bin"));
        if cfg!(windows) {
            dirs.push(home.join("AppData/Roaming/npm"));
        }
        // Version managers keep their shims outside any stable location, and
        // the current selection is the only one worth looking at.
        dirs.push(home.join(".nvm/current/bin"));
        dirs.push(home.join(".volta/bin"));
        dirs.push(home.join(".asdf/shims"));
        dirs.push(home.join(".fnm/aliases/default/bin"));
    }
    if !cfg!(windows) {
        // Homebrew on Apple silicon and on Intel, then the system prefix.
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/usr/bin"));
    }
    dirs
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Resolve the runtime executable, searching beyond `PATH`.
pub fn resolve(program: &str) -> Option<PathBuf> {
    if let Some(found) = find_in_path(program) {
        return Some(found);
    }
    let exts: &[&str] = if cfg!(windows) {
        &[".cmd", ".exe", ".bat", ""]
    } else {
        &[""]
    };
    for dir in well_known_dirs() {
        for ext in exts {
            let full = dir.join(format!("{program}{ext}"));
            if is_executable(&full) {
                return Some(full);
            }
        }
    }
    None
}

/// Ask the runtime what version it is.
///
/// Also the liveness check: a file that exists but cannot run — a broken
/// symlink from a removed Node version, a partial install — must not be
/// reported as a working runtime, because the failure would otherwise surface
/// only when a task is already waiting on it.
async fn version_of(exe: &Path) -> Option<String> {
    let out = Command::new(exe)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Everything the panel needs to render the runtime section.
pub async fn status(program: &str) -> RuntimeStatus {
    let exe = resolve(program);
    let version = match exe.as_deref() {
        Some(path) => version_of(path).await,
        None => None,
    };
    RuntimeStatus {
        // A runtime that cannot report a version is not usable, so it is not
        // "installed" as far as the user is concerned.
        installed: version.is_some(),
        path: exe.map(|p| p.to_string_lossy().into_owned()),
        version,
        installers: INSTALLERS
            .iter()
            .filter(|i| find_in_path(i).is_some())
            .map(|i| i.to_string())
            .collect(),
    }
}

/// Install or update the runtime with `manager`.
///
/// Returns the combined output on failure, because the useful part of a failed
/// npm install is in its log and a generic "安装失败" would send the user to
/// a terminal to reproduce it.
pub async fn install(manager: &str) -> Result<String, String> {
    // Only from the fixed list: this string arrives from the panel, and
    // spawning an arbitrary program named by the UI would be a command
    // injection with extra steps.
    if !INSTALLERS.contains(&manager) {
        return Err(format!("不支持的安装方式：{manager}"));
    }
    let exe = find_in_path(manager).ok_or_else(|| format!("未找到 {manager}"))?;

    let args: Vec<&str> = match manager {
        // `--ignore-scripts` mirrors pi's own documented install line: the
        // package needs no lifecycle scripts, and running them would execute
        // third-party code on a user's machine on the app's initiative.
        "npm" => vec!["install", "-g", "--ignore-scripts", RUNTIME_PACKAGE],
        "pnpm" => vec!["add", "-g", RUNTIME_PACKAGE],
        _ => vec!["install", "-g", RUNTIME_PACKAGE],
    };

    let out = Command::new(exe)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("无法运行 {manager}: {e}"))?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if out.status.success() {
        Ok(format!("{stdout}{stderr}").trim().to_string())
    } else {
        // Tail rather than head: package managers print the failure last.
        let combined = format!("{stdout}{stderr}");
        let tail: String = combined
            .lines()
            .rev()
            .take(12)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        Err(tail.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_package_managers_can_be_spawned() {
        // The manager name comes from the panel. Anything not on the list must
        // be refused before it reaches `Command::new`.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for bad in ["sh", "rm", "npm; rm -rf /", "", "NPM"] {
            let err = rt
                .block_on(install(bad))
                .expect_err("only the fixed list may run");
            assert!(err.contains("不支持的安装方式"), "got: {err}");
        }
    }

    #[test]
    fn an_absolute_program_is_honoured_exactly() {
        // A user who points the setting at one specific build must get that
        // build, not whichever one happens to be first on PATH.
        assert_eq!(resolve("/definitely/not/here/pi"), None);
    }

    #[tokio::test]
    async fn a_missing_runtime_reads_as_not_installed() {
        let s = status("yunova-no-such-runtime-binary").await;
        assert!(!s.installed);
        assert_eq!(s.path, None);
        assert_eq!(s.version, None);
    }

    #[tokio::test]
    async fn a_file_that_cannot_run_is_not_reported_as_installed() {
        // A broken symlink or a half-finished install leaves a file behind.
        // Treating "the file exists" as "the runtime works" would defer the
        // failure to the moment a task is already waiting on it.
        let dir = std::env::temp_dir().join(format!("yunova-rt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("pi");
        std::fs::write(&fake, "not an executable").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let s = status(&fake.to_string_lossy()).await;
        assert!(
            !s.installed,
            "a file that cannot report a version is not a runtime"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_search_covers_locations_a_gui_app_cannot_see_on_path() {
        // The macOS case this exists for: an app launched from Finder gets a
        // minimal PATH, so the usual install prefixes must be searched
        // explicitly or a working runtime reads as missing.
        let dirs = well_known_dirs();
        assert!(!dirs.is_empty());
        if std::env::var_os("HOME").is_some() {
            let joined: Vec<String> = dirs
                .iter()
                .map(|d| d.to_string_lossy().into_owned())
                .collect();
            assert!(
                joined.iter().any(|d| d.ends_with(".bun/bin")),
                "bun's prefix must be searched: {joined:?}"
            );
            assert!(
                joined.iter().any(|d| d.contains("npm")),
                "an npm global prefix must be searched: {joined:?}"
            );
        }
    }
}
