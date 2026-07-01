//! The menu framework: an `AbstractContainerMenu`-style abstraction with slots
//! and click resolution.
//!
//! The click state machine ([`Menu::clicked`] → `do_click`) is a 1:1 port of the
//! decompiled 26.2 `AbstractContainerMenu.doClick`, including `moveItemStackTo`,
//! the `Slot` insert/remove helpers, and the per-menu `quickMoveStack` shift-move
//! routing (`InventoryMenu` and `ChestMenu`). Nothing copies Mojang code; the
//! logic is transcribed from the reference and rewritten in Rust idioms.
//!
//! **Storage model.** Vanilla `Slot`s point at live `Container`s. We instead
//! build a `Menu` over a *snapshot* of the backing containers (the player's
//! 46-slot inventory and, for a chest, its contents), resolve one click against
//! that snapshot, then write the result back. Because the server processes one
//! click at a time and fully re-syncs the client afterwards, snapshot-in /
//! write-back is behaviourally identical to mutating live containers.
//!
//! **Known gaps (documented, not silent).** No crafting-recipe resolution (the
//! 2×2 grid's result slot stays empty), no real item-drop entities (THROW and
//! click-outside discard the stack rather than spawning a `ItemEntity`), no armor
//! auto-equip routing in the player-inventory shift-move (needs equipment
//! categories), and `creative` is always `false` (the server advertises
//! survival), so CLONE and clone-drag are inert.

use bevy_ecs::prelude::*;

use super::container::PLAYER_INVENTORY_SLOTS;
use super::item_stack::ItemStack;

/// A clicked-outside slot index (`AbstractContainerMenu.SLOT_CLICKED_OUTSIDE`).
const SLOT_CLICKED_OUTSIDE: i32 = -999;

/// `ContainerInput` — the click mode of a `ServerboundContainerClickPacket`.
/// Ordinals match the decompiled `ContainerInput` enum.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClickType {
    Pickup,
    QuickMove,
    Swap,
    Clone,
    Throw,
    QuickCraft,
    PickupAll,
}

impl ClickType {
    /// Decode the wire ordinal. Out-of-range values fall back to `Pickup`,
    /// matching `ContainerInput`'s `ByIdMap.continuous(..., ZERO)` strategy.
    pub fn from_id(id: i32) -> ClickType {
        match id {
            0 => ClickType::Pickup,
            1 => ClickType::QuickMove,
            2 => ClickType::Swap,
            3 => ClickType::Clone,
            4 => ClickType::Throw,
            5 => ClickType::QuickCraft,
            6 => ClickType::PickupAll,
            _ => ClickType::Pickup,
        }
    }
}

/// `ClickAction`: which mouse button drove a PICKUP/QUICK_MOVE click.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClickAction {
    Primary,
    Secondary,
}

/// Where a menu slot's item physically lives in the snapshot.
#[derive(Clone, Copy)]
enum Backing {
    /// Index into the player's 46-slot inventory array.
    Player(usize),
    /// Index into the open container's (chest's) contents.
    Chest(usize),
}

/// A menu slot: its backing storage plus the minimal placement rule we model.
struct SlotDef {
    backing: Backing,
    /// Result slots (the crafting output) reject placement and forbid partial
    /// removal (`ResultSlot.mayPlace == false`).
    result: bool,
}

/// Which menu was opened — selects the `quickMoveStack` routing.
#[derive(Clone, Copy)]
enum MenuKind {
    /// The always-open player inventory menu (`InventoryMenu`, container id 0).
    PlayerInventory,
    /// A generic `9×rows` chest (`ChestMenu`).
    Chest { rows: usize },
}

/// An open menu over a snapshot of its backing containers. Build it, run
/// [`Menu::clicked`], then read the results back with the accessors.
pub struct Menu {
    kind: MenuKind,
    slots: Vec<SlotDef>,
    /// The player inventory snapshot (menu-ordered: 0 result, 1–4 grid, 5–8
    /// armor, 9–35 main, 36–44 hotbar, 45 offhand).
    player: [ItemStack; PLAYER_INVENTORY_SLOTS],
    /// The chest snapshot (empty for the player-inventory menu).
    chest: Vec<ItemStack>,
    /// The cursor item.
    carried: ItemStack,
    // Quick-craft (drag) state, threaded in/out via the `Inventory` component so
    // it survives across the packets of one drag.
    qc_status: i32,
    qc_type: i32,
    qc_slots: Vec<usize>,
}

