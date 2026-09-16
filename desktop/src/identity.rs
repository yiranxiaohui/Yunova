//! Who this machine is, and how it proves it.
//!
//! The client signs in with the user's account once and then keeps a
//! device-scoped token. That split matters: the token is stored on a personal
//! computer, so what sits on disk must be revocable to one device rather than
//! equivalent to the account. The password is never written anywhere.
//!
//! The stored file also carries the fingerprint, so a machine that is
//! re-authenticated (after a revoke, or a password change) rebinds the device
//! row it already had instead of accumulating duplicates in the user's list.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Persisted credential for one server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Credential {
    /// Device token minted by the server at sign-in.
    pub token: String,
    /// Which account it belongs to, shown at startup so the user can tell
    /// whose machine this is registered as.
    #[serde(default)]
    pub username: String,
    /// Server-side device id, for log lines that match the web device list.
    #[serde(default)]
    pub device_id: i64,
}

/// Where credentials live, keyed by server so one machine can attach to more
/// than one instance.
///
/// Under the OS config directory rather than the workspace: the workspace is
/// what the agent may rewrite, and a credential the agent can edit is a
/// credential the agent can exfiltrate or replace.
pub fn credential_path(state_dir: Option<&Path>, url: &str) -> PathBuf {
    let base = match state_dir {
        Some(dir) => dir.to_path_buf(),
        None => config_root(),
    };
    base.join(format!("device-{}.json", server_key(url)))
}

/// The directory this client owns under the OS config location.
///
/// Shared with the window shell's settings file so one uninstall cleans up
/// everything, and so a user looking for "where does this keep its state"
/// finds one directory instead of two.
pub fn config_root() -> PathBuf {
    config_dir().join("yunova")
}

/// Stable short key for a server URL.
///
/// Hashed rather than sanitised so an unusual host or a long path can never
/// produce a surprising filename, while the same server always resolves to the
/// same file.
fn server_key(url: &str) -> String {
    let mut h = Sha256::new();
    h.update(
        url.trim()
            .trim_end_matches('/')
            .to_ascii_lowercase()
            .as_bytes(),
    );
    hex::encode(h.finalize())[..16].to_string()
}

fn config_dir() -> PathBuf {
    // Deliberately hand-rolled rather than pulling in a directories crate:
    // three env lookups with a documented fallback is the whole requirement,
    // and the client is distributed as a single static binary.
    if cfg!(target_os = "windows") {
        if let Ok(v) = std::env::var("APPDATA")
            && !v.is_empty()
        {
            return PathBuf::from(v);
        }
    } else if cfg!(target_os = "macos") {
        if let Ok(v) = std::env::var("HOME")
            && !v.is_empty()
        {
            return PathBuf::from(v).join("Library/Application Support");
        }
    } else if let Ok(v) = std::env::var("XDG_CONFIG_HOME")
        && !v.is_empty()
    {
        return PathBuf::from(v);
    }
    if let Ok(v) = std::env::var("HOME")
        && !v.is_empty()
    {
        return PathBuf::from(v).join(".config");
    }
    PathBuf::from(".")
}

pub fn load_credential(path: &Path) -> Option<Credential> {
    let raw = std::fs::read_to_string(path).ok()?;
    let cred: Credential = serde_json::from_str(&raw).ok()?;
    (!cred.token.trim().is_empty()).then_some(cred)
}

/// Persist a credential, owner-readable only.
pub fn save_credential(path: &Path, cred: &Credential) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("无法创建配置目录 {}: {e}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(cred).map_err(|e| e.to_string())?;
    std::fs::write(path, body).map_err(|e| format!("无法写入 {}: {e}", path.display()))?;
    restrict(path);
    Ok(())
}

/// Remove a credential the server has rejected.
///
/// Deleting rather than keeping it is what makes recovery possible: the next
/// start finds no token and asks the user to sign in again, instead of looping
/// forever against a token that will never be accepted.
pub fn forget_credential(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(unix)]
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    // A device token is a bearer credential; other local users must not be
    // able to read it out of a home directory.
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_path: &Path) {
    // Windows inherits the user profile's ACL, which is already per-user.
}

