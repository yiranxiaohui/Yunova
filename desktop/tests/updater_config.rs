//! The updater config has to be complete, or updates fail on users' machines.
//!
//! Every part of this feature fails *silently* and *late*. A missing `pubkey`
//! means the plugin rejects every bundle it downloads; a placeholder left in
//! its place means the same, except it also looks configured. An endpoint
//! pointing at the wrong repository returns 404 forever. `createUpdaterArtifacts`
//! left off produces a release with installers but no `.sig`, so clients can
//! never verify anything. None of that breaks a build, and none of it is
//! visible in the app until someone tries to update months later — by which
//! point the fix has to be delivered by the mechanism that is broken.
//!
//! The private half of the key cannot be checked from here, so the workflow
//! asserts that `.sig` files were produced instead. These tests cover the
//! half that lives in the repository.

use serde_json::Value;

fn manifest_dir() -> &'static std::path::Path {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn config() -> Value {
    let path = manifest_dir().join("tauri.conf.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("tauri.conf.json must be valid JSON")
}

#[test]
fn the_updater_has_a_real_public_key() {
    let conf = config();
    let pubkey = conf["plugins"]["updater"]["pubkey"]
        .as_str()
        .expect("plugins.updater.pubkey must be set, or no update can be verified");

    assert!(
        !pubkey.trim().is_empty(),
        "plugins.updater.pubkey is empty: the updater would reject every download"
    );
    // The generated key is base64 and decodes to a minisign public key block.
    // A path, a placeholder or a truncated paste all fail here rather than on
    // a user's machine.
    let decoded = String::from_utf8(
        base64_decode(pubkey).expect("plugins.updater.pubkey must be valid base64"),
    )
    .expect("the decoded public key must be UTF-8");
    assert!(
        decoded.contains("minisign public key"),
        "plugins.updater.pubkey does not decode to a minisign public key; it must be the \
         base64 blob printed by `tauri signer generate`, not a file path or a placeholder"
    );
}

#[test]
fn the_updater_endpoint_is_https_and_points_at_this_project() {
    let conf = config();
    let endpoints = conf["plugins"]["updater"]["endpoints"]
        .as_array()
        .expect("plugins.updater.endpoints must be an array");
    assert!(
        !endpoints.is_empty(),
        "plugins.updater.endpoints is empty, so the updater has nothing to ask"
    );

    for endpoint in endpoints {
        let url = endpoint.as_str().expect("each endpoint must be a string");
        assert!(
            url.starts_with("https://"),
            "updater endpoint `{url}` is not HTTPS: the updater enforces TLS in release \
             builds, so this would fail at runtime"
        );
        assert!(
            url.contains("yiranxiaohui/Yunova"),
            "updater endpoint `{url}` does not point at this project's releases"
        );
    }

    // `dangerousInsecureTransportProtocol` would let the manifest arrive over
    // plain HTTP. Signature verification still applies, but the update check
    // would leak the installed version and be trivially blockable.
    assert!(
        conf["plugins"]["updater"]["dangerousInsecureTransportProtocol"].is_null(),
        "dangerousInsecureTransportProtocol must stay unset for a shipped client"
    );
}

#[test]
fn the_bundler_is_told_to_create_updater_artifacts() {
    let conf = config();
    assert_eq!(
        conf["bundle"]["createUpdaterArtifacts"].as_bool(),
        Some(true),
        "bundle.createUpdaterArtifacts must be true, or the release contains installers with \
         no signatures and every client's update check finds nothing to verify"
    );
}

#[test]
fn the_updater_capable_bundles_are_still_built() {
    // The manifest is assembled from the `.sig` files of these bundle types,
    // so dropping one from `targets` removes self-update for that platform
    // while the download page keeps offering the installer.
    let conf = config();
    let targets: Vec<&str> = conf["bundle"]["targets"]
        .as_array()
        .expect("bundle.targets must be an array")
        .iter()
        .filter_map(Value::as_str)
        .collect();

    for needed in ["appimage", "nsis", "app"] {
        assert!(
            targets.contains(&needed),
            "bundle.targets must keep `{needed}`: it is the updater artifact for its platform, \
             and without it `latest.json` gets no entry for that platform"
        );
    }
}

/// Minimal base64 decoder, so this test adds no dependency for one check.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [255u8; 256];
    for (i, c) in TABLE.iter().enumerate() {
        lookup[*c as usize] = i as u8;
    }

    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in input.trim().bytes() {
        if byte == b'=' || byte.is_ascii_whitespace() {
            continue;
        }
        let value = lookup[byte as usize];
        if value == 255 {
            return None;
        }
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}
