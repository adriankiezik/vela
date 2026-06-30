//! The server's JSON player-list files: `ops.json`, `whitelist.json`,
//! `banned-players.json`, `banned-ips.json`, and `usercache.json`.
//!
//! Each is a flat JSON array of entries. We load them at startup and keep the
//! parsed lists in memory; enforcement (whitelist gate, ban checks, op-level
//! permission lookups) lands with the subsystems that consume them. The structs
//! mirror the decompiled 26.2 entry shapes exactly so files are interchangeable
//! with a vanilla server.
//!
//! UUIDs and timestamps are kept as strings: the on-disk form is canonical and
//! we have no reason to reparse them yet (timestamps use vanilla's
//! `yyyy-MM-dd HH:mm:ss Z`).

use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::warn;

/// An operator entry in `ops.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpEntry {
    pub uuid: String,
    pub name: String,
    /// Permission level 0..=4 (4 = owner).
    pub level: u8,
    #[serde(rename = "bypassesPlayerLimit", default)]
    pub bypasses_player_limit: bool,
}

/// A whitelist entry in `whitelist.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhitelistEntry {
    pub uuid: String,
    pub name: String,
}

/// A player ban entry in `banned-players.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BannedPlayerEntry {
    pub uuid: String,
    pub name: String,
    pub created: String,
    pub source: String,
    /// `"forever"` or a `yyyy-MM-dd HH:mm:ss Z` timestamp.
    pub expires: String,
    pub reason: String,
}

/// An IP ban entry in `banned-ips.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BannedIpEntry {
    pub ip: String,
    pub created: String,
    pub source: String,
    pub expires: String,
    pub reason: String,
}

/// A name→uuid cache entry in `usercache.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserCacheEntry {
    pub uuid: String,
    pub name: String,
    #[serde(rename = "expiresOn")]
    pub expires_on: String,
}

/// All player-list files loaded together. Each list is empty when its file is
/// absent (the common first-run case).
///
/// The ban/cache lists are loaded and round-tripped now; enforcement (ban
/// checks at the login gate, the name→uuid cache) arrives with those subsystems,
/// so they are `allow(dead_code)` until then.
#[derive(Debug, Clone, Default)]
pub struct PlayerLists {
    pub ops: Vec<OpEntry>,
    pub whitelist: Vec<WhitelistEntry>,
    #[allow(dead_code)]
    pub banned_players: Vec<BannedPlayerEntry>,
    #[allow(dead_code)]
    pub banned_ips: Vec<BannedIpEntry>,
    #[allow(dead_code)]
    pub user_cache: Vec<UserCacheEntry>,
}

impl PlayerLists {
    /// Load all the player-list files from `dir` (the server root), creating any
    /// that are missing with an empty `[]` body — exactly as vanilla's
    /// `DedicatedPlayerList` constructor does on first run.
    ///
    /// The ban/op/whitelist files are created in vanilla's order
    /// (banned-players → banned-ips → ops → whitelist). `usercache.json` is the
    /// exception: vanilla only loads it and writes it once a player joins, so we
    /// load it without creating it.
    pub fn load_or_create(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        Self {
            banned_players: load_or_create_array(dir.join("banned-players.json")),
            banned_ips: load_or_create_array(dir.join("banned-ips.json")),
            ops: load_or_create_array(dir.join("ops.json")),
            whitelist: load_or_create_array(dir.join("whitelist.json")),
            user_cache: load_array(dir.join("usercache.json")),
        }
    }
}

/// Read a JSON array of `T` from `path`, returning an empty `Vec` if the file is
/// absent and logging (then returning empty) on a parse error.
fn load_array<T: for<'de> Deserialize<'de>>(path: impl AsRef<Path>) -> Vec<T> {
    let path = path.as_ref();
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(list) => list,
            Err(e) => {
                warn!(file = %path.display(), error = %e, "malformed JSON list; ignoring");
                Vec::new()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            warn!(file = %path.display(), error = %e, "failed to read JSON list");
            Vec::new()
        }
    }
}

/// Load a JSON array of `T` from `path`, creating it with an empty `[]` body if
/// absent (vanilla `load()` followed by `save()`).
fn load_or_create_array<T>(path: impl AsRef<Path>) -> Vec<T>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    let path = path.as_ref();
    if !path.exists() {
        if let Err(e) = save_array::<T>(path, &[]) {
            warn!(file = %path.display(), error = %e, "failed to create JSON list");
        }
        return Vec::new();
    }
    load_array(path)
}

/// Write a JSON array of `T` to `path` (pretty-printed like vanilla's GSON
/// output). Used at first-run creation and once commands can mutate the lists.
pub fn save_array<T: Serialize>(path: impl AsRef<Path>, list: &[T]) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(list).unwrap_or_else(|_| "[]".to_string());
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vanilla_ops_shape() {
        let json = r#"[{"uuid":"00000000-0000-0000-0000-000000000000","name":"Steve","level":4,"bypassesPlayerLimit":false}]"#;
        let ops: Vec<OpEntry> = serde_json::from_str(json).unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].name, "Steve");
        assert_eq!(ops[0].level, 4);
        assert!(!ops[0].bypasses_player_limit);
    }

    #[test]
    fn op_round_trips_with_camelcase_key() {
        let op = OpEntry {
            uuid: "u".into(),
            name: "n".into(),
            level: 2,
            bypasses_player_limit: true,
        };
        let text = serde_json::to_string(&[op]).unwrap();
        assert!(text.contains("\"bypassesPlayerLimit\":true"));
    }

    #[test]
    fn first_run_creates_empty_lists_like_vanilla() {
        let dir = std::env::temp_dir().join(format!("vela-players-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Clean slate.
        for f in [
            "banned-players.json",
            "banned-ips.json",
            "ops.json",
            "whitelist.json",
            "usercache.json",
        ] {
            std::fs::remove_file(dir.join(f)).ok();
        }

        let lists = PlayerLists::load_or_create(&dir);
        assert!(lists.ops.is_empty());
        assert!(lists.whitelist.is_empty());
        assert!(lists.banned_players.is_empty());

        // The four ban/op/whitelist files are created empty; usercache is not
        // (vanilla only writes it on first join).
        assert!(dir.join("banned-players.json").exists());
        assert!(dir.join("banned-ips.json").exists());
        assert!(dir.join("ops.json").exists());
        assert!(dir.join("whitelist.json").exists());
        assert!(!dir.join("usercache.json").exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("ops.json")).unwrap(),
            "[]"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
