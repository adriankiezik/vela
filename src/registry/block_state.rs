//! Full block-state palette — the global `Block.BLOCK_STATE_REGISTRY` id space.
//!
//! Generated from the vanilla block dump (`--reports` → `generated/reports/
//! blocks.json`). The client decodes chunk sections and block-update packets
//! against this palette: a flat id space over every `(block, property
//! assignment)` state, in block registration order. There are 32366 states
//! across the 1196 blocks of [`super::builtin::BLOCK`].
//!
//! Rather than 32366 flat rows, each block stores its properties in vanilla
//! *definition order* (most-significant first, last property varying fastest —
//! matching Guava `cartesianProduct`) plus each property's value order. A
//! state's id is `base + mixed_radix(property indices)`. This representation was
//! verified to reproduce all 32366 state ids from the report exactly (see the
//! `whole_palette_round_trips` test).

/// A block property and its values, in the order vanilla assigns state ids.
pub struct Property {
    pub name: &'static str,
    pub values: &'static [&'static str],
}

/// The block-states of one block: a contiguous id run `base .. base + count`.
pub struct BlockStates {
    /// Global id of this block's first state.
    pub base: u32,
    /// Number of states (product of the property value counts, or 1).
    pub count: u16,
    /// Offset of the default state from `base` (`default = base + default_offset`).
    pub default_offset: u16,
    /// Properties in definition order: the first is most significant, the last
    /// varies fastest as the id increments.
    pub properties: &'static [Property],
}