impl Menu {
    /// Build the player-inventory menu from the 46-slot array and the cursor.
    pub fn player(slots: &[Option<ItemStack>; PLAYER_INVENTORY_SLOTS], carried: Option<ItemStack>) -> Menu {
        let mut player = [ItemStack::EMPTY; PLAYER_INVENTORY_SLOTS];
        for (i, s) in slots.iter().enumerate() {
            player[i] = ItemStack::from_option(*s);
        }
        // Menu slot i maps straight to player array index i; slot 0 is the result.
        let defs = (0..PLAYER_INVENTORY_SLOTS)
            .map(|i| SlotDef {
                backing: Backing::Player(i),
                result: i == 0,
            })
            .collect();
        Menu {
            kind: MenuKind::PlayerInventory,
            slots: defs,
            player,
            chest: Vec::new(),
            carried: ItemStack::from_option(carried),
            qc_status: 0,
            qc_type: -1,
            qc_slots: Vec::new(),
        }
    }

    /// Build a `9×rows` chest menu: the chest grid first, then the player's main
    /// inventory (menu slots 9–35) and hotbar (36–44), exactly as `ChestMenu`
    /// lays them out via `addStandardInventorySlots`.
    pub fn chest(
        rows: usize,
        chest: &[Option<ItemStack>],
        player_slots: &[Option<ItemStack>; PLAYER_INVENTORY_SLOTS],
        carried: Option<ItemStack>,
    ) -> Menu {
        let mut player = [ItemStack::EMPTY; PLAYER_INVENTORY_SLOTS];
        for (i, s) in player_slots.iter().enumerate() {
            player[i] = ItemStack::from_option(*s);
        }
        let mut chest_items = vec![ItemStack::EMPTY; rows * 9];
        for (i, s) in chest.iter().take(rows * 9).enumerate() {
            chest_items[i] = ItemStack::from_option(*s);
        }
        let mut defs: Vec<SlotDef> = Vec::with_capacity(rows * 9 + 36);
        for i in 0..rows * 9 {
            defs.push(SlotDef {
                backing: Backing::Chest(i),
                result: false,
            });
        }
        // Standard inventory: main (player menu-index 9..36) then hotbar (36..45).
        for i in 9..PLAYER_INVENTORY_SLOTS - 1 {
            defs.push(SlotDef {
                backing: Backing::Player(i),
                result: false,
            });
        }
        Menu {
            kind: MenuKind::Chest { rows },
            slots: defs,
            player,
            chest: chest_items,
            carried: ItemStack::from_option(carried),
            qc_status: 0,
            qc_type: -1,
            qc_slots: Vec::new(),
        }
    }

    /// Seed the drag state from persisted component fields before a click.
    pub fn set_drag(&mut self, status: i32, kind: i32, slots: Vec<usize>) {
        self.qc_status = status;
        self.qc_type = kind;
        self.qc_slots = slots;
    }

    /// The drag state to persist after a click.
    pub fn drag(&self) -> (i32, i32, Vec<usize>) {
        (self.qc_status, self.qc_type, self.qc_slots.clone())
    }

    /// The cursor after resolution.
    pub fn carried(&self) -> Option<ItemStack> {
        self.carried.to_option()
    }

    /// The player inventory snapshot after resolution.
    pub fn player_slots(&self) -> [Option<ItemStack>; PLAYER_INVENTORY_SLOTS] {
        let mut out = [None; PLAYER_INVENTORY_SLOTS];
        for (i, s) in self.player.iter().enumerate() {
            out[i] = s.to_option();
        }
        out
    }

    /// The chest snapshot after resolution.
    pub fn chest_slots(&self) -> Vec<Option<ItemStack>> {
        self.chest.iter().map(|s| s.to_option()).collect()
    }

    /// All menu slots in wire order (for `ContainerSetContent`).
    pub fn content(&self) -> Vec<Option<ItemStack>> {
        (0..self.slots.len()).map(|i| self.get_item(i).to_option()).collect()
    }

    /// `isValidSlotIndex`.
    pub fn is_valid_slot_index(&self, index: i32) -> bool {
        index == -1 || index == SLOT_CLICKED_OUTSIDE || (index >= 0 && (index as usize) < self.slots.len())
    }

    // --- slot accessors --------------------------------------------------------

    fn get_item(&self, slot: usize) -> ItemStack {
        match self.slots[slot].backing {
            Backing::Player(i) => self.player[i],
            Backing::Chest(i) => self.chest[i],
        }
    }

    fn set_item(&mut self, slot: usize, stack: ItemStack) {
        match self.slots[slot].backing {
            Backing::Player(i) => self.player[i] = stack,
            Backing::Chest(i) => self.chest[i] = stack,
        }
    }

    fn has_item(&self, slot: usize) -> bool {
        !self.get_item(slot).is_empty()
    }

    /// `Slot.mayPlace`: result slots reject placement; everything else accepts it
    /// (we don't model armor/category restrictions).
    fn may_place(&self, slot: usize, _stack: &ItemStack) -> bool {
        !self.slots[slot].result
    }

    /// `Slot.mayPickup`: always true in the menus we model.
    fn may_pickup(&self, _slot: usize) -> bool {
        true
    }

