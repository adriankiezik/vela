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

    /// The container slots that `Inventory.add` may store into, in vanilla's
    /// `Inventory.items` scan order. Vanilla's backing list is 36 entries —
    /// indices `0..=8` the hotbar, `9..=35` the main inventory — so a plain
    /// `for i in 0..36` visits the hotbar first, then the main grid. This maps
    /// that order onto Vela's 46-slot `InventoryMenu` layout, where the hotbar is
    /// container slots `36..=44` and the main inventory is `9..=35`.
    fn storage_slots() -> impl Iterator<Item = usize> {
        (HOTBAR_START..HOTBAR_START + 9).chain(9..HOTBAR_START)
    }

    /// The offhand container slot (vanilla `Inventory.SLOT_OFFHAND` = index 40,
    /// which `getSlotWithRemainingSpace` probes second). In Vela's `InventoryMenu`
    /// mapping that is container slot 45.
    const OFFHAND_SLOT: usize = 45;

    /// The selected hotbar container slot (`HOTBAR_START + selected`), which
    /// `getSlotWithRemainingSpace` probes first.
    fn selected_slot(&self) -> usize {
        HOTBAR_START + self.selected as usize
    }

    /// `Inventory.hasRemainingSpaceForItem`: `slot` holds a stackable stack of the
    /// same item with room to grow.
    fn has_remaining_space(&self, slot: usize, incoming: &ItemStack) -> bool {
        match &self.slots[slot] {
            Some(s) => {
                ItemStack::same_item_same_components(s, incoming)
                    && s.is_stackable()
                    && s.count < s.max_stack_size()
            }
            None => false,
        }
    }

    /// `Inventory.getSlotWithRemainingSpace`: the selected slot, then the offhand,
    /// then the first storage slot with room for `incoming`.
    fn slot_with_remaining_space(&self, incoming: &ItemStack) -> Option<usize> {
        if self.has_remaining_space(self.selected_slot(), incoming) {
            return Some(self.selected_slot());
        }
        if self.has_remaining_space(Self::OFFHAND_SLOT, incoming) {
            return Some(Self::OFFHAND_SLOT);
        }
        Self::storage_slots().find(|&s| self.has_remaining_space(s, incoming))
    }

    /// `Inventory.getFreeSlot`: the first empty storage slot.
    fn first_free_slot(&self) -> Option<usize> {
        Self::storage_slots().find(|&s| self.slots[s].is_none())
    }

    /// `Inventory.addResource(slot, stack)`: drop as much of `incoming` into
    /// `slot` as fits, returning the leftover count that did not fit.
    fn add_resource_to(&mut self, slot: usize, incoming: &ItemStack) -> i32 {
        let mut count = incoming.count;
        let cur = self.slots[slot].map_or(0, |s| s.count);
        let max_to_add = incoming.max_stack_size() - cur;
        let to_add = count.min(max_to_add);
        if to_add <= 0 {
            return count;
        }
        count -= to_add;
        self.slots[slot] = Some(ItemStack { id: incoming.id, count: cur + to_add });
        count
    }

    /// `Inventory.addResource(stack)`: place `incoming` into an existing partial
    /// stack if one has room, else the first free slot, returning the leftover.
    fn add_resource(&mut self, incoming: &ItemStack) -> i32 {
        let slot = self
            .slot_with_remaining_space(incoming)
            .or_else(|| self.first_free_slot());
        match slot {
            Some(s) => self.add_resource_to(s, incoming),
            None => incoming.count,
        }
    }

    /// Port of vanilla `Inventory.add(-1, itemStack)` for a non-damaged,
    /// stackable pickup (`net.minecraft.world.entity.player.Inventory.add`). Fills
    /// existing partial stacks of the same item first (selected → offhand →
    /// storage order), then the first free slot, repeating until no further
    /// progress. `stack.count` is decremented in place by the number stored;
    /// returns the number of items actually added (0 if the inventory was full).
    ///
    /// We do not model item damage or infinite-materials (creative) here, so the
    /// damaged-item and `hasInfiniteMaterials` branches of vanilla are omitted.
    pub fn add(&mut self, stack: &mut ItemStack) -> i32 {
        if stack.is_empty() {
            return 0;
        }
        let start = stack.count;
        loop {
            let before = stack.count;
            let leftover = self.add_resource(stack);
            stack.set_count(leftover);
            if stack.is_empty() || stack.count >= before {
                break;
            }
        }
        start - stack.count
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

    #[test]
    fn add_into_empty_inventory_uses_first_hotbar_slot() {
        // Empty inventory: the first free storage slot in vanilla scan order is
        // hotbar slot 0 -> Vela container slot 36.
        let mut inv = Inventory::new();
        let mut stack = ItemStack::new(1, 10);
        assert_eq!(inv.add(&mut stack), 10);
        assert!(stack.is_empty());
        assert_eq!(inv.slots[36], Some(ItemStack::new(1, 10)));
    }

    #[test]
    fn add_tops_up_existing_partial_stack_first() {
        // A partial stack of the same item in the main inventory is filled before
        // a fresh slot is used (getSlotWithRemainingSpace precedes getFreeSlot).
        let mut inv = Inventory::new();
        inv.slots[20] = Some(ItemStack::new(1, 60)); // room for 4 (max 64)
        let mut stack = ItemStack::new(1, 10);
        assert_eq!(inv.add(&mut stack), 10);
        assert_eq!(inv.slots[20], Some(ItemStack::new(1, 64))); // topped up
        // The remaining 6 spilled into the first free slot (hotbar 36).
        assert_eq!(inv.slots[36], Some(ItemStack::new(1, 6)));
        assert!(stack.is_empty());
    }

    #[test]
    fn add_partial_when_inventory_full_leaves_remainder() {
        // Every storage slot full of a different item: nothing fits, count intact.
        let mut inv = Inventory::new();
        for s in 9..45 {
            inv.slots[s] = Some(ItemStack::new(2, 64));
        }
        let mut stack = ItemStack::new(1, 5);
        assert_eq!(inv.add(&mut stack), 0);
        assert_eq!(stack.count, 5); // untouched

        // One partial same-item slot with room for 3: a partial pickup of 3.
        inv.slots[10] = Some(ItemStack::new(1, 61));
        let mut stack = ItemStack::new(1, 5);
        assert_eq!(inv.add(&mut stack), 3);
        assert_eq!(stack.count, 2);
        assert_eq!(inv.slots[10], Some(ItemStack::new(1, 64)));
    }
}