/// Block-states indexed by block id (parallel to [`super::builtin::BLOCK`]).
#[rustfmt::skip]
static STATES: &[BlockStates] = &[
    BlockStates { base: 0, count: 1, default_offset: 0, properties: &[] }, // minecraft:air
    BlockStates { base: 1, count: 1, default_offset: 0, properties: &[] }, // minecraft:stone
    BlockStates { base: 2, count: 1, default_offset: 0, properties: &[] }, // minecraft:granite
    BlockStates { base: 3, count: 1, default_offset: 0, properties: &[] }, // minecraft:polished_granite
    BlockStates { base: 4, count: 1, default_offset: 0, properties: &[] }, // minecraft:diorite
    BlockStates { base: 5, count: 1, default_offset: 0, properties: &[] }, // minecraft:polished_diorite
    BlockStates { base: 6, count: 1, default_offset: 0, properties: &[] }, // minecraft:andesite
    BlockStates { base: 7, count: 1, default_offset: 0, properties: &[] }, // minecraft:polished_andesite
    BlockStates { base: 8, count: 2, default_offset: 1, properties: &[Property { name: "snowy", values: &["true", "false"] }] }, // minecraft:grass_block
    BlockStates { base: 10, count: 1, default_offset: 0, properties: &[] }, // minecraft:dirt
    BlockStates { base: 11, count: 1, default_offset: 0, properties: &[] }, // minecraft:coarse_dirt
    BlockStates { base: 12, count: 2, default_offset: 1, properties: &[Property { name: "snowy", values: &["true", "false"] }] }, // minecraft:podzol
    BlockStates { base: 14, count: 1, default_offset: 0, properties: &[] }, // minecraft:cobblestone
    BlockStates { base: 15, count: 1, default_offset: 0, properties: &[] }, // minecraft:oak_planks
    BlockStates { base: 16, count: 1, default_offset: 0, properties: &[] }, // minecraft:spruce_planks
    BlockStates { base: 17, count: 1, default_offset: 0, properties: &[] }, // minecraft:birch_planks
    BlockStates { base: 18, count: 1, default_offset: 0, properties: &[] }, // minecraft:jungle_planks
    BlockStates { base: 19, count: 1, default_offset: 0, properties: &[] }, // minecraft:acacia_planks
    BlockStates { base: 20, count: 1, default_offset: 0, properties: &[] }, // minecraft:cherry_planks
    BlockStates { base: 21, count: 1, default_offset: 0, properties: &[] }, // minecraft:dark_oak_planks
    BlockStates { base: 22, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:pale_oak_wood
    BlockStates { base: 25, count: 1, default_offset: 0, properties: &[] }, // minecraft:pale_oak_planks
    BlockStates { base: 26, count: 1, default_offset: 0, properties: &[] }, // minecraft:mangrove_planks
    BlockStates { base: 27, count: 1, default_offset: 0, properties: &[] }, // minecraft:bamboo_planks
    BlockStates { base: 28, count: 1, default_offset: 0, properties: &[] }, // minecraft:bamboo_mosaic
    BlockStates { base: 29, count: 2, default_offset: 0, properties: &[Property { name: "stage", values: &["0", "1"] }] }, // minecraft:oak_sapling
    BlockStates { base: 31, count: 2, default_offset: 0, properties: &[Property { name: "stage", values: &["0", "1"] }] }, // minecraft:spruce_sapling
    BlockStates { base: 33, count: 2, default_offset: 0, properties: &[Property { name: "stage", values: &["0", "1"] }] }, // minecraft:birch_sapling
    BlockStates { base: 35, count: 2, default_offset: 0, properties: &[Property { name: "stage", values: &["0", "1"] }] }, // minecraft:jungle_sapling
    BlockStates { base: 37, count: 2, default_offset: 0, properties: &[Property { name: "stage", values: &["0", "1"] }] }, // minecraft:acacia_sapling
    BlockStates { base: 39, count: 2, default_offset: 0, properties: &[Property { name: "stage", values: &["0", "1"] }] }, // minecraft:cherry_sapling
    BlockStates { base: 41, count: 2, default_offset: 0, properties: &[Property { name: "stage", values: &["0", "1"] }] }, // minecraft:dark_oak_sapling
    BlockStates { base: 43, count: 2, default_offset: 0, properties: &[Property { name: "stage", values: &["0", "1"] }] }, // minecraft:pale_oak_sapling
    BlockStates { base: 45, count: 40, default_offset: 5, properties: &[Property { name: "age", values: &["0", "1", "2", "3", "4"] }, Property { name: "hanging", values: &["true", "false"] }, Property { name: "stage", values: &["0", "1"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:mangrove_propagule
    BlockStates { base: 85, count: 1, default_offset: 0, properties: &[] }, // minecraft:bedrock
    BlockStates { base: 86, count: 16, default_offset: 0, properties: &[Property { name: "level", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:water
    BlockStates { base: 102, count: 16, default_offset: 0, properties: &[Property { name: "level", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:lava
    BlockStates { base: 118, count: 1, default_offset: 0, properties: &[] }, // minecraft:sand
    BlockStates { base: 119, count: 4, default_offset: 0, properties: &[Property { name: "dusted", values: &["0", "1", "2", "3"] }] }, // minecraft:suspicious_sand
    BlockStates { base: 123, count: 1, default_offset: 0, properties: &[] }, // minecraft:red_sand
    BlockStates { base: 124, count: 1, default_offset: 0, properties: &[] }, // minecraft:gravel
    BlockStates { base: 125, count: 4, default_offset: 0, properties: &[Property { name: "dusted", values: &["0", "1", "2", "3"] }] }, // minecraft:suspicious_gravel
    BlockStates { base: 129, count: 1, default_offset: 0, properties: &[] }, // minecraft:gold_ore
    BlockStates { base: 130, count: 1, default_offset: 0, properties: &[] }, // minecraft:deepslate_gold_ore
    BlockStates { base: 131, count: 1, default_offset: 0, properties: &[] }, // minecraft:iron_ore
    BlockStates { base: 132, count: 1, default_offset: 0, properties: &[] }, // minecraft:deepslate_iron_ore
    BlockStates { base: 133, count: 1, default_offset: 0, properties: &[] }, // minecraft:coal_ore
    BlockStates { base: 134, count: 1, default_offset: 0, properties: &[] }, // minecraft:deepslate_coal_ore
    BlockStates { base: 135, count: 1, default_offset: 0, properties: &[] }, // minecraft:nether_gold_ore
    BlockStates { base: 136, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:oak_log
    BlockStates { base: 139, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:spruce_log
    BlockStates { base: 142, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:birch_log
    BlockStates { base: 145, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:jungle_log
    BlockStates { base: 148, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:acacia_log
    BlockStates { base: 151, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:cherry_log
    BlockStates { base: 154, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:dark_oak_log
    BlockStates { base: 157, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:pale_oak_log
    BlockStates { base: 160, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:mangrove_log
    BlockStates { base: 163, count: 2, default_offset: 1, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:mangrove_roots
    BlockStates { base: 165, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:muddy_mangrove_roots
    BlockStates { base: 168, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:bamboo_block
    BlockStates { base: 171, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_spruce_log
    BlockStates { base: 174, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_birch_log
    BlockStates { base: 177, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_jungle_log
    BlockStates { base: 180, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_acacia_log
    BlockStates { base: 183, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_cherry_log
    BlockStates { base: 186, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_dark_oak_log
    BlockStates { base: 189, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_pale_oak_log
    BlockStates { base: 192, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_oak_log
    BlockStates { base: 195, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_mangrove_log
    BlockStates { base: 198, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_bamboo_block
    BlockStates { base: 201, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:oak_wood
    BlockStates { base: 204, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:spruce_wood
    BlockStates { base: 207, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:birch_wood
    BlockStates { base: 210, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:jungle_wood
    BlockStates { base: 213, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:acacia_wood
    BlockStates { base: 216, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:cherry_wood
    BlockStates { base: 219, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:dark_oak_wood
    BlockStates { base: 222, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:mangrove_wood
    BlockStates { base: 225, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_oak_wood
    BlockStates { base: 228, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_spruce_wood
    BlockStates { base: 231, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_birch_wood
    BlockStates { base: 234, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_jungle_wood
    BlockStates { base: 237, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_acacia_wood
    BlockStates { base: 240, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_cherry_wood
    BlockStates { base: 243, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_dark_oak_wood
    BlockStates { base: 246, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_pale_oak_wood
    BlockStates { base: 249, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_mangrove_wood
    BlockStates { base: 252, count: 28, default_offset: 27, properties: &[Property { name: "distance", values: &["1", "2", "3", "4", "5", "6", "7"] }, Property { name: "persistent", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:oak_leaves
    BlockStates { base: 280, count: 28, default_offset: 27, properties: &[Property { name: "distance", values: &["1", "2", "3", "4", "5", "6", "7"] }, Property { name: "persistent", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:spruce_leaves
    BlockStates { base: 308, count: 28, default_offset: 27, properties: &[Property { name: "distance", values: &["1", "2", "3", "4", "5", "6", "7"] }, Property { name: "persistent", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:birch_leaves
    BlockStates { base: 336, count: 28, default_offset: 27, properties: &[Property { name: "distance", values: &["1", "2", "3", "4", "5", "6", "7"] }, Property { name: "persistent", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:jungle_leaves
    BlockStates { base: 364, count: 28, default_offset: 27, properties: &[Property { name: "distance", values: &["1", "2", "3", "4", "5", "6", "7"] }, Property { name: "persistent", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:acacia_leaves
    BlockStates { base: 392, count: 28, default_offset: 27, properties: &[Property { name: "distance", values: &["1", "2", "3", "4", "5", "6", "7"] }, Property { name: "persistent", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cherry_leaves
    BlockStates { base: 420, count: 28, default_offset: 27, properties: &[Property { name: "distance", values: &["1", "2", "3", "4", "5", "6", "7"] }, Property { name: "persistent", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dark_oak_leaves
    BlockStates { base: 448, count: 28, default_offset: 27, properties: &[Property { name: "distance", values: &["1", "2", "3", "4", "5", "6", "7"] }, Property { name: "persistent", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:pale_oak_leaves
    BlockStates { base: 476, count: 28, default_offset: 27, properties: &[Property { name: "distance", values: &["1", "2", "3", "4", "5", "6", "7"] }, Property { name: "persistent", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:mangrove_leaves
    BlockStates { base: 504, count: 28, default_offset: 27, properties: &[Property { name: "distance", values: &["1", "2", "3", "4", "5", "6", "7"] }, Property { name: "persistent", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:azalea_leaves
    BlockStates { base: 532, count: 28, default_offset: 27, properties: &[Property { name: "distance", values: &["1", "2", "3", "4", "5", "6", "7"] }, Property { name: "persistent", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:flowering_azalea_leaves
    BlockStates { base: 560, count: 1, default_offset: 0, properties: &[] }, // minecraft:sponge
    BlockStates { base: 561, count: 1, default_offset: 0, properties: &[] }, // minecraft:wet_sponge
    BlockStates { base: 562, count: 1, default_offset: 0, properties: &[] }, // minecraft:glass
    BlockStates { base: 563, count: 1, default_offset: 0, properties: &[] }, // minecraft:lapis_ore
    BlockStates { base: 564, count: 1, default_offset: 0, properties: &[] }, // minecraft:deepslate_lapis_ore
    BlockStates { base: 565, count: 1, default_offset: 0, properties: &[] }, // minecraft:lapis_block
    BlockStates { base: 566, count: 12, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }, Property { name: "triggered", values: &["true", "false"] }] }, // minecraft:dispenser
    BlockStates { base: 578, count: 1, default_offset: 0, properties: &[] }, // minecraft:sandstone
    BlockStates { base: 579, count: 1, default_offset: 0, properties: &[] }, // minecraft:chiseled_sandstone
    BlockStates { base: 580, count: 1, default_offset: 0, properties: &[] }, // minecraft:cut_sandstone
    BlockStates { base: 581, count: 1350, default_offset: 1, properties: &[Property { name: "instrument", values: &["harp", "basedrum", "snare", "hat", "bass", "flute", "bell", "guitar", "chime", "xylophone", "iron_xylophone", "cow_bell", "didgeridoo", "bit", "banjo", "pling", "trumpet", "trumpet_exposed", "trumpet_oxidized", "trumpet_weathered", "zombie", "skeleton", "creeper", "dragon", "wither_skeleton", "piglin", "custom_head"] }, Property { name: "note", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16", "17", "18", "19", "20", "21", "22", "23", "24"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:note_block
    BlockStates { base: 1931, count: 16, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "occupied", values: &["true", "false"] }, Property { name: "part", values: &["head", "foot"] }] }, // minecraft:white_bed
    BlockStates { base: 1947, count: 16, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "occupied", values: &["true", "false"] }, Property { name: "part", values: &["head", "foot"] }] }, // minecraft:orange_bed
    BlockStates { base: 1963, count: 16, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "occupied", values: &["true", "false"] }, Property { name: "part", values: &["head", "foot"] }] }, // minecraft:magenta_bed
    BlockStates { base: 1979, count: 16, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "occupied", values: &["true", "false"] }, Property { name: "part", values: &["head", "foot"] }] }, // minecraft:light_blue_bed
    BlockStates { base: 1995, count: 16, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "occupied", values: &["true", "false"] }, Property { name: "part", values: &["head", "foot"] }] }, // minecraft:yellow_bed
    BlockStates { base: 2011, count: 16, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "occupied", values: &["true", "false"] }, Property { name: "part", values: &["head", "foot"] }] }, // minecraft:lime_bed
    BlockStates { base: 2027, count: 16, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "occupied", values: &["true", "false"] }, Property { name: "part", values: &["head", "foot"] }] }, // minecraft:pink_bed
    BlockStates { base: 2043, count: 16, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "occupied", values: &["true", "false"] }, Property { name: "part", values: &["head", "foot"] }] }, // minecraft:gray_bed
    BlockStates { base: 2059, count: 16, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "occupied", values: &["true", "false"] }, Property { name: "part", values: &["head", "foot"] }] }, // minecraft:light_gray_bed
    BlockStates { base: 2075, count: 16, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "occupied", values: &["true", "false"] }, Property { name: "part", values: &["head", "foot"] }] }, // minecraft:cyan_bed
    BlockStates { base: 2091, count: 16, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "occupied", values: &["true", "false"] }, Property { name: "part", values: &["head", "foot"] }] }, // minecraft:purple_bed
    BlockStates { base: 2107, count: 16, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "occupied", values: &["true", "false"] }, Property { name: "part", values: &["head", "foot"] }] }, // minecraft:blue_bed
    BlockStates { base: 2123, count: 16, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "occupied", values: &["true", "false"] }, Property { name: "part", values: &["head", "foot"] }] }, // minecraft:brown_bed
    BlockStates { base: 2139, count: 16, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "occupied", values: &["true", "false"] }, Property { name: "part", values: &["head", "foot"] }] }, // minecraft:green_bed
    BlockStates { base: 2155, count: 16, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "occupied", values: &["true", "false"] }, Property { name: "part", values: &["head", "foot"] }] }, // minecraft:red_bed
    BlockStates { base: 2171, count: 16, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "occupied", values: &["true", "false"] }, Property { name: "part", values: &["head", "foot"] }] }, // minecraft:black_bed
    BlockStates { base: 2187, count: 24, default_offset: 13, properties: &[Property { name: "powered", values: &["true", "false"] }, Property { name: "shape", values: &["north_south", "east_west", "ascending_east", "ascending_west", "ascending_north", "ascending_south"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:powered_rail
    BlockStates { base: 2211, count: 24, default_offset: 13, properties: &[Property { name: "powered", values: &["true", "false"] }, Property { name: "shape", values: &["north_south", "east_west", "ascending_east", "ascending_west", "ascending_north", "ascending_south"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:detector_rail
    BlockStates { base: 2235, count: 12, default_offset: 6, properties: &[Property { name: "extended", values: &["true", "false"] }, Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:sticky_piston
    BlockStates { base: 2247, count: 1, default_offset: 0, properties: &[] }, // minecraft:cobweb
    BlockStates { base: 2248, count: 1, default_offset: 0, properties: &[] }, // minecraft:short_grass
    BlockStates { base: 2249, count: 1, default_offset: 0, properties: &[] }, // minecraft:fern
    BlockStates { base: 2250, count: 1, default_offset: 0, properties: &[] }, // minecraft:dead_bush
    BlockStates { base: 2251, count: 1, default_offset: 0, properties: &[] }, // minecraft:bush
    BlockStates { base: 2252, count: 1, default_offset: 0, properties: &[] }, // minecraft:short_dry_grass
    BlockStates { base: 2253, count: 1, default_offset: 0, properties: &[] }, // minecraft:tall_dry_grass
    BlockStates { base: 2254, count: 1, default_offset: 0, properties: &[] }, // minecraft:seagrass
    BlockStates { base: 2255, count: 2, default_offset: 1, properties: &[Property { name: "half", values: &["upper", "lower"] }] }, // minecraft:tall_seagrass
    BlockStates { base: 2257, count: 12, default_offset: 6, properties: &[Property { name: "extended", values: &["true", "false"] }, Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:piston
    BlockStates { base: 2269, count: 24, default_offset: 2, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }, Property { name: "short", values: &["true", "false"] }, Property { name: "type", values: &["normal", "sticky"] }] }, // minecraft:piston_head
    BlockStates { base: 2293, count: 1, default_offset: 0, properties: &[] }, // minecraft:white_wool
    BlockStates { base: 2294, count: 1, default_offset: 0, properties: &[] }, // minecraft:orange_wool
    BlockStates { base: 2295, count: 1, default_offset: 0, properties: &[] }, // minecraft:magenta_wool
    BlockStates { base: 2296, count: 1, default_offset: 0, properties: &[] }, // minecraft:light_blue_wool
    BlockStates { base: 2297, count: 1, default_offset: 0, properties: &[] }, // minecraft:yellow_wool
    BlockStates { base: 2298, count: 1, default_offset: 0, properties: &[] }, // minecraft:lime_wool
    BlockStates { base: 2299, count: 1, default_offset: 0, properties: &[] }, // minecraft:pink_wool
    BlockStates { base: 2300, count: 1, default_offset: 0, properties: &[] }, // minecraft:gray_wool
    BlockStates { base: 2301, count: 1, default_offset: 0, properties: &[] }, // minecraft:light_gray_wool
    BlockStates { base: 2302, count: 1, default_offset: 0, properties: &[] }, // minecraft:cyan_wool
    BlockStates { base: 2303, count: 1, default_offset: 0, properties: &[] }, // minecraft:purple_wool
    BlockStates { base: 2304, count: 1, default_offset: 0, properties: &[] }, // minecraft:blue_wool
    BlockStates { base: 2305, count: 1, default_offset: 0, properties: &[] }, // minecraft:brown_wool
    BlockStates { base: 2306, count: 1, default_offset: 0, properties: &[] }, // minecraft:green_wool
    BlockStates { base: 2307, count: 1, default_offset: 0, properties: &[] }, // minecraft:red_wool
    BlockStates { base: 2308, count: 1, default_offset: 0, properties: &[] }, // minecraft:black_wool
    BlockStates { base: 2309, count: 12, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }, Property { name: "type", values: &["normal", "sticky"] }] }, // minecraft:moving_piston
    BlockStates { base: 2321, count: 1, default_offset: 0, properties: &[] }, // minecraft:dandelion
    BlockStates { base: 2322, count: 1, default_offset: 0, properties: &[] }, // minecraft:golden_dandelion
    BlockStates { base: 2323, count: 1, default_offset: 0, properties: &[] }, // minecraft:torchflower
    BlockStates { base: 2324, count: 1, default_offset: 0, properties: &[] }, // minecraft:poppy
    BlockStates { base: 2325, count: 1, default_offset: 0, properties: &[] }, // minecraft:blue_orchid
    BlockStates { base: 2326, count: 1, default_offset: 0, properties: &[] }, // minecraft:allium
    BlockStates { base: 2327, count: 1, default_offset: 0, properties: &[] }, // minecraft:azure_bluet
    BlockStates { base: 2328, count: 1, default_offset: 0, properties: &[] }, // minecraft:red_tulip
    BlockStates { base: 2329, count: 1, default_offset: 0, properties: &[] }, // minecraft:orange_tulip
    BlockStates { base: 2330, count: 1, default_offset: 0, properties: &[] }, // minecraft:white_tulip
    BlockStates { base: 2331, count: 1, default_offset: 0, properties: &[] }, // minecraft:pink_tulip
    BlockStates { base: 2332, count: 1, default_offset: 0, properties: &[] }, // minecraft:oxeye_daisy
    BlockStates { base: 2333, count: 1, default_offset: 0, properties: &[] }, // minecraft:cornflower
    BlockStates { base: 2334, count: 1, default_offset: 0, properties: &[] }, // minecraft:wither_rose
    BlockStates { base: 2335, count: 1, default_offset: 0, properties: &[] }, // minecraft:lily_of_the_valley
    BlockStates { base: 2336, count: 1, default_offset: 0, properties: &[] }, // minecraft:brown_mushroom
    BlockStates { base: 2337, count: 1, default_offset: 0, properties: &[] }, // minecraft:red_mushroom
    BlockStates { base: 2338, count: 1, default_offset: 0, properties: &[] }, // minecraft:gold_block
    BlockStates { base: 2339, count: 1, default_offset: 0, properties: &[] }, // minecraft:iron_block
    BlockStates { base: 2340, count: 1, default_offset: 0, properties: &[] }, // minecraft:bricks
    BlockStates { base: 2341, count: 2, default_offset: 1, properties: &[Property { name: "unstable", values: &["true", "false"] }] }, // minecraft:tnt
    BlockStates { base: 2343, count: 1, default_offset: 0, properties: &[] }, // minecraft:bookshelf
    BlockStates { base: 2344, count: 256, default_offset: 63, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "slot_0_occupied", values: &["true", "false"] }, Property { name: "slot_1_occupied", values: &["true", "false"] }, Property { name: "slot_2_occupied", values: &["true", "false"] }, Property { name: "slot_3_occupied", values: &["true", "false"] }, Property { name: "slot_4_occupied", values: &["true", "false"] }, Property { name: "slot_5_occupied", values: &["true", "false"] }] }, // minecraft:chiseled_bookshelf
    BlockStates { base: 2600, count: 64, default_offset: 9, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "side_chain", values: &["unconnected", "right", "center", "left"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:acacia_shelf
    BlockStates { base: 2664, count: 64, default_offset: 9, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "side_chain", values: &["unconnected", "right", "center", "left"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:bamboo_shelf
    BlockStates { base: 2728, count: 64, default_offset: 9, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "side_chain", values: &["unconnected", "right", "center", "left"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:birch_shelf
    BlockStates { base: 2792, count: 64, default_offset: 9, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "side_chain", values: &["unconnected", "right", "center", "left"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cherry_shelf
    BlockStates { base: 2856, count: 64, default_offset: 9, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "side_chain", values: &["unconnected", "right", "center", "left"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:crimson_shelf
    BlockStates { base: 2920, count: 64, default_offset: 9, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "side_chain", values: &["unconnected", "right", "center", "left"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dark_oak_shelf
    BlockStates { base: 2984, count: 64, default_offset: 9, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "side_chain", values: &["unconnected", "right", "center", "left"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:jungle_shelf
    BlockStates { base: 3048, count: 64, default_offset: 9, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "side_chain", values: &["unconnected", "right", "center", "left"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:mangrove_shelf
    BlockStates { base: 3112, count: 64, default_offset: 9, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "side_chain", values: &["unconnected", "right", "center", "left"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:oak_shelf
    BlockStates { base: 3176, count: 64, default_offset: 9, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "side_chain", values: &["unconnected", "right", "center", "left"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:pale_oak_shelf
    BlockStates { base: 3240, count: 64, default_offset: 9, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "side_chain", values: &["unconnected", "right", "center", "left"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:spruce_shelf
    BlockStates { base: 3304, count: 64, default_offset: 9, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "side_chain", values: &["unconnected", "right", "center", "left"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:warped_shelf
    BlockStates { base: 3368, count: 1, default_offset: 0, properties: &[] }, // minecraft:mossy_cobblestone
    BlockStates { base: 3369, count: 1, default_offset: 0, properties: &[] }, // minecraft:obsidian
    BlockStates { base: 3370, count: 1, default_offset: 0, properties: &[] }, // minecraft:torch
    BlockStates { base: 3371, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:wall_torch
    BlockStates { base: 3375, count: 512, default_offset: 31, properties: &[Property { name: "age", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:fire
    BlockStates { base: 3887, count: 1, default_offset: 0, properties: &[] }, // minecraft:soul_fire
    BlockStates { base: 3888, count: 1, default_offset: 0, properties: &[] }, // minecraft:spawner
    BlockStates { base: 3889, count: 18, default_offset: 7, properties: &[Property { name: "axis", values: &["x", "y", "z"] }, Property { name: "creaking_heart_state", values: &["uprooted", "dormant", "awake"] }, Property { name: "natural", values: &["true", "false"] }] }, // minecraft:creaking_heart
    BlockStates { base: 3907, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:oak_stairs
    BlockStates { base: 3987, count: 24, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "type", values: &["single", "left", "right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:chest
    BlockStates { base: 4011, count: 1296, default_offset: 1160, properties: &[Property { name: "east", values: &["up", "side", "none"] }, Property { name: "north", values: &["up", "side", "none"] }, Property { name: "power", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "south", values: &["up", "side", "none"] }, Property { name: "west", values: &["up", "side", "none"] }] }, // minecraft:redstone_wire
    BlockStates { base: 5307, count: 1, default_offset: 0, properties: &[] }, // minecraft:diamond_ore
    BlockStates { base: 5308, count: 1, default_offset: 0, properties: &[] }, // minecraft:deepslate_diamond_ore
    BlockStates { base: 5309, count: 1, default_offset: 0, properties: &[] }, // minecraft:diamond_block
    BlockStates { base: 5310, count: 1, default_offset: 0, properties: &[] }, // minecraft:crafting_table
    BlockStates { base: 5311, count: 8, default_offset: 0, properties: &[Property { name: "age", values: &["0", "1", "2", "3", "4", "5", "6", "7"] }] }, // minecraft:wheat
    BlockStates { base: 5319, count: 8, default_offset: 0, properties: &[Property { name: "moisture", values: &["0", "1", "2", "3", "4", "5", "6", "7"] }] }, // minecraft:farmland
    BlockStates { base: 5327, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "lit", values: &["true", "false"] }] }, // minecraft:furnace
    BlockStates { base: 5335, count: 32, default_offset: 17, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:oak_sign
    BlockStates { base: 5367, count: 32, default_offset: 17, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:spruce_sign
    BlockStates { base: 5399, count: 32, default_offset: 17, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:birch_sign
    BlockStates { base: 5431, count: 32, default_offset: 17, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:acacia_sign
    BlockStates { base: 5463, count: 32, default_offset: 17, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cherry_sign
    BlockStates { base: 5495, count: 32, default_offset: 17, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:jungle_sign
    BlockStates { base: 5527, count: 32, default_offset: 17, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dark_oak_sign
    BlockStates { base: 5559, count: 32, default_offset: 17, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:pale_oak_sign
    BlockStates { base: 5591, count: 32, default_offset: 17, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:mangrove_sign
    BlockStates { base: 5623, count: 32, default_offset: 17, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:bamboo_sign
    BlockStates { base: 5655, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:oak_door
    BlockStates { base: 5719, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:ladder
    BlockStates { base: 5727, count: 20, default_offset: 1, properties: &[Property { name: "shape", values: &["north_south", "east_west", "ascending_east", "ascending_west", "ascending_north", "ascending_south", "south_east", "south_west", "north_west", "north_east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:rail
    BlockStates { base: 5747, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cobblestone_stairs
    BlockStates { base: 5827, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:oak_wall_sign
    BlockStates { base: 5835, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:spruce_wall_sign
    BlockStates { base: 5843, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:birch_wall_sign
    BlockStates { base: 5851, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:acacia_wall_sign
    BlockStates { base: 5859, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cherry_wall_sign
    BlockStates { base: 5867, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:jungle_wall_sign
    BlockStates { base: 5875, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dark_oak_wall_sign
    BlockStates { base: 5883, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:pale_oak_wall_sign
    BlockStates { base: 5891, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:mangrove_wall_sign
    BlockStates { base: 5899, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:bamboo_wall_sign
    BlockStates { base: 5907, count: 64, default_offset: 49, properties: &[Property { name: "attached", values: &["true", "false"] }, Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:oak_hanging_sign
    BlockStates { base: 5971, count: 64, default_offset: 49, properties: &[Property { name: "attached", values: &["true", "false"] }, Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:spruce_hanging_sign
    BlockStates { base: 6035, count: 64, default_offset: 49, properties: &[Property { name: "attached", values: &["true", "false"] }, Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:birch_hanging_sign
    BlockStates { base: 6099, count: 64, default_offset: 49, properties: &[Property { name: "attached", values: &["true", "false"] }, Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:acacia_hanging_sign
    BlockStates { base: 6163, count: 64, default_offset: 49, properties: &[Property { name: "attached", values: &["true", "false"] }, Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cherry_hanging_sign
    BlockStates { base: 6227, count: 64, default_offset: 49, properties: &[Property { name: "attached", values: &["true", "false"] }, Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:jungle_hanging_sign
    BlockStates { base: 6291, count: 64, default_offset: 49, properties: &[Property { name: "attached", values: &["true", "false"] }, Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dark_oak_hanging_sign
    BlockStates { base: 6355, count: 64, default_offset: 49, properties: &[Property { name: "attached", values: &["true", "false"] }, Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:pale_oak_hanging_sign
    BlockStates { base: 6419, count: 64, default_offset: 49, properties: &[Property { name: "attached", values: &["true", "false"] }, Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:crimson_hanging_sign
    BlockStates { base: 6483, count: 64, default_offset: 49, properties: &[Property { name: "attached", values: &["true", "false"] }, Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:warped_hanging_sign
    BlockStates { base: 6547, count: 64, default_offset: 49, properties: &[Property { name: "attached", values: &["true", "false"] }, Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:mangrove_hanging_sign
    BlockStates { base: 6611, count: 64, default_offset: 49, properties: &[Property { name: "attached", values: &["true", "false"] }, Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:bamboo_hanging_sign
    BlockStates { base: 6675, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:oak_wall_hanging_sign
    BlockStates { base: 6683, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:spruce_wall_hanging_sign
    BlockStates { base: 6691, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:birch_wall_hanging_sign
    BlockStates { base: 6699, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:acacia_wall_hanging_sign
    BlockStates { base: 6707, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cherry_wall_hanging_sign
    BlockStates { base: 6715, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:jungle_wall_hanging_sign
    BlockStates { base: 6723, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dark_oak_wall_hanging_sign
    BlockStates { base: 6731, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:pale_oak_wall_hanging_sign
    BlockStates { base: 6739, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:mangrove_wall_hanging_sign
    BlockStates { base: 6747, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:crimson_wall_hanging_sign
    BlockStates { base: 6755, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:warped_wall_hanging_sign
    BlockStates { base: 6763, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:bamboo_wall_hanging_sign
    BlockStates { base: 6771, count: 24, default_offset: 9, properties: &[Property { name: "face", values: &["floor", "wall", "ceiling"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:lever
    BlockStates { base: 6795, count: 2, default_offset: 1, properties: &[Property { name: "powered", values: &["true", "false"] }] }, // minecraft:stone_pressure_plate
    BlockStates { base: 6797, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:iron_door
    BlockStates { base: 6861, count: 2, default_offset: 1, properties: &[Property { name: "powered", values: &["true", "false"] }] }, // minecraft:oak_pressure_plate
    BlockStates { base: 6863, count: 2, default_offset: 1, properties: &[Property { name: "powered", values: &["true", "false"] }] }, // minecraft:spruce_pressure_plate
    BlockStates { base: 6865, count: 2, default_offset: 1, properties: &[Property { name: "powered", values: &["true", "false"] }] }, // minecraft:birch_pressure_plate
    BlockStates { base: 6867, count: 2, default_offset: 1, properties: &[Property { name: "powered", values: &["true", "false"] }] }, // minecraft:jungle_pressure_plate
    BlockStates { base: 6869, count: 2, default_offset: 1, properties: &[Property { name: "powered", values: &["true", "false"] }] }, // minecraft:acacia_pressure_plate
    BlockStates { base: 6871, count: 2, default_offset: 1, properties: &[Property { name: "powered", values: &["true", "false"] }] }, // minecraft:cherry_pressure_plate
    BlockStates { base: 6873, count: 2, default_offset: 1, properties: &[Property { name: "powered", values: &["true", "false"] }] }, // minecraft:dark_oak_pressure_plate
    BlockStates { base: 6875, count: 2, default_offset: 1, properties: &[Property { name: "powered", values: &["true", "false"] }] }, // minecraft:pale_oak_pressure_plate
    BlockStates { base: 6877, count: 2, default_offset: 1, properties: &[Property { name: "powered", values: &["true", "false"] }] }, // minecraft:mangrove_pressure_plate
    BlockStates { base: 6879, count: 2, default_offset: 1, properties: &[Property { name: "powered", values: &["true", "false"] }] }, // minecraft:bamboo_pressure_plate
    BlockStates { base: 6881, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:redstone_ore
    BlockStates { base: 6883, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:deepslate_redstone_ore
    BlockStates { base: 6885, count: 2, default_offset: 0, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:redstone_torch
    BlockStates { base: 6887, count: 8, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "lit", values: &["true", "false"] }] }, // minecraft:redstone_wall_torch
    BlockStates { base: 6895, count: 24, default_offset: 9, properties: &[Property { name: "face", values: &["floor", "wall", "ceiling"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:stone_button
    BlockStates { base: 6919, count: 8, default_offset: 0, properties: &[Property { name: "layers", values: &["1", "2", "3", "4", "5", "6", "7", "8"] }] }, // minecraft:snow
    BlockStates { base: 6927, count: 1, default_offset: 0, properties: &[] }, // minecraft:ice
    BlockStates { base: 6928, count: 1, default_offset: 0, properties: &[] }, // minecraft:snow_block
    BlockStates { base: 6929, count: 16, default_offset: 0, properties: &[Property { name: "age", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:cactus
    BlockStates { base: 6945, count: 1, default_offset: 0, properties: &[] }, // minecraft:cactus_flower
    BlockStates { base: 6946, count: 1, default_offset: 0, properties: &[] }, // minecraft:clay
    BlockStates { base: 6947, count: 16, default_offset: 0, properties: &[Property { name: "age", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:sugar_cane
    BlockStates { base: 6963, count: 2, default_offset: 1, properties: &[Property { name: "has_record", values: &["true", "false"] }] }, // minecraft:jukebox
    BlockStates { base: 6965, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:oak_fence
    BlockStates { base: 6997, count: 1, default_offset: 0, properties: &[] }, // minecraft:netherrack
    BlockStates { base: 6998, count: 1, default_offset: 0, properties: &[] }, // minecraft:soul_sand
    BlockStates { base: 6999, count: 1, default_offset: 0, properties: &[] }, // minecraft:soul_soil
    BlockStates { base: 7000, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:basalt
    BlockStates { base: 7003, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:polished_basalt
    BlockStates { base: 7006, count: 1, default_offset: 0, properties: &[] }, // minecraft:soul_torch
    BlockStates { base: 7007, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:soul_wall_torch
    BlockStates { base: 7011, count: 1, default_offset: 0, properties: &[] }, // minecraft:copper_torch
    BlockStates { base: 7012, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:copper_wall_torch
    BlockStates { base: 7016, count: 1, default_offset: 0, properties: &[] }, // minecraft:glowstone
    BlockStates { base: 7017, count: 2, default_offset: 0, properties: &[Property { name: "axis", values: &["x", "z"] }] }, // minecraft:nether_portal
    BlockStates { base: 7019, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:carved_pumpkin
    BlockStates { base: 7023, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:jack_o_lantern
    BlockStates { base: 7027, count: 7, default_offset: 0, properties: &[Property { name: "bites", values: &["0", "1", "2", "3", "4", "5", "6"] }] }, // minecraft:cake
    BlockStates { base: 7034, count: 64, default_offset: 3, properties: &[Property { name: "delay", values: &["1", "2", "3", "4"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "locked", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:repeater
    BlockStates { base: 7098, count: 1, default_offset: 0, properties: &[] }, // minecraft:white_stained_glass
    BlockStates { base: 7099, count: 1, default_offset: 0, properties: &[] }, // minecraft:orange_stained_glass
    BlockStates { base: 7100, count: 1, default_offset: 0, properties: &[] }, // minecraft:magenta_stained_glass
    BlockStates { base: 7101, count: 1, default_offset: 0, properties: &[] }, // minecraft:light_blue_stained_glass
    BlockStates { base: 7102, count: 1, default_offset: 0, properties: &[] }, // minecraft:yellow_stained_glass
    BlockStates { base: 7103, count: 1, default_offset: 0, properties: &[] }, // minecraft:lime_stained_glass
    BlockStates { base: 7104, count: 1, default_offset: 0, properties: &[] }, // minecraft:pink_stained_glass
    BlockStates { base: 7105, count: 1, default_offset: 0, properties: &[] }, // minecraft:gray_stained_glass
    BlockStates { base: 7106, count: 1, default_offset: 0, properties: &[] }, // minecraft:light_gray_stained_glass
    BlockStates { base: 7107, count: 1, default_offset: 0, properties: &[] }, // minecraft:cyan_stained_glass
    BlockStates { base: 7108, count: 1, default_offset: 0, properties: &[] }, // minecraft:purple_stained_glass
    BlockStates { base: 7109, count: 1, default_offset: 0, properties: &[] }, // minecraft:blue_stained_glass
    BlockStates { base: 7110, count: 1, default_offset: 0, properties: &[] }, // minecraft:brown_stained_glass
    BlockStates { base: 7111, count: 1, default_offset: 0, properties: &[] }, // minecraft:green_stained_glass
    BlockStates { base: 7112, count: 1, default_offset: 0, properties: &[] }, // minecraft:red_stained_glass
    BlockStates { base: 7113, count: 1, default_offset: 0, properties: &[] }, // minecraft:black_stained_glass
    BlockStates { base: 7114, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:oak_trapdoor
    BlockStates { base: 7178, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:spruce_trapdoor
    BlockStates { base: 7242, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:birch_trapdoor
    BlockStates { base: 7306, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:jungle_trapdoor
    BlockStates { base: 7370, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:acacia_trapdoor
    BlockStates { base: 7434, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cherry_trapdoor
    BlockStates { base: 7498, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dark_oak_trapdoor
    BlockStates { base: 7562, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:pale_oak_trapdoor
    BlockStates { base: 7626, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:mangrove_trapdoor
    BlockStates { base: 7690, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:bamboo_trapdoor
    BlockStates { base: 7754, count: 1, default_offset: 0, properties: &[] }, // minecraft:stone_bricks
    BlockStates { base: 7755, count: 1, default_offset: 0, properties: &[] }, // minecraft:mossy_stone_bricks
    BlockStates { base: 7756, count: 1, default_offset: 0, properties: &[] }, // minecraft:cracked_stone_bricks
    BlockStates { base: 7757, count: 1, default_offset: 0, properties: &[] }, // minecraft:chiseled_stone_bricks
    BlockStates { base: 7758, count: 1, default_offset: 0, properties: &[] }, // minecraft:packed_mud
    BlockStates { base: 7759, count: 1, default_offset: 0, properties: &[] }, // minecraft:mud_bricks
    BlockStates { base: 7760, count: 1, default_offset: 0, properties: &[] }, // minecraft:infested_stone
    BlockStates { base: 7761, count: 1, default_offset: 0, properties: &[] }, // minecraft:infested_cobblestone
    BlockStates { base: 7762, count: 1, default_offset: 0, properties: &[] }, // minecraft:infested_stone_bricks
    BlockStates { base: 7763, count: 1, default_offset: 0, properties: &[] }, // minecraft:infested_mossy_stone_bricks
    BlockStates { base: 7764, count: 1, default_offset: 0, properties: &[] }, // minecraft:infested_cracked_stone_bricks
    BlockStates { base: 7765, count: 1, default_offset: 0, properties: &[] }, // minecraft:infested_chiseled_stone_bricks
    BlockStates { base: 7766, count: 64, default_offset: 0, properties: &[Property { name: "down", values: &["true", "false"] }, Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:brown_mushroom_block
    BlockStates { base: 7830, count: 64, default_offset: 0, properties: &[Property { name: "down", values: &["true", "false"] }, Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:red_mushroom_block
    BlockStates { base: 7894, count: 64, default_offset: 0, properties: &[Property { name: "down", values: &["true", "false"] }, Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:mushroom_stem
    BlockStates { base: 7958, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:iron_bars
    BlockStates { base: 7990, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:copper_bars
    BlockStates { base: 8022, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:exposed_copper_bars
    BlockStates { base: 8054, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:weathered_copper_bars
    BlockStates { base: 8086, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:oxidized_copper_bars
    BlockStates { base: 8118, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:waxed_copper_bars
    BlockStates { base: 8150, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:waxed_exposed_copper_bars
    BlockStates { base: 8182, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:waxed_weathered_copper_bars
    BlockStates { base: 8214, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:waxed_oxidized_copper_bars
    BlockStates { base: 8246, count: 6, default_offset: 3, properties: &[Property { name: "axis", values: &["x", "y", "z"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:iron_chain
    BlockStates { base: 8252, count: 6, default_offset: 3, properties: &[Property { name: "axis", values: &["x", "y", "z"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:copper_chain
    BlockStates { base: 8258, count: 6, default_offset: 3, properties: &[Property { name: "axis", values: &["x", "y", "z"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:exposed_copper_chain
    BlockStates { base: 8264, count: 6, default_offset: 3, properties: &[Property { name: "axis", values: &["x", "y", "z"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:weathered_copper_chain
    BlockStates { base: 8270, count: 6, default_offset: 3, properties: &[Property { name: "axis", values: &["x", "y", "z"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:oxidized_copper_chain
    BlockStates { base: 8276, count: 6, default_offset: 3, properties: &[Property { name: "axis", values: &["x", "y", "z"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_copper_chain
    BlockStates { base: 8282, count: 6, default_offset: 3, properties: &[Property { name: "axis", values: &["x", "y", "z"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_exposed_copper_chain
    BlockStates { base: 8288, count: 6, default_offset: 3, properties: &[Property { name: "axis", values: &["x", "y", "z"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_weathered_copper_chain
    BlockStates { base: 8294, count: 6, default_offset: 3, properties: &[Property { name: "axis", values: &["x", "y", "z"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_oxidized_copper_chain
    BlockStates { base: 8300, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:glass_pane
    BlockStates { base: 8332, count: 1, default_offset: 0, properties: &[] }, // minecraft:pumpkin
    BlockStates { base: 8333, count: 1, default_offset: 0, properties: &[] }, // minecraft:melon
    BlockStates { base: 8334, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:attached_pumpkin_stem
    BlockStates { base: 8338, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:attached_melon_stem
    BlockStates { base: 8342, count: 8, default_offset: 0, properties: &[Property { name: "age", values: &["0", "1", "2", "3", "4", "5", "6", "7"] }] }, // minecraft:pumpkin_stem
    BlockStates { base: 8350, count: 8, default_offset: 0, properties: &[Property { name: "age", values: &["0", "1", "2", "3", "4", "5", "6", "7"] }] }, // minecraft:melon_stem
    BlockStates { base: 8358, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:vine
    BlockStates { base: 8390, count: 128, default_offset: 127, properties: &[Property { name: "down", values: &["true", "false"] }, Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:glow_lichen
    BlockStates { base: 8518, count: 128, default_offset: 127, properties: &[Property { name: "down", values: &["true", "false"] }, Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:resin_clump
    BlockStates { base: 8646, count: 32, default_offset: 7, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "in_wall", values: &["true", "false"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:oak_fence_gate
    BlockStates { base: 8678, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:brick_stairs
    BlockStates { base: 8758, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:stone_brick_stairs
    BlockStates { base: 8838, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:mud_brick_stairs
    BlockStates { base: 8918, count: 2, default_offset: 1, properties: &[Property { name: "snowy", values: &["true", "false"] }] }, // minecraft:mycelium
    BlockStates { base: 8920, count: 1, default_offset: 0, properties: &[] }, // minecraft:lily_pad
    BlockStates { base: 8921, count: 1, default_offset: 0, properties: &[] }, // minecraft:resin_block
    BlockStates { base: 8922, count: 1, default_offset: 0, properties: &[] }, // minecraft:resin_bricks
    BlockStates { base: 8923, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:resin_brick_stairs
    BlockStates { base: 9003, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:resin_brick_slab
    BlockStates { base: 9009, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:resin_brick_wall
    BlockStates { base: 9333, count: 1, default_offset: 0, properties: &[] }, // minecraft:chiseled_resin_bricks
    BlockStates { base: 9334, count: 1, default_offset: 0, properties: &[] }, // minecraft:nether_bricks
    BlockStates { base: 9335, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:nether_brick_fence
    BlockStates { base: 9367, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:nether_brick_stairs
    BlockStates { base: 9447, count: 4, default_offset: 0, properties: &[Property { name: "age", values: &["0", "1", "2", "3"] }] }, // minecraft:nether_wart
    BlockStates { base: 9451, count: 1, default_offset: 0, properties: &[] }, // minecraft:enchanting_table
    BlockStates { base: 9452, count: 8, default_offset: 7, properties: &[Property { name: "has_bottle_0", values: &["true", "false"] }, Property { name: "has_bottle_1", values: &["true", "false"] }, Property { name: "has_bottle_2", values: &["true", "false"] }] }, // minecraft:brewing_stand
    BlockStates { base: 9460, count: 1, default_offset: 0, properties: &[] }, // minecraft:cauldron
    BlockStates { base: 9461, count: 3, default_offset: 0, properties: &[Property { name: "level", values: &["1", "2", "3"] }] }, // minecraft:water_cauldron
    BlockStates { base: 9464, count: 1, default_offset: 0, properties: &[] }, // minecraft:lava_cauldron
    BlockStates { base: 9465, count: 3, default_offset: 0, properties: &[Property { name: "level", values: &["1", "2", "3"] }] }, // minecraft:powder_snow_cauldron
    BlockStates { base: 9468, count: 1, default_offset: 0, properties: &[] }, // minecraft:end_portal
    BlockStates { base: 9469, count: 8, default_offset: 4, properties: &[Property { name: "eye", values: &["true", "false"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:end_portal_frame
    BlockStates { base: 9477, count: 1, default_offset: 0, properties: &[] }, // minecraft:end_stone
    BlockStates { base: 9478, count: 1, default_offset: 0, properties: &[] }, // minecraft:dragon_egg
    BlockStates { base: 9479, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:redstone_lamp
    BlockStates { base: 9481, count: 12, default_offset: 0, properties: &[Property { name: "age", values: &["0", "1", "2"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:cocoa
    BlockStates { base: 9493, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:sandstone_stairs
    BlockStates { base: 9573, count: 1, default_offset: 0, properties: &[] }, // minecraft:emerald_ore
    BlockStates { base: 9574, count: 1, default_offset: 0, properties: &[] }, // minecraft:deepslate_emerald_ore
    BlockStates { base: 9575, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:ender_chest
    BlockStates { base: 9583, count: 16, default_offset: 9, properties: &[Property { name: "attached", values: &["true", "false"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:tripwire_hook
    BlockStates { base: 9599, count: 128, default_offset: 127, properties: &[Property { name: "attached", values: &["true", "false"] }, Property { name: "disarmed", values: &["true", "false"] }, Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:tripwire
    BlockStates { base: 9727, count: 1, default_offset: 0, properties: &[] }, // minecraft:emerald_block
    BlockStates { base: 9728, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:spruce_stairs
    BlockStates { base: 9808, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:birch_stairs
    BlockStates { base: 9888, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:jungle_stairs
    BlockStates { base: 9968, count: 12, default_offset: 6, properties: &[Property { name: "conditional", values: &["true", "false"] }, Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:command_block
    BlockStates { base: 9980, count: 1, default_offset: 0, properties: &[] }, // minecraft:beacon
    BlockStates { base: 9981, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:cobblestone_wall
    BlockStates { base: 10305, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:mossy_cobblestone_wall
    BlockStates { base: 10629, count: 1, default_offset: 0, properties: &[] }, // minecraft:flower_pot
    BlockStates { base: 10630, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_torchflower
    BlockStates { base: 10631, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_oak_sapling
    BlockStates { base: 10632, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_spruce_sapling
    BlockStates { base: 10633, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_birch_sapling
    BlockStates { base: 10634, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_jungle_sapling
    BlockStates { base: 10635, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_acacia_sapling
    BlockStates { base: 10636, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_cherry_sapling
    BlockStates { base: 10637, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_dark_oak_sapling
    BlockStates { base: 10638, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_pale_oak_sapling
    BlockStates { base: 10639, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_mangrove_propagule
    BlockStates { base: 10640, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_fern
    BlockStates { base: 10641, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_dandelion
    BlockStates { base: 10642, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_golden_dandelion
    BlockStates { base: 10643, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_poppy
    BlockStates { base: 10644, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_blue_orchid
    BlockStates { base: 10645, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_allium
    BlockStates { base: 10646, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_azure_bluet
    BlockStates { base: 10647, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_red_tulip
    BlockStates { base: 10648, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_orange_tulip
    BlockStates { base: 10649, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_white_tulip
    BlockStates { base: 10650, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_pink_tulip
    BlockStates { base: 10651, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_oxeye_daisy
    BlockStates { base: 10652, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_cornflower
    BlockStates { base: 10653, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_lily_of_the_valley
    BlockStates { base: 10654, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_wither_rose
    BlockStates { base: 10655, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_red_mushroom
    BlockStates { base: 10656, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_brown_mushroom
    BlockStates { base: 10657, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_dead_bush
    BlockStates { base: 10658, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_cactus
    BlockStates { base: 10659, count: 8, default_offset: 0, properties: &[Property { name: "age", values: &["0", "1", "2", "3", "4", "5", "6", "7"] }] }, // minecraft:carrots
    BlockStates { base: 10667, count: 8, default_offset: 0, properties: &[Property { name: "age", values: &["0", "1", "2", "3", "4", "5", "6", "7"] }] }, // minecraft:potatoes
    BlockStates { base: 10675, count: 24, default_offset: 9, properties: &[Property { name: "face", values: &["floor", "wall", "ceiling"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:oak_button
    BlockStates { base: 10699, count: 24, default_offset: 9, properties: &[Property { name: "face", values: &["floor", "wall", "ceiling"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:spruce_button
    BlockStates { base: 10723, count: 24, default_offset: 9, properties: &[Property { name: "face", values: &["floor", "wall", "ceiling"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:birch_button
    BlockStates { base: 10747, count: 24, default_offset: 9, properties: &[Property { name: "face", values: &["floor", "wall", "ceiling"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:jungle_button
    BlockStates { base: 10771, count: 24, default_offset: 9, properties: &[Property { name: "face", values: &["floor", "wall", "ceiling"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:acacia_button
    BlockStates { base: 10795, count: 24, default_offset: 9, properties: &[Property { name: "face", values: &["floor", "wall", "ceiling"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:cherry_button
    BlockStates { base: 10819, count: 24, default_offset: 9, properties: &[Property { name: "face", values: &["floor", "wall", "ceiling"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:dark_oak_button
    BlockStates { base: 10843, count: 24, default_offset: 9, properties: &[Property { name: "face", values: &["floor", "wall", "ceiling"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:pale_oak_button
    BlockStates { base: 10867, count: 24, default_offset: 9, properties: &[Property { name: "face", values: &["floor", "wall", "ceiling"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:mangrove_button
    BlockStates { base: 10891, count: 24, default_offset: 9, properties: &[Property { name: "face", values: &["floor", "wall", "ceiling"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:bamboo_button
    BlockStates { base: 10915, count: 32, default_offset: 16, properties: &[Property { name: "powered", values: &["true", "false"] }, Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:skeleton_skull
    BlockStates { base: 10947, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:skeleton_wall_skull
    BlockStates { base: 10955, count: 32, default_offset: 16, properties: &[Property { name: "powered", values: &["true", "false"] }, Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:wither_skeleton_skull
    BlockStates { base: 10987, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:wither_skeleton_wall_skull
    BlockStates { base: 10995, count: 32, default_offset: 16, properties: &[Property { name: "powered", values: &["true", "false"] }, Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:zombie_head
    BlockStates { base: 11027, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:zombie_wall_head
    BlockStates { base: 11035, count: 32, default_offset: 16, properties: &[Property { name: "powered", values: &["true", "false"] }, Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:player_head
    BlockStates { base: 11067, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:player_wall_head
    BlockStates { base: 11075, count: 32, default_offset: 16, properties: &[Property { name: "powered", values: &["true", "false"] }, Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:creeper_head
    BlockStates { base: 11107, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:creeper_wall_head
    BlockStates { base: 11115, count: 32, default_offset: 16, properties: &[Property { name: "powered", values: &["true", "false"] }, Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:dragon_head
    BlockStates { base: 11147, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:dragon_wall_head
    BlockStates { base: 11155, count: 32, default_offset: 16, properties: &[Property { name: "powered", values: &["true", "false"] }, Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:piglin_head
    BlockStates { base: 11187, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:piglin_wall_head
    BlockStates { base: 11195, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:anvil
    BlockStates { base: 11199, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:chipped_anvil
    BlockStates { base: 11203, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:damaged_anvil
    BlockStates { base: 11207, count: 24, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "type", values: &["single", "left", "right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:trapped_chest
    BlockStates { base: 11231, count: 16, default_offset: 0, properties: &[Property { name: "power", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:light_weighted_pressure_plate
    BlockStates { base: 11247, count: 16, default_offset: 0, properties: &[Property { name: "power", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:heavy_weighted_pressure_plate
    BlockStates { base: 11263, count: 16, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "mode", values: &["compare", "subtract"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:comparator
    BlockStates { base: 11279, count: 32, default_offset: 16, properties: &[Property { name: "inverted", values: &["true", "false"] }, Property { name: "power", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:daylight_detector
    BlockStates { base: 11311, count: 1, default_offset: 0, properties: &[] }, // minecraft:redstone_block
    BlockStates { base: 11312, count: 1, default_offset: 0, properties: &[] }, // minecraft:nether_quartz_ore
    BlockStates { base: 11313, count: 10, default_offset: 0, properties: &[Property { name: "enabled", values: &["true", "false"] }, Property { name: "facing", values: &["down", "north", "south", "west", "east"] }] }, // minecraft:hopper
    BlockStates { base: 11323, count: 1, default_offset: 0, properties: &[] }, // minecraft:quartz_block
    BlockStates { base: 11324, count: 1, default_offset: 0, properties: &[] }, // minecraft:chiseled_quartz_block
    BlockStates { base: 11325, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:quartz_pillar
    BlockStates { base: 11328, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:quartz_stairs
    BlockStates { base: 11408, count: 24, default_offset: 13, properties: &[Property { name: "powered", values: &["true", "false"] }, Property { name: "shape", values: &["north_south", "east_west", "ascending_east", "ascending_west", "ascending_north", "ascending_south"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:activator_rail
    BlockStates { base: 11432, count: 12, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }, Property { name: "triggered", values: &["true", "false"] }] }, // minecraft:dropper
    BlockStates { base: 11444, count: 1, default_offset: 0, properties: &[] }, // minecraft:white_terracotta
    BlockStates { base: 11445, count: 1, default_offset: 0, properties: &[] }, // minecraft:orange_terracotta
    BlockStates { base: 11446, count: 1, default_offset: 0, properties: &[] }, // minecraft:magenta_terracotta
    BlockStates { base: 11447, count: 1, default_offset: 0, properties: &[] }, // minecraft:light_blue_terracotta
    BlockStates { base: 11448, count: 1, default_offset: 0, properties: &[] }, // minecraft:yellow_terracotta
    BlockStates { base: 11449, count: 1, default_offset: 0, properties: &[] }, // minecraft:lime_terracotta
    BlockStates { base: 11450, count: 1, default_offset: 0, properties: &[] }, // minecraft:pink_terracotta
    BlockStates { base: 11451, count: 1, default_offset: 0, properties: &[] }, // minecraft:gray_terracotta
    BlockStates { base: 11452, count: 1, default_offset: 0, properties: &[] }, // minecraft:light_gray_terracotta
    BlockStates { base: 11453, count: 1, default_offset: 0, properties: &[] }, // minecraft:cyan_terracotta
    BlockStates { base: 11454, count: 1, default_offset: 0, properties: &[] }, // minecraft:purple_terracotta
    BlockStates { base: 11455, count: 1, default_offset: 0, properties: &[] }, // minecraft:blue_terracotta
    BlockStates { base: 11456, count: 1, default_offset: 0, properties: &[] }, // minecraft:brown_terracotta
    BlockStates { base: 11457, count: 1, default_offset: 0, properties: &[] }, // minecraft:green_terracotta
    BlockStates { base: 11458, count: 1, default_offset: 0, properties: &[] }, // minecraft:red_terracotta
    BlockStates { base: 11459, count: 1, default_offset: 0, properties: &[] }, // minecraft:black_terracotta
    BlockStates { base: 11460, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:white_stained_glass_pane
    BlockStates { base: 11492, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:orange_stained_glass_pane
    BlockStates { base: 11524, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:magenta_stained_glass_pane
    BlockStates { base: 11556, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:light_blue_stained_glass_pane
    BlockStates { base: 11588, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:yellow_stained_glass_pane
    BlockStates { base: 11620, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:lime_stained_glass_pane
    BlockStates { base: 11652, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:pink_stained_glass_pane
    BlockStates { base: 11684, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:gray_stained_glass_pane
    BlockStates { base: 11716, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:light_gray_stained_glass_pane
    BlockStates { base: 11748, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:cyan_stained_glass_pane
    BlockStates { base: 11780, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:purple_stained_glass_pane
    BlockStates { base: 11812, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:blue_stained_glass_pane
    BlockStates { base: 11844, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:brown_stained_glass_pane
    BlockStates { base: 11876, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:green_stained_glass_pane
    BlockStates { base: 11908, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:red_stained_glass_pane
    BlockStates { base: 11940, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:black_stained_glass_pane
    BlockStates { base: 11972, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:acacia_stairs
    BlockStates { base: 12052, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cherry_stairs
    BlockStates { base: 12132, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dark_oak_stairs
    BlockStates { base: 12212, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:pale_oak_stairs
    BlockStates { base: 12292, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:mangrove_stairs
    BlockStates { base: 12372, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:bamboo_stairs
    BlockStates { base: 12452, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:bamboo_mosaic_stairs
    BlockStates { base: 12532, count: 1, default_offset: 0, properties: &[] }, // minecraft:slime_block
    BlockStates { base: 12533, count: 2, default_offset: 1, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:barrier
    BlockStates { base: 12535, count: 32, default_offset: 31, properties: &[Property { name: "level", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:light
    BlockStates { base: 12567, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:iron_trapdoor
    BlockStates { base: 12631, count: 1, default_offset: 0, properties: &[] }, // minecraft:prismarine
    BlockStates { base: 12632, count: 1, default_offset: 0, properties: &[] }, // minecraft:prismarine_bricks
    BlockStates { base: 12633, count: 1, default_offset: 0, properties: &[] }, // minecraft:dark_prismarine
    BlockStates { base: 12634, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:prismarine_stairs
    BlockStates { base: 12714, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:prismarine_brick_stairs
    BlockStates { base: 12794, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dark_prismarine_stairs
    BlockStates { base: 12874, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:prismarine_slab
    BlockStates { base: 12880, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:prismarine_brick_slab
    BlockStates { base: 12886, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dark_prismarine_slab
    BlockStates { base: 12892, count: 1, default_offset: 0, properties: &[] }, // minecraft:sea_lantern
    BlockStates { base: 12893, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:hay_block
    BlockStates { base: 12896, count: 1, default_offset: 0, properties: &[] }, // minecraft:white_carpet
    BlockStates { base: 12897, count: 1, default_offset: 0, properties: &[] }, // minecraft:orange_carpet
    BlockStates { base: 12898, count: 1, default_offset: 0, properties: &[] }, // minecraft:magenta_carpet
    BlockStates { base: 12899, count: 1, default_offset: 0, properties: &[] }, // minecraft:light_blue_carpet
    BlockStates { base: 12900, count: 1, default_offset: 0, properties: &[] }, // minecraft:yellow_carpet
    BlockStates { base: 12901, count: 1, default_offset: 0, properties: &[] }, // minecraft:lime_carpet
    BlockStates { base: 12902, count: 1, default_offset: 0, properties: &[] }, // minecraft:pink_carpet
    BlockStates { base: 12903, count: 1, default_offset: 0, properties: &[] }, // minecraft:gray_carpet
    BlockStates { base: 12904, count: 1, default_offset: 0, properties: &[] }, // minecraft:light_gray_carpet
    BlockStates { base: 12905, count: 1, default_offset: 0, properties: &[] }, // minecraft:cyan_carpet
    BlockStates { base: 12906, count: 1, default_offset: 0, properties: &[] }, // minecraft:purple_carpet
    BlockStates { base: 12907, count: 1, default_offset: 0, properties: &[] }, // minecraft:blue_carpet
    BlockStates { base: 12908, count: 1, default_offset: 0, properties: &[] }, // minecraft:brown_carpet
    BlockStates { base: 12909, count: 1, default_offset: 0, properties: &[] }, // minecraft:green_carpet
    BlockStates { base: 12910, count: 1, default_offset: 0, properties: &[] }, // minecraft:red_carpet
    BlockStates { base: 12911, count: 1, default_offset: 0, properties: &[] }, // minecraft:black_carpet
    BlockStates { base: 12912, count: 1, default_offset: 0, properties: &[] }, // minecraft:terracotta
    BlockStates { base: 12913, count: 1, default_offset: 0, properties: &[] }, // minecraft:coal_block
    BlockStates { base: 12914, count: 1, default_offset: 0, properties: &[] }, // minecraft:packed_ice
    BlockStates { base: 12915, count: 2, default_offset: 1, properties: &[Property { name: "half", values: &["upper", "lower"] }] }, // minecraft:sunflower
    BlockStates { base: 12917, count: 2, default_offset: 1, properties: &[Property { name: "half", values: &["upper", "lower"] }] }, // minecraft:lilac
    BlockStates { base: 12919, count: 2, default_offset: 1, properties: &[Property { name: "half", values: &["upper", "lower"] }] }, // minecraft:rose_bush
    BlockStates { base: 12921, count: 2, default_offset: 1, properties: &[Property { name: "half", values: &["upper", "lower"] }] }, // minecraft:peony
    BlockStates { base: 12923, count: 2, default_offset: 1, properties: &[Property { name: "half", values: &["upper", "lower"] }] }, // minecraft:tall_grass
    BlockStates { base: 12925, count: 2, default_offset: 1, properties: &[Property { name: "half", values: &["upper", "lower"] }] }, // minecraft:large_fern
    BlockStates { base: 12927, count: 16, default_offset: 8, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:white_banner
    BlockStates { base: 12943, count: 16, default_offset: 8, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:orange_banner
    BlockStates { base: 12959, count: 16, default_offset: 8, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:magenta_banner
    BlockStates { base: 12975, count: 16, default_offset: 8, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:light_blue_banner
    BlockStates { base: 12991, count: 16, default_offset: 8, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:yellow_banner
    BlockStates { base: 13007, count: 16, default_offset: 8, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:lime_banner
    BlockStates { base: 13023, count: 16, default_offset: 8, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:pink_banner
    BlockStates { base: 13039, count: 16, default_offset: 8, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:gray_banner
    BlockStates { base: 13055, count: 16, default_offset: 8, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:light_gray_banner
    BlockStates { base: 13071, count: 16, default_offset: 8, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:cyan_banner
    BlockStates { base: 13087, count: 16, default_offset: 8, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:purple_banner
    BlockStates { base: 13103, count: 16, default_offset: 8, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:blue_banner
    BlockStates { base: 13119, count: 16, default_offset: 8, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:brown_banner
    BlockStates { base: 13135, count: 16, default_offset: 8, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:green_banner
    BlockStates { base: 13151, count: 16, default_offset: 8, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:red_banner
    BlockStates { base: 13167, count: 16, default_offset: 8, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:black_banner
    BlockStates { base: 13183, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:white_wall_banner
    BlockStates { base: 13187, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:orange_wall_banner
    BlockStates { base: 13191, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:magenta_wall_banner
    BlockStates { base: 13195, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:light_blue_wall_banner
    BlockStates { base: 13199, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:yellow_wall_banner
    BlockStates { base: 13203, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:lime_wall_banner
    BlockStates { base: 13207, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:pink_wall_banner
    BlockStates { base: 13211, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:gray_wall_banner
    BlockStates { base: 13215, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:light_gray_wall_banner
    BlockStates { base: 13219, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:cyan_wall_banner
    BlockStates { base: 13223, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:purple_wall_banner
    BlockStates { base: 13227, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:blue_wall_banner
    BlockStates { base: 13231, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:brown_wall_banner
    BlockStates { base: 13235, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:green_wall_banner
    BlockStates { base: 13239, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:red_wall_banner
    BlockStates { base: 13243, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:black_wall_banner
    BlockStates { base: 13247, count: 1, default_offset: 0, properties: &[] }, // minecraft:red_sandstone
    BlockStates { base: 13248, count: 1, default_offset: 0, properties: &[] }, // minecraft:chiseled_red_sandstone
    BlockStates { base: 13249, count: 1, default_offset: 0, properties: &[] }, // minecraft:cut_red_sandstone
    BlockStates { base: 13250, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:red_sandstone_stairs
    BlockStates { base: 13330, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:oak_slab
    BlockStates { base: 13336, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:spruce_slab
    BlockStates { base: 13342, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:birch_slab
    BlockStates { base: 13348, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:jungle_slab
    BlockStates { base: 13354, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:acacia_slab
    BlockStates { base: 13360, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cherry_slab
    BlockStates { base: 13366, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dark_oak_slab
    BlockStates { base: 13372, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:pale_oak_slab
    BlockStates { base: 13378, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:mangrove_slab
    BlockStates { base: 13384, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:bamboo_slab
    BlockStates { base: 13390, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:bamboo_mosaic_slab
    BlockStates { base: 13396, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:stone_slab
    BlockStates { base: 13402, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:smooth_stone_slab
    BlockStates { base: 13408, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:sandstone_slab
    BlockStates { base: 13414, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cut_sandstone_slab
    BlockStates { base: 13420, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:petrified_oak_slab
    BlockStates { base: 13426, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cobblestone_slab
    BlockStates { base: 13432, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:brick_slab
    BlockStates { base: 13438, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:stone_brick_slab
    BlockStates { base: 13444, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:mud_brick_slab
    BlockStates { base: 13450, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:nether_brick_slab
    BlockStates { base: 13456, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:quartz_slab
    BlockStates { base: 13462, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:red_sandstone_slab
    BlockStates { base: 13468, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cut_red_sandstone_slab
    BlockStates { base: 13474, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:purpur_slab
    BlockStates { base: 13480, count: 1, default_offset: 0, properties: &[] }, // minecraft:smooth_stone
    BlockStates { base: 13481, count: 1, default_offset: 0, properties: &[] }, // minecraft:smooth_sandstone
    BlockStates { base: 13482, count: 1, default_offset: 0, properties: &[] }, // minecraft:smooth_quartz
    BlockStates { base: 13483, count: 1, default_offset: 0, properties: &[] }, // minecraft:smooth_red_sandstone
    BlockStates { base: 13484, count: 32, default_offset: 7, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "in_wall", values: &["true", "false"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:spruce_fence_gate
    BlockStates { base: 13516, count: 32, default_offset: 7, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "in_wall", values: &["true", "false"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:birch_fence_gate
    BlockStates { base: 13548, count: 32, default_offset: 7, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "in_wall", values: &["true", "false"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:jungle_fence_gate
    BlockStates { base: 13580, count: 32, default_offset: 7, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "in_wall", values: &["true", "false"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:acacia_fence_gate
    BlockStates { base: 13612, count: 32, default_offset: 7, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "in_wall", values: &["true", "false"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:cherry_fence_gate
    BlockStates { base: 13644, count: 32, default_offset: 7, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "in_wall", values: &["true", "false"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:dark_oak_fence_gate
    BlockStates { base: 13676, count: 32, default_offset: 7, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "in_wall", values: &["true", "false"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:pale_oak_fence_gate
    BlockStates { base: 13708, count: 32, default_offset: 7, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "in_wall", values: &["true", "false"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:mangrove_fence_gate
    BlockStates { base: 13740, count: 32, default_offset: 7, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "in_wall", values: &["true", "false"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:bamboo_fence_gate
    BlockStates { base: 13772, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:spruce_fence
    BlockStates { base: 13804, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:birch_fence
    BlockStates { base: 13836, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:jungle_fence
    BlockStates { base: 13868, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:acacia_fence
    BlockStates { base: 13900, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:cherry_fence
    BlockStates { base: 13932, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:dark_oak_fence
    BlockStates { base: 13964, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:pale_oak_fence
    BlockStates { base: 13996, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:mangrove_fence
    BlockStates { base: 14028, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:bamboo_fence
    BlockStates { base: 14060, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:spruce_door
    BlockStates { base: 14124, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:birch_door
    BlockStates { base: 14188, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:jungle_door
    BlockStates { base: 14252, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:acacia_door
    BlockStates { base: 14316, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:cherry_door
    BlockStates { base: 14380, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:dark_oak_door
    BlockStates { base: 14444, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:pale_oak_door
    BlockStates { base: 14508, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:mangrove_door
    BlockStates { base: 14572, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:bamboo_door
    BlockStates { base: 14636, count: 6, default_offset: 4, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:end_rod
    BlockStates { base: 14642, count: 64, default_offset: 63, properties: &[Property { name: "down", values: &["true", "false"] }, Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:chorus_plant
    BlockStates { base: 14706, count: 6, default_offset: 0, properties: &[Property { name: "age", values: &["0", "1", "2", "3", "4", "5"] }] }, // minecraft:chorus_flower
    BlockStates { base: 14712, count: 1, default_offset: 0, properties: &[] }, // minecraft:purpur_block
    BlockStates { base: 14713, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:purpur_pillar
    BlockStates { base: 14716, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:purpur_stairs
    BlockStates { base: 14796, count: 1, default_offset: 0, properties: &[] }, // minecraft:end_stone_bricks
    BlockStates { base: 14797, count: 2, default_offset: 0, properties: &[Property { name: "age", values: &["0", "1"] }] }, // minecraft:torchflower_crop
    BlockStates { base: 14799, count: 10, default_offset: 1, properties: &[Property { name: "age", values: &["0", "1", "2", "3", "4"] }, Property { name: "half", values: &["upper", "lower"] }] }, // minecraft:pitcher_crop
    BlockStates { base: 14809, count: 2, default_offset: 1, properties: &[Property { name: "half", values: &["upper", "lower"] }] }, // minecraft:pitcher_plant
    BlockStates { base: 14811, count: 4, default_offset: 0, properties: &[Property { name: "age", values: &["0", "1", "2", "3"] }] }, // minecraft:beetroots
    BlockStates { base: 14815, count: 1, default_offset: 0, properties: &[] }, // minecraft:dirt_path
    BlockStates { base: 14816, count: 1, default_offset: 0, properties: &[] }, // minecraft:end_gateway
    BlockStates { base: 14817, count: 12, default_offset: 6, properties: &[Property { name: "conditional", values: &["true", "false"] }, Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:repeating_command_block
    BlockStates { base: 14829, count: 12, default_offset: 6, properties: &[Property { name: "conditional", values: &["true", "false"] }, Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:chain_command_block
    BlockStates { base: 14841, count: 4, default_offset: 0, properties: &[Property { name: "age", values: &["0", "1", "2", "3"] }] }, // minecraft:frosted_ice
    BlockStates { base: 14845, count: 1, default_offset: 0, properties: &[] }, // minecraft:magma_block
    BlockStates { base: 14846, count: 1, default_offset: 0, properties: &[] }, // minecraft:nether_wart_block
    BlockStates { base: 14847, count: 1, default_offset: 0, properties: &[] }, // minecraft:red_nether_bricks
    BlockStates { base: 14848, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:bone_block
    BlockStates { base: 14851, count: 1, default_offset: 0, properties: &[] }, // minecraft:structure_void
    BlockStates { base: 14852, count: 12, default_offset: 5, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:observer
    BlockStates { base: 14864, count: 6, default_offset: 4, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:shulker_box
    BlockStates { base: 14870, count: 6, default_offset: 4, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:white_shulker_box
    BlockStates { base: 14876, count: 6, default_offset: 4, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:orange_shulker_box
    BlockStates { base: 14882, count: 6, default_offset: 4, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:magenta_shulker_box
    BlockStates { base: 14888, count: 6, default_offset: 4, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:light_blue_shulker_box
    BlockStates { base: 14894, count: 6, default_offset: 4, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:yellow_shulker_box
    BlockStates { base: 14900, count: 6, default_offset: 4, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:lime_shulker_box
    BlockStates { base: 14906, count: 6, default_offset: 4, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:pink_shulker_box
    BlockStates { base: 14912, count: 6, default_offset: 4, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:gray_shulker_box
    BlockStates { base: 14918, count: 6, default_offset: 4, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:light_gray_shulker_box
    BlockStates { base: 14924, count: 6, default_offset: 4, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:cyan_shulker_box
    BlockStates { base: 14930, count: 6, default_offset: 4, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:purple_shulker_box
    BlockStates { base: 14936, count: 6, default_offset: 4, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:blue_shulker_box
    BlockStates { base: 14942, count: 6, default_offset: 4, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:brown_shulker_box
    BlockStates { base: 14948, count: 6, default_offset: 4, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:green_shulker_box
    BlockStates { base: 14954, count: 6, default_offset: 4, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:red_shulker_box
    BlockStates { base: 14960, count: 6, default_offset: 4, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }] }, // minecraft:black_shulker_box
    BlockStates { base: 14966, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:white_glazed_terracotta
    BlockStates { base: 14970, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:orange_glazed_terracotta
    BlockStates { base: 14974, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:magenta_glazed_terracotta
    BlockStates { base: 14978, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:light_blue_glazed_terracotta
    BlockStates { base: 14982, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:yellow_glazed_terracotta
    BlockStates { base: 14986, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:lime_glazed_terracotta
    BlockStates { base: 14990, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:pink_glazed_terracotta
    BlockStates { base: 14994, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:gray_glazed_terracotta
    BlockStates { base: 14998, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:light_gray_glazed_terracotta
    BlockStates { base: 15002, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:cyan_glazed_terracotta
    BlockStates { base: 15006, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:purple_glazed_terracotta
    BlockStates { base: 15010, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:blue_glazed_terracotta
    BlockStates { base: 15014, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:brown_glazed_terracotta
    BlockStates { base: 15018, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:green_glazed_terracotta
    BlockStates { base: 15022, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:red_glazed_terracotta
    BlockStates { base: 15026, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:black_glazed_terracotta
    BlockStates { base: 15030, count: 1, default_offset: 0, properties: &[] }, // minecraft:white_concrete
    BlockStates { base: 15031, count: 1, default_offset: 0, properties: &[] }, // minecraft:orange_concrete
    BlockStates { base: 15032, count: 1, default_offset: 0, properties: &[] }, // minecraft:magenta_concrete
    BlockStates { base: 15033, count: 1, default_offset: 0, properties: &[] }, // minecraft:light_blue_concrete
    BlockStates { base: 15034, count: 1, default_offset: 0, properties: &[] }, // minecraft:yellow_concrete
    BlockStates { base: 15035, count: 1, default_offset: 0, properties: &[] }, // minecraft:lime_concrete
    BlockStates { base: 15036, count: 1, default_offset: 0, properties: &[] }, // minecraft:pink_concrete
    BlockStates { base: 15037, count: 1, default_offset: 0, properties: &[] }, // minecraft:gray_concrete
    BlockStates { base: 15038, count: 1, default_offset: 0, properties: &[] }, // minecraft:light_gray_concrete
    BlockStates { base: 15039, count: 1, default_offset: 0, properties: &[] }, // minecraft:cyan_concrete
    BlockStates { base: 15040, count: 1, default_offset: 0, properties: &[] }, // minecraft:purple_concrete
    BlockStates { base: 15041, count: 1, default_offset: 0, properties: &[] }, // minecraft:blue_concrete
    BlockStates { base: 15042, count: 1, default_offset: 0, properties: &[] }, // minecraft:brown_concrete
    BlockStates { base: 15043, count: 1, default_offset: 0, properties: &[] }, // minecraft:green_concrete
    BlockStates { base: 15044, count: 1, default_offset: 0, properties: &[] }, // minecraft:red_concrete
    BlockStates { base: 15045, count: 1, default_offset: 0, properties: &[] }, // minecraft:black_concrete
    BlockStates { base: 15046, count: 1, default_offset: 0, properties: &[] }, // minecraft:white_concrete_powder
    BlockStates { base: 15047, count: 1, default_offset: 0, properties: &[] }, // minecraft:orange_concrete_powder
    BlockStates { base: 15048, count: 1, default_offset: 0, properties: &[] }, // minecraft:magenta_concrete_powder
    BlockStates { base: 15049, count: 1, default_offset: 0, properties: &[] }, // minecraft:light_blue_concrete_powder
    BlockStates { base: 15050, count: 1, default_offset: 0, properties: &[] }, // minecraft:yellow_concrete_powder
    BlockStates { base: 15051, count: 1, default_offset: 0, properties: &[] }, // minecraft:lime_concrete_powder
    BlockStates { base: 15052, count: 1, default_offset: 0, properties: &[] }, // minecraft:pink_concrete_powder
    BlockStates { base: 15053, count: 1, default_offset: 0, properties: &[] }, // minecraft:gray_concrete_powder
    BlockStates { base: 15054, count: 1, default_offset: 0, properties: &[] }, // minecraft:light_gray_concrete_powder
    BlockStates { base: 15055, count: 1, default_offset: 0, properties: &[] }, // minecraft:cyan_concrete_powder
    BlockStates { base: 15056, count: 1, default_offset: 0, properties: &[] }, // minecraft:purple_concrete_powder
    BlockStates { base: 15057, count: 1, default_offset: 0, properties: &[] }, // minecraft:blue_concrete_powder
    BlockStates { base: 15058, count: 1, default_offset: 0, properties: &[] }, // minecraft:brown_concrete_powder
    BlockStates { base: 15059, count: 1, default_offset: 0, properties: &[] }, // minecraft:green_concrete_powder
    BlockStates { base: 15060, count: 1, default_offset: 0, properties: &[] }, // minecraft:red_concrete_powder
    BlockStates { base: 15061, count: 1, default_offset: 0, properties: &[] }, // minecraft:black_concrete_powder
    BlockStates { base: 15062, count: 26, default_offset: 0, properties: &[Property { name: "age", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16", "17", "18", "19", "20", "21", "22", "23", "24", "25"] }] }, // minecraft:kelp
    BlockStates { base: 15088, count: 1, default_offset: 0, properties: &[] }, // minecraft:kelp_plant
    BlockStates { base: 15089, count: 1, default_offset: 0, properties: &[] }, // minecraft:dried_kelp_block
    BlockStates { base: 15090, count: 12, default_offset: 0, properties: &[Property { name: "eggs", values: &["1", "2", "3", "4"] }, Property { name: "hatch", values: &["0", "1", "2"] }] }, // minecraft:turtle_egg
    BlockStates { base: 15102, count: 3, default_offset: 0, properties: &[Property { name: "hatch", values: &["0", "1", "2"] }] }, // minecraft:sniffer_egg
    BlockStates { base: 15105, count: 32, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "hydration", values: &["0", "1", "2", "3"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dried_ghast
    BlockStates { base: 15137, count: 1, default_offset: 0, properties: &[] }, // minecraft:dead_tube_coral_block
    BlockStates { base: 15138, count: 1, default_offset: 0, properties: &[] }, // minecraft:dead_brain_coral_block
    BlockStates { base: 15139, count: 1, default_offset: 0, properties: &[] }, // minecraft:dead_bubble_coral_block
    BlockStates { base: 15140, count: 1, default_offset: 0, properties: &[] }, // minecraft:dead_fire_coral_block
    BlockStates { base: 15141, count: 1, default_offset: 0, properties: &[] }, // minecraft:dead_horn_coral_block
    BlockStates { base: 15142, count: 1, default_offset: 0, properties: &[] }, // minecraft:tube_coral_block
    BlockStates { base: 15143, count: 1, default_offset: 0, properties: &[] }, // minecraft:brain_coral_block
    BlockStates { base: 15144, count: 1, default_offset: 0, properties: &[] }, // minecraft:bubble_coral_block
    BlockStates { base: 15145, count: 1, default_offset: 0, properties: &[] }, // minecraft:fire_coral_block
    BlockStates { base: 15146, count: 1, default_offset: 0, properties: &[] }, // minecraft:horn_coral_block
    BlockStates { base: 15147, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dead_tube_coral
    BlockStates { base: 15149, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dead_brain_coral
    BlockStates { base: 15151, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dead_bubble_coral
    BlockStates { base: 15153, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dead_fire_coral
    BlockStates { base: 15155, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dead_horn_coral
    BlockStates { base: 15157, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:tube_coral
    BlockStates { base: 15159, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:brain_coral
    BlockStates { base: 15161, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:bubble_coral
    BlockStates { base: 15163, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:fire_coral
    BlockStates { base: 15165, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:horn_coral
    BlockStates { base: 15167, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dead_tube_coral_fan
    BlockStates { base: 15169, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dead_brain_coral_fan
    BlockStates { base: 15171, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dead_bubble_coral_fan
    BlockStates { base: 15173, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dead_fire_coral_fan
    BlockStates { base: 15175, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dead_horn_coral_fan
    BlockStates { base: 15177, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:tube_coral_fan
    BlockStates { base: 15179, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:brain_coral_fan
    BlockStates { base: 15181, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:bubble_coral_fan
    BlockStates { base: 15183, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:fire_coral_fan
    BlockStates { base: 15185, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:horn_coral_fan
    BlockStates { base: 15187, count: 8, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dead_tube_coral_wall_fan
    BlockStates { base: 15195, count: 8, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dead_brain_coral_wall_fan
    BlockStates { base: 15203, count: 8, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dead_bubble_coral_wall_fan
    BlockStates { base: 15211, count: 8, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dead_fire_coral_wall_fan
    BlockStates { base: 15219, count: 8, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:dead_horn_coral_wall_fan
    BlockStates { base: 15227, count: 8, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:tube_coral_wall_fan
    BlockStates { base: 15235, count: 8, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:brain_coral_wall_fan
    BlockStates { base: 15243, count: 8, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:bubble_coral_wall_fan
    BlockStates { base: 15251, count: 8, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:fire_coral_wall_fan
    BlockStates { base: 15259, count: 8, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:horn_coral_wall_fan
    BlockStates { base: 15267, count: 8, default_offset: 0, properties: &[Property { name: "pickles", values: &["1", "2", "3", "4"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:sea_pickle
    BlockStates { base: 15275, count: 1, default_offset: 0, properties: &[] }, // minecraft:blue_ice
    BlockStates { base: 15276, count: 2, default_offset: 0, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:conduit
    BlockStates { base: 15278, count: 1, default_offset: 0, properties: &[] }, // minecraft:bamboo_sapling
    BlockStates { base: 15279, count: 12, default_offset: 0, properties: &[Property { name: "age", values: &["0", "1"] }, Property { name: "leaves", values: &["none", "small", "large"] }, Property { name: "stage", values: &["0", "1"] }] }, // minecraft:bamboo
    BlockStates { base: 15291, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_bamboo
    BlockStates { base: 15292, count: 1, default_offset: 0, properties: &[] }, // minecraft:void_air
    BlockStates { base: 15293, count: 1, default_offset: 0, properties: &[] }, // minecraft:cave_air
    BlockStates { base: 15294, count: 2, default_offset: 0, properties: &[Property { name: "drag", values: &["true", "false"] }] }, // minecraft:bubble_column
    BlockStates { base: 15296, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:polished_granite_stairs
    BlockStates { base: 15376, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:smooth_red_sandstone_stairs
    BlockStates { base: 15456, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:mossy_stone_brick_stairs
    BlockStates { base: 15536, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:polished_diorite_stairs
    BlockStates { base: 15616, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:mossy_cobblestone_stairs
    BlockStates { base: 15696, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:end_stone_brick_stairs
    BlockStates { base: 15776, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:stone_stairs
    BlockStates { base: 15856, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:smooth_sandstone_stairs
    BlockStates { base: 15936, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:smooth_quartz_stairs
    BlockStates { base: 16016, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:granite_stairs
    BlockStates { base: 16096, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:andesite_stairs
    BlockStates { base: 16176, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:red_nether_brick_stairs
    BlockStates { base: 16256, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:polished_andesite_stairs
    BlockStates { base: 16336, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:diorite_stairs
    BlockStates { base: 16416, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:polished_granite_slab
    BlockStates { base: 16422, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:smooth_red_sandstone_slab
    BlockStates { base: 16428, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:mossy_stone_brick_slab
    BlockStates { base: 16434, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:polished_diorite_slab
    BlockStates { base: 16440, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:mossy_cobblestone_slab
    BlockStates { base: 16446, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:end_stone_brick_slab
    BlockStates { base: 16452, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:smooth_sandstone_slab
    BlockStates { base: 16458, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:smooth_quartz_slab
    BlockStates { base: 16464, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:granite_slab
    BlockStates { base: 16470, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:andesite_slab
    BlockStates { base: 16476, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:red_nether_brick_slab
    BlockStates { base: 16482, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:polished_andesite_slab
    BlockStates { base: 16488, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:diorite_slab
    BlockStates { base: 16494, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:brick_wall
    BlockStates { base: 16818, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:prismarine_wall
    BlockStates { base: 17142, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:red_sandstone_wall
    BlockStates { base: 17466, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:mossy_stone_brick_wall
    BlockStates { base: 17790, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:granite_wall
    BlockStates { base: 18114, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:stone_brick_wall
    BlockStates { base: 18438, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:mud_brick_wall
    BlockStates { base: 18762, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:nether_brick_wall
    BlockStates { base: 19086, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:andesite_wall
    BlockStates { base: 19410, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:red_nether_brick_wall
    BlockStates { base: 19734, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:sandstone_wall
    BlockStates { base: 20058, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:end_stone_brick_wall
    BlockStates { base: 20382, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:diorite_wall
    BlockStates { base: 20706, count: 32, default_offset: 31, properties: &[Property { name: "bottom", values: &["true", "false"] }, Property { name: "distance", values: &["0", "1", "2", "3", "4", "5", "6", "7"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:scaffolding
    BlockStates { base: 20738, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:loom
    BlockStates { base: 20742, count: 12, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }, Property { name: "open", values: &["true", "false"] }] }, // minecraft:barrel
    BlockStates { base: 20754, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "lit", values: &["true", "false"] }] }, // minecraft:smoker
    BlockStates { base: 20762, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "lit", values: &["true", "false"] }] }, // minecraft:blast_furnace
    BlockStates { base: 20770, count: 1, default_offset: 0, properties: &[] }, // minecraft:cartography_table
    BlockStates { base: 20771, count: 1, default_offset: 0, properties: &[] }, // minecraft:fletching_table
    BlockStates { base: 20772, count: 12, default_offset: 4, properties: &[Property { name: "face", values: &["floor", "wall", "ceiling"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:grindstone
    BlockStates { base: 20784, count: 16, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "has_book", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:lectern
    BlockStates { base: 20800, count: 1, default_offset: 0, properties: &[] }, // minecraft:smithing_table
    BlockStates { base: 20801, count: 4, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }] }, // minecraft:stonecutter
    BlockStates { base: 20805, count: 32, default_offset: 1, properties: &[Property { name: "attachment", values: &["floor", "ceiling", "single_wall", "double_wall"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:bell
    BlockStates { base: 20837, count: 4, default_offset: 3, properties: &[Property { name: "hanging", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:lantern
    BlockStates { base: 20841, count: 4, default_offset: 3, properties: &[Property { name: "hanging", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:soul_lantern
    BlockStates { base: 20845, count: 4, default_offset: 3, properties: &[Property { name: "hanging", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:copper_lantern
    BlockStates { base: 20849, count: 4, default_offset: 3, properties: &[Property { name: "hanging", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:exposed_copper_lantern
    BlockStates { base: 20853, count: 4, default_offset: 3, properties: &[Property { name: "hanging", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:weathered_copper_lantern
    BlockStates { base: 20857, count: 4, default_offset: 3, properties: &[Property { name: "hanging", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:oxidized_copper_lantern
    BlockStates { base: 20861, count: 4, default_offset: 3, properties: &[Property { name: "hanging", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_copper_lantern
    BlockStates { base: 20865, count: 4, default_offset: 3, properties: &[Property { name: "hanging", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_exposed_copper_lantern
    BlockStates { base: 20869, count: 4, default_offset: 3, properties: &[Property { name: "hanging", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_weathered_copper_lantern
    BlockStates { base: 20873, count: 4, default_offset: 3, properties: &[Property { name: "hanging", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_oxidized_copper_lantern
    BlockStates { base: 20877, count: 32, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "lit", values: &["true", "false"] }, Property { name: "signal_fire", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:campfire
    BlockStates { base: 20909, count: 32, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "lit", values: &["true", "false"] }, Property { name: "signal_fire", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:soul_campfire
    BlockStates { base: 20941, count: 4, default_offset: 0, properties: &[Property { name: "age", values: &["0", "1", "2", "3"] }] }, // minecraft:sweet_berry_bush
    BlockStates { base: 20945, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:warped_stem
    BlockStates { base: 20948, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_warped_stem
    BlockStates { base: 20951, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:warped_hyphae
    BlockStates { base: 20954, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_warped_hyphae
    BlockStates { base: 20957, count: 1, default_offset: 0, properties: &[] }, // minecraft:warped_nylium
    BlockStates { base: 20958, count: 1, default_offset: 0, properties: &[] }, // minecraft:warped_fungus
    BlockStates { base: 20959, count: 1, default_offset: 0, properties: &[] }, // minecraft:warped_wart_block
    BlockStates { base: 20960, count: 1, default_offset: 0, properties: &[] }, // minecraft:warped_roots
    BlockStates { base: 20961, count: 1, default_offset: 0, properties: &[] }, // minecraft:nether_sprouts
    BlockStates { base: 20962, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:crimson_stem
    BlockStates { base: 20965, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_crimson_stem
    BlockStates { base: 20968, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:crimson_hyphae
    BlockStates { base: 20971, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:stripped_crimson_hyphae
    BlockStates { base: 20974, count: 1, default_offset: 0, properties: &[] }, // minecraft:crimson_nylium
    BlockStates { base: 20975, count: 1, default_offset: 0, properties: &[] }, // minecraft:crimson_fungus
    BlockStates { base: 20976, count: 1, default_offset: 0, properties: &[] }, // minecraft:shroomlight
    BlockStates { base: 20977, count: 26, default_offset: 0, properties: &[Property { name: "age", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16", "17", "18", "19", "20", "21", "22", "23", "24", "25"] }] }, // minecraft:weeping_vines
    BlockStates { base: 21003, count: 1, default_offset: 0, properties: &[] }, // minecraft:weeping_vines_plant
    BlockStates { base: 21004, count: 26, default_offset: 0, properties: &[Property { name: "age", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16", "17", "18", "19", "20", "21", "22", "23", "24", "25"] }] }, // minecraft:twisting_vines
    BlockStates { base: 21030, count: 1, default_offset: 0, properties: &[] }, // minecraft:twisting_vines_plant
    BlockStates { base: 21031, count: 1, default_offset: 0, properties: &[] }, // minecraft:crimson_roots
    BlockStates { base: 21032, count: 1, default_offset: 0, properties: &[] }, // minecraft:crimson_planks
    BlockStates { base: 21033, count: 1, default_offset: 0, properties: &[] }, // minecraft:warped_planks
    BlockStates { base: 21034, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:crimson_slab
    BlockStates { base: 21040, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:warped_slab
    BlockStates { base: 21046, count: 2, default_offset: 1, properties: &[Property { name: "powered", values: &["true", "false"] }] }, // minecraft:crimson_pressure_plate
    BlockStates { base: 21048, count: 2, default_offset: 1, properties: &[Property { name: "powered", values: &["true", "false"] }] }, // minecraft:warped_pressure_plate
    BlockStates { base: 21050, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:crimson_fence
    BlockStates { base: 21082, count: 32, default_offset: 31, properties: &[Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:warped_fence
    BlockStates { base: 21114, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:crimson_trapdoor
    BlockStates { base: 21178, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:warped_trapdoor
    BlockStates { base: 21242, count: 32, default_offset: 7, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "in_wall", values: &["true", "false"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:crimson_fence_gate
    BlockStates { base: 21274, count: 32, default_offset: 7, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "in_wall", values: &["true", "false"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:warped_fence_gate
    BlockStates { base: 21306, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:crimson_stairs
    BlockStates { base: 21386, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:warped_stairs
    BlockStates { base: 21466, count: 24, default_offset: 9, properties: &[Property { name: "face", values: &["floor", "wall", "ceiling"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:crimson_button
    BlockStates { base: 21490, count: 24, default_offset: 9, properties: &[Property { name: "face", values: &["floor", "wall", "ceiling"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:warped_button
    BlockStates { base: 21514, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:crimson_door
    BlockStates { base: 21578, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:warped_door
    BlockStates { base: 21642, count: 32, default_offset: 17, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:crimson_sign
    BlockStates { base: 21674, count: 32, default_offset: 17, properties: &[Property { name: "rotation", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:warped_sign
    BlockStates { base: 21706, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:crimson_wall_sign
    BlockStates { base: 21714, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:warped_wall_sign
    BlockStates { base: 21722, count: 4, default_offset: 1, properties: &[Property { name: "mode", values: &["save", "load", "corner", "data"] }] }, // minecraft:structure_block
    BlockStates { base: 21726, count: 12, default_offset: 10, properties: &[Property { name: "orientation", values: &["down_east", "down_north", "down_south", "down_west", "up_east", "up_north", "up_south", "up_west", "west_up", "east_up", "north_up", "south_up"] }] }, // minecraft:jigsaw
    BlockStates { base: 21738, count: 4, default_offset: 0, properties: &[Property { name: "mode", values: &["start", "log", "fail", "accept"] }] }, // minecraft:test_block
    BlockStates { base: 21742, count: 1, default_offset: 0, properties: &[] }, // minecraft:test_instance_block
    BlockStates { base: 21743, count: 9, default_offset: 0, properties: &[Property { name: "level", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8"] }] }, // minecraft:composter
    BlockStates { base: 21752, count: 16, default_offset: 0, properties: &[Property { name: "power", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }] }, // minecraft:target
    BlockStates { base: 21768, count: 24, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "honey_level", values: &["0", "1", "2", "3", "4", "5"] }] }, // minecraft:bee_nest
    BlockStates { base: 21792, count: 24, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "honey_level", values: &["0", "1", "2", "3", "4", "5"] }] }, // minecraft:beehive
    BlockStates { base: 21816, count: 1, default_offset: 0, properties: &[] }, // minecraft:honey_block
    BlockStates { base: 21817, count: 1, default_offset: 0, properties: &[] }, // minecraft:honeycomb_block
    BlockStates { base: 21818, count: 1, default_offset: 0, properties: &[] }, // minecraft:netherite_block
    BlockStates { base: 21819, count: 1, default_offset: 0, properties: &[] }, // minecraft:ancient_debris
    BlockStates { base: 21820, count: 1, default_offset: 0, properties: &[] }, // minecraft:crying_obsidian
    BlockStates { base: 21821, count: 5, default_offset: 0, properties: &[Property { name: "charges", values: &["0", "1", "2", "3", "4"] }] }, // minecraft:respawn_anchor
    BlockStates { base: 21826, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_crimson_fungus
    BlockStates { base: 21827, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_warped_fungus
    BlockStates { base: 21828, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_crimson_roots
    BlockStates { base: 21829, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_warped_roots
    BlockStates { base: 21830, count: 1, default_offset: 0, properties: &[] }, // minecraft:lodestone
    BlockStates { base: 21831, count: 1, default_offset: 0, properties: &[] }, // minecraft:blackstone
    BlockStates { base: 21832, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:blackstone_stairs
    BlockStates { base: 21912, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:blackstone_wall
    BlockStates { base: 22236, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:blackstone_slab
    BlockStates { base: 22242, count: 1, default_offset: 0, properties: &[] }, // minecraft:polished_blackstone
    BlockStates { base: 22243, count: 1, default_offset: 0, properties: &[] }, // minecraft:polished_blackstone_bricks
    BlockStates { base: 22244, count: 1, default_offset: 0, properties: &[] }, // minecraft:cracked_polished_blackstone_bricks
    BlockStates { base: 22245, count: 1, default_offset: 0, properties: &[] }, // minecraft:chiseled_polished_blackstone
    BlockStates { base: 22246, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:polished_blackstone_brick_slab
    BlockStates { base: 22252, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:polished_blackstone_brick_stairs
    BlockStates { base: 22332, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:polished_blackstone_brick_wall
    BlockStates { base: 22656, count: 1, default_offset: 0, properties: &[] }, // minecraft:gilded_blackstone
    BlockStates { base: 22657, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:polished_blackstone_stairs
    BlockStates { base: 22737, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:polished_blackstone_slab
    BlockStates { base: 22743, count: 2, default_offset: 1, properties: &[Property { name: "powered", values: &["true", "false"] }] }, // minecraft:polished_blackstone_pressure_plate
    BlockStates { base: 22745, count: 24, default_offset: 9, properties: &[Property { name: "face", values: &["floor", "wall", "ceiling"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:polished_blackstone_button
    BlockStates { base: 22769, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:polished_blackstone_wall
    BlockStates { base: 23093, count: 1, default_offset: 0, properties: &[] }, // minecraft:chiseled_nether_bricks
    BlockStates { base: 23094, count: 1, default_offset: 0, properties: &[] }, // minecraft:cracked_nether_bricks
    BlockStates { base: 23095, count: 1, default_offset: 0, properties: &[] }, // minecraft:quartz_bricks
    BlockStates { base: 23096, count: 16, default_offset: 3, properties: &[Property { name: "candles", values: &["1", "2", "3", "4"] }, Property { name: "lit", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:candle
    BlockStates { base: 23112, count: 16, default_offset: 3, properties: &[Property { name: "candles", values: &["1", "2", "3", "4"] }, Property { name: "lit", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:white_candle
    BlockStates { base: 23128, count: 16, default_offset: 3, properties: &[Property { name: "candles", values: &["1", "2", "3", "4"] }, Property { name: "lit", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:orange_candle
    BlockStates { base: 23144, count: 16, default_offset: 3, properties: &[Property { name: "candles", values: &["1", "2", "3", "4"] }, Property { name: "lit", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:magenta_candle
    BlockStates { base: 23160, count: 16, default_offset: 3, properties: &[Property { name: "candles", values: &["1", "2", "3", "4"] }, Property { name: "lit", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:light_blue_candle
    BlockStates { base: 23176, count: 16, default_offset: 3, properties: &[Property { name: "candles", values: &["1", "2", "3", "4"] }, Property { name: "lit", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:yellow_candle
    BlockStates { base: 23192, count: 16, default_offset: 3, properties: &[Property { name: "candles", values: &["1", "2", "3", "4"] }, Property { name: "lit", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:lime_candle
    BlockStates { base: 23208, count: 16, default_offset: 3, properties: &[Property { name: "candles", values: &["1", "2", "3", "4"] }, Property { name: "lit", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:pink_candle
    BlockStates { base: 23224, count: 16, default_offset: 3, properties: &[Property { name: "candles", values: &["1", "2", "3", "4"] }, Property { name: "lit", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:gray_candle
    BlockStates { base: 23240, count: 16, default_offset: 3, properties: &[Property { name: "candles", values: &["1", "2", "3", "4"] }, Property { name: "lit", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:light_gray_candle
    BlockStates { base: 23256, count: 16, default_offset: 3, properties: &[Property { name: "candles", values: &["1", "2", "3", "4"] }, Property { name: "lit", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cyan_candle
    BlockStates { base: 23272, count: 16, default_offset: 3, properties: &[Property { name: "candles", values: &["1", "2", "3", "4"] }, Property { name: "lit", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:purple_candle
    BlockStates { base: 23288, count: 16, default_offset: 3, properties: &[Property { name: "candles", values: &["1", "2", "3", "4"] }, Property { name: "lit", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:blue_candle
    BlockStates { base: 23304, count: 16, default_offset: 3, properties: &[Property { name: "candles", values: &["1", "2", "3", "4"] }, Property { name: "lit", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:brown_candle
    BlockStates { base: 23320, count: 16, default_offset: 3, properties: &[Property { name: "candles", values: &["1", "2", "3", "4"] }, Property { name: "lit", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:green_candle
    BlockStates { base: 23336, count: 16, default_offset: 3, properties: &[Property { name: "candles", values: &["1", "2", "3", "4"] }, Property { name: "lit", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:red_candle
    BlockStates { base: 23352, count: 16, default_offset: 3, properties: &[Property { name: "candles", values: &["1", "2", "3", "4"] }, Property { name: "lit", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:black_candle
    BlockStates { base: 23368, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:candle_cake
    BlockStates { base: 23370, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:white_candle_cake
    BlockStates { base: 23372, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:orange_candle_cake
    BlockStates { base: 23374, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:magenta_candle_cake
    BlockStates { base: 23376, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:light_blue_candle_cake
    BlockStates { base: 23378, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:yellow_candle_cake
    BlockStates { base: 23380, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:lime_candle_cake
    BlockStates { base: 23382, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:pink_candle_cake
    BlockStates { base: 23384, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:gray_candle_cake
    BlockStates { base: 23386, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:light_gray_candle_cake
    BlockStates { base: 23388, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:cyan_candle_cake
    BlockStates { base: 23390, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:purple_candle_cake
    BlockStates { base: 23392, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:blue_candle_cake
    BlockStates { base: 23394, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:brown_candle_cake
    BlockStates { base: 23396, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:green_candle_cake
    BlockStates { base: 23398, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:red_candle_cake
    BlockStates { base: 23400, count: 2, default_offset: 1, properties: &[Property { name: "lit", values: &["true", "false"] }] }, // minecraft:black_candle_cake
    BlockStates { base: 23402, count: 1, default_offset: 0, properties: &[] }, // minecraft:amethyst_block
    BlockStates { base: 23403, count: 1, default_offset: 0, properties: &[] }, // minecraft:budding_amethyst
    BlockStates { base: 23404, count: 12, default_offset: 9, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:amethyst_cluster
    BlockStates { base: 23416, count: 12, default_offset: 9, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:large_amethyst_bud
    BlockStates { base: 23428, count: 12, default_offset: 9, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:medium_amethyst_bud
    BlockStates { base: 23440, count: 12, default_offset: 9, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:small_amethyst_bud
    BlockStates { base: 23452, count: 1, default_offset: 0, properties: &[] }, // minecraft:tuff
    BlockStates { base: 23453, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:tuff_slab
    BlockStates { base: 23459, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:tuff_stairs
    BlockStates { base: 23539, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:tuff_wall
    BlockStates { base: 23863, count: 1, default_offset: 0, properties: &[] }, // minecraft:polished_tuff
    BlockStates { base: 23864, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:polished_tuff_slab
    BlockStates { base: 23870, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:polished_tuff_stairs
    BlockStates { base: 23950, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:polished_tuff_wall
    BlockStates { base: 24274, count: 1, default_offset: 0, properties: &[] }, // minecraft:chiseled_tuff
    BlockStates { base: 24275, count: 1, default_offset: 0, properties: &[] }, // minecraft:tuff_bricks
    BlockStates { base: 24276, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:tuff_brick_slab
    BlockStates { base: 24282, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:tuff_brick_stairs
    BlockStates { base: 24362, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:tuff_brick_wall
    BlockStates { base: 24686, count: 1, default_offset: 0, properties: &[] }, // minecraft:chiseled_tuff_bricks
    BlockStates { base: 24687, count: 1, default_offset: 0, properties: &[] }, // minecraft:sulfur
    BlockStates { base: 24688, count: 5, default_offset: 0, properties: &[Property { name: "potent_sulfur_state", values: &["dry", "wet", "dormant", "erupting", "continuous"] }] }, // minecraft:potent_sulfur
    BlockStates { base: 24693, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:sulfur_slab
    BlockStates { base: 24699, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:sulfur_stairs
    BlockStates { base: 24779, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:sulfur_wall
    BlockStates { base: 25103, count: 1, default_offset: 0, properties: &[] }, // minecraft:polished_sulfur
    BlockStates { base: 25104, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:polished_sulfur_slab
    BlockStates { base: 25110, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:polished_sulfur_stairs
    BlockStates { base: 25190, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:polished_sulfur_wall
    BlockStates { base: 25514, count: 1, default_offset: 0, properties: &[] }, // minecraft:sulfur_bricks
    BlockStates { base: 25515, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:sulfur_brick_slab
    BlockStates { base: 25521, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:sulfur_brick_stairs
    BlockStates { base: 25601, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:sulfur_brick_wall
    BlockStates { base: 25925, count: 1, default_offset: 0, properties: &[] }, // minecraft:chiseled_sulfur
    BlockStates { base: 25926, count: 1, default_offset: 0, properties: &[] }, // minecraft:cinnabar
    BlockStates { base: 25927, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cinnabar_slab
    BlockStates { base: 25933, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cinnabar_stairs
    BlockStates { base: 26013, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:cinnabar_wall
    BlockStates { base: 26337, count: 1, default_offset: 0, properties: &[] }, // minecraft:polished_cinnabar
    BlockStates { base: 26338, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:polished_cinnabar_slab
    BlockStates { base: 26344, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:polished_cinnabar_stairs
    BlockStates { base: 26424, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:polished_cinnabar_wall
    BlockStates { base: 26748, count: 1, default_offset: 0, properties: &[] }, // minecraft:cinnabar_bricks
    BlockStates { base: 26749, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cinnabar_brick_slab
    BlockStates { base: 26755, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cinnabar_brick_stairs
    BlockStates { base: 26835, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:cinnabar_brick_wall
    BlockStates { base: 27159, count: 1, default_offset: 0, properties: &[] }, // minecraft:chiseled_cinnabar
    BlockStates { base: 27160, count: 1, default_offset: 0, properties: &[] }, // minecraft:calcite
    BlockStates { base: 27161, count: 1, default_offset: 0, properties: &[] }, // minecraft:tinted_glass
    BlockStates { base: 27162, count: 1, default_offset: 0, properties: &[] }, // minecraft:powder_snow
    BlockStates { base: 27163, count: 96, default_offset: 1, properties: &[Property { name: "power", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "sculk_sensor_phase", values: &["inactive", "active", "cooldown"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:sculk_sensor
    BlockStates { base: 27259, count: 384, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "power", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"] }, Property { name: "sculk_sensor_phase", values: &["inactive", "active", "cooldown"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:calibrated_sculk_sensor
    BlockStates { base: 27643, count: 1, default_offset: 0, properties: &[] }, // minecraft:sculk
    BlockStates { base: 27644, count: 128, default_offset: 127, properties: &[Property { name: "down", values: &["true", "false"] }, Property { name: "east", values: &["true", "false"] }, Property { name: "north", values: &["true", "false"] }, Property { name: "south", values: &["true", "false"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["true", "false"] }] }, // minecraft:sculk_vein
    BlockStates { base: 27772, count: 2, default_offset: 1, properties: &[Property { name: "bloom", values: &["true", "false"] }] }, // minecraft:sculk_catalyst
    BlockStates { base: 27774, count: 8, default_offset: 7, properties: &[Property { name: "can_summon", values: &["true", "false"] }, Property { name: "shrieking", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:sculk_shrieker
    BlockStates { base: 27782, count: 1, default_offset: 0, properties: &[] }, // minecraft:copper_block
    BlockStates { base: 27783, count: 1, default_offset: 0, properties: &[] }, // minecraft:exposed_copper
    BlockStates { base: 27784, count: 1, default_offset: 0, properties: &[] }, // minecraft:weathered_copper
    BlockStates { base: 27785, count: 1, default_offset: 0, properties: &[] }, // minecraft:oxidized_copper
    BlockStates { base: 27786, count: 1, default_offset: 0, properties: &[] }, // minecraft:waxed_copper_block
    BlockStates { base: 27787, count: 1, default_offset: 0, properties: &[] }, // minecraft:waxed_exposed_copper
    BlockStates { base: 27788, count: 1, default_offset: 0, properties: &[] }, // minecraft:waxed_weathered_copper
    BlockStates { base: 27789, count: 1, default_offset: 0, properties: &[] }, // minecraft:waxed_oxidized_copper
    BlockStates { base: 27790, count: 1, default_offset: 0, properties: &[] }, // minecraft:copper_ore
    BlockStates { base: 27791, count: 1, default_offset: 0, properties: &[] }, // minecraft:deepslate_copper_ore
    BlockStates { base: 27792, count: 1, default_offset: 0, properties: &[] }, // minecraft:cut_copper
    BlockStates { base: 27793, count: 1, default_offset: 0, properties: &[] }, // minecraft:exposed_cut_copper
    BlockStates { base: 27794, count: 1, default_offset: 0, properties: &[] }, // minecraft:weathered_cut_copper
    BlockStates { base: 27795, count: 1, default_offset: 0, properties: &[] }, // minecraft:oxidized_cut_copper
    BlockStates { base: 27796, count: 1, default_offset: 0, properties: &[] }, // minecraft:waxed_cut_copper
    BlockStates { base: 27797, count: 1, default_offset: 0, properties: &[] }, // minecraft:waxed_exposed_cut_copper
    BlockStates { base: 27798, count: 1, default_offset: 0, properties: &[] }, // minecraft:waxed_weathered_cut_copper
    BlockStates { base: 27799, count: 1, default_offset: 0, properties: &[] }, // minecraft:waxed_oxidized_cut_copper
    BlockStates { base: 27800, count: 1, default_offset: 0, properties: &[] }, // minecraft:chiseled_copper
    BlockStates { base: 27801, count: 1, default_offset: 0, properties: &[] }, // minecraft:exposed_chiseled_copper
    BlockStates { base: 27802, count: 1, default_offset: 0, properties: &[] }, // minecraft:weathered_chiseled_copper
    BlockStates { base: 27803, count: 1, default_offset: 0, properties: &[] }, // minecraft:oxidized_chiseled_copper
    BlockStates { base: 27804, count: 1, default_offset: 0, properties: &[] }, // minecraft:waxed_chiseled_copper
    BlockStates { base: 27805, count: 1, default_offset: 0, properties: &[] }, // minecraft:waxed_exposed_chiseled_copper
    BlockStates { base: 27806, count: 1, default_offset: 0, properties: &[] }, // minecraft:waxed_weathered_chiseled_copper
    BlockStates { base: 27807, count: 1, default_offset: 0, properties: &[] }, // minecraft:waxed_oxidized_chiseled_copper
    BlockStates { base: 27808, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cut_copper_stairs
    BlockStates { base: 27888, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:exposed_cut_copper_stairs
    BlockStates { base: 27968, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:weathered_cut_copper_stairs
    BlockStates { base: 28048, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:oxidized_cut_copper_stairs
    BlockStates { base: 28128, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_cut_copper_stairs
    BlockStates { base: 28208, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_exposed_cut_copper_stairs
    BlockStates { base: 28288, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_weathered_cut_copper_stairs
    BlockStates { base: 28368, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_oxidized_cut_copper_stairs
    BlockStates { base: 28448, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cut_copper_slab
    BlockStates { base: 28454, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:exposed_cut_copper_slab
    BlockStates { base: 28460, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:weathered_cut_copper_slab
    BlockStates { base: 28466, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:oxidized_cut_copper_slab
    BlockStates { base: 28472, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_cut_copper_slab
    BlockStates { base: 28478, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_exposed_cut_copper_slab
    BlockStates { base: 28484, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_weathered_cut_copper_slab
    BlockStates { base: 28490, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_oxidized_cut_copper_slab
    BlockStates { base: 28496, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:copper_door
    BlockStates { base: 28560, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:exposed_copper_door
    BlockStates { base: 28624, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:weathered_copper_door
    BlockStates { base: 28688, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:oxidized_copper_door
    BlockStates { base: 28752, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:waxed_copper_door
    BlockStates { base: 28816, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:waxed_exposed_copper_door
    BlockStates { base: 28880, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:waxed_weathered_copper_door
    BlockStates { base: 28944, count: 64, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "hinge", values: &["left", "right"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:waxed_oxidized_copper_door
    BlockStates { base: 29008, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:copper_trapdoor
    BlockStates { base: 29072, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:exposed_copper_trapdoor
    BlockStates { base: 29136, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:weathered_copper_trapdoor
    BlockStates { base: 29200, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:oxidized_copper_trapdoor
    BlockStates { base: 29264, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_copper_trapdoor
    BlockStates { base: 29328, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_exposed_copper_trapdoor
    BlockStates { base: 29392, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_weathered_copper_trapdoor
    BlockStates { base: 29456, count: 64, default_offset: 15, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "open", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_oxidized_copper_trapdoor
    BlockStates { base: 29520, count: 2, default_offset: 1, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:copper_grate
    BlockStates { base: 29522, count: 2, default_offset: 1, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:exposed_copper_grate
    BlockStates { base: 29524, count: 2, default_offset: 1, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:weathered_copper_grate
    BlockStates { base: 29526, count: 2, default_offset: 1, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:oxidized_copper_grate
    BlockStates { base: 29528, count: 2, default_offset: 1, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_copper_grate
    BlockStates { base: 29530, count: 2, default_offset: 1, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_exposed_copper_grate
    BlockStates { base: 29532, count: 2, default_offset: 1, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_weathered_copper_grate
    BlockStates { base: 29534, count: 2, default_offset: 1, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_oxidized_copper_grate
    BlockStates { base: 29536, count: 4, default_offset: 3, properties: &[Property { name: "lit", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:copper_bulb
    BlockStates { base: 29540, count: 4, default_offset: 3, properties: &[Property { name: "lit", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:exposed_copper_bulb
    BlockStates { base: 29544, count: 4, default_offset: 3, properties: &[Property { name: "lit", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:weathered_copper_bulb
    BlockStates { base: 29548, count: 4, default_offset: 3, properties: &[Property { name: "lit", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:oxidized_copper_bulb
    BlockStates { base: 29552, count: 4, default_offset: 3, properties: &[Property { name: "lit", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:waxed_copper_bulb
    BlockStates { base: 29556, count: 4, default_offset: 3, properties: &[Property { name: "lit", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:waxed_exposed_copper_bulb
    BlockStates { base: 29560, count: 4, default_offset: 3, properties: &[Property { name: "lit", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:waxed_weathered_copper_bulb
    BlockStates { base: 29564, count: 4, default_offset: 3, properties: &[Property { name: "lit", values: &["true", "false"] }, Property { name: "powered", values: &["true", "false"] }] }, // minecraft:waxed_oxidized_copper_bulb
    BlockStates { base: 29568, count: 24, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "type", values: &["single", "left", "right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:copper_chest
    BlockStates { base: 29592, count: 24, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "type", values: &["single", "left", "right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:exposed_copper_chest
    BlockStates { base: 29616, count: 24, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "type", values: &["single", "left", "right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:weathered_copper_chest
    BlockStates { base: 29640, count: 24, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "type", values: &["single", "left", "right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:oxidized_copper_chest
    BlockStates { base: 29664, count: 24, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "type", values: &["single", "left", "right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_copper_chest
    BlockStates { base: 29688, count: 24, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "type", values: &["single", "left", "right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_exposed_copper_chest
    BlockStates { base: 29712, count: 24, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "type", values: &["single", "left", "right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_weathered_copper_chest
    BlockStates { base: 29736, count: 24, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "type", values: &["single", "left", "right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_oxidized_copper_chest
    BlockStates { base: 29760, count: 32, default_offset: 1, properties: &[Property { name: "copper_golem_pose", values: &["standing", "sitting", "running", "star"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:copper_golem_statue
    BlockStates { base: 29792, count: 32, default_offset: 1, properties: &[Property { name: "copper_golem_pose", values: &["standing", "sitting", "running", "star"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:exposed_copper_golem_statue
    BlockStates { base: 29824, count: 32, default_offset: 1, properties: &[Property { name: "copper_golem_pose", values: &["standing", "sitting", "running", "star"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:weathered_copper_golem_statue
    BlockStates { base: 29856, count: 32, default_offset: 1, properties: &[Property { name: "copper_golem_pose", values: &["standing", "sitting", "running", "star"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:oxidized_copper_golem_statue
    BlockStates { base: 29888, count: 32, default_offset: 1, properties: &[Property { name: "copper_golem_pose", values: &["standing", "sitting", "running", "star"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_copper_golem_statue
    BlockStates { base: 29920, count: 32, default_offset: 1, properties: &[Property { name: "copper_golem_pose", values: &["standing", "sitting", "running", "star"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_exposed_copper_golem_statue
    BlockStates { base: 29952, count: 32, default_offset: 1, properties: &[Property { name: "copper_golem_pose", values: &["standing", "sitting", "running", "star"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_weathered_copper_golem_statue
    BlockStates { base: 29984, count: 32, default_offset: 1, properties: &[Property { name: "copper_golem_pose", values: &["standing", "sitting", "running", "star"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_oxidized_copper_golem_statue
    BlockStates { base: 30016, count: 24, default_offset: 19, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:lightning_rod
    BlockStates { base: 30040, count: 24, default_offset: 19, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:exposed_lightning_rod
    BlockStates { base: 30064, count: 24, default_offset: 19, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:weathered_lightning_rod
    BlockStates { base: 30088, count: 24, default_offset: 19, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:oxidized_lightning_rod
    BlockStates { base: 30112, count: 24, default_offset: 19, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_lightning_rod
    BlockStates { base: 30136, count: 24, default_offset: 19, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_exposed_lightning_rod
    BlockStates { base: 30160, count: 24, default_offset: 19, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_weathered_lightning_rod
    BlockStates { base: 30184, count: 24, default_offset: 19, properties: &[Property { name: "facing", values: &["north", "east", "south", "west", "up", "down"] }, Property { name: "powered", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:waxed_oxidized_lightning_rod
    BlockStates { base: 30208, count: 1, default_offset: 0, properties: &[] }, // minecraft:dripstone_block
    BlockStates { base: 30209, count: 20, default_offset: 5, properties: &[Property { name: "thickness", values: &["tip_merge", "tip", "frustum", "middle", "base"] }, Property { name: "vertical_direction", values: &["up", "down"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:pointed_dripstone
    BlockStates { base: 30229, count: 20, default_offset: 5, properties: &[Property { name: "thickness", values: &["tip_merge", "tip", "frustum", "middle", "base"] }, Property { name: "vertical_direction", values: &["up", "down"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:sulfur_spike
    BlockStates { base: 30249, count: 52, default_offset: 1, properties: &[Property { name: "age", values: &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16", "17", "18", "19", "20", "21", "22", "23", "24", "25"] }, Property { name: "berries", values: &["true", "false"] }] }, // minecraft:cave_vines
    BlockStates { base: 30301, count: 2, default_offset: 1, properties: &[Property { name: "berries", values: &["true", "false"] }] }, // minecraft:cave_vines_plant
    BlockStates { base: 30303, count: 1, default_offset: 0, properties: &[] }, // minecraft:spore_blossom
    BlockStates { base: 30304, count: 1, default_offset: 0, properties: &[] }, // minecraft:azalea
    BlockStates { base: 30305, count: 1, default_offset: 0, properties: &[] }, // minecraft:flowering_azalea
    BlockStates { base: 30306, count: 1, default_offset: 0, properties: &[] }, // minecraft:moss_carpet
    BlockStates { base: 30307, count: 16, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "flower_amount", values: &["1", "2", "3", "4"] }] }, // minecraft:pink_petals
    BlockStates { base: 30323, count: 16, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "flower_amount", values: &["1", "2", "3", "4"] }] }, // minecraft:wildflowers
    BlockStates { base: 30339, count: 16, default_offset: 0, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "segment_amount", values: &["1", "2", "3", "4"] }] }, // minecraft:leaf_litter
    BlockStates { base: 30355, count: 1, default_offset: 0, properties: &[] }, // minecraft:moss_block
    BlockStates { base: 30356, count: 32, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "tilt", values: &["none", "unstable", "partial", "full"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:big_dripleaf
    BlockStates { base: 30388, count: 8, default_offset: 1, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:big_dripleaf_stem
    BlockStates { base: 30396, count: 16, default_offset: 3, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["upper", "lower"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:small_dripleaf
    BlockStates { base: 30412, count: 2, default_offset: 1, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:hanging_roots
    BlockStates { base: 30414, count: 1, default_offset: 0, properties: &[] }, // minecraft:rooted_dirt
    BlockStates { base: 30415, count: 1, default_offset: 0, properties: &[] }, // minecraft:mud
    BlockStates { base: 30416, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:deepslate
    BlockStates { base: 30419, count: 1, default_offset: 0, properties: &[] }, // minecraft:cobbled_deepslate
    BlockStates { base: 30420, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cobbled_deepslate_stairs
    BlockStates { base: 30500, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:cobbled_deepslate_slab
    BlockStates { base: 30506, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:cobbled_deepslate_wall
    BlockStates { base: 30830, count: 1, default_offset: 0, properties: &[] }, // minecraft:polished_deepslate
    BlockStates { base: 30831, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:polished_deepslate_stairs
    BlockStates { base: 30911, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:polished_deepslate_slab
    BlockStates { base: 30917, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:polished_deepslate_wall
    BlockStates { base: 31241, count: 1, default_offset: 0, properties: &[] }, // minecraft:deepslate_tiles
    BlockStates { base: 31242, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:deepslate_tile_stairs
    BlockStates { base: 31322, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:deepslate_tile_slab
    BlockStates { base: 31328, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:deepslate_tile_wall
    BlockStates { base: 31652, count: 1, default_offset: 0, properties: &[] }, // minecraft:deepslate_bricks
    BlockStates { base: 31653, count: 80, default_offset: 11, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "half", values: &["top", "bottom"] }, Property { name: "shape", values: &["straight", "inner_left", "inner_right", "outer_left", "outer_right"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:deepslate_brick_stairs
    BlockStates { base: 31733, count: 6, default_offset: 3, properties: &[Property { name: "type", values: &["top", "bottom", "double"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:deepslate_brick_slab
    BlockStates { base: 31739, count: 324, default_offset: 3, properties: &[Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "up", values: &["true", "false"] }, Property { name: "waterlogged", values: &["true", "false"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:deepslate_brick_wall
    BlockStates { base: 32063, count: 1, default_offset: 0, properties: &[] }, // minecraft:chiseled_deepslate
    BlockStates { base: 32064, count: 1, default_offset: 0, properties: &[] }, // minecraft:cracked_deepslate_bricks
    BlockStates { base: 32065, count: 1, default_offset: 0, properties: &[] }, // minecraft:cracked_deepslate_tiles
    BlockStates { base: 32066, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:infested_deepslate
    BlockStates { base: 32069, count: 1, default_offset: 0, properties: &[] }, // minecraft:smooth_basalt
    BlockStates { base: 32070, count: 1, default_offset: 0, properties: &[] }, // minecraft:raw_iron_block
    BlockStates { base: 32071, count: 1, default_offset: 0, properties: &[] }, // minecraft:raw_copper_block
    BlockStates { base: 32072, count: 1, default_offset: 0, properties: &[] }, // minecraft:raw_gold_block
    BlockStates { base: 32073, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_azalea_bush
    BlockStates { base: 32074, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_flowering_azalea_bush
    BlockStates { base: 32075, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:ochre_froglight
    BlockStates { base: 32078, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:verdant_froglight
    BlockStates { base: 32081, count: 3, default_offset: 1, properties: &[Property { name: "axis", values: &["x", "y", "z"] }] }, // minecraft:pearlescent_froglight
    BlockStates { base: 32084, count: 1, default_offset: 0, properties: &[] }, // minecraft:frogspawn
    BlockStates { base: 32085, count: 1, default_offset: 0, properties: &[] }, // minecraft:reinforced_deepslate
    BlockStates { base: 32086, count: 16, default_offset: 9, properties: &[Property { name: "cracked", values: &["true", "false"] }, Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:decorated_pot
    BlockStates { base: 32102, count: 48, default_offset: 45, properties: &[Property { name: "crafting", values: &["true", "false"] }, Property { name: "orientation", values: &["down_east", "down_north", "down_south", "down_west", "up_east", "up_north", "up_south", "up_west", "west_up", "east_up", "north_up", "south_up"] }, Property { name: "triggered", values: &["true", "false"] }] }, // minecraft:crafter
    BlockStates { base: 32150, count: 12, default_offset: 6, properties: &[Property { name: "ominous", values: &["true", "false"] }, Property { name: "trial_spawner_state", values: &["inactive", "waiting_for_players", "active", "waiting_for_reward_ejection", "ejecting_reward", "cooldown"] }] }, // minecraft:trial_spawner
    BlockStates { base: 32162, count: 32, default_offset: 4, properties: &[Property { name: "facing", values: &["north", "south", "west", "east"] }, Property { name: "ominous", values: &["true", "false"] }, Property { name: "vault_state", values: &["inactive", "active", "unlocking", "ejecting"] }] }, // minecraft:vault
    BlockStates { base: 32194, count: 2, default_offset: 1, properties: &[Property { name: "waterlogged", values: &["true", "false"] }] }, // minecraft:heavy_core
    BlockStates { base: 32196, count: 1, default_offset: 0, properties: &[] }, // minecraft:pale_moss_block
    BlockStates { base: 32197, count: 162, default_offset: 0, properties: &[Property { name: "bottom", values: &["true", "false"] }, Property { name: "east", values: &["none", "low", "tall"] }, Property { name: "north", values: &["none", "low", "tall"] }, Property { name: "south", values: &["none", "low", "tall"] }, Property { name: "west", values: &["none", "low", "tall"] }] }, // minecraft:pale_moss_carpet
    BlockStates { base: 32359, count: 2, default_offset: 0, properties: &[Property { name: "tip", values: &["true", "false"] }] }, // minecraft:pale_hanging_moss
    BlockStates { base: 32361, count: 1, default_offset: 0, properties: &[] }, // minecraft:open_eyeblossom
    BlockStates { base: 32362, count: 1, default_offset: 0, properties: &[] }, // minecraft:closed_eyeblossom
    BlockStates { base: 32363, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_open_eyeblossom
    BlockStates { base: 32364, count: 1, default_offset: 0, properties: &[] }, // minecraft:potted_closed_eyeblossom
    BlockStates { base: 32365, count: 1, default_offset: 0, properties: &[] }, // minecraft:firefly_bush
];

/// Total number of global block-states; ids run `0 .. TOTAL_STATES`.
pub const TOTAL_STATES: u32 = 32366;

/// The default block-state id for a block id (see [`super::builtin::BLOCK`]).
#[allow(dead_code)]
pub fn default_state(block_id: i32) -> Option<u32> {
    let b = STATES.get(usize::try_from(block_id).ok()?)?;
    Some(b.base + b.default_offset as u32)
}

/// The default block-state id for a block by `namespace:path`.
#[allow(dead_code)]
pub fn default_state_of(name: &str) -> Option<u32> {
    super::builtin::BLOCK.id_of(name).and_then(default_state)
}

/// The block id owning a given global state id (via the state runs).
#[allow(dead_code)]
pub fn block_of_state(state_id: u32) -> Option<i32> {
    if state_id >= TOTAL_STATES {
        return None;
    }
    // runs are contiguous and ascending by base; find the last base <= state_id.
    let idx = STATES.partition_point(|b| b.base <= state_id) - 1;
    Some(idx as i32)
}

/// Describe a global state id as `(block name, [(property, value), …])` — the
/// exact shape the Anvil chunk NBT palette stores each entry as (`{Name,
/// Properties}` in `SerializableChunkData`). Properties are listed in the block's
/// definition order, each carrying the value this state selects. Returns `None`
/// for an out-of-range id.
pub fn describe_state(state_id: u32) -> Option<(&'static str, Vec<(&'static str, &'static str)>)> {
    let block = block_of_state(state_id)? as usize;
    let name = super::builtin::BLOCK.name_of(block as i32)?;
    let b = &STATES[block];
    let mut offset = state_id - b.base;
    let mut stride: u32 = b.count as u32;
    let mut props = Vec::with_capacity(b.properties.len());
    for p in b.properties {
        stride /= p.values.len() as u32;
        let index = (offset / stride) as usize % p.values.len();
        props.push((p.name, p.values[index]));
        offset %= stride;
    }
    Some((name, props))
}

/// Decode one property's value for a state id, e.g. `facing` of a stair state.
#[allow(dead_code)]
pub fn property_value(state_id: u32, property: &str) -> Option<&'static str> {
    let block = block_of_state(state_id)? as usize;
    let b = &STATES[block];
    let mut offset = state_id - b.base;
    // stride of a property = product of the sizes of properties after it.
    let mut stride: u32 = b.count as u32;
    for p in b.properties {
        stride /= p.values.len() as u32;
        let index = (offset / stride) as usize % p.values.len();
        if p.name == property {
            return Some(p.values[index]);
        }
        offset %= stride;
    }
    None
}

/// The state id of `name` with the given property overrides applied to its
/// default state. Unlisted properties keep their default; an unknown block,
/// property, or value yields `None`.
#[allow(dead_code)]
pub fn with_properties(name: &str, overrides: &[(&str, &str)]) -> Option<u32> {
    let block = super::builtin::BLOCK.id_of(name)? as usize;
    let b = STATES.get(block)?;
    let mut offset = b.default_offset as u32;
    for &(prop, val) in overrides {
        let mut stride: u32 = b.count as u32;
        let mut matched = false;
        for p in b.properties {
            stride /= p.values.len() as u32;
            if p.name == prop {
                let new = p.values.iter().position(|v| *v == val)? as u32;
                let cur = (offset / stride) % p.values.len() as u32;
                offset = offset - cur * stride + new * stride;
                matched = true;
                break;
            }
        }
        if !matched {
            return None;
        }
    }
    Some(b.base + offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::builtin::BLOCK;

    #[test]
    fn defaults_and_lookups() {
        assert_eq!(default_state_of("minecraft:air"), Some(0));
        assert_eq!(default_state_of("minecraft:stone"), Some(1));
        // grass_block default is snowy=false, its second state.
        assert_eq!(default_state_of("minecraft:grass_block"), Some(9));
        assert_eq!(property_value(8, "snowy"), Some("true"));
        assert_eq!(property_value(9, "snowy"), Some("false"));
        assert_eq!(with_properties("minecraft:grass_block", &[("snowy", "true")]), Some(8));
        assert_eq!(block_of_state(9), BLOCK.id_of("minecraft:grass_block"));
        assert_eq!(default_state_of("minecraft:not_a_block"), None);
        assert_eq!(block_of_state(TOTAL_STATES), None);
    }

    #[test]
    fn describe_state_round_trips_through_with_properties() {
        // A propertyless block: name only, no properties.
        let (name, props) = describe_state(1).unwrap();
        assert_eq!(name, "minecraft:stone");
        assert!(props.is_empty());

        // grass_block default (snowy=false) describes with its one property, and
        // feeding that description back to with_properties recovers the id.
        let (name, props) = describe_state(9).unwrap();
        assert_eq!(name, "minecraft:grass_block");
        assert_eq!(props, vec![("snowy", "false")]);
        assert_eq!(with_properties(name, &props), Some(9));

        // A multi-property block round-trips every state exactly.
        let door = with_properties("minecraft:oak_door", &[("facing", "west"), ("open", "true")]).unwrap();
        let (name, props) = describe_state(door).unwrap();
        assert_eq!(with_properties(name, &props), Some(door));

        assert_eq!(describe_state(TOTAL_STATES), None);
    }

    #[test]
    fn whole_palette_round_trips() {
        // every state id maps to a block whose run contains it, and the table
        // covers exactly TOTAL_STATES ids with no gaps or overlaps.
        assert_eq!(STATES.len(), BLOCK.len());
        let mut expect = 0u32;
        for (id, b) in STATES.iter().enumerate() {
            assert_eq!(b.base, expect, "gap before block {id}");
            let prod: u32 = b.properties.iter().map(|p| p.values.len() as u32).product::<u32>().max(1);
            assert_eq!(prod, b.count as u32, "count mismatch for block {id}");
            assert!((b.default_offset as u32) < b.count as u32);
            expect += b.count as u32;
        }
        assert_eq!(expect, TOTAL_STATES);
        // spot-check decode/encode inverse on a strided property block.
        let door = "minecraft:oak_door";
        let s = with_properties(door, &[("facing", "south"), ("half", "upper"), ("open", "true")]).unwrap();
        assert_eq!(property_value(s, "facing"), Some("south"));
        assert_eq!(property_value(s, "half"), Some("upper"));
        assert_eq!(property_value(s, "open"), Some("true"));
        assert_eq!(block_of_state(s), BLOCK.id_of(door));
    }
}