    /// `Slot.allowModification`: `mayPickup && mayPlace(item)`.
    fn allow_modification(&self, slot: usize) -> bool {
        self.may_pickup(slot) && self.may_place(slot, &self.get_item(slot))
    }

    /// Container max stack size is 64; the per-slot limit also honours the item's
    /// own max (`Slot.getMaxStackSize(stack)`).
    fn slot_max_stack_size(&self, _slot: usize, stack: &ItemStack) -> i32 {
        64.min(stack.max_stack_size())
    }

    /// `Slot.safeInsert(input, amount)` — merge up to `amount` of `input` into the
    /// slot, returning the (possibly shrunk) input.
    fn safe_insert(&mut self, slot: usize, mut input: ItemStack, amount: i32) -> ItemStack {
        if input.is_empty() || !self.may_place(slot, &input) {
            return input;
        }
        let slot_stack = self.get_item(slot);
        let transferable =
            amount.min(input.count).min(self.slot_max_stack_size(slot, &input) - slot_stack.count);
        if transferable <= 0 {
            return input;
        }
        if slot_stack.is_empty() {
            let moved = input.split(transferable);
            self.set_item(slot, moved);
        } else if ItemStack::same_item_same_components(&slot_stack, &input) {
            input.shrink(transferable);
            let mut s = slot_stack;
            s.grow(transferable);
            self.set_item(slot, s);
        }
        input
    }

    /// `Container.removeItem` for the slot — split up to `amount` off.
    fn remove(&mut self, slot: usize, amount: i32) -> ItemStack {
        let mut it = self.get_item(slot);
        let removed = it.split(amount);
        self.set_item(slot, it);
        removed
    }

    /// `Slot.tryRemove`.
    fn try_remove(&mut self, slot: usize, amount: i32, max_amount: i32) -> Option<ItemStack> {
        if !self.may_pickup(slot) {
            return None;
        }
        if !self.allow_modification(slot) && max_amount < self.get_item(slot).count {
            return None;
        }
        let amount = amount.min(max_amount);
        let result = self.remove(slot, amount);
        if result.is_empty() {
            return None;
        }
        Some(result)
    }

    /// `Slot.safeTake` (onTake side effects are no-ops in our model).
    fn safe_take(&mut self, slot: usize, amount: i32, max_amount: i32) -> ItemStack {
        self.try_remove(slot, amount, max_amount).unwrap_or(ItemStack::EMPTY)
    }

    /// `Slot.safeClone` — a full stack copy (creative middle-click).
    fn safe_clone(&self, slot: usize) -> ItemStack {
        let item = self.get_item(slot);
        item.copy_with_count(item.max_stack_size())
    }

    // --- moveItemStackTo / quickMove ------------------------------------------

    // See [`directed`] (module bottom) for the shared forward/backward slot walk
    // used by the two passes below and `do_pickup_all`.

    /// `AbstractContainerMenu.moveItemStackTo` — merge then place `stack` into the
    /// `[start, end)` slot range, optionally scanning backwards. Mutates `stack`
    /// (the remainder) and returns whether anything moved.
    fn move_item_stack_to(&mut self, stack: &mut ItemStack, start: usize, end: usize, backwards: bool) -> bool {
        let mut changed = false;

        // Pass 1: merge into existing matching stacks.
        if stack.is_stackable() {
            for slot in directed(start, end, backwards) {
                if stack.is_empty() {
                    break;
                }
                let target = self.get_item(slot);
                if !target.is_empty() && ItemStack::same_item_same_components(stack, &target) {
                    let total = target.count + stack.count;
                    let max = self.slot_max_stack_size(slot, &target);
                    if total <= max {
                        stack.set_count(0);
                        let mut t = target;
                        t.set_count(total);
                        self.set_item(slot, t);
                        changed = true;
                    } else if target.count < max {
                        stack.shrink(max - target.count);
                        let mut t = target;
                        t.set_count(max);
                        self.set_item(slot, t);
                        changed = true;
                    }
                }
            }
        }

        // Pass 2: drop the remainder into the first empty placeable slot.
        if !stack.is_empty() {
            for slot in directed(start, end, backwards) {
                let target = self.get_item(slot);
                if target.is_empty() && self.may_place(slot, stack) {
                    let max = self.slot_max_stack_size(slot, stack);
                    let moved = stack.split(stack.count.min(max));
                    self.set_item(slot, moved);
                    changed = true;
                    break;
                }
            }
        }

        changed
    }

    /// `quickMoveStack` — resolve a shift-click on `slot_index`, returning a copy
    /// of the originally-clicked stack (or `EMPTY` if nothing moved), per the
    /// per-menu routing.
    fn quick_move_stack(&mut self, slot_index: usize) -> ItemStack {
        match self.kind {
            MenuKind::PlayerInventory => self.quick_move_player(slot_index),
            MenuKind::Chest { rows } => self.quick_move_chest(rows, slot_index),
        }
    }

