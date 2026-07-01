//! Biome assignment and per-biome surface material / decoration parameters.
//!
//! A believable overworld from a small climate field: two low-frequency noise
//! layers (temperature, humidity) pick a land biome, and the column's height
//! relative to sea level overrides it to ocean/beach where appropriate. Biomes
//! feed three things: the surface block choice ([`super::surface`]), the chunk's
//! biome `PalettedContainer` (via [`network_id`]), and the decoration pass
//! ([`super::feature`]).
//!
//! This is a hand-picked subset, not vanilla's `MultiNoiseBiomeSource` parameter
//! space — enough variety (plains, forests, taiga, desert, savanna, jungle,
//! snowy, ocean, beach, hills, swamp) to look like Minecraft.

use crate::ids::BlockState;
use crate::registry::SYNCED;

use super::blocks::{get as blocks, Blocks};

/// The handful of biomes the generator can assign.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Biome {
    Ocean,
    Beach,
    Plains,
    SunflowerPlains,
    Forest,
    BirchForest,
    DarkForest,
    Taiga,
    SnowyTaiga,
    SnowyPlains,
    Desert,
    Savanna,
    Jungle,
    Swamp,
    WindsweptHills,
    StonyShore,
}

/// The tree species a biome decorates with (or none).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TreeKind {
    None,
    Oak,
    Birch,
    Spruce,
    Jungle,
    Acacia,
    DarkOak,
}

impl Biome {
    /// The registry id string (`minecraft:…`) for this biome.
    pub fn name(self) -> &'static str {
        match self {
            Biome::Ocean => "minecraft:ocean",
            Biome::Beach => "minecraft:beach",
            Biome::Plains => "minecraft:plains",
            Biome::SunflowerPlains => "minecraft:sunflower_plains",
            Biome::Forest => "minecraft:forest",
            Biome::BirchForest => "minecraft:birch_forest",
            Biome::DarkForest => "minecraft:dark_forest",
            Biome::Taiga => "minecraft:taiga",
            Biome::SnowyTaiga => "minecraft:snowy_taiga",
            Biome::SnowyPlains => "minecraft:snowy_plains",
            Biome::Desert => "minecraft:desert",
            Biome::Savanna => "minecraft:savanna",
            Biome::Jungle => "minecraft:jungle",
            Biome::Swamp => "minecraft:swamp",
            Biome::WindsweptHills => "minecraft:windswept_hills",
            Biome::StonyShore => "minecraft:stony_shore",
        }
    }

    /// The network registry index for this biome — its position in the synced
    /// `minecraft:worldgen/biome` list, which the client indexes into when
    /// decoding the chunk biome palette. Derived from the registry so it can't
    /// drift from the wire order.
    pub fn network_id(self) -> u32 {
        network_id(self.name())
    }

    /// Whether the biome is cold enough to freeze surface water and cap grass with
    /// a snow layer.
    pub fn is_snowy(self) -> bool {
        matches!(self, Biome::SnowyPlains | Biome::SnowyTaiga)
    }

    /// The block placed at the very top of a dry land column.
    pub fn top_block(self, b: &Blocks) -> BlockState {
        match self {
            Biome::Desert => b.sand,
            Biome::Beach => b.sand,
            Biome::StonyShore | Biome::WindsweptHills => b.stone,
            Biome::Taiga | Biome::SnowyTaiga => b.grass_block,
            Biome::SnowyPlains => b.grass_block,
            _ => b.grass_block,
        }
    }

    /// The filler placed just under the top block (the 3–4 blocks above stone).
    pub fn filler_block(self, b: &Blocks) -> BlockState {
        match self {
            Biome::Desert | Biome::Beach => b.sand,
            Biome::StonyShore | Biome::WindsweptHills => b.stone,
            _ => b.dirt,
        }
    }

    /// The material an underwater floor of this biome shows (ocean/river beds).
    pub fn underwater_block(self, b: &Blocks) -> BlockState {
        match self {
            Biome::Swamp => b.dirt,
            _ => b.sand,
        }
    }

    /// The tree species and roughly how many trees to try per chunk.
    pub fn trees(self) -> (TreeKind, i32) {
        match self {
            Biome::Forest => (TreeKind::Oak, 8),
            Biome::BirchForest => (TreeKind::Birch, 8),
            Biome::DarkForest => (TreeKind::DarkOak, 9),
            Biome::Taiga => (TreeKind::Spruce, 8),
            Biome::SnowyTaiga => (TreeKind::Spruce, 6),
            Biome::Jungle => (TreeKind::Jungle, 10),
            Biome::Savanna => (TreeKind::Acacia, 2),
            Biome::Plains | Biome::SunflowerPlains => (TreeKind::Oak, 1),
            Biome::Swamp => (TreeKind::Oak, 2),
            Biome::WindsweptHills => (TreeKind::Spruce, 2),
            _ => (TreeKind::None, 0),
        }
    }

    /// Rough count of grass/fern tufts to scatter per chunk.
    pub fn grass_tufts(self) -> i32 {
        match self {
            Biome::Plains | Biome::SunflowerPlains => 10,
            Biome::Forest | Biome::BirchForest | Biome::DarkForest => 5,
            Biome::Taiga | Biome::SnowyTaiga => 6,
            Biome::Savanna => 14,
            Biome::Jungle => 24,
            Biome::Swamp => 4,
            _ => 2,
        }
    }

    /// Rough count of flowers to scatter per chunk.
    pub fn flowers(self) -> i32 {
        match self {
            Biome::Plains | Biome::SunflowerPlains => 4,
            Biome::Forest => 2,
            Biome::BirchForest => 2,
            _ => 1,
        }
    }
}

