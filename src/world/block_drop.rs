//! Block-state → dropped-item mapping (the reverse of [`super::block_item`]).
//!
//! When a block is destroyed, vanilla resolves its drops through a *data-driven
//! loot table* (`Block.getDrops` → `state.getDrops(LootParams)` →
//! `LootTable.getRandomItems`), where the table JSON lives in the data pack, not
//! in `.java` source. `Block.playerDestroy` → `Block.dropResources` →
//! `Block.popResource` then spawns an `ItemEntity` per resulting stack. We can't
//! read the loot-table JSON here, so this is a hand-written Rust table that
//! reproduces the *observed* vanilla drops for exactly the block-states Vela can
//! currently have in the world (the terrain generator's states plus everything
//! `block_state_for_item` can place).
//!
//! **Simplification (documented on purpose).** These are the empty-hand,
//! no-enchant drops only. Silk Touch, Fortune and tool-requirement gating (e.g.
//! stone dropping nothing when mined by hand in strict "correct tool" servers)
//! are NOT modelled — `Block.getDrops` takes the tool/enchantments via
//! `LootContextParams.TOOL`, which we ignore. This matches vanilla's default
//! `block_drops` game-rule-on, hand-mined behavior for these blocks and is enough
//! for the current gameplay slice. Extend the table (and revisit tool gating)
//! as more block-states become reachable.
//!
//! Reference: `net.minecraft.world.level.block.Block#getDrops` / `#dropResources`
//! / `#playerDestroy` (MC 26.2), and the vanilla loot tables for each block
//! (`data/minecraft/loot_table/blocks/*.json`).

use crate::ids::BlockState;
use crate::inventory::ItemStack;

// Item registry ids (`BuiltInRegistries.ITEM`, see `registry::item`) used as drop
// results. Kept as named constants so the mapping reads as block → item.
const ITEM_GRANITE: i32 = 2;
const ITEM_DIORITE: i32 = 4;
const ITEM_ANDESITE: i32 = 6;
const ITEM_DIRT: i32 = 55;
const ITEM_COBBLESTONE: i32 = 62;
const ITEM_OAK_PLANKS: i32 = 63;

/// The item stacks a block-state drops when destroyed by a bare hand in survival.
///
/// Returns an empty vector for states that drop nothing (e.g. bedrock, which is
/// unbreakable in survival and has an empty loot table) and for any state not yet
/// in the table. Mirrors `Block.getDrops`, whose result `dropResources` feeds one
/// stack at a time into `popResource`.
pub fn drops_for(state: BlockState) -> Vec<ItemStack> {
    // Block-state ids are the global palette ids from the server's block
    // registration order (see `world::states` / `block_item`). The self-dropping
    // blocks still list an explicit item id because block-state ids and item ids
    // are different numbering schemes.
    let item = match state.get() {
        1 => ITEM_COBBLESTONE,  // stone       → cobblestone (no Silk Touch)
        2 => ITEM_GRANITE,      // granite     → itself
        4 => ITEM_DIORITE,      // diorite     → itself
        6 => ITEM_ANDESITE,     // andesite    → itself
        9 => ITEM_DIRT,         // grass_block → dirt (no Silk Touch)
        10 => ITEM_DIRT,        // dirt        → itself
        14 => ITEM_COBBLESTONE, // cobblestone → itself
        15 => ITEM_OAK_PLANKS,  // oak_planks  → itself
        // bedrock (85) and everything else: no drops.
        _ => return Vec::new(),
    };
    vec![ItemStack::new(item, 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(state: u32) -> ItemStack {
        let d = drops_for(BlockState(state));
        assert_eq!(d.len(), 1, "state {state} should drop exactly one stack");
        d[0]
    }

    #[test]
    fn self_dropping_blocks() {
        assert_eq!(only(2).id.get(), ITEM_GRANITE);
        assert_eq!(only(4).id.get(), ITEM_DIORITE);
        assert_eq!(only(6).id.get(), ITEM_ANDESITE);
        assert_eq!(only(10).id.get(), ITEM_DIRT);
        assert_eq!(only(14).id.get(), ITEM_COBBLESTONE);
        assert_eq!(only(15).id.get(), ITEM_OAK_PLANKS);
    }

    #[test]
    fn special_case_drops() {
        // stone → cobblestone, grass_block → dirt (empty-hand, no Silk Touch).
        assert_eq!(only(1).id.get(), ITEM_COBBLESTONE);
        assert_eq!(only(9).id.get(), ITEM_DIRT);
    }

    #[test]
    fn every_drop_is_a_single_item() {
        for s in [1u32, 2, 4, 6, 9, 10, 14, 15] {
            assert_eq!(only(s).count, 1);
        }
    }

    #[test]
    fn no_drops_for_bedrock_air_and_unknown() {
        assert!(drops_for(BlockState(0)).is_empty()); // air
        assert!(drops_for(BlockState(85)).is_empty()); // bedrock (unbreakable)
        assert!(drops_for(BlockState(9999)).is_empty()); // unmapped
    }
}
