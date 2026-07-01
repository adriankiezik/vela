//! `eula.txt` — the EULA acknowledgement gate.
//!
//! Mirrors vanilla's `net.minecraft.server.Eula` exactly: the file holds a
//! single `eula=<bool>` and the server refuses to start unless it is `true`,
//! creating the file with `eula=false` on first run. The one escape hatch is
//! vanilla's `SharedConstants.IS_RUNNING_IN_IDE`, which auto-agrees and skips
//! writing the file — we expose the same via the `VELA_RUNNING_IN_IDE`
//! environment variable (used by the integration test and for local dev).

use std::path::Path;

use tracing::warn;

/// Vanilla's EULA URL (`CommonLinks.EULA`), reproduced in the file comment.
const EULA_URL: &str = "https://aka.ms/MinecraftEULA";

/// Mirror of `SharedConstants.IS_RUNNING_IN_IDE`: when set (to a non-empty,
/// non-`0`/`false` value) the EULA is auto-agreed and `eula.txt` is not written,
/// exactly as vanilla does when launched from a dev environment.
pub fn running_in_ide() -> bool {
    std::env::var("VELA_RUNNING_IN_IDE")
        .map(|v| !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false"))
        .unwrap_or(false)
}

/// Returns whether the EULA is agreed, creating `eula.txt` with `eula=false` if
/// it is missing/unreadable (unless running in IDE mode). Matches vanilla's
/// `Eula(file)` constructor + `hasAgreedToEULA()`.
pub fn check(path: impl AsRef<Path>) -> bool {
    running_in_ide() || read_file(path.as_ref())
}

/// Vanilla `Eula.readFile`: parse the `eula` key; on a read failure write the
/// default file and report not-agreed. A missing file is the normal first-run
/// case (we're about to create it), so unlike vanilla we don't warn on it — only
/// a genuine read error (permissions, etc.) is worth surfacing.
fn read_file(path: &Path) -> bool {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_eula(&text),
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(file = %path.display(), error = %e, "Failed to load {}", path.display());
            }
            save_defaults(path);
            false
        }
    }
}

/// Vanilla `Eula.saveDefaults`: write the comment + `eula=false`. Skipped in IDE
/// mode (matching the `!IS_RUNNING_IN_IDE` guard).
fn save_defaults(path: &Path) {
    if running_in_ide() {
        return;
    }
    let body = format!(
        "#By changing the setting below to TRUE you are indicating your agreement to our EULA ({EULA_URL}).\neula=false\n"
    );
    if let Err(e) = std::fs::write(path, body) {
        warn!(file = %path.display(), error = %e, "Failed to save {}", path.display());
    }
}

/// Parse the `eula` key. Vanilla loads `eula.txt` as a `java.util.Properties`
/// file, so we use the shared properties parser. Only the literal `true`
/// (case-insensitive) counts as agreed, matching Java's `Boolean.parseBoolean`.
fn parse_eula(text: &str) -> bool {
    super::properties::parse_properties(text)
        .get("eula")
        .is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

#[cfg(test)]
mod tests {
    use super::parse_eula;

    #[test]
    fn only_true_is_accepted() {
        assert!(parse_eula("eula=true"));
        assert!(parse_eula("#comment\neula=TRUE\n"));
        assert!(!parse_eula("eula=false"));
        assert!(!parse_eula("eula=yes"));
        assert!(!parse_eula(""));
    }
}
