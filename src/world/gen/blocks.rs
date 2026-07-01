//! Block-state ids the generator writes, resolved once from the block registry.
//!
//! Per the project rule we never hard-code palette magic numbers here: every id
//! comes from [`crate::registry::block_state`] (`default_state_of` /
//! `with_properties`) against the generated global palette, so a registry change
//! keeps the generator honest. Resolution happens once behind a `OnceLock`.

use std::sync::OnceLock;

use crate::ids::BlockState;
use crate::registry::block_state::{default_state_of, with_properties};

/// A resolved block-state id, falling back to a sane default if the name is ever
/// absent from the palette (keeps generation total rather than panicking).
fn solid(name: &str) -> BlockState {
    // Stone is the universal solid fallback (state id 1).
    BlockState(default_state_of(name).unwrap_or(1))
}

/// A resolved plant/decoration state, falling back to air (state 0) if absent —
/// a missing decoration simply doesn't place rather than corrupting the section.
fn plant(name: &str) -> BlockState {
    BlockState(default_state_of(name).unwrap_or(0))
}

/// An upright log (`axis=y`); logs default to `axis=y` but we ask explicitly so a
/// registry reorder can't silently rotate every tree.
fn log(name: &str) -> BlockState {
    BlockState(with_properties(name, &[("axis", "y")]).or_else(|| default_state_of(name)).unwrap_or(1))
}

/// The generator's block-state palette, resolved from the registry once. A few
/// entries are held for completeness / future features and may be unused today.
#[allow(dead_code)]
pub struct Blocks {
    pub air: BlockState,
    pub stone: BlockState,
    pub deepslate: BlockState,
    pub dirt: BlockState,
    pub coarse_dirt: BlockState,
    pub grass_block: BlockState,
    pub podzol: BlockState,
    pub bedrock: BlockState,
    pub water: BlockState,
    pub lava: BlockState,
    pub sand: BlockState,
    pub red_sand: BlockState,
    pub sandstone: BlockState,
    pub gravel: BlockState,
    pub snow_block: BlockState,
    pub snow_layer: BlockState,
    pub ice: BlockState,
    pub packed_ice: BlockState,
    pub clay: BlockState,

    // Ore blobs, stone- and deepslate-hosted variants.
    pub coal_ore: BlockState,
    pub deepslate_coal_ore: BlockState,
    pub iron_ore: BlockState,
    pub deepslate_iron_ore: BlockState,
    pub copper_ore: BlockState,
    pub deepslate_copper_ore: BlockState,
    pub gold_ore: BlockState,
    pub deepslate_gold_ore: BlockState,
    pub redstone_ore: BlockState,
    pub deepslate_redstone_ore: BlockState,
    pub lapis_ore: BlockState,
    pub deepslate_lapis_ore: BlockState,
    pub diamond_ore: BlockState,
    pub deepslate_diamond_ore: BlockState,
    pub emerald_ore: BlockState,
    pub deepslate_emerald_ore: BlockState,
    // Stone-blob variants scattered in the fill.
    pub granite: BlockState,
    pub diorite: BlockState,
    pub andesite: BlockState,
    pub tuff: BlockState,

    // Trees.
    pub oak_log: BlockState,
    pub oak_leaves: BlockState,
    pub birch_log: BlockState,
    pub birch_leaves: BlockState,
    pub spruce_log: BlockState,
    pub spruce_leaves: BlockState,
    pub jungle_log: BlockState,
    pub jungle_leaves: BlockState,
    pub acacia_log: BlockState,
    pub acacia_leaves: BlockState,
    pub dark_oak_log: BlockState,
    pub dark_oak_leaves: BlockState,

    // Ground plants.
    pub short_grass: BlockState,
    pub tall_grass: BlockState,
    pub fern: BlockState,
    pub dead_bush: BlockState,
    pub dandelion: BlockState,
    pub poppy: BlockState,
    pub cornflower: BlockState,
    pub oxeye_daisy: BlockState,
    pub azure_bluet: BlockState,
    pub cactus: BlockState,
    pub sugar_cane: BlockState,
    pub brown_mushroom: BlockState,
    pub red_mushroom: BlockState,
    pub pumpkin: BlockState,
}