    /// `ChestMenu.quickMoveStack`.
    fn quick_move_chest(&mut self, rows: usize, slot_index: usize) -> ItemStack {
        let mut clicked = ItemStack::EMPTY;
        if self.has_item(slot_index) {
            let mut stack = self.get_item(slot_index);
            clicked = stack;
            let grid = rows * 9;
            let size = self.slots.len();
            if slot_index < grid {
                if !self.move_item_stack_to(&mut stack, grid, size, true) {
                    return ItemStack::EMPTY;
                }
            } else if !self.move_item_stack_to(&mut stack, 0, grid, false) {
                return ItemStack::EMPTY;
            }
            if stack.is_empty() {
                self.set_item(slot_index, ItemStack::EMPTY);
            } else {
                self.set_item(slot_index, stack);
            }
        }
        clicked
    }

    /// `InventoryMenu.quickMoveStack` (sans armor/offhand auto-equip routing,
    /// which needs equipment categories — documented gap).
    fn quick_move_player(&mut self, slot_index: usize) -> ItemStack {
        let mut clicked = ItemStack::EMPTY;
        if self.has_item(slot_index) {
            let mut stack = self.get_item(slot_index);
            clicked = stack;
            let moved = if slot_index == 0 {
                // Result slot → into main/hotbar, backwards.
                self.move_item_stack_to(&mut stack, 9, 45, true)
            } else if (1..9).contains(&slot_index) {
                // Crafting grid / armor → main+hotbar.
                self.move_item_stack_to(&mut stack, 9, 45, false)
            } else if (9..36).contains(&slot_index) {
                // Main inventory → hotbar.
                self.move_item_stack_to(&mut stack, 36, 45, false)
            } else if (36..45).contains(&slot_index) {
                // Hotbar → main inventory.
                self.move_item_stack_to(&mut stack, 9, 36, false)
            } else {
                // Offhand or anything else → main+hotbar.
                self.move_item_stack_to(&mut stack, 9, 45, false)
            };
            if !moved {
                return ItemStack::EMPTY;
            }
            if stack.is_empty() {
                self.set_item(slot_index, ItemStack::EMPTY);
            } else {
                self.set_item(slot_index, stack);
            }
            if stack.count == clicked.count {
                return ItemStack::EMPTY;
            }
        }
        clicked
    }

    /// A minimal `Inventory.add` for the SWAP overflow path: stack into matching
    /// hotbar/main slots, then the first empty one. Returns whether it all fit.
    fn inventory_add(&mut self, mut stack: ItemStack) -> bool {
        // Order mirrors vanilla's "selected, then 0..36" preference loosely: we
        // scan hotbar (36..45) then main (9..36). Merge pass, then placement.
        let regions: [(usize, usize); 2] = [(36, 45), (9, 36)];
        for &(s, e) in &regions {
            for i in s..e {
                if stack.is_empty() {
                    return true;
                }
                let target = self.player[i];
                if !target.is_empty() && ItemStack::same_item_same_components(&stack, &target) {
                    let max = 64.min(stack.max_stack_size());
                    let room = max - target.count;
                    if room > 0 {
                        let moved = stack.split(room);
                        let mut t = target;
                        t.grow(moved.count);
                        self.player[i] = t;
                    }
                }
            }
        }
        for &(s, e) in &regions {
            for i in s..e {
                if stack.is_empty() {
                    return true;
                }
                if self.player[i].is_empty() {
                    self.player[i] = stack;
                    stack = ItemStack::EMPTY;
                }
            }
        }
        stack.is_empty()
    }

    /// Read a player-inventory item by *container* index (0–8 hotbar, 40 offhand)
    /// as the SWAP source addresses it — translated to our menu-ordered array
    /// (hotbar 0–8 → 36–44, offhand 40 → 45).
    fn inv_container_get(&self, container_slot: i32) -> ItemStack {
        match self.player_index_for_container(container_slot) {
            Some(i) => self.player[i],
            None => ItemStack::EMPTY,
        }
    }

    fn inv_container_set(&mut self, container_slot: i32, stack: ItemStack) {
        if let Some(i) = self.player_index_for_container(container_slot) {
            self.player[i] = stack;
        }
    }

    fn player_index_for_container(&self, container_slot: i32) -> Option<usize> {
        match container_slot {
            0..=8 => Some(36 + container_slot as usize),
            40 => Some(45),
            _ => None,
        }
    }

    // --- the click state machine ----------------------------------------------

    /// `AbstractContainerMenu.clicked` → `doClick`.
    pub fn clicked(&mut self, slot_index: i32, button: i32, click: ClickType, creative: bool) {
        self.do_click(slot_index, button, click, creative);
    }

