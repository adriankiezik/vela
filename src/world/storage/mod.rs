//! World persistence: the Anvil save directory and its region-file cache.
//!
//! This module owns the on-disk world layout and exposes a small free-function
//! API the rest of the server calls without holding a handle — mirroring the
//! process-global chunk store in [`super::chunk_data`]. Persistence is *opt-in*:
//! until [`init`] runs (at server boot), every read returns "absent" and every
//! write is a no-op, so unit tests that touch the chunk store never hit disk.
//!
//! Directory layout (rooted at the `level-name` from `server.properties`):
//!
//! ```text
//! <level-name>/
//! ├── level.dat                world metadata (see [`level_dat`])
//! ├── region/r.<rx>.<rz>.mca   32×32-chunk region files (see [`region`])
//! └── playerdata/<uuid>.dat    per-player state (see [`player_dat`])
//! ```
//!
//! Region files are cached open per `(rx, rz)`; a `.mca` holds a 32×32 block of
//! chunks, so `(cx, cz)` maps to region `(cx >> 5, cz >> 5)` at local
//! `(cx & 31, cz & 31)`.

mod chunk_nbt;
pub mod level_dat;
pub mod player_dat;
mod region;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use tracing::{warn, error};

use region::RegionFile;

use crate::ids::BlockState;

pub use level_dat::LevelData;
pub use player_dat::PlayerData;

/// The active save directory plus its open-region-file cache. Absent until
/// [`init`] runs, in which case all persistence is disabled.
struct Storage {
    root: PathBuf,
    /// Open region files keyed by region coord `(rx, rz)`. Opened lazily.
    regions: HashMap<(i32, i32), RegionFile>,
}

/// Process-wide storage handle. `None` until `init`; guarded by a `Mutex` since,
/// like the chunk store, nothing about the API promises single-threaded access.
fn storage() -> &'static Mutex<Option<Storage>> {
    static STORAGE: OnceLock<Mutex<Option<Storage>>> = OnceLock::new();
    STORAGE.get_or_init(|| Mutex::new(None))
}

/// Enable persistence rooted at `<cwd>/<level_name>`, creating the `region` and
/// `playerdata` subdirectories. Call once at boot after config load. A failure
/// to create the directories logs and leaves persistence disabled rather than
/// aborting startup.
pub fn init(level_name: &str) {
    let root = PathBuf::from(level_name);
    if let Err(e) = std::fs::create_dir_all(root.join("region")) {
        error!(dir = %root.display(), error = %e, "failed to create region directory; persistence disabled");
        return;
    }
    if let Err(e) = std::fs::create_dir_all(root.join("playerdata")) {
        error!(dir = %root.display(), error = %e, "failed to create playerdata directory; persistence disabled");
        return;
    }
    *storage().lock().expect("storage mutex poisoned") = Some(Storage {
        root,
        regions: HashMap::new(),
    });
}

/// Whether persistence is active (i.e. [`init`] ran successfully).
pub fn is_enabled() -> bool {
    storage().lock().expect("storage mutex poisoned").is_some()
}

/// The save-directory paths, if persistence is enabled.
pub fn level_dat_path() -> Option<PathBuf> {
    let guard = storage().lock().expect("storage mutex poisoned");
    guard.as_ref().map(|s| s.root.join("level.dat"))
}

/// The `playerdata` directory, if persistence is enabled.
pub fn player_data_dir() -> Option<PathBuf> {
    let guard = storage().lock().expect("storage mutex poisoned");
    guard.as_ref().map(|s| s.root.join("playerdata"))
}

/// Load chunk `(cx, cz)` from its region file, returning the dense block-state
/// grid (`section * CELLS + cell`) if a payload exists and decodes. `None` when
/// persistence is off, the chunk is absent, or the payload is unreadable (an
/// unreadable chunk is logged and regenerated rather than failing the load).
pub fn load_chunk(cx: i32, cz: i32) -> Option<Vec<BlockState>> {
    let mut guard = storage().lock().expect("storage mutex poisoned");
    let storage = guard.as_mut()?;
    let region = storage.region(cx, cz)?;
    let (lx, lz) = local(cx, cz);
    match region.read_chunk(lx, lz) {
        Ok(Some(bytes)) => decode_chunk(cx, cz, &bytes),
        Ok(None) => None,
        Err(e) => {
            warn!(cx, cz, error = %e, "failed to read chunk from region; regenerating");
            None
        }
    }
}