/// A stable identifier for this machine.
///
/// Derived from properties that survive a reboot and a client upgrade, and
/// hashed so the value carries no readable hostname or username to the server
/// beyond what the client already sends. It selects which device row to reuse
/// and is never an authorisation input.
pub fn fingerprint(name: &str) -> String {
    let mut h = Sha256::new();
    h.update(name.trim().to_ascii_lowercase().as_bytes());
    h.update(b"\0");
    h.update(std::env::consts::OS.as_bytes());
    h.update(b"\0");
    h.update(std::env::consts::ARCH.as_bytes());
    h.update(b"\0");
    // The account the client runs as: two users on one shared computer are two
    // devices, because their workspaces and permissions differ.
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    h.update(user.as_bytes());
    hex::encode(h.finalize())[..32].to_string()
}

/// Credentials for a sign-in, from the environment or from a prompt.
pub struct Login {
    pub username: String,
    pub password: String,
}

/// Collect credentials for the first connection.
///
/// Environment variables come first so the client can run unattended (a
/// service unit, a container); otherwise the user is asked. Without a terminal
/// there is nothing to ask, so that case fails with instructions rather than
/// blocking forever on a pipe nobody is writing to.
pub fn prompt_login(env_user: Option<String>, env_pass: Option<String>) -> Result<Login, String> {
    if let (Some(username), Some(password)) = (env_user.clone(), env_pass.clone()) {
        return Ok(Login { username, password });
    }
    if !std::io::stdin().is_terminal() {
        return Err(
            "需要登录：请设置 YUNOVA_USERNAME 与 YUNOVA_PASSWORD，或在终端中运行以便交互登录"
                .into(),
        );
    }

    let username = match env_user {
        Some(u) => u,
        None => read_line("Yunova 账号: ")?,
    };
    let password = match env_pass {
        Some(p) => p,
        None => read_password("密码: ")?,
    };
    if username.trim().is_empty() || password.is_empty() {
        return Err("账号和密码都不能为空".into());
    }
    Ok(Login { username, password })
}

fn read_line(prompt: &str) -> Result<String, String> {
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let mut buf = String::new();
    std::io::stdin()
        .read_line(&mut buf)
        .map_err(|e| format!("读取输入失败: {e}"))?;
    Ok(buf.trim().to_string())
}

/// Read a password without echoing it.
///
/// Echo is disabled through the platform's own terminal API rather than by
/// printing control characters, so an interrupted read cannot leave the
/// terminal in a state where the user's later typing stays invisible.
fn read_password(prompt: &str) -> Result<String, String> {
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let guard = echo::disable();
    let mut buf = String::new();
    let read = std::io::stdin().read_line(&mut buf);
    drop(guard);
    println!();
    read.map_err(|e| format!("读取密码失败: {e}"))?;
    Ok(buf.trim_end_matches(['\n', '\r']).to_string())
}

#[cfg(unix)]
mod echo {
    /// Restores the terminal on drop, including on an early return.
    pub struct Guard(Option<libc_termios::Termios>);

    impl Drop for Guard {
        fn drop(&mut self) {
            if let Some(saved) = self.0.take() {
                libc_termios::set(&saved);
            }
        }
    }

    pub fn disable() -> Guard {
        match libc_termios::get() {
            Some(current) => {
                let mut quiet = current;
                libc_termios::clear_echo(&mut quiet);
                libc_termios::set(&quiet);
                Guard(Some(current))
            }
            // Not a tty we can configure: the caller already checked
            // `is_terminal`, so rather than refuse input we accept the echo.
            None => Guard(None),
        }
    }

    /// The three `termios` calls we need, without a libc dependency.
    mod libc_termios {
        // `termios` is 60 bytes on Linux and 72 on macOS; over-allocating is
        // safe because the kernel only writes the prefix it knows, and the
        // struct is only ever handed back verbatim.
        #[repr(C, align(8))]
        #[derive(Clone, Copy)]
        pub struct Termios([u8; 128]);

        unsafe extern "C" {
            fn tcgetattr(fd: i32, termios_p: *mut Termios) -> i32;
            fn tcsetattr(fd: i32, optional_actions: i32, termios_p: *const Termios) -> i32;
        }

        /// Offset of `c_lflag` inside `termios`, and the `ECHO` bit.
        #[cfg(target_os = "linux")]
        const LFLAG_OFFSET: usize = 12;
        #[cfg(target_os = "linux")]
        const ECHO: u32 = 0o10;
        #[cfg(not(target_os = "linux"))]
        const LFLAG_OFFSET: usize = 24;
        #[cfg(not(target_os = "linux"))]
        const ECHO: u32 = 0x8;

        const STDIN: i32 = 0;
        /// TCSAFLUSH: apply after draining input, so buffered keystrokes are
        /// not echoed after the change.
        #[cfg(target_os = "linux")]
        const TCSAFLUSH: i32 = 2;
        #[cfg(not(target_os = "linux"))]
        const TCSAFLUSH: i32 = 3;

