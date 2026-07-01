//! World data representation, split into focused modules:
//!
//! * [`gen`] — world generation: noise heights, biomes, surface rules, features;
//! * [`chunk_data`] — chunk storage/lifecycle and the public block read/write API;
//! * [`encoding`] — the per-chunk block-section wire encoding;
//! * [`heightmap`] — the client-facing `WORLD_SURFACE`/`MOTION_BLOCKING` maps;
//! * [`bitpack`] — the `SimpleBitStorage` packing primitive underneath both;
//! * [`block_item`] — the item-id → block-state mapping for placement;
//! * [`block_drop`] — the reverse block-state → dropped-item mapping for breaks.
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

use crate::ids::BlockState;

mod bitpack;
pub mod block_drop;
mod block_item;
mod chunk_data;
mod encoding;
pub mod gen;
mod heightmap;
mod light;
pub mod storage;

pub use block_item::block_state_for_item;
pub use chunk_data::{
    block_state_at, chunk_columns, evict_chunk, evict_unused_chunks, save_dirty_chunks, set_block,
};
pub use gen::{seed, set_seed, spawn_column, surface_height, DEFAULT_SEED};
pub use light::ChunkLight;

// The wire-columns type is reached through `chunk_columns`' return value rather
// than named directly, but stays part of the public API surface.
#[allow(unused_imports)]
pub use chunk_data::ChunkColumns;

/// Serializes tests that mutate process-wide world singletons — the global chunk
/// store and the persistence handle. Those tests flip persistence on/off and
/// assert on eviction, which is only deterministic when no other such test runs
/// concurrently (cargo runs tests multithreaded). Hold this across the whole test
/// body; tests that merely read/generate far-apart chunks don't need it.
#[cfg(test)]
pub(crate) static WORLD_STATE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
pub const AIR_STATE: BlockState = BlockState(0);

/// Total world height in blocks (`SECTION_COUNT * 16`), and the exclusive top y.
const WORLD_HEIGHT: i32 = SECTION_COUNT * 16;
const MAX_Y_EXCL: i32 = MIN_Y + WORLD_HEIGHT;

/// Global block-state palette ids — the default state of each block, taken from
/// the server's block registration order in `Blocks.java` (AIR registered first
/// → state 0, STONE second → state 1) and the generated `reports/blocks.json`
/// for 26.2.
mod states {
    #![allow(dead_code)] // AIR/BEDROCK are hot; STONE/GRASS_BLOCK/DIRT back the tests.
    use crate::ids::BlockState;
    pub const AIR: BlockState = BlockState(0);
    /// STONE is the second block registered (single state) → state id 1.
    pub const STONE: BlockState = BlockState(1);
    pub const GRASS_BLOCK: BlockState = BlockState(9);
    pub const DIRT: BlockState = BlockState(10);
    pub const BEDROCK: BlockState = BlockState(85);
}