/// Serialize and write chunk `(cx, cz)` to its region file. A no-op when
/// persistence is off. Errors are logged, not propagated — a failed save must
/// not crash the tick.
pub fn save_chunk(
    cx: i32,
    cz: i32,
    heights: &[i32; super::COLUMNS],
    edits: &HashMap<u32, BlockState>,
    game_time: i64,
) {
    let mut guard = storage().lock().expect("storage mutex poisoned");
    let Some(storage) = guard.as_mut() else {
        return;
    };
    let tag = chunk_nbt::to_nbt(cx, cz, heights, edits, game_time);
    let mut body = bytes::BytesMut::new();
    crate::protocol::nbt::write_named(&mut body, "", &tag);
    let (lx, lz) = local(cx, cz);
    let Some(region) = storage.region(cx, cz) else {
        return;
    };
    if let Err(e) = region.write_chunk(lx, lz, &body) {
        warn!(cx, cz, error = %e, "failed to write chunk to region");
    }
}

/// Flush all open region files to the OS (called on periodic save / shutdown).
pub fn flush() {
    let mut guard = storage().lock().expect("storage mutex poisoned");
    if let Some(storage) = guard.as_mut() {
        for ((rx, rz), region) in storage.regions.iter_mut() {
            if let Err(e) = region.flush() {
                warn!(rx, rz, error = %e, "failed to flush region file");
            }
        }
    }
}

impl Storage {
    /// The open region file for chunk `(cx, cz)`, opening (and caching) it on
    /// first touch. `None` if the file cannot be opened (logged by the caller
    /// path via the returned option collapsing to a regenerate/no-op).
    fn region(&mut self, cx: i32, cz: i32) -> Option<&mut RegionFile> {
        let key = (cx >> 5, cz >> 5);
        if !self.regions.contains_key(&key) {
            let path = self
                .root
                .join("region")
                .join(format!("r.{}.{}.mca", key.0, key.1));
            match RegionFile::open(&path) {
                Ok(region) => {
                    self.regions.insert(key, region);
                }
                Err(e) => {
                    error!(path = %path.display(), error = %e, "failed to open region file");
                    return None;
                }
            }
        }
        self.regions.get_mut(&key)
    }
}

/// The chunk's local coordinates within its region (`chunk & 31`).
fn local(cx: i32, cz: i32) -> (usize, usize) {
    ((cx & 31) as usize, (cz & 31) as usize)
}

/// Decompress-then-decode already happened in the region layer; here we parse the
/// NBT bytes and turn them into the dense grid. A parse failure is logged and
/// treated as "regenerate".
fn decode_chunk(cx: i32, cz: i32, bytes: &[u8]) -> Option<Vec<BlockState>> {
    let mut slice = bytes::Bytes::copy_from_slice(bytes);
    let (_, tag) = match crate::protocol::nbt::read_named(&mut slice) {
        Ok(v) => v,
        Err(e) => {
            warn!(cx, cz, error = %e, "corrupt chunk NBT; regenerating");
            return None;
        }
    };
    chunk_nbt::from_nbt(&tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end: init a temp world, save an edited chunk, drop the region
    /// cache, and reload it — the decoded grid must match what we saved.
    #[test]
    fn chunk_survives_a_region_round_trip() {
        // Use a unique temp cwd-relative level name so the global handle points at
        // an isolated directory. We restore the previous handle afterwards.
        let name = format!(
            "vela-world-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(&name);
        std::fs::create_dir_all(root.join("region")).unwrap();
        std::fs::create_dir_all(root.join("playerdata")).unwrap();

        // Install a storage handle pointed at the temp root directly.
        {
            let mut guard = storage().lock().expect("storage mutex poisoned");
            *guard = Some(Storage {
                root: root.clone(),
                regions: HashMap::new(),
            });
        }

        // Save a chunk with a couple of edits at a far-away coord.
        let (cx, cz) = (40, -70);
        let heights = super::super::chunk_data::chunk_heights(cx, cz);
        let mut edits = HashMap::new();
        let key = |lx: i32, y: i32, lz: i32| {
            ((y - super::super::MIN_Y) as u32) * super::super::COLUMNS as u32
                + (lz as u32) * 16
                + lx as u32
        };
        edits.insert(key(2, 120, 3), BlockState(1)); // stone
        edits.insert(key(2, 121, 3), BlockState(10)); // dirt
        save_chunk(cx, cz, &heights, &edits, 7);

        // Drop the region cache so the reload opens the file fresh from disk.
        {
            let mut guard = storage().lock().expect("storage mutex poisoned");
            if let Some(s) = guard.as_mut() {
                s.regions.clear();
            }
        }

        let grid = load_chunk(cx, cz).expect("reload");
        // Rebuild the expected grid from heights+edits and compare.
        let expected = chunk_nbt::from_nbt(&chunk_nbt::to_nbt(cx, cz, &heights, &edits, 7)).unwrap();
        assert_eq!(grid, expected);

        // Tear down: clear the global handle and remove the temp dir.
        {
            let mut guard = storage().lock().expect("storage mutex poisoned");
            *guard = None;
        }
        std::fs::remove_dir_all(&root).ok();
    }
}
