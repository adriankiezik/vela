//! Per-player `<uuid>.dat` — a player's saved position, orientation, and held
//! hotbar slot. Stored as gzip-compressed NBT (an empty-named root compound), the
//! same framing vanilla `PlayerDataStorage.save` writes.
//!
//! Vanilla persists the full player entity (health, inventory, abilities, …).
//! Vela stores the movement/inventory subset the simulation currently owns, using
//! vanilla's own keys and formats so a saved file is byte-compatible: `Pos`,
//! `Rotation`, `OnGround`, `SelectedItemSlot`, the `Inventory` list and the
//! `equipment` compound (see [`crate::inventory::inventory_to_nbt`]), plus the
//! `DataVersion` stamp vanilla always writes.

use std::io::{self, Read, Write};
use std::path::Path;

use bytes::BytesMut;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use uuid::Uuid;

use crate::inventory::{inventory_from_nbt, inventory_to_nbt, ItemStack, PLAYER_INVENTORY_SLOTS};
use crate::protocol::nbt::{self, Nbt};

/// The MC 26.2 world save version (`SharedConstants.WORLD_VERSION`), written to
/// every player file as `DataVersion` so vanilla's `DataFixer` treats the data as
/// current rather than trying to upgrade it.
const DATA_VERSION: i32 = 4903;

/// A player's persisted state (the subset Vela currently tracks).
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerData {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    /// Selected hotbar slot (0..9), vanilla `Inventory.selected`.
    pub selected_slot: i32,
    /// The player's inventory in the 46-slot menu-store layout (see
    /// [`crate::inventory::inventory_to_nbt`] for the vanilla mapping).
    pub inventory: [Option<ItemStack>; PLAYER_INVENTORY_SLOTS],
}

impl PlayerData {
    /// Serialize to the player entity NBT compound.
    fn to_nbt(&self) -> Nbt {
        let (inventory, equipment) = inventory_to_nbt(&self.inventory);
        let mut entries = vec![
            ("DataVersion".to_string(), Nbt::Int(DATA_VERSION)),
            (
                "Pos".to_string(),
                Nbt::List(vec![Nbt::Double(self.x), Nbt::Double(self.y), Nbt::Double(self.z)]),
            ),
            (
                "Rotation".to_string(),
                Nbt::List(vec![Nbt::Float(self.yaw), Nbt::Float(self.pitch)]),
            ),
            ("OnGround".to_string(), Nbt::bool(self.on_ground)),
            ("Inventory".to_string(), Nbt::List(inventory)),
            ("SelectedItemSlot".to_string(), Nbt::Int(self.selected_slot)),
        ];
        // Vanilla omits `equipment` entirely when nothing is worn.
        if let Some(equipment) = equipment {
            entries.push(("equipment".to_string(), equipment));
        }
        Nbt::Compound(entries)
    }

    /// Parse the player entity NBT compound, defaulting missing fields.
    fn from_nbt(tag: &Nbt) -> Option<Self> {
        let (x, y, z) = match tag.get("Pos") {
            Some(Nbt::List(p)) if p.len() == 3 => (
                as_double(&p[0])?,
                as_double(&p[1])?,
                as_double(&p[2])?,
            ),
            _ => return None,
        };
        let (yaw, pitch) = match tag.get("Rotation") {
            Some(Nbt::List(r)) if r.len() == 2 => (as_float(&r[0])?, as_float(&r[1])?),
            _ => (0.0, 0.0),
        };
        let on_ground = matches!(tag.get("OnGround"), Some(Nbt::Byte(b)) if *b != 0);
        let selected_slot = match tag.get("SelectedItemSlot") {
            Some(Nbt::Int(s)) => *s,
            _ => 0,
        };
        let inventory_list = match tag.get("Inventory") {
            Some(Nbt::List(list)) => Some(list),
            _ => None,
        };
        let inventory = inventory_from_nbt(inventory_list, tag.get("equipment"));
        Some(Self {
            x,
            y,
            z,
            yaw,
            pitch,
            on_ground,
            selected_slot,
            inventory,
        })
    }

