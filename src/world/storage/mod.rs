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
use std::io::{self, Write};
use std::path::{Path, PathBuf};
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

/// Enable persistence rooted at `save_dir` (the runtime dir joined with the
/// configured `level-name`; see [`crate::runtime::dir`]), creating the `region`
/// and `playerdata` subdirectories. Call once at boot after config load. A
/// failure to create the directories logs and leaves persistence disabled rather
/// than aborting startup.
pub fn init(save_dir: impl AsRef<Path>) {
    let root = save_dir.as_ref().to_path_buf();
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

/// Serialize and write chunk `(cx, cz)` to its region file. `Ok(())` when
/// persistence is off (nothing to do). Propagates region-open and write errors
/// so the caller can keep the chunk dirty and retry on the next save — a failed
/// write must never silently drop the edit.
pub fn save_chunk(
    cx: i32,
    cz: i32,
    gen: &super::gen::GenChunk,
    edits: &HashMap<u32, BlockState>,
    game_time: i64,
) -> io::Result<()> {
    let mut guard = storage().lock().expect("storage mutex poisoned");
    let Some(storage) = guard.as_mut() else {
        return Ok(());
    };
    let tag = chunk_nbt::to_nbt(cx, cz, gen, edits, game_time);
    let mut body = bytes::BytesMut::new();
    crate::protocol::nbt::write_named(&mut body, "", &tag);
    let (lx, lz) = local(cx, cz);
    let region = storage.region(cx, cz).ok_or_else(|| {
        io::Error::other(format!("failed to open region for chunk ({cx}, {cz})"))
    })?;
    region.write_chunk(lx, lz, &body)
}

/// Close every cached region file whose region holds no chunk in `keep_chunks`
/// (the live working set), returning the number closed. Each open `RegionFile`
/// pins a ~10 KB header/bitmap **and an OS file handle**; without this the cache
/// grows one entry per region ever touched as a player travels, leaking handles.
/// A region is closed by dropping it from the map — pending writes already went
/// straight to the file (`std::fs::File` is unbuffered), so we `flush` (fsync)
/// first only for durability. The next touch of that region reopens it lazily.
pub fn evict_regions_except(keep_chunks: &std::collections::HashSet<(i32, i32)>) -> usize {
    let mut guard = storage().lock().expect("storage mutex poisoned");
    let Some(storage) = guard.as_mut() else {
        return 0;
    };
    let needed: std::collections::HashSet<(i32, i32)> = keep_chunks
        .iter()
        .map(|&(cx, cz)| (cx >> 5, cz >> 5))
        .collect();
    evict_regions_from(&mut storage.regions, &needed)
}

/// The region-eviction core, factored out of the global-storage wrapper so it can
/// be unit-tested against a local map. Retains regions in `needed` and closes the
/// rest (fsyncing each first for durability). Returns the number closed.
fn evict_regions_from(
    regions: &mut HashMap<(i32, i32), RegionFile>,
    needed: &std::collections::HashSet<(i32, i32)>,
) -> usize {
    let before = regions.len();
    regions.retain(|key, region| {
        if needed.contains(key) {
            return true;
        }
        if let Err(e) = region.flush() {
            warn!(rx = key.0, rz = key.1, error = %e, "failed to flush region file before closing");
        }
        false
    });
    before - regions.len()
}

/// Close the cached region file for `(rx, rz)`, fsyncing it first for durability,
/// if it is currently open. Called by [`super::evict_chunk`] when the region's
/// last resident chunk is evicted, so the open-file-handle cache stays bounded on
/// the incremental unload path (the single-region counterpart to
/// [`evict_regions_except`]). A no-op — returning `false` — when persistence is
/// off or the region isn't cached. The next touch of the region reopens it
/// lazily. Returns whether a handle was closed.
pub fn close_region(rx: i32, rz: i32) -> bool {
    let mut guard = storage().lock().expect("storage mutex poisoned");
    let Some(storage) = guard.as_mut() else {
        return false;
    };
    // Dropping the `RegionFile` closes it; pending writes already went straight to
    // the file (`std::fs::File` is unbuffered), so we `flush` (fsync) only for
    // durability before it goes away.
    if let Some(mut region) = storage.regions.remove(&(rx, rz)) {
        if let Err(e) = region.flush() {
            warn!(rx, rz, error = %e, "failed to flush region file before closing");
        }
        return true;
    }
    false
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

/// Atomically replace `path`'s contents with `bytes`, keeping the previous
/// version as `<path>_old`. Mirrors vanilla's safe-write pattern
/// (`LevelStorageSource`/`PlayerDataStorage`): write to a `<path>.tmp` sibling,
/// fsync it, roll the current file aside to `<path>_old`, then rename the temp
/// over the target. On any interruption the old file survives as the `_old`
/// fallback rather than leaving a half-written target.
fn safe_replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = with_suffix(path, ".tmp");
    let old = with_suffix(path, "_old");

    // Write the new contents to the temp file and force them to disk before we
    // touch the live target.
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }

    // Roll the current file (if any) aside as the `_old` fallback, then move the
    // freshly-synced temp into place.
    if path.exists() {
        let _ = std::fs::remove_file(&old);
        std::fs::rename(path, &old)?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Append a suffix to the final path component (`foo.dat` + `_old` -> `foo.dat_old`).
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
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

    #[test]
    fn region_eviction_closes_unreferenced_regions() {
        use std::collections::HashSet;
        // Build a *local* region map from temp files so this never touches the
        // process-wide storage handle (no lock, no race). Two regions open;
        // keeping a chunk only in region (0,0) must close region (2,2).
        let dir = std::env::temp_dir().join(format!(
            "vela-region-evict-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut regions: HashMap<(i32, i32), RegionFile> = HashMap::new();
        regions.insert((0, 0), RegionFile::open(&dir.join("r.0.0.mca")).unwrap());
        regions.insert((2, 2), RegionFile::open(&dir.join("r.2.2.mca")).unwrap());

        // Keep chunk (5,5) → region (0,0); region (2,2) has no kept chunk.
        let needed: HashSet<(i32, i32)> = [(0, 0)].into_iter().collect();
        let closed = evict_regions_from(&mut regions, &needed);

        assert_eq!(closed, 1, "the unreferenced region is closed");
        assert!(regions.contains_key(&(0, 0)), "the referenced region stays open");
        assert!(!regions.contains_key(&(2, 2)), "the unreferenced region is dropped");

        // Confirm the wrapper derives regions from chunk coords: chunk (5,5) → (0,0).
        assert_eq!((5i32 >> 5, 5i32 >> 5), (0, 0));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `close_region` closes exactly the named region's cached handle on the
    /// incremental unload path (called when a region's last resident chunk is
    /// evicted), leaving other regions open. Installs a temp global handle under
    /// the world-state lock, then restores it.
    #[test]
    fn close_region_closes_only_the_named_region() {
        let _guard = crate::world::WORLD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!(
            "vela-close-region-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("region")).unwrap();

        // Install a temp storage handle with two regions open.
        {
            let mut guard = storage().lock().expect("storage mutex poisoned");
            let mut regions: HashMap<(i32, i32), RegionFile> = HashMap::new();
            regions.insert((0, 0), RegionFile::open(&root.join("region/r.0.0.mca")).unwrap());
            regions.insert((3, 4), RegionFile::open(&root.join("region/r.3.4.mca")).unwrap());
            *guard = Some(Storage { root: root.clone(), regions });
        }

        assert!(close_region(3, 4), "an open region is closed and reports true");
        assert!(!close_region(3, 4), "closing an already-closed region is a no-op");
        {
            let guard = storage().lock().expect("storage mutex poisoned");
            let s = guard.as_ref().unwrap();
            assert!(s.regions.contains_key(&(0, 0)), "the other region stays open");
            assert!(!s.regions.contains_key(&(3, 4)), "the named region is closed");
        }

        // Persistence off: nothing to close.
        {
            let mut guard = storage().lock().expect("storage mutex poisoned");
            *guard = None;
        }
        assert!(!close_region(0, 0), "no-op when persistence is disabled");
        std::fs::remove_dir_all(&root).ok();
    }

    /// End-to-end: init a temp world, save an edited chunk, drop the region
    /// cache, and reload it — the decoded grid must match what we saved.
    #[test]
    fn chunk_survives_a_region_round_trip() {
        // Use a unique temp cwd-relative level name so the global handle points at
        // an isolated directory. We restore the previous handle afterwards.
        let _guard = crate::world::WORLD_STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let gen = super::super::gen::GenChunk::generate(cx, cz);
        let mut edits = HashMap::new();
        let key = |lx: i32, y: i32, lz: i32| {
            ((y - super::super::MIN_Y) as u32) * super::super::COLUMNS as u32
                + (lz as u32) * 16
                + lx as u32
        };
        edits.insert(key(2, 120, 3), BlockState(1)); // stone
        edits.insert(key(2, 121, 3), BlockState(10)); // dirt
        save_chunk(cx, cz, &gen, &edits, 7).expect("save");

        // Drop the region cache so the reload opens the file fresh from disk.
        {
            let mut guard = storage().lock().expect("storage mutex poisoned");
            if let Some(s) = guard.as_mut() {
                s.regions.clear();
            }
        }

        let grid = load_chunk(cx, cz).expect("reload");
        // Rebuild the expected grid from the baseline+edits and compare.
        let expected = chunk_nbt::from_nbt(&chunk_nbt::to_nbt(cx, cz, &gen, &edits, 7)).unwrap();
        assert_eq!(grid, expected);

        // Tear down: clear the global handle and remove the temp dir.
        {
            let mut guard = storage().lock().expect("storage mutex poisoned");
            *guard = None;
        }
        std::fs::remove_dir_all(&root).ok();
    }
}
