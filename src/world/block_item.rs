//! Item → default block-state mapping for block placement.
//!
//! `ServerboundUseItemOnPacket` tells us which item the player used; to place a
//! block we need the *block-state* id that item produces. Item ids and
//! block-state ids are different numbering schemes (`BuiltInRegistries.ITEM`
//! indices vs `Block.BLOCK_STATE_REGISTRY` ids), so this is an explicit table.
//!
//! **Id source.** Block-state ids are the global palette ids the client decodes
//! via `Block.BLOCK_STATE_REGISTRY.byId`, i.e. the cumulative state index in the
//! server's block registration order (`Blocks.java` / a `--reports` block dump).
//! These are *observable* output, not copied source. The low stone-family and
//! dirt ids are pinned by the same deduction `crate::world` uses for terrain
//! (AIR 0, STONE 1, …, GRASS_BLOCK 9, DIRT 10), and BEDROCK 85 matches the
//! terrain generator. `cobblestone` (14) and `oak_planks` (15) sit before the
//! wood-type expansions, so they are stable too.
//!
//! Items absent from this table are treated as non-placeable (`None`) — the
//! server simply acknowledges the interaction without changing the world rather
//! than risk emitting an invalid block-state id. Extend the table as more
//! block-state ids are verified for 26.2.

/// The default block-state id placed by `item_id`, or `None` if the item is not
/// a block we can place yet. Item ids match `crate::inventory`'s `ITEMS` table.
pub fn block_state_for_item(item_id: i32) -> Option<u32> {
    let state = match item_id {
        1 => 1,   // stone        → stone
        2 => 2,   // granite      → granite
        4 => 4,   // diorite      → diorite
        6 => 6,   // andesite     → andesite
        54 => 9,  // grass_block  → grass_block[snowy=false] (default state)
        55 => 10, // dirt         → dirt
        62 => 14, // cobblestone  → cobblestone
        63 => 15, // oak_planks   → oak_planks
        85 => 85, // bedrock      → bedrock
        _ => return None,
    };
    Some(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_items_map_to_expected_states() {
        assert_eq!(block_state_for_item(1), Some(1)); // stone
        assert_eq!(block_state_for_item(54), Some(9)); // grass_block
        assert_eq!(block_state_for_item(55), Some(10)); // dirt
        assert_eq!(block_state_for_item(85), Some(85)); // bedrock
    }

    #[test]
    fn air_and_non_blocks_are_not_placeable() {
        assert_eq!(block_state_for_item(0), None); // air
        assert_eq!(block_state_for_item(724), None); // diamond_sword
        assert_eq!(block_state_for_item(686), None); // diamond
    }
}
