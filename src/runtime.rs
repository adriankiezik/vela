//! Where the server keeps its runtime files — `server.properties`, `eula.txt`,
//! the player-list JSONs, and the world save.
//!
//! Vanilla's dedicated server resolves all of these relative to the process's
//! current working directory: drop the jar in a folder, run it, and its files
//! appear there. A shipped Vela binary matches that exactly.
//!
//! During development we launch with `cargo run`, whose CWD is the crate root.
//! Rooting runtime files there scatters generated data across the source tree —
//! and the generated `world/` directory in particular collides, by leaf name,
//! with the `src/world` module in any tool that matches paths by directory name
//! (e.g. editor file-tree exclusions). So when the executable is a Cargo build
//! artifact — it lives under `target/{debug,release}/` — we root runtime files
//! at the executable's own directory instead, keeping generated data inside
//! `target/` and out of the source tree. Direct launches keep vanilla's CWD
//! behavior.

use std::path::{Path, PathBuf};

/// The directory the server reads and writes its runtime files under.
///
/// Resolution order:
///   * the executable's directory when started by `cargo run` (keeps generated
///     data inside `target/` — see the module docs);
///   * the executable's directory on a double-click launch, where the current
///     working directory is arbitrary (often unwritable) and the user expects
///     the world / `eula.txt` to appear next to the exe they ran;
///   * otherwise the current working directory (vanilla behavior — a terminal or
///     hosting-panel launch that deliberately picks the CWD).
pub fn dir() -> PathBuf {
    if let Some(dir) = cargo_run_exe_dir() {
        return dir;
    }
    if crate::platform::owns_console() {
        if let Some(dir) = current_exe_dir() {
            return dir;
        }
    }
    PathBuf::from(".")
}

/// The directory containing the running executable, if it can be determined.
fn current_exe_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.to_path_buf())
}

/// The executable's directory when — and only when — the process was launched by
/// `cargo run` from the crate root, else `None`.
///
/// Two conditions must both hold, which together exclude a shipped binary *and*
/// the integration-test harness (which launches the same `target/…` artifact but
/// from a temporary working directory, and expects vanilla CWD behavior):
///
///   * the executable is a Cargo build artifact (`target/{debug,release}/…`), and
///   * the current working directory is the crate root (`CARGO_MANIFEST_DIR`) —
///     where `cargo run` runs from, but a test's temp CWD or a real deployment
///     never is.
fn cargo_run_exe_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    if !is_cargo_artifact(&exe) {
        return None;
    }
    let cwd = std::env::current_dir().ok()?;
    if !same_dir(&cwd, Path::new(env!("CARGO_MANIFEST_DIR"))) {
        return None;
    }
    Some(exe.parent()?.to_path_buf())
}

/// Whether two paths refer to the same directory, comparing canonical forms so
/// symlinks / case / separator differences don't cause a false mismatch. Falls
/// back to a literal comparison if either path can't be canonicalized.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Whether `exe` sits inside a Cargo build tree, i.e. it was launched via
/// `cargo run` rather than as a shipped binary. Cargo lays binaries out as
/// `target/<profile>/<bin>` or, with an explicit target triple,
/// `target/<triple>/<profile>/<bin>` — so we require the immediate parent to be
/// the `debug` or `release` profile directory and some ancestor to be `target`.
fn is_cargo_artifact(exe: &Path) -> bool {
    let profile_is_build = exe
        .parent()
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "debug" || n == "release");
    let under_target = exe.components().any(|c| c.as_os_str() == "target");
    profile_is_build && under_target
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_debug_and_release_artifacts_are_detected() {
        assert!(is_cargo_artifact(Path::new("/home/u/proj/target/debug/vela")));
        assert!(is_cargo_artifact(Path::new("/home/u/proj/target/release/vela")));
        // With an explicit target triple in the path.
        assert!(is_cargo_artifact(Path::new(
            "/proj/target/x86_64-unknown-linux-gnu/release/vela"
        )));
    }

    #[test]
    fn shipped_binaries_are_not_treated_as_cargo_artifacts() {
        // A binary installed alongside a server directory (vanilla layout).
        assert!(!is_cargo_artifact(Path::new("/opt/mc-server/vela")));
        assert!(!is_cargo_artifact(Path::new("/usr/local/bin/vela")));
        // `target` present but not the profile-dir parent: not the run layout.
        assert!(!is_cargo_artifact(Path::new("/proj/target/vela")));
    }
}
