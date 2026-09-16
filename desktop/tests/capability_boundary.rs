//! The boundary between the remote page and this machine.
//!
//! The desktop app shows a server-delivered UI *and* holds the power to
//! reconfigure what an agent may do locally. Those two facts are only safe
//! together because the page never gets the power: the site is rendered in a
//! webview named in no capability, so Tauri refuses its IPC calls outright.
//!
//! That is a one-line mistake away from being untrue — adding the site window
//! to a capability, or dropping the window list entirely, would silently hand
//! local control to whatever server the user typed in. So it is asserted here
//! against the shipped configuration rather than left to review.

use serde_json::Value;

/// Window labels, kept in step with `shell.rs`.
const SITE_WINDOW: &str = "site";
const PANEL_WINDOW: &str = "panel";

/// Every capability this crate declares.
///
/// Read from the committed `capabilities/` directory rather than from Tauri's
/// generated manifest: the generated copy only exists after a build has run,
/// and a boundary test that can be skipped by build ordering is not a boundary
/// test. These files are the input that decides the outcome anyway.
fn capabilities() -> Vec<(String, Value)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"));

    let mut out = Vec::new();
    for entry in entries {
        let raw = std::fs::read_to_string(entry.path())
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", entry.path().display()));
        let parsed: Value = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", entry.path().display()));
        out.push((entry.file_name().to_string_lossy().into_owned(), parsed));
    }
    out
}

#[test]
fn the_remote_site_window_is_granted_nothing() {
    let caps = capabilities();
    assert!(!caps.is_empty(), "the panel needs at least one capability");

    for (name, cap) in &caps {
        let windows = cap
            .get("windows")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!(
                    "capability `{name}` has no window list, so it applies to every window \
                     including the remote site"
                )
            });
        assert!(
            !windows.is_empty(),
            "capability `{name}` lists no windows, which grants it everywhere"
        );
        for w in windows {
            let label = w.as_str().unwrap_or_default();
            assert_ne!(
                label, SITE_WINDOW,
                "capability `{name}` would let a page from the configured server call local \
                 commands"
            );
            assert!(
                !label.contains('*'),
                "capability `{name}` uses a wildcard window pattern (`{label}`), which can match \
                 the remote site window"
            );
        }
        // `local: false` would additionally allow a remote origin to use the
        // capability even on a listed window.
        assert_eq!(
            cap.get("local").and_then(Value::as_bool),
            Some(true),
            "capability `{name}` must be restricted to local app URLs explicitly"
        );
        assert!(
            cap.get("remote").is_none(),
            "capability `{name}` declares remote origins, which is exactly what this app must \
             not do"
        );
    }
}

#[test]
fn the_local_panel_can_still_do_its_job() {
    // The inverse failure is just as bad and much easier to ship: a boundary
    // tightened until the panel cannot open a folder picker or post a
    // notification leaves the app unable to do the one thing a browser can't.
    let caps = capabilities();
    let (_, panel) = caps
        .iter()
        .find(|(_, c)| {
            c.get("windows")
                .and_then(Value::as_array)
                .is_some_and(|w| w.iter().any(|x| x.as_str() == Some(PANEL_WINDOW)))
        })
        .expect("the panel window must have a capability");

    let permissions: Vec<&str> = panel
        .get("permissions")
        .and_then(Value::as_array)
        .expect("a permission list")
        .iter()
        .filter_map(Value::as_str)
        .collect();

    for needed in ["dialog:allow-open", "notification:default"] {
        assert!(
            permissions.contains(&needed),
            "the panel needs `{needed}`; without it the workspace picker or the \
             blocked-agent notification silently stops working"
        );
    }
}

#[test]
fn the_site_window_is_actually_created_as_a_remote_page() {
    // The boundary above is only meaningful if the site really is a separate
    // window. If someone ever pointed the site at `WebviewUrl::App`, it would
    // share the panel's origin and inherit its IPC.
    let shell = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/shell.rs"),
    )
    .expect("shell.rs must be readable");
    assert!(
        shell.contains("WebviewUrl::External"),
        "the site window must load the server as an external URL"
    );
    // And the app must declare no startup windows, or a window outside any
    // capability check could be created by configuration alone.
    let conf = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json"),
    )
    .expect("tauri.conf.json must be readable");
    let conf: Value = serde_json::from_str(&conf).expect("valid config");
    assert_eq!(
        conf["app"]["windows"].as_array().map(Vec::len),
        Some(0),
        "windows are created in code so each one's URL and capability are explicit"
    );
}
