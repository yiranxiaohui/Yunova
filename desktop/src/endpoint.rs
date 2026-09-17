//! Address handling and the few machine facts the connector reports.
//!
//! Separated from the connection loop because these are pure functions with
//! security consequences: guessing a scheme wrong here is the difference
//! between a password on the wire and a password in a TLS tunnel, and that
//! deserves to be testable without a socket.

use serde_json::Value;

/// The site this client belongs to.
///
/// Baked in rather than asked for: this is a product with one address, and a
/// first-run screen demanding a URL the user has no reason to know is how the
/// app ends up looking broken ("本地电脑未在线") on a perfectly good install.
/// It stays *overridable* — a self-hosted build can bake its own address, and
/// the settings file and `YUNOVA_DEVICE_URL` still win at runtime — so nothing
/// is lost by having an answer by default.
pub fn default_site_url() -> &'static str {
    match option_env!("YUNOVA_DEFAULT_SITE_URL") {
        Some(v) if !v.is_empty() => v,
        _ => "https://chat.yunnet.top",
    }
}

/// Name of the site's session cookie.
///
/// The desktop app reads it out of its own site webview to bind this machine
/// without asking for a password the user already typed in that window. Kept
/// in step with the server's `auth::SESSION_COOKIE` by
/// `tests/session_cookie_single_source.rs`.
pub const SESSION_COOKIE: &str = "nc_session";

/// Accept a site address and derive the device WebSocket endpoint.
///
/// Users paste what they see in the browser, so the bare origin must work:
///   https://yunnet.top   -> wss://yunnet.top/api/agent/devices/connect
///   http://10.0.0.1:3000 -> ws://10.0.0.1:3000/api/agent/devices/connect
///   yunnet.top           -> wss://yunnet.top/api/agent/devices/connect
/// An already-complete endpoint is left alone.
pub fn device_endpoint(raw: &str) -> String {
    const PATH: &str = "/api/agent/devices/connect";
    let raw = raw.trim();
    let (scheme, rest) = match raw.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        // No scheme: assume TLS rather than silently downgrading.
        None => ("https".to_string(), raw),
    };
    let ws_scheme = match scheme.as_str() {
        "http" | "ws" => "ws",
        _ => "wss",
    };
    let rest = rest.trim_end_matches('/');
    if rest.ends_with(PATH) {
        return format!("{ws_scheme}://{rest}");
    }
    format!("{ws_scheme}://{rest}{PATH}")
}

/// The address to open in the window: the site itself, not the socket.
///
/// Derived from the same input the connector uses so the user configures one
/// thing. A bare host is upgraded to HTTPS for the same reason the socket is —
/// a session cookie is not worth sending in the clear because someone omitted
/// a scheme.
pub fn site_origin(raw: &str) -> String {
    let raw = raw.trim().trim_end_matches('/');
    let (scheme, rest) = match raw.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r.to_string()),
        None => ("https".to_string(), raw.to_string()),
    };
    let scheme = match scheme.as_str() {
        "http" | "ws" => "http",
        _ => "https",
    };
    let rest = rest.trim_end_matches('/');
    let rest = rest
        .strip_suffix("/api/agent/devices/connect")
        .unwrap_or(rest);
    format!("{scheme}://{rest}")
}

/// Whether the endpoint is this machine, where plain HTTP is a development
/// setup rather than an exposed password.
pub fn is_loopback(url: &str) -> bool {
    let host = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url)
        .split(['/', '?'])
        .next()
        .unwrap_or("");
    let host = match host.strip_prefix('[') {
        // Bracketed IPv6: the colons inside belong to the address, not to a
        // port, so the generic split would truncate it.
        Some(rest) => rest.split_once(']').map(|(h, _)| h).unwrap_or(rest),
        None => host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host),
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

/// Whether a runtime frame means an agent is waiting for a human, and what to
/// say about it.
///
/// Only blocking dialogs count. The fire-and-forget UI methods (`notify`,
/// `setStatus`, …) arrive constantly, and notifying on those would turn a
/// useful interruption into noise the user learns to dismiss — at which point
/// the one notification that mattered is lost too.
pub fn blocking_prompt(frame: &Value) -> Option<(String, String)> {
    if frame.get("type").and_then(Value::as_str)? != "extension_ui_request" {
        return None;
    }
    let method = frame.get("method").and_then(Value::as_str)?;
    if !matches!(method, "confirm" | "select" | "input" | "editor") {
        return None;
    }
    let detail = frame
        .get("options")
        .and_then(|o| o.get("title").or_else(|| o.get("message")))
        .and_then(Value::as_str)
        .or_else(|| frame.get("title").and_then(Value::as_str))
        .unwrap_or("Agent 正在等待你的确认");
    Some((
        "本机任务等待批准".to_string(),
        detail.chars().take(160).collect(),
    ))
}