        pub fn get() -> Option<Termios> {
            let mut t = Termios([0; 128]);
            let rc = unsafe { tcgetattr(STDIN, &mut t) };
            (rc == 0).then_some(t)
        }

        pub fn set(t: &Termios) {
            unsafe { tcsetattr(STDIN, TCSAFLUSH, t) };
        }

        pub fn clear_echo(t: &mut Termios) {
            let bytes = &mut t.0[LFLAG_OFFSET..LFLAG_OFFSET + 4];
            let mut lflag = u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            lflag &= !ECHO;
            bytes.copy_from_slice(&lflag.to_ne_bytes());
        }
    }
}

#[cfg(windows)]
mod echo {
    pub struct Guard(Option<u32>);

    impl Drop for Guard {
        fn drop(&mut self) {
            if let Some(mode) = self.0.take() {
                unsafe {
                    let h = GetStdHandle(STD_INPUT_HANDLE);
                    SetConsoleMode(h, mode);
                }
            }
        }
    }

    const STD_INPUT_HANDLE: u32 = 0xFFFF_FFF6; // (DWORD)-10
    const ENABLE_ECHO_INPUT: u32 = 0x0004;

    unsafe extern "system" {
        fn GetStdHandle(nStdHandle: u32) -> isize;
        fn GetConsoleMode(hConsoleHandle: isize, lpMode: *mut u32) -> i32;
        fn SetConsoleMode(hConsoleHandle: isize, dwMode: u32) -> i32;
    }

    pub fn disable() -> Guard {
        unsafe {
            let h = GetStdHandle(STD_INPUT_HANDLE);
            let mut mode: u32 = 0;
            if GetConsoleMode(h, &mut mode) == 0 {
                return Guard(None);
            }
            SetConsoleMode(h, mode & !ENABLE_ECHO_INPUT);
            Guard(Some(mode))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_server_gets_its_own_credential_file() {
        let a = credential_path(None, "https://a.example.com");
        let b = credential_path(None, "https://b.example.com");
        assert_ne!(a, b, "two servers must not share one token file");
        // Trailing slash and case are not a different server.
        assert_eq!(
            credential_path(None, "https://a.example.com/"),
            credential_path(None, "https://A.example.com")
        );
    }

    #[test]
    fn a_credential_filename_never_contains_url_punctuation() {
        let p = credential_path(None, "https://host:3000/some/path?x=1");
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("device-") && name.ends_with(".json"));
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.'),
            "unexpected characters in {name}"
        );
    }

    #[test]
    fn a_credential_survives_a_round_trip_and_is_owner_only() {
        let dir = std::env::temp_dir().join(format!("yunova-cred-{}", std::process::id()));
        let path = dir.join("device.json");
        let cred = Credential {
            token: "ynd_abc".into(),
            username: "orca".into(),
            device_id: 7,
        };
        save_credential(&path, &cred).unwrap();
        assert_eq!(load_credential(&path), Some(cred));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "token file must not be world-readable");
        }

        forget_credential(&path);
        assert_eq!(load_credential(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_blank_or_broken_credential_reads_as_absent() {
        // Otherwise the client would "reconnect" with an empty token forever
        // instead of asking the user to sign in.
        let dir = std::env::temp_dir().join(format!("yunova-cred-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let empty = dir.join("empty.json");
        std::fs::write(&empty, r#"{"token":"  "}"#).unwrap();
        assert_eq!(load_credential(&empty), None);
        let junk = dir.join("junk.json");
        std::fs::write(&junk, "not json").unwrap();
        assert_eq!(load_credential(&junk), None);
        assert_eq!(load_credential(&dir.join("missing.json")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fingerprint_is_stable_per_machine_and_name() {
        let a = fingerprint("laptop");
        assert_eq!(a, fingerprint("laptop"), "must survive a restart");
        assert_eq!(
            a,
            fingerprint("  LAPTOP "),
            "naming noise is not a new machine"
        );
        assert_ne!(a, fingerprint("desktop"));
        // Hashed, so no hostname leaks in the value itself.
        assert!(!a.contains("laptop"));
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn env_credentials_allow_an_unattended_start() {
        let login =
            prompt_login(Some("orca".into()), Some("secret".into())).expect("env is enough");
        assert_eq!(login.username, "orca");
        assert_eq!(login.password, "secret");
    }
}