/// The lazily-resolved singleton palette.
pub fn get() -> &'static Blocks {
    static BLOCKS: OnceLock<Blocks> = OnceLock::new();
    BLOCKS.get_or_init(|| Blocks {
        air: BlockState(0),
        stone: solid("minecraft:stone"),
        deepslate: log("minecraft:deepslate"), // deepslate has an axis; default upright
        dirt: solid("minecraft:dirt"),
        coarse_dirt: solid("minecraft:coarse_dirt"),
        grass_block: solid("minecraft:grass_block"),
        podzol: solid("minecraft:podzol"),
        bedrock: solid("minecraft:bedrock"),
        water: solid("minecraft:water"),
        lava: solid("minecraft:lava"),
        sand: solid("minecraft:sand"),
        red_sand: solid("minecraft:red_sand"),
        sandstone: solid("minecraft:sandstone"),
        gravel: solid("minecraft:gravel"),
        snow_block: solid("minecraft:snow_block"),
        snow_layer: plant("minecraft:snow"),
        ice: solid("minecraft:ice"),
        packed_ice: solid("minecraft:packed_ice"),
        clay: solid("minecraft:clay"),

        coal_ore: solid("minecraft:coal_ore"),
        deepslate_coal_ore: solid("minecraft:deepslate_coal_ore"),
        iron_ore: solid("minecraft:iron_ore"),
        deepslate_iron_ore: solid("minecraft:deepslate_iron_ore"),
        copper_ore: solid("minecraft:copper_ore"),
        deepslate_copper_ore: solid("minecraft:deepslate_copper_ore"),
        gold_ore: solid("minecraft:gold_ore"),
        deepslate_gold_ore: solid("minecraft:deepslate_gold_ore"),
        redstone_ore: solid("minecraft:redstone_ore"),
        deepslate_redstone_ore: solid("minecraft:deepslate_redstone_ore"),
        lapis_ore: solid("minecraft:lapis_ore"),
        deepslate_lapis_ore: solid("minecraft:deepslate_lapis_ore"),
        diamond_ore: solid("minecraft:diamond_ore"),
        deepslate_diamond_ore: solid("minecraft:deepslate_diamond_ore"),
        emerald_ore: solid("minecraft:emerald_ore"),
        deepslate_emerald_ore: solid("minecraft:deepslate_emerald_ore"),
        granite: solid("minecraft:granite"),
        diorite: solid("minecraft:diorite"),
        andesite: solid("minecraft:andesite"),
        tuff: solid("minecraft:tuff"),

        oak_log: log("minecraft:oak_log"),
        oak_leaves: solid("minecraft:oak_leaves"),
        birch_log: log("minecraft:birch_log"),
        birch_leaves: solid("minecraft:birch_leaves"),
        spruce_log: log("minecraft:spruce_log"),
        spruce_leaves: solid("minecraft:spruce_leaves"),
        jungle_log: log("minecraft:jungle_log"),
        jungle_leaves: solid("minecraft:jungle_leaves"),
        acacia_log: log("minecraft:acacia_log"),
        acacia_leaves: solid("minecraft:acacia_leaves"),
        dark_oak_log: log("minecraft:dark_oak_log"),
        dark_oak_leaves: solid("minecraft:dark_oak_leaves"),

        short_grass: plant("minecraft:short_grass"),
        tall_grass: plant("minecraft:tall_grass"),
        fern: plant("minecraft:fern"),
        dead_bush: plant("minecraft:dead_bush"),
        dandelion: plant("minecraft:dandelion"),
        poppy: plant("minecraft:poppy"),
        cornflower: plant("minecraft:cornflower"),
        oxeye_daisy: plant("minecraft:oxeye_daisy"),
        azure_bluet: plant("minecraft:azure_bluet"),
        cactus: solid("minecraft:cactus"),
        sugar_cane: plant("minecraft:sugar_cane"),
        brown_mushroom: plant("minecraft:brown_mushroom"),
        red_mushroom: plant("minecraft:red_mushroom"),
        pumpkin: solid("minecraft:pumpkin"),
    })
}

/// True for cells that neither block motion nor hold a fluid — the plant/decoration
/// set the `MOTION_BLOCKING` heightmap skips (`!blocksMotion && fluid.isEmpty`).
/// Everything else the generator emits (solids, logs, leaves, water) is motion-
/// blocking for heightmap purposes.
pub fn is_non_motion_blocking(state: BlockState) -> bool {
    let b = get();
    state == b.air
        || state == b.short_grass
        || state == b.tall_grass
        || state == b.fern
        || state == b.dead_bush
        || state == b.dandelion
        || state == b.poppy
        || state == b.cornflower
        || state == b.oxeye_daisy
        || state == b.azure_bluet
        || state == b.sugar_cane
        || state == b.brown_mushroom
        || state == b.red_mushroom
        || state == b.snow_layer
}

/// Light dampening (`getLightDampening`, pre-`max(1,…)`): 0 for fully transparent
/// cells (air, plants, a thin snow layer), 1 for translucent blocks that let light
/// through attenuating by one (water, ice, leaves), 15 for opaque solids.
pub fn light_dampening(state: BlockState) -> u8 {
    let b = get();
    if is_non_motion_blocking(state) {
        return 0;
    }
    if state == b.water
        || state == b.ice
        || state == b.oak_leaves
        || state == b.birch_leaves
        || state == b.spruce_leaves
        || state == b.jungle_leaves
        || state == b.acacia_leaves
        || state == b.dark_oak_leaves
    {
        return 1;
    }
    15
}