    fn do_click(&mut self, slot_index: i32, button: i32, click: ClickType, creative: bool) {
        if click == ClickType::QuickCraft {
            self.do_quick_craft(slot_index, button, creative);
            return;
        }
        if self.qc_status != 0 {
            self.reset_quick_craft();
            return;
        }

        match click {
            ClickType::Pickup | ClickType::QuickMove if button == 0 || button == 1 => {
                let action = if button == 0 { ClickAction::Primary } else { ClickAction::Secondary };
                if slot_index == SLOT_CLICKED_OUTSIDE {
                    if !self.carried.is_empty() {
                        if action == ClickAction::Primary {
                            // Drop the whole cursor (no item entity — discarded).
                            self.carried = ItemStack::EMPTY;
                        } else {
                            self.carried.split(1); // drop one (discarded)
                        }
                    }
                } else if click == ClickType::QuickMove {
                    if slot_index < 0 {
                        return;
                    }
                    let slot = slot_index as usize;
                    if !self.may_pickup(slot) {
                        return;
                    }
                    let mut moved = self.quick_move_stack(slot);
                    while !moved.is_empty() && ItemStack::same_item(&self.get_item(slot), &moved) {
                        moved = self.quick_move_stack(slot);
                    }
                } else {
                    if slot_index < 0 {
                        return;
                    }
                    self.do_pickup(slot_index as usize, action);
                }
            }
            ClickType::Swap if (0..9).contains(&button) || button == 40 => {
                self.do_swap(slot_index as usize, button);
            }
            ClickType::Clone if creative && self.carried.is_empty() && slot_index >= 0 => {
                let slot = slot_index as usize;
                if self.has_item(slot) {
                    self.carried = self.safe_clone(slot);
                }
            }
            ClickType::Throw if self.carried.is_empty() && slot_index >= 0 => {
                let slot = slot_index as usize;
                let amount = if button == 0 { 1 } else { self.get_item(slot).count };
                // Items are discarded (no drop entities yet).
                let mut taken = self.safe_take(slot, amount, i32::MAX);
                if button == 1 {
                    while !taken.is_empty() && ItemStack::same_item(&self.get_item(slot), &taken) {
                        taken = self.safe_take(slot, amount, i32::MAX);
                    }
                }
            }
            ClickType::PickupAll if slot_index >= 0 => {
                self.do_pickup_all(slot_index as usize, button);
            }
            _ => {}
        }
    }

    /// The PICKUP body (carried ↔ slot), `doClick`'s plain-pickup branch.
    fn do_pickup(&mut self, slot: usize, action: ClickAction) {
        let clicked = self.get_item(slot);
        let carried = self.carried;
        if clicked.is_empty() {
            if !carried.is_empty() {
                let amount = if action == ClickAction::Primary { carried.count } else { 1 };
                let remainder = self.safe_insert(slot, carried, amount);
                self.carried = remainder;
            }
        } else if self.may_pickup(slot) {
            if carried.is_empty() {
                let amount = if action == ClickAction::Primary {
                    clicked.count
                } else {
                    (clicked.count + 1) / 2
                };
                if let Some(taken) = self.try_remove(slot, amount, i32::MAX) {
                    self.carried = taken;
                }
            } else if self.may_place(slot, &carried) {
                if ItemStack::same_item_same_components(&clicked, &carried) {
                    let amount = if action == ClickAction::Primary { carried.count } else { 1 };
                    let remainder = self.safe_insert(slot, carried, amount);
                    self.carried = remainder;
                } else if carried.count <= self.slot_max_stack_size(slot, &carried) {
                    // Swap slot and cursor.
                    self.set_item(slot, carried);
                    self.carried = clicked;
                }
            } else if ItemStack::same_item_same_components(&clicked, &carried) {
                // Cursor full of the same item but slot can't take it: pull from
                // the slot into the cursor up to the cursor's headroom.
                let headroom = carried.max_stack_size() - carried.count;
                if let Some(taken) = self.try_remove(slot, clicked.count, headroom) {
                    let mut c = self.carried;
                    c.grow(taken.count);
                    self.carried = c;
                }
            }
        }
    }

    /// The SWAP body (hotbar number key / offhand `F`).
    fn do_swap(&mut self, slot: usize, button: i32) {
        let source = self.inv_container_get(button);
        let target = self.get_item(slot);
        if source.is_empty() && target.is_empty() {
            return;
        }
        if source.is_empty() {
            if self.may_pickup(slot) {
                self.inv_container_set(button, target);
                self.set_item(slot, ItemStack::EMPTY);
            }
        } else if target.is_empty() {
            if self.may_place(slot, &source) {
                let max = self.slot_max_stack_size(slot, &source);
                if source.count > max {
                    let mut s = source;
                    let placed = s.split(max);
                    self.set_item(slot, placed);
                    self.inv_container_set(button, s);
                } else {
                    self.inv_container_set(button, ItemStack::EMPTY);
                    self.set_item(slot, source);
                }
            }
        } else if self.may_pickup(slot) && self.may_place(slot, &source) {
            let max = self.slot_max_stack_size(slot, &source);
            if source.count > max {
                let mut s = source;
                let placed = s.split(max);
                self.set_item(slot, placed);
                self.inv_container_set(button, s);
                if !self.inventory_add(target) {
                    // Would drop — discarded (no item entity).
                }
            } else {
                self.inv_container_set(button, target);
                self.set_item(slot, source);
            }
        }
    }

