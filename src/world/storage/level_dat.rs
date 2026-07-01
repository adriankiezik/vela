//! `level.dat` — the world's top-level metadata: seed, spawn, clock, and game
//! rules. Stored as gzip-compressed NBT with an empty-named root compound whose
//! single `Data` child holds the fields, mirroring vanilla `NbtIo.writeCompressed`
//! + `PrimaryLevelData.setTagData`.
//!
//! Vela does not model vanilla's full world-generation settings block (dimension
//! stacks, noise routers, …), so this is a focused subset: the fields the server
//! actually owns and can round-trip. It is written well-formed enough for a
//! vanilla client to open, but is not a byte-for-byte `PrimaryLevelData`.

use std::io::{self, Read, Write};
use std::path::Path;

use bytes::BytesMut;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

use crate::protocol::nbt::{self, Nbt};

/// `SharedConstants.WORLD_VERSION` (MC 26.2).
const DATA_VERSION: i32 = 4903;
/// Legacy Anvil format marker vanilla still writes as `version` (`19133`).
const ANVIL_VERSION: i32 = 19133;

/// The world metadata Vela persists. A subset of vanilla `PrimaryLevelData`.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelData {
    pub level_name: String,
    pub seed: i64,
    /// Spawn position and orientation (feet position; yaw/pitch in degrees).
    pub spawn_x: i32,
    pub spawn_y: i32,
    pub spawn_z: i32,
    pub spawn_yaw: f32,
    pub spawn_pitch: f32,
    /// World age (`Time`) and time of day (`DayTime`), both in ticks.
    pub game_time: i64,
    pub day_time: i64,
    /// Game rules as `(id, value-string)` pairs, vanilla's serialized form.
    pub game_rules: Vec<(String, String)>,
}

impl LevelData {
    /// A fresh world's defaults for `level_name` and `seed`, spawning at the
    /// origin column top. Time starts at 0 (midnight of day 0), game rules empty
    /// (the caller fills them from its live `GameRules`).
    pub fn new(level_name: impl Into<String>, seed: i64, spawn_y: i32) -> Self {
        Self {
            level_name: level_name.into(),
            seed,
            spawn_x: 0,
            spawn_y,
            spawn_z: 0,
            spawn_yaw: 0.0,
            spawn_pitch: 0.0,
            game_time: 0,
            day_time: 0,
            game_rules: Vec::new(),
        }
    }

    /// Serialize to the `Data`-wrapped NBT compound (the named root's payload).
    fn to_nbt(&self) -> Nbt {
        let version = Nbt::compound([
            ("Name", Nbt::string("26.2")),
            ("Id", Nbt::Int(DATA_VERSION)),
            ("Snapshot", Nbt::bool(false)),
            ("Series", Nbt::string("main")),
        ]);
        // Spawn as the 26.2 `RespawnData` shape: a GlobalPos (dimension + packed
        // block-pos int array) plus yaw/pitch.
        let spawn = Nbt::compound([
            ("dimension", Nbt::string("minecraft:overworld")),
            (
                "pos",
                Nbt::IntArray(vec![self.spawn_x, self.spawn_y, self.spawn_z]),
            ),
            ("yaw", Nbt::Float(self.spawn_yaw)),
            ("pitch", Nbt::Float(self.spawn_pitch)),
        ]);
        let rules = Nbt::compound(
            self.game_rules
                .iter()
                .map(|(k, v)| (k.clone(), Nbt::string(v.clone()))),
        );

        let data = Nbt::compound([
            ("Version", version),
            ("DataVersion", Nbt::Int(DATA_VERSION)),
            ("version", Nbt::Int(ANVIL_VERSION)),
            ("LevelName", Nbt::string(self.level_name.clone())),
            ("seed", Nbt::Long(self.seed)),
            ("spawn", spawn),
            ("Time", Nbt::Long(self.game_time)),
            ("DayTime", Nbt::Long(self.day_time)),
            ("GameRules", rules),
            ("initialized", Nbt::bool(true)),
        ]);
        Nbt::compound([("Data", data)])
    }

