//! On-disk server configuration: the files a vanilla dedicated server reads from
//! its working directory.
//!
//! [`ServerConfig::load`] gathers them all at startup:
//!   * `server.properties` — typed settings ([`ServerProperties`]);
//!   * `eula.txt` — the EULA acknowledgement (compatibility only, see [`eula`]);
//!   * `ops.json` / `whitelist.json` / `banned-players.json` / `banned-ips.json`
//!     / `usercache.json` — the player lists ([`PlayerLists`]);
//!   * `server-icon.png` — the multiplayer-list favicon, base64-encoded.
//!
//! `level.dat` (the NBT world-data file) is deliberately out of scope here; it
//! belongs with world save/load.
//!
//! The loaded config is shared (`Arc`) across the network and simulation halves.

mod eula;
mod players;
mod properties;

use std::path::{Path, PathBuf};

use tracing::{info, warn};

pub use players::PlayerLists;
pub use properties::ServerProperties;

/// Everything Vela reads off disk at boot. Cheap to wrap in an `Arc` and clone
/// across connection tasks and the simulation thread.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub properties: ServerProperties,
    /// Loaded and round-tripped now; consumed by the login gate / permission
    /// system as those land.
    #[allow(dead_code)]
    pub players: PlayerLists,
    /// `server-icon.png` as a `data:image/png;base64,…` URI, if present and read.
    pub favicon: Option<String>,
}

impl ServerConfig {
    /// Load every config file from `dir` (the server's working directory),
    /// following vanilla's bootstrap order: write `server.properties` (creating
    /// it if absent), then check `eula.txt`. If the EULA is not agreed, log
    /// vanilla's message and return `None` — the caller exits without creating
    /// the world or player-list files, exactly as the dedicated server does.
    ///
    /// Once agreed, the player-list files are created (empty `[]`) and the
    /// favicon is read.
    pub fn load(dir: impl AsRef<Path>) -> Option<Self> {
        let dir = dir.as_ref();
        let properties = ServerProperties::load_or_create(dir.join("server.properties"));

        if !eula::check(dir.join("eula.txt")) {
            // Vanilla `Main`: Main.java logs this and returns before any further
            // files are created.
            info!("You need to agree to the EULA in order to run the server. Go to eula.txt for more info.");
            return None;
        }

        let players = PlayerLists::load_or_create(dir);
        let favicon = load_favicon(dir.join("server-icon.png"));

        info!(
            max_players = properties.max_players(),
            view_distance = properties.view_distance(),
            ops = players.ops.len(),
            whitelisted = players.whitelist.len(),
            favicon = favicon.is_some(),
            "configuration loaded"
        );
        Some(Self {
            properties,
            players,
            favicon,
        })
    }

    /// Load config from the current working directory — the normal entry point.
    /// Returns `None` when the EULA gate blocks startup (clean exit).
    pub fn load_from_cwd() -> Option<Self> {
        Self::load(PathBuf::from("."))
    }
}

/// Read `server-icon.png` and encode it as a data URI for the status response.
/// Returns `None` if the file is absent or unreadable (the server simply shows
/// no icon, as vanilla does).
fn load_favicon(path: impl AsRef<Path>) -> Option<String> {
    let path = path.as_ref();
    match std::fs::read(path) {
        Ok(bytes) => Some(format!("data:image/png;base64,{}", base64_encode(&bytes))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            warn!(file = %path.display(), error = %e, "failed to read server-icon.png");
            None
        }
    }
}

/// Standard base64 encoding (RFC 4648, with `=` padding). Hand-rolled to keep the
/// dependency set small — it's a dozen lines and only used for the favicon.
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18 & 0x3F) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6 & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64_encode;

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }
}