/// The synced biome registry list, in wire order.
fn biome_registry() -> &'static [&'static str] {
    SYNCED
        .iter()
        .find(|(reg, _)| *reg == "minecraft:worldgen/biome")
        .map(|(_, entries)| *entries)
        .expect("worldgen/biome registry is synced")
}

/// The network index of a biome name (its position in [`biome_registry`]).
pub fn network_id(name: &str) -> u32 {
    biome_registry()
        .iter()
        .position(|n| *n == name)
        .unwrap_or(0) as u32
}

/// The number of biomes in the synced registry — the id space the biome
/// `PalettedContainer`'s global palette indexes into.
pub fn registry_size() -> usize {
    biome_registry().len()
}

/// Classify a column into a biome from its climate (`temperature`, `humidity`,
/// each in `[-1, 1]`), its height, and the sea level. Ocean/beach are height
/// overrides; the rest is a coarse temperature×humidity matrix.
pub fn classify(temperature: f64, humidity: f64, height: i32, sea_level: i32) -> Biome {
    // Deep water columns are ocean; the shallow shelf just under/at sea level is
    // beach (sandy) on warm-to-temperate coasts, stony shore on cold ones.
    if height < sea_level - 4 {
        return Biome::Ocean;
    }
    if height <= sea_level + 1 {
        return if temperature < -0.3 {
            Biome::StonyShore
        } else {
            Biome::Beach
        };
    }

    // High, exposed land reads as windswept hills regardless of climate.
    if height > sea_level + 34 {
        return Biome::WindsweptHills;
    }

    // Cold band.
    if temperature < -0.35 {
        return if humidity > 0.0 {
            Biome::SnowyTaiga
        } else {
            Biome::SnowyPlains
        };
    }
    // Cool band.
    if temperature < 0.0 {
        return if humidity > 0.2 {
            Biome::Taiga
        } else if humidity > -0.2 {
            Biome::Forest
        } else {
            Biome::Plains
        };
    }
    // Temperate band.
    if temperature < 0.45 {
        if humidity > 0.45 {
            return Biome::Swamp;
        }
        if humidity > 0.15 {
            return Biome::BirchForest;
        }
        if humidity > -0.1 {
            return Biome::Forest;
        }
        // Drier temperate flats: the flower-rich sunflower variant on the damper
        // edge, plain grass otherwise.
        return if humidity > -0.3 {
            Biome::SunflowerPlains
        } else {
            Biome::Plains
        };
    }
    // Warm band.
    if humidity < -0.2 {
        return Biome::Desert;
    }
    if humidity > 0.35 {
        return Biome::Jungle;
    }
    if humidity > 0.0 {
        return Biome::DarkForest;
    }
    Biome::Savanna
}

/// Convenience: the top/filler/underwater material triple for a biome, resolved
/// against the shared palette.
pub fn materials(biome: Biome) -> (BlockState, BlockState, BlockState) {
    let b = blocks();
    (
        biome.top_block(b),
        biome.filler_block(b),
        biome.underwater_block(b),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_ids_resolve_from_registry() {
        // Spot-check a couple of biomes against the synced list order.
        assert_eq!(network_id("minecraft:plains"), Biome::Plains.network_id());
        assert!(Biome::Ocean.network_id() < registry_size() as u32);
        assert!(Biome::Desert.network_id() < registry_size() as u32);
    }

    #[test]
    fn deep_columns_are_ocean_and_high_are_hills() {
        assert_eq!(classify(0.0, 0.0, 40, 63), Biome::Ocean);
        assert_eq!(classify(0.6, 0.6, 120, 63), Biome::WindsweptHills);
    }

    #[test]
    fn warm_dry_is_desert() {
        assert_eq!(classify(0.8, -0.6, 70, 63), Biome::Desert);
    }
}
