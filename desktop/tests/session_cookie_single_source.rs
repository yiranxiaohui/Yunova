//! The desktop app attaches by reading the site's session cookie, so both
//! sides have to agree on its name.
//!
//! This is the quiet failure mode of the whole automatic-attach path: rename
//! the cookie on the server and nothing breaks loudly — the app still opens,
//! the site still works, the user still signs in, and the machine simply never
//! appears in the device list. That is the exact symptom the automatic attach
//! was built to remove, so the agreement is asserted rather than remembered.
//!
//! Read out of the server's source instead of shared through a crate because
//! the client is deliberately a standalone binary with its own copy of the
//! protocol (see `desktop/src/proto.rs`); the test is what keeps the copy
//! honest.

fn server_session_cookie() -> String {
    let auth = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate lives inside the workspace")
        .join("src/auth.rs");
    let raw = std::fs::read_to_string(&auth)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", auth.display()));

    let decl = raw
        .lines()
        .find_map(|l| l.trim().strip_prefix("pub const SESSION_COOKIE: &str = "))
        .expect("src/auth.rs must declare `pub const SESSION_COOKIE`");
    decl.trim()
        .trim_end_matches(';')
        .trim_matches('"')
        .to_string()
}

#[test]
fn the_client_reads_the_cookie_the_server_actually_sets() {
    // `SESSION_COOKIE` lives in `endpoint.rs`, which the binary's own unit
    // tests cover; here it is compared against the server's declaration.
    let client = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/endpoint.rs"),
    )
    .expect("endpoint.rs must be readable");

    let declared = client
        .lines()
        .find_map(|l| l.trim().strip_prefix("pub const SESSION_COOKIE: &str = "))
        .expect("endpoint.rs must declare `pub const SESSION_COOKIE`")
        .trim()
        .trim_end_matches(';')
        .trim_matches('"')
        .to_string();

    assert_eq!(
        declared,
        server_session_cookie(),
        "the desktop client looks for a session cookie the server does not set, so signing in \
         on the site would never bind this machine and the device would stay offline with no \
         error anywhere"
    );
}
