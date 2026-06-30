//! The player-inventory container: the 46 slots and the selected hotbar index.

use bevy_ecs::prelude::*;

use super::item_stack::ItemStack;

/// Number of slots in the player inventory container (window id 0), matching
/// vanilla's `InventoryMenu`:
///
/// * `0` — crafting result
/// * `1..=4` — 2×2 crafting grid
/// * `5..=8` — armor (head, chest, legs, feet)
/// * `9..=35` — main inventory (27)
/// * `36..=44` — hotbar (9)
/// * `45` — offhand
///
/// `ServerboundSetCreativeModeSlotPacket` and `ClientboundContainerSetContentPacket`
/// both index these 46 slots directly.
pub const PLAYER_INVENTORY_SLOTS: usize = 46;

/// First hotbar container slot; `selected` (0..9) maps to `HOTBAR_START + selected`.
#[allow(dead_code)] // scaffolding: used by callers translating selected → container slot.
pub const HOTBAR_START: usize = 36;

/// The player's inventory: the 46 container slots plus the selected hotbar slot.
/// Declared as a `bevy_ecs` `Component` here so the rest of the sim never has to
/// know its shape — it is attached to the player entity lazily.
#[derive(Component)]
pub struct Inventory {
    /// All 46 container slots; `None` is an empty slot.
    pub slots: [Option<ItemStack>; PLAYER_INVENTORY_SLOTS],
    /// Selected hotbar index, `0..=8` (vanilla `Inventory.selected`).
    pub selected: u8,
    /// The cursor / carried item — what the player is holding on the pointer
    /// while a menu is open (`AbstractContainerMenu.carried`). Persists across
    /// clicks until the menu closes.
    pub carried: Option<ItemStack>,
    /// Menu state id (`AbstractContainerMenu.stateId`), bumped each sync. The
    /// client echoes it on click; a mismatch triggers a full resync.
    pub state_id: i32,
    /// Quick-craft (item-drag) state carried between the `START`/`CONTINUE`/`END`
    /// `ContainerClick` packets that make up one drag gesture
    /// (`AbstractContainerMenu.quickcraft*`). `status` is the drag phase, `kind`
    /// the drag type (charitable/greedy/clone), `slots` the menu-slot indices the
    /// drag has touched.
    pub drag_status: i32,
    pub drag_type: i32,
    pub drag_slots: Vec<usize>,
}

impl Inventory {
    /// A fresh, empty inventory with the first hotbar slot selected.
    pub fn new() -> Self {
        Self {
            slots: [None; PLAYER_INVENTORY_SLOTS],
            selected: 0,
            carried: None,
            state_id: 0,
            drag_status: 0,
            drag_type: -1,
            drag_slots: Vec::new(),
        }
    }

    /// Advance and return the menu state id (`incrementStateId`: 15-bit wrap).
    pub fn next_state_id(&mut self) -> i32 {
        self.state_id = (self.state_id + 1) & 32767;
        self.state_id
    }

    /// Write `stack` into container `slot`, ignoring out-of-range indices (a
    /// hostile/buggy client could send any short). Returns whether it landed.
    pub fn set_slot(&mut self, slot: i16, stack: Option<ItemStack>) -> bool {
        if (0..PLAYER_INVENTORY_SLOTS as i16).contains(&slot) {
            self.slots[slot as usize] = stack;
            true
        } else {
            false
        }
    }
}

impl Default for Inventory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_set_slot_bounds() {
        let mut inv = Inventory::new();
        assert!(inv.set_slot(36, Some(ItemStack::new(1, 1))));
        assert_eq!(inv.slots[36], Some(ItemStack::new(1, 1)));
        assert!(!inv.set_slot(46, Some(ItemStack::new(1, 1)))); // out of range
        assert!(!inv.set_slot(-1, None));
    }
}