    /// Write to `dir/<uuid>.dat` as gzip-compressed NBT, using the atomic
    /// safe-replace pattern (temp + fsync + rename, keeping `<uuid>.dat_old`)
    /// so a crash mid-write cannot corrupt the player's live data. Matches
    /// vanilla `PlayerDataStorage.save`.
    pub fn save(&self, dir: &Path, uuid: Uuid) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let mut body = BytesMut::new();
        nbt::write_named(&mut body, "", &self.to_nbt());
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&body)?;
        super::safe_replace(&player_path(dir, uuid), &enc.finish()?)
    }

    /// Read `dir/<uuid>.dat`. `None` if the player has no saved data yet; an
    /// `Err` for an unreadable/corrupt file. Falls back to the `<uuid>.dat_old`
    /// safe-replace backup if the primary file is missing or fails to decode,
    /// matching vanilla `PlayerDataStorage.load`.
    pub fn load(dir: &Path, uuid: Uuid) -> io::Result<Option<Self>> {
        let primary = player_path(dir, uuid);
        match Self::load_file(&primary) {
            Ok(Some(data)) => Ok(Some(data)),
            // Primary absent or corrupt: try the `_old` fallback before giving up.
            Ok(None) | Err(_) => {
                let mut old = primary.into_os_string();
                old.push("_old");
                Self::load_file(Path::new(&old))
            }
        }
    }

    /// Read one gzip-NBT player file. `None` if absent; an `Err` for an
    /// unreadable/corrupt file.
    fn load_file(path: &Path) -> io::Result<Option<Self>> {
        let gz = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut raw = Vec::new();
        GzDecoder::new(&gz[..]).read_to_end(&mut raw)?;
        let mut slice = bytes::Bytes::from(raw);
        let (_, root) = nbt::read_named(&mut slice)?;
        Ok(Self::from_nbt(&root))
    }
}

/// The on-disk path for a player's data file (`<dir>/<uuid>.dat`), using the
/// canonical hyphenated UUID string vanilla uses.
fn player_path(dir: &Path, uuid: Uuid) -> std::path::PathBuf {
    dir.join(format!("{uuid}.dat"))
}

fn as_double(tag: &Nbt) -> Option<f64> {
    match tag {
        Nbt::Double(v) => Some(*v),
        _ => None,
    }
}

fn as_float(tag: &Nbt) -> Option<f32> {
    match tag {
        Nbt::Float(v) => Some(*v),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "vela-playerdata-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        p
    }

    fn sample() -> PlayerData {
        let mut inventory = [None; PLAYER_INVENTORY_SLOTS];
        inventory[36] = Some(ItemStack::new(1, 64)); // hotbar 0 -> vanilla slot 0
        inventory[9] = Some(ItemStack::new(55, 12)); // main -> vanilla slot 9
        inventory[6] = Some(ItemStack::new(1, 1)); // chest armor -> equipment
        PlayerData {
            x: 12.5,
            y: 71.0,
            z: -3.25,
            yaw: 45.0,
            pitch: -10.0,
            on_ground: true,
            selected_slot: 4,
            inventory,
        }
    }

    #[test]
    fn nbt_round_trips_in_memory() {
        let data = sample();
        assert_eq!(PlayerData::from_nbt(&data.to_nbt()), Some(data));
    }

    #[test]
    fn data_version_is_stamped() {
        assert!(matches!(sample().to_nbt().get("DataVersion"), Some(Nbt::Int(4903))));
    }

    #[test]
    fn inventory_survives_the_gzip_round_trip() {
        let dir = temp_dir();
        let uuid = Uuid::from_u128(0xabcd);
        let data = sample();
        data.save(&dir, uuid).unwrap();
        let loaded = PlayerData::load(&dir, uuid).unwrap().unwrap();
        assert_eq!(loaded.inventory, data.inventory);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_round_trips_through_gzip() {
        let dir = temp_dir();
        let uuid = Uuid::from_u128(0x1234_5678_9abc_def0_1122_3344_5566_7788);
        let data = sample();
        data.save(&dir, uuid).unwrap();
        assert_eq!(PlayerData::load(&dir, uuid).unwrap(), Some(data));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn absent_player_is_none() {
        let dir = temp_dir();
        let uuid = Uuid::from_u128(1);
        assert_eq!(PlayerData::load(&dir, uuid).unwrap(), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