pub fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "我的电脑".into())
}

pub fn platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_built_in_site_address_is_usable_without_configuration() {
        // The whole point of the default is that a fresh install can connect,
        // so it has to survive the same derivation the user's own input does.
        let url = default_site_url();
        assert!(
            url.starts_with("https://"),
            "the shipped default must not downgrade the session cookie: {url}"
        );
        assert_eq!(
            device_endpoint(url),
            format!(
                "wss://{}/api/agent/devices/connect",
                &url["https://".len()..]
            )
        );
        assert_eq!(site_origin(url), url);
    }

    #[test]
    fn a_bare_site_address_becomes_a_secure_endpoint() {
        // What the user copies out of the browser has to work.
        assert_eq!(
            device_endpoint("https://yunnet.top"),
            "wss://yunnet.top/api/agent/devices/connect"
        );
        assert_eq!(
            device_endpoint("yunnet.top"),
            "wss://yunnet.top/api/agent/devices/connect"
        );
        // A trailing slash must not produce a doubled path.
        assert_eq!(
            device_endpoint("https://yunnet.top/"),
            "wss://yunnet.top/api/agent/devices/connect"
        );
    }

    #[test]
    fn plain_http_is_preserved_for_local_development() {
        assert_eq!(
            device_endpoint("http://127.0.0.1:3000"),
            "ws://127.0.0.1:3000/api/agent/devices/connect"
        );
        assert_eq!(
            device_endpoint("ws://127.0.0.1:3000"),
            "ws://127.0.0.1:3000/api/agent/devices/connect"
        );
    }

    #[test]
    fn an_unknown_scheme_is_treated_as_encrypted() {
        // Guessing wrong in the safe direction fails loudly; guessing wrong in
        // the unsafe direction would send a password in the clear.
        assert!(device_endpoint("gopher://example.com").starts_with("wss://"));
    }

    #[test]
    fn a_complete_endpoint_is_left_alone() {
        assert_eq!(
            device_endpoint("wss://yunnet.top/api/agent/devices/connect"),
            "wss://yunnet.top/api/agent/devices/connect"
        );
    }

    #[test]
    fn the_window_shows_the_site_not_the_socket() {
        // One setting drives both, so the same input the connector accepts
        // must also yield a page the user can look at.
        assert_eq!(site_origin("https://yunnet.top/"), "https://yunnet.top");
        assert_eq!(site_origin("yunnet.top"), "https://yunnet.top");
        assert_eq!(
            site_origin("http://127.0.0.1:3000"),
            "http://127.0.0.1:3000"
        );
        assert_eq!(
            site_origin("wss://yunnet.top/api/agent/devices/connect"),
            "https://yunnet.top"
        );
    }

    #[test]
    fn only_this_machine_counts_as_loopback() {
        // The password is allowed over plain HTTP exactly where the traffic
        // never leaves the machine.
        assert!(is_loopback("ws://127.0.0.1:3000/api/agent/devices/connect"));
        assert!(is_loopback("ws://localhost:3000/api/agent/devices/connect"));
        assert!(is_loopback("ws://[::1]:3000/api/agent/devices/connect"));
        assert!(!is_loopback(
            "ws://10.1.51.1:4300/api/agent/devices/connect"
        ));
        assert!(!is_loopback("ws://yunnet.top/api/agent/devices/connect"));
        // A hostname that merely starts with a loopback label is not loopback.
        assert!(!is_loopback("ws://localhost.evil.com/api"));
        assert!(!is_loopback("ws://127.0.0.1.evil.com/api"));
    }

    #[test]
    fn only_a_blocked_agent_is_worth_interrupting_for() {
        let confirm = json!({
            "type": "extension_ui_request",
            "id": "u1",
            "method": "confirm",
            "options": {"title": "允许在本机执行 bash？"}
        });
        let (title, body) = blocking_prompt(&confirm).expect("a confirm blocks the agent");
        assert!(title.contains("等待批准"));
        assert_eq!(body, "允许在本机执行 bash？");

        // Fire-and-forget UI and ordinary events must stay silent.
        assert!(
            blocking_prompt(&json!({
                "type": "extension_ui_request",
                "id": "u2",
                "method": "notify",
                "options": {"title": "done"}
            }))
            .is_none()
        );
        assert!(blocking_prompt(&json!({"type": "agent_settled"})).is_none());
    }

    #[test]
    fn a_prompt_without_a_message_still_says_something_useful() {
        // An empty notification body reads as a bug; the user needs to know
        // where to go answer it.
        let (_, body) = blocking_prompt(&json!({
            "type": "extension_ui_request",
            "id": "u3",
            "method": "input"
        }))
        .expect("input blocks the agent");
        assert!(!body.is_empty());
    }
}
