//! One version number for the whole product.
//!
//! The desktop app and the server ship from the same `vX.Y.Z` tag, and a user
//! comparing the installed app against the site has to see one release, not
//! two. So the number lives once — `[workspace.package] version` in the root
//! `Cargo.toml` — and everything else inherits it:
//!
//! * the server's `/api/health` and admin system info, via `CARGO_PKG_VERSION`
//! * this app's `--version`, via the same
//! * the installer file names, the Windows file-version resource and the macOS
//!   `CFBundleShortVersionString`, because `tauri.conf.json` declares no
//!   `version` and Tauri then falls back to the crate's Cargo version
//!
//! That last one is the fragile link. Re-adding `version` to the Tauri config
//! would silently win over the Cargo version, and nothing would fail: the
//! build would succeed and simply name the installers after a number nobody
//! remembered to bump, which is exactly how the config came to say `0.1.0`
//! while the project was releasing `v0.4.x`. So it is asserted here.

use serde_json::Value;

fn manifest_dir() -> &'static std::path::Path {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn the_tauri_config_does_not_restate_the_version() {
    let raw = std::fs::read_to_string(manifest_dir().join("tauri.conf.json"))
        .expect("tauri.conf.json must be readable");
    let conf: Value = serde_json::from_str(&raw).expect("valid config");

    assert!(
        conf.get("version").is_none(),
        "tauri.conf.json must not declare `version`: it would override the Cargo version and \
         the installers would be named from a second copy of the number that no release step \
         updates. Bump `[workspace.package] version` in the root Cargo.toml instead."
    );
}

#[test]
fn this_crate_inherits_the_workspace_version() {
    // Read as text rather than through `cargo metadata`: the point is that the
    // manifest *says* `workspace = true`, not merely that today's resolved
    // value happens to match. A pinned literal that coincidentally equals the
    // workspace version would pass a value comparison and then drift on the
    // next release.
    let raw = std::fs::read_to_string(manifest_dir().join("Cargo.toml"))
        .expect("desktop/Cargo.toml must be readable");
    let manifest: toml::Value = toml::from_str(&raw).expect("valid manifest");
    let version = &manifest["package"]["version"];

    assert_eq!(
        version.get("workspace").and_then(toml::Value::as_bool),
        Some(true),
        "desktop/Cargo.toml must use `version.workspace = true` so the app cannot disagree with \
         the server about which release it is; found {version:?}"
    );
}

#[test]
fn the_workspace_version_is_what_this_binary_reports() {
    // Ties the inherited value to the number the running app prints, so the
    // chain from the root manifest to `--version` is checked end to end.
    let root = manifest_dir()
        .parent()
        .expect("the crate lives inside the workspace");
    let raw = std::fs::read_to_string(root.join("Cargo.toml")).expect("root Cargo.toml");
    let manifest: toml::Value = toml::from_str(&raw).expect("valid manifest");

    let declared = manifest["workspace"]["package"]["version"]
        .as_str()
        .expect("`[workspace.package] version` must be a string");

    assert_eq!(
        declared,
        env!("CARGO_PKG_VERSION"),
        "the workspace version and the compiled-in version disagree"
    );
}