    /// The PICKUP_ALL body (double-click: gather matching stacks into the cursor).
    fn do_pickup_all(&mut self, slot: usize, button: i32) {
        let mut carried = self.carried;
        if carried.is_empty() || (self.has_item(slot) && self.may_pickup(slot)) {
            return;
        }
        let size = self.slots.len();
        // Button 0 scans forward from the first slot, 1 backwards from the last.
        let backwards = button != 0;
        for pass in 0..2 {
            for t in directed(0, size, backwards) {
                if carried.count >= carried.max_stack_size() {
                    break;
                }
                let item = self.get_item(t);
                if !item.is_empty()
                    && can_item_quick_replace(&item, &carried, true)
                    && self.may_pickup(t)
                    && self.can_take_item_for_pick_all(t)
                    && (pass != 0 || item.count != item.max_stack_size())
                {
                    let removed = self.safe_take(t, item.count, carried.max_stack_size() - carried.count);
                    carried.grow(removed.count);
                }
            }
        }
        self.carried = carried;
    }

    /// `canTakeItemForPickAll` — the player-inventory menu excludes the crafting
    /// result slot; chests allow everything.
    fn can_take_item_for_pick_all(&self, slot: usize) -> bool {
        !self.slots[slot].result
    }

    // --- quick-craft (item drag) ----------------------------------------------

    fn reset_quick_craft(&mut self) {
        self.qc_status = 0;
        self.qc_slots.clear();
    }

    /// `doClick`'s QUICK_CRAFT branch — the multi-packet drag gesture.
    // The first two arms both reset but on distinct conditions (bad header
    // sequence vs. empty cursor); kept separate to mirror the reference 1:1.
    #[allow(clippy::if_same_then_else)]
    fn do_quick_craft(&mut self, slot_index: i32, button: i32, creative: bool) {
        let expected_status = self.qc_status;
        self.qc_status = quickcraft_header(button);
        if (expected_status != 1 || self.qc_status != 2) && expected_status != self.qc_status {
            self.reset_quick_craft();
        } else if self.carried.is_empty() {
            self.reset_quick_craft();
        } else if self.qc_status == 0 {
            self.qc_type = quickcraft_type(button);
            if is_valid_quickcraft_type(self.qc_type, creative) {
                self.qc_status = 1;
                self.qc_slots.clear();
            } else {
                self.reset_quick_craft();
            }
        } else if self.qc_status == 1 {
            if slot_index < 0 {
                return;
            }
            let slot = slot_index as usize;
            let carried = self.carried;
            if can_item_quick_replace(&self.get_item(slot), &carried, true)
                && self.may_place(slot, &carried)
                && (self.qc_type == 2 || carried.count > self.qc_slots.len() as i32)
                && !self.qc_slots.contains(&slot)
            {
                self.qc_slots.push(slot);
            }
        } else if self.qc_status == 2 {
            if !self.qc_slots.is_empty() {
                if self.qc_slots.len() == 1 {
                    let slot = self.qc_slots[0] as i32;
                    let qc_type = self.qc_type;
                    self.reset_quick_craft();
                    // A single-slot drag degrades to an ordinary pickup click.
                    self.do_click(slot, qc_type, ClickType::Pickup, creative);
                    return;
                }

                let mut source = self.carried;
                if source.is_empty() {
                    self.reset_quick_craft();
                    return;
                }
                let mut remaining = self.carried.count;
                let slots = self.qc_slots.clone();
                for slot in slots {
                    let carried = self.carried;
                    if can_item_quick_replace(&self.get_item(slot), &carried, true)
                        && self.may_place(slot, &carried)
                        && (self.qc_type == 2 || carried.count >= self.qc_slots.len() as i32)
                    {
                        let existing = if self.has_item(slot) { self.get_item(slot).count } else { 0 };
                        let max = source.max_stack_size().min(self.slot_max_stack_size(slot, &source));
                        let new_count = (quick_craft_place_count(self.qc_slots.len(), self.qc_type, &source) + existing).min(max);
                        remaining -= new_count - existing;
                        self.set_item(slot, source.copy_with_count(new_count));
                    }
                }
                source.set_count(remaining);
                self.carried = source;
            }
            self.reset_quick_craft();
        } else {
            self.reset_quick_craft();
        }
    }
}

/// A `[start, end)` slot walk in forward or reverse order — the traversal shared
/// by `moveItemStackTo`'s two passes and `do_pickup_all`. It replaces the
/// hand-rolled `i32` cursor whose `if backwards` direction test had to be spelled
/// three times per loop (init, bound, step) with the signed/`usize` casts that
/// implied. A dedicated enum keeps it allocation-free: `Range` and `Rev<Range>`
/// are distinct types, so they can't share an `impl Trait` return without either
/// boxing or an external `Either`.
enum Directed {
    Forward(std::ops::Range<usize>),
    Backward(std::iter::Rev<std::ops::Range<usize>>),
}

