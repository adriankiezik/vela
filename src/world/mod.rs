//! World data representation, split into focused modules:
//!
//! * [`terrain`] — the noise-based heightmap generator (`surface_height`);
//! * [`chunk_data`] — chunk storage/lifecycle and the public block read/write API;
//! * [`encoding`] — the per-chunk block-section wire encoding;
//! * [`heightmap`] — the client-facing `WORLD_SURFACE`/`MOTION_BLOCKING` maps;
//! * [`bitpack`] — the `SimpleBitStorage` packing primitive underneath both;
//! * [`block_item`] — the item-id → block-state mapping for placement.
//!
//! A chunk column is 24 stacked sections of 16×16×16 cells rising from the
//! world floor (`MIN_Y` = -64). Each section serializes exactly as vanilla's
//! `LevelChunkSection`: a non-air block count, a fluid count, a block-state
//! `PalettedContainer`, then a biome `PalettedContainer`. We emit the wire bytes
//! for a *static* world directly rather than modelling the full mutable
//! container — enough to stream a generated world a chunk at a time.
//!
//! Reference: decompiled `LevelChunkSection`, `PalettedContainer`, `Strategy`,
//! and `Heightmap` (MC 26.2). The numeric block-state ids come from the server's
//! own block registration order (`Blocks.java`) / `--reports` block dump
//! (observable output), not copied source.

mod bitpack;
mod block_item;
mod chunk_data;
mod encoding;
mod heightmap;
mod terrain;

pub use block_item::block_state_for_item;
pub use chunk_data::{block_state_at, chunk_columns, set_block};
pub use terrain::surface_height;

// The wire-columns type is reached through `chunk_columns`' return value rather
// than named directly, but stays part of the public API surface.
#[allow(unused_imports)]
pub use chunk_data::ChunkColumns;

/// World floor. Sections stack upward from here; the overworld is 384 blocks
/// tall, so 24 sections of 16.
pub const MIN_Y: i32 = -64;
/// Sections per column (384 / 16).
pub const SECTION_COUNT: i32 = 24;
/// Cells per 16×16×16 section.
const CELLS: usize = 16 * 16 * 16;
/// Columns per chunk (16×16), one heightmap entry each.
const COLUMNS: usize = 16 * 16;

/// Reference surface height. Terrain is centred on this so a player spawned at
/// y=64 lands on the ground near the origin.
pub const SURFACE_Y: i32 = 63;

/// The air block-state id (palette 0) — the "empty" cell and the result of a
/// break. Public so the simulation can place/clear blocks without reaching into
/// the private `states` table.
pub const AIR_STATE: u32 = 0;

/// Total world height in blocks (`SECTION_COUNT * 16`), and the exclusive top y.
const WORLD_HEIGHT: i32 = SECTION_COUNT * 16;
const MAX_Y_EXCL: i32 = MIN_Y + WORLD_HEIGHT;

/// The biome a section's biome `PalettedContainer` reports, as a *network*
/// registry index into the biome registry we sync in `crate::registry`. Index
/// 39 is `minecraft:plains` in that list — a sensible match for green grassy
/// terrain (index 0 would be `badlands`, which tints grass orange). The whole
/// world reports this single biome for now.
const PLAINS_BIOME: u32 = 39;

/// Global block-state palette ids — the default state of each block, taken from
/// the server's block registration order in `Blocks.java` (AIR registered first
/// → state 0, STONE second → state 1) and the generated `reports/blocks.json`
/// for 26.2.
mod states {
    pub const AIR: u32 = 0;
    /// STONE is the second block registered (single state) → state id 1.
    pub const STONE: u32 = 1;
    pub const GRASS_BLOCK: u32 = 9;
    pub const DIRT: u32 = 10;
    pub const BEDROCK: u32 = 85;
}