    /// Parse a `Data`-wrapped NBT compound back into a `LevelData`. Missing
    /// optional fields fall back to sensible defaults; a wholly malformed tag
    /// (no `Data` compound) yields `None`.
    fn from_nbt(root: &Nbt) -> Option<Self> {
        let data = root.get("Data")?;
        let spawn = data.get("spawn");
        let (spawn_x, spawn_y, spawn_z) = match spawn.and_then(|s| s.get("pos")) {
            Some(Nbt::IntArray(p)) if p.len() == 3 => (p[0], p[1], p[2]),
            _ => (0, 0, 0),
        };
        let spawn_yaw = match spawn.and_then(|s| s.get("yaw")) {
            Some(Nbt::Float(f)) => *f,
            _ => 0.0,
        };
        let spawn_pitch = match spawn.and_then(|s| s.get("pitch")) {
            Some(Nbt::Float(f)) => *f,
            _ => 0.0,
        };
        let game_rules = match data.get("GameRules") {
            Some(Nbt::Compound(entries)) => entries
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect(),
            _ => Vec::new(),
        };

        Some(Self {
            level_name: data
                .get("LevelName")
                .and_then(Nbt::as_str)
                .unwrap_or("world")
                .to_string(),
            seed: long_or(data, "seed", 0),
            spawn_x,
            spawn_y,
            spawn_z,
            spawn_yaw,
            spawn_pitch,
            game_time: long_or(data, "Time", 0),
            day_time: long_or(data, "DayTime", 0),
            game_rules,
        })
    }

    /// Write to `path` as gzip-compressed, empty-named-root NBT, using the
    /// atomic safe-replace pattern (temp + fsync + rename, keeping `<path>_old`)
    /// so a crash mid-write cannot corrupt the live `level.dat`.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut body = BytesMut::new();
        nbt::write_named(&mut body, "", &self.to_nbt());
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&body)?;
        let gz = enc.finish()?;
        super::safe_replace(path, &gz)
    }

    /// Read from `path`, decompressing gzip and parsing the NBT. Returns `None`
    /// if the file is absent; an `Err` for an unreadable/corrupt file.
    pub fn load(path: &Path) -> io::Result<Option<Self>> {
        let gz = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut raw = Vec::new();
        GzDecoder::new(&gz[..]).read_to_end(&mut raw)?;
        let mut slice = bytes::Bytes::from(raw);
        let (_, root) = nbt::read_named(&mut slice)?;
        Self::from_nbt(&root)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "level.dat missing Data"))
            .map(Some)
    }
}

/// Read a `Long` child (`long_or` mirrors vanilla `getLongOr`).
fn long_or(tag: &Nbt, key: &str, default: i64) -> i64 {
    match tag.get(key) {
        Some(Nbt::Long(v)) => *v,
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "vela-level-{}-{}.dat",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        p
    }

    fn sample() -> LevelData {
        LevelData {
            level_name: "myworld".into(),
            seed: 0x5EED_C0DE,
            spawn_x: 8,
            spawn_y: 72,
            spawn_z: -4,
            spawn_yaw: 90.0,
            spawn_pitch: -12.5,
            game_time: 123_456,
            day_time: 6_000,
            game_rules: vec![
                ("advance_time".into(), "true".into()),
                ("keep_inventory".into(), "false".into()),
                ("random_tick_speed".into(), "3".into()),
            ],
        }
    }

    #[test]
    fn nbt_round_trips_in_memory() {
        let data = sample();
        let tag = data.to_nbt();
        assert_eq!(LevelData::from_nbt(&tag), Some(data));
    }

    #[test]
    fn file_round_trips_through_gzip() {
        let path = temp_path();
        let data = sample();
        data.save(&path).unwrap();
        let loaded = LevelData::load(&path).unwrap();
        assert_eq!(loaded, Some(data));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn absent_file_is_none() {
        let path = temp_path(); // never created
        assert_eq!(LevelData::load(&path).unwrap(), None);
    }
}