impl Iterator for Directed {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        match self {
            Directed::Forward(range) => range.next(),
            Directed::Backward(range) => range.next(),
        }
    }
}

/// Walk `start..end` forwards, or `end-1..=start` backwards when `backwards`.
/// `.rev()` visits exactly the same indices, so callers keep vanilla's two-pass
/// `moveItemStackTo` semantics unchanged.
fn directed(start: usize, end: usize, backwards: bool) -> Directed {
    if backwards {
        Directed::Backward((start..end).rev())
    } else {
        Directed::Forward(start..end)
    }
}

/// `getQuickcraftHeader(mask) = mask & 3`.
fn quickcraft_header(mask: i32) -> i32 {
    mask & 3
}

/// `getQuickcraftType(mask) = mask >> 2 & 3`.
fn quickcraft_type(mask: i32) -> i32 {
    (mask >> 2) & 3
}

/// `isValidQuickcraftType` — charitable/greedy always valid; clone needs creative.
fn is_valid_quickcraft_type(kind: i32, creative: bool) -> bool {
    match kind {
        0 | 1 => true,
        2 => creative,
        _ => false,
    }
}

/// `getQuickCraftPlaceCount`.
fn quick_craft_place_count(slots: usize, kind: i32, stack: &ItemStack) -> i32 {
    match kind {
        0 => stack.count / slots as i32,
        1 => 1,
        2 => stack.max_stack_size(),
        _ => stack.count,
    }
}

/// `canItemQuickReplace` — whether `stack` may merge onto `slot`'s item.
fn can_item_quick_replace(slot_item: &ItemStack, stack: &ItemStack, ignore_size: bool) -> bool {
    let slot_empty = slot_item.is_empty();
    if !slot_empty && ItemStack::same_item_same_components(stack, slot_item) {
        slot_item.count + (if ignore_size { 0 } else { stack.count }) <= stack.max_stack_size()
    } else {
        slot_empty
    }
}

/// A chest's contents and row count, stored on the player entity while a chest
/// menu is open. Since Vela has no block entities yet, the chest is a per-open
/// in-memory container (it does not persist to a world block).
#[derive(Component)]
pub struct OpenContainer {
    /// The non-zero menu/container id assigned when the screen opened.
    pub container_id: i32,
    /// Chest rows (`9 × rows` slots).
    pub rows: usize,
    /// Chest contents (`rows * 9` slots).
    pub items: Vec<Option<ItemStack>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_player() -> [Option<ItemStack>; PLAYER_INVENTORY_SLOTS] {
        [None; PLAYER_INVENTORY_SLOTS]
    }

    /// A left-click (PICKUP primary) on a slot with an item and an empty cursor
    /// picks the whole stack up into the cursor.
    #[test]
    fn pickup_full_stack_into_cursor() {
        let mut slots = empty_player();
        slots[36] = Some(ItemStack::new(1, 32)); // stone x32 in first hotbar slot
        let mut menu = Menu::player(&slots, None);
        menu.clicked(36, 0, ClickType::Pickup, false);
        assert_eq!(menu.carried(), Some(ItemStack::new(1, 32)));
        assert_eq!(menu.player_slots()[36], None);
    }

    /// A right-click (PICKUP secondary) on a stack with an empty cursor takes
    /// half (rounded up) into the cursor.
    #[test]
    fn right_click_takes_half() {
        let mut slots = empty_player();
        slots[36] = Some(ItemStack::new(1, 7));
        let mut menu = Menu::player(&slots, None);
        menu.clicked(36, 1, ClickType::Pickup, false);
        assert_eq!(menu.carried(), Some(ItemStack::new(1, 4))); // ceil(7/2)
        assert_eq!(menu.player_slots()[36], Some(ItemStack::new(1, 3)));
    }

    /// A right-click with a held cursor drops exactly one item into an empty slot.
    #[test]
    fn right_click_places_one() {
        let slots = empty_player();
        let mut menu = Menu::player(&slots, Some(ItemStack::new(1, 10)));
        menu.clicked(9, 1, ClickType::Pickup, false);
        assert_eq!(menu.player_slots()[9], Some(ItemStack::new(1, 1)));
        assert_eq!(menu.carried(), Some(ItemStack::new(1, 9)));
    }

    /// Left-clicking a matching slot with a held cursor merges up to the max
    /// stack size, leaving the overflow on the cursor.
    #[test]
    fn merge_respects_max_stack_size() {
        let mut slots = empty_player();
        slots[9] = Some(ItemStack::new(1, 60));
        let mut menu = Menu::player(&slots, Some(ItemStack::new(1, 20)));
        menu.clicked(9, 0, ClickType::Pickup, false);
        assert_eq!(menu.player_slots()[9], Some(ItemStack::new(1, 64)));
        assert_eq!(menu.carried(), Some(ItemStack::new(1, 16)));
    }

