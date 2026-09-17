//! The desktop marker, spelled the same on both sides.
//!
//! The site window is a remote page with no Tauri capability, so it cannot ask
//! the shell whether it is running inside the client. The shell instead injects
//! one read-only global, and the page reads it to hide the affordances for
//! installing an app that is already installed.
//!
//! That makes the *name* of the global a contract between a Rust string and a
//! TypeScript property lookup, with nothing between them to catch a rename.
//! Get it wrong and nothing breaks loudly: the client simply goes back to
//! advertising its own download, which is the bug this indirection exists to
//! prevent. So the two spellings are asserted to match here.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the desktop crate sits inside the repository")
        .to_path_buf()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The marker name as declared in `shell.rs`, parsed from the constant rather
/// than duplicated, so this test cannot drift along with a rename.
fn marker_from_shell(shell: &str) -> String {
    let line = shell
        .lines()
        .find(|l| l.contains("const DESKTOP_MARKER"))
        .expect("shell.rs must declare DESKTOP_MARKER");
    let (_, rest) = line.split_once('"').expect("a quoted marker name");
    let (name, _) = rest.split_once('"').expect("a closing quote");
    assert!(!name.is_empty(), "the marker name must not be empty");
    name.to_owned()
}

#[test]
fn the_page_reads_the_same_marker_the_shell_injects() {
    let shell = read(&repo_root().join("desktop/src/shell.rs"));
    let marker = marker_from_shell(&shell);

    // It has to actually be injected, not merely declared.
    assert!(
        shell.contains("initialization_script"),
        "the site window must inject the marker, or the page has no way to know"
    );

    let platform = read(&repo_root().join("web/src/lib/platform.ts"));
    assert!(
        platform.contains(&marker),
        "web/src/lib/platform.ts does not read `{marker}`; the client would keep offering \
         its own download"
    );
}

#[test]
fn the_marker_is_read_only_and_carries_nothing_about_this_machine() {
    let shell = read(&repo_root().join("desktop/src/shell.rs"));
    let marker = marker_from_shell(&shell);
    // Matched on the constant's identifier, not the marker value: the script
    // interpolates the constant, which is what keeps the two in step.
    let script = shell
        .lines()
        .find(|l| l.contains("DESKTOP_MARKER") && l.contains("defineProperty"))
        .unwrap_or_else(|| panic!("`{marker}` must be defined with Object.defineProperty"));

    // `value: true` and nothing else: a writable property could be cleared by
    // the page before its own bundle runs, and any richer payload would leak
    // details of the user's machine to whatever server they configured.
    assert!(
        script.contains("value: true"),
        "the marker must be the boolean `true`, not a payload describing this machine"
    );
    assert!(
        !script.contains("writable"),
        "the marker must stay non-writable, which is defineProperty's default"
    );
}
