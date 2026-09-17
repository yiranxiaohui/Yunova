//! The macOS bundle must be code signed, even if only ad-hoc.
//!
//! A Tauri bundle with no `signingIdentity` is not merely "unsigned in a way
//! we accept until there is a certificate": the bundler skips `codesign`
//! entirely, so nothing seals the `.app`'s resources or `Info.plist`. On Apple
//! Silicon that is not enough for Gatekeeper once a browser has attached
//! `com.apple.quarantine`, and macOS reports the download as *damaged* rather
//! than offering the "unidentified developer" prompt a user could approve.
//! Telling someone their file is corrupt when it is intact sends them to the
//! Trash, or back to download it again.
//!
//! `signingIdentity: "-"` makes the bundler run `codesign --force -s -` over
//! the bundle, which restores the honest path. It proves nothing about who
//! built the app and does not remove the prompt — only a Developer ID plus
//! notarization does that — so this is a floor, not a solution.
//!
//! Asserted because the failure is invisible from here: the field lives in a
//! config no Rust code reads, the consequence appears only on a real Mac after
//! a real download, and this repository is built and tested on Linux. Deleting
//! the line would produce a green build and a broken release.

use serde_json::Value;

fn config() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("tauri.conf.json must be valid JSON")
}

#[test]
fn the_macos_bundle_declares_a_signing_identity() {
    let conf = config();
    let identity = conf["bundle"]["macOS"]["signingIdentity"].as_str();

    assert!(
        identity.is_some_and(|i| !i.trim().is_empty()),
        "bundle.macOS.signingIdentity is missing: Tauri would skip `codesign` and macOS would \
         call the download damaged instead of prompting about an unidentified developer. Use \
         \"-\" for ad-hoc signing, or a Developer ID once one exists."
    );
}

#[test]
fn the_config_stays_strict_json() {
    // `tauri.conf.json` is parsed as strict JSON, so a `//` comment added to
    // explain a field like `signingIdentity` breaks the build rather than the
    // formatting. The rationale belongs in the README; this keeps a
    // well-intentioned edit from failing the release.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    let raw = std::fs::read_to_string(path).expect("readable");
    let parsed: Result<Value, _> = serde_json::from_str(&raw);
    assert!(
        parsed.is_ok(),
        "tauri.conf.json is not strict JSON (comments are not allowed here): {:?}",
        parsed.err()
    );
}