    /// Left-clicking a different-item slot with a held cursor swaps the two when
    /// the cursor fits the slot.
    #[test]
    fn pickup_swaps_different_items() {
        let mut slots = empty_player();
        slots[9] = Some(ItemStack::new(1, 5)); // stone
        let mut menu = Menu::player(&slots, Some(ItemStack::new(55, 3))); // dirt
        menu.clicked(9, 0, ClickType::Pickup, false);
        assert_eq!(menu.player_slots()[9], Some(ItemStack::new(55, 3)));
        assert_eq!(menu.carried(), Some(ItemStack::new(1, 5)));
    }

    /// Shift-clicking a hotbar item with space in the main inventory moves it up.
    #[test]
    fn quick_move_hotbar_to_main() {
        let mut slots = empty_player();
        slots[36] = Some(ItemStack::new(1, 10));
        let mut menu = Menu::player(&slots, None);
        menu.clicked(36, 0, ClickType::QuickMove, false);
        assert_eq!(menu.player_slots()[36], None);
        // Lands in the first main slot (menu index 9).
        assert_eq!(menu.player_slots()[9], Some(ItemStack::new(1, 10)));
    }

    /// A number-key SWAP exchanges a chest slot with the addressed hotbar slot.
    #[test]
    fn swap_hotbar_with_chest_slot() {
        let mut player = empty_player();
        player[36] = Some(ItemStack::new(1, 8)); // hotbar slot 0 holds stone
        let chest = vec![None; 27];
        let mut menu = Menu::chest(3, &chest, &player, None);
        // Click chest slot 0 with button 0 (hotbar container slot 0).
        menu.clicked(0, 0, ClickType::Swap, false);
        assert_eq!(menu.chest_slots()[0], Some(ItemStack::new(1, 8)));
        assert_eq!(menu.player_slots()[36], None);
    }

    /// Shift-clicking a chest item with an empty player inventory moves it down
    /// into the player's section.
    #[test]
    fn chest_quick_move_to_player() {
        let player = empty_player();
        let mut chest = vec![None; 27];
        chest[0] = Some(ItemStack::new(1, 16));
        let mut menu = Menu::chest(3, &chest, &player, None);
        menu.clicked(0, 0, ClickType::QuickMove, false);
        assert_eq!(menu.chest_slots()[0], None);
        // Chest → player shift-move scans the player section backwards (vanilla
        // `moveItemStackTo(.., true)`), so an empty inventory fills from the last
        // hotbar slot first (player menu index 44).
        assert_eq!(menu.player_slots()[44], Some(ItemStack::new(1, 16)));
    }

    /// Dragging the cursor across two empty slots (charitable/left-drag) splits
    /// the held stack evenly between them.
    #[test]
    fn quick_craft_left_drag_splits_evenly() {
        let slots = empty_player();
        let mut menu = Menu::player(&slots, Some(ItemStack::new(1, 8)));
        // Header START (type CHARITABLE=0): mask = 0.
        menu.clicked(-999, 0, ClickType::QuickCraft, false);
        // CONTINUE over slot 9 then slot 18: mask = header 1 | type 0<<2 = 1.
        menu.clicked(9, 1, ClickType::QuickCraft, false);
        menu.clicked(18, 1, ClickType::QuickCraft, false);
        // END: mask = header 2.
        menu.clicked(-999, 2, ClickType::QuickCraft, false);
        assert_eq!(menu.player_slots()[9], Some(ItemStack::new(1, 4)));
        assert_eq!(menu.player_slots()[18], Some(ItemStack::new(1, 4)));
        assert_eq!(menu.carried(), None);
    }

    /// Clicking outside with a held cursor (primary) drops the whole cursor; we
    /// have no drop entities, so it is discarded.
    #[test]
    fn click_outside_discards_cursor() {
        let slots = empty_player();
        let mut menu = Menu::player(&slots, Some(ItemStack::new(1, 5)));
        menu.clicked(-999, 0, ClickType::Pickup, false);
        assert_eq!(menu.carried(), None);
    }

    /// CLONE only works in creative; in survival it is inert.
    #[test]
    fn clone_inert_in_survival() {
        let mut slots = empty_player();
        slots[9] = Some(ItemStack::new(1, 5));
        let mut menu = Menu::player(&slots, None);
        menu.clicked(9, 2, ClickType::Clone, false);
        assert_eq!(menu.carried(), None);
        let mut menu2 = Menu::player(&slots, None);
        menu2.clicked(9, 2, ClickType::Clone, true);
        assert_eq!(menu2.carried(), Some(ItemStack::new(1, 64))); // full clone
    }
}
