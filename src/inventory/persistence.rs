//! Player-inventory persistence — a 1:1 port of vanilla `Inventory.save`/`load`
//! (`net.minecraft.world.entity.player.Inventory`, MC 26.2) plus the equipment
//! split that `LivingEntity` writes under the `"equipment"` tag
//! (`EntityEquipment.CODEC`).
//!
//! **Layout bridge.** Vela stores the player inventory in the 46-slot
//! `InventoryMenu` *view* layout (which is also the wire layout for window 0):
//!
//! * `0` crafting result, `1..=4` crafting grid (never persisted — vanilla keeps
//!   these in the menu, not the `Inventory`, and drops them on close),
//! * `5..=8` armor (head, chest, legs, feet), `9..=35` main, `36..=44` hotbar,
//!   `45` offhand.
//!
//! Vanilla persists from the 41-slot `Inventory` *model*: the `items` list
//! (indices `0..36` = hotbar `0..9`, main `9..36`) is written to the `"Inventory"`
//! list via `ItemStackWithSlot`, while armor/offhand live in a separate
//! `EntityEquipment` serialized under `"equipment"`. This module maps between the
//! two so the on-disk bytes are identical to what vanilla writes:
//!
//! * `Inventory.save` writes `items[i]` for `i in 0..36` (skipping empties) as
//!   `{ Slot: unsigned-byte, id: string, count: int }` — `ItemStackWithSlot.CODEC`
//!   over `ItemStack.MAP_CODEC`, where `count` is always present (default 1) and
//!   the empty `components` patch is omitted (always, here).
//! * `EntityEquipment.CODEC` is an unbounded `slot-name -> ItemStack` map with
//!   empties removed; the whole `"equipment"` tag is omitted when nothing is worn.
//!

use super::container::PLAYER_INVENTORY_SLOTS;
use super::item_stack::ItemStack;
use crate::protocol::nbt::Nbt;
use crate::registry::item;

/// Number of `Inventory.items` storage slots that go in the `"Inventory"` list
/// (`Inventory.INVENTORY_SIZE` = 36: hotbar 0..9 + main 9..36). `Inventory.load`
/// gates each entry on `isValidInContainer(items.size())`, i.e. `slot < 36`.
const INVENTORY_SIZE: usize = 36;

/// The menu-store index holding vanilla `Inventory` storage slot `vanilla`
/// (`0..36`). Inverse of `InventoryMenu.addStandardInventorySlots`: the hotbar
/// (vanilla `0..9`) sits at menu `36..45`; the main inventory (vanilla `9..36`)
/// is identity-mapped.
fn menu_index_for_storage(vanilla: usize) -> usize {
    if vanilla < 9 {
        PLAYER_INVENTORY_SLOTS - 10 + vanilla // hotbar: vanilla 0..9 -> menu 36..45
    } else {
        vanilla // main: vanilla 9..36 -> menu 9..36 (identity)
    }
}

/// Menu-store indices of the equipment slots and their `EquipmentSlot`
/// serialized names (`EquipmentSlot.getSerializedName`). Vela's menu armor block
/// is `5..=8` = head, chest, legs, feet; offhand is `45`. Body/saddle
/// (`EquipmentSlot.BODY`/`SADDLE`) are mob-only and not modelled.
const EQUIPMENT_SLOTS: [(usize, &str); 5] = [
    (5, "head"),
    (6, "chest"),
    (7, "legs"),
    (8, "feet"),
    (45, "offhand"),
];

/// `ItemStack.MAP_CODEC` → `{ id: string, count: int }`. Data components are not
/// modelled, so the optional `"components"` field is always omitted (matching an
/// empty `DataComponentPatch`). Returns `None` for an unregistered id (unreachable
/// for a live stack) so the caller can skip it rather than write a bad `"id"`.
fn item_stack_nbt(stack: &ItemStack) -> Option<Nbt> {
    let name = item::name_of(stack.id)?;
    Some(Nbt::compound([
        ("id", Nbt::string(name)),
        ("count", Nbt::Int(stack.count)),
    ]))
}

/// One `ItemStackWithSlot.CODEC` entry: the flat `{ Slot, id, count }` compound
/// used inside the `"Inventory"` list. `Slot` is `ExtraCodecs.UNSIGNED_BYTE`.
fn item_stack_with_slot_nbt(slot: usize, stack: &ItemStack) -> Option<Nbt> {
    let name = item::name_of(stack.id)?;
    Some(Nbt::compound([
        ("Slot", Nbt::Byte(slot as i8)),
        ("id", Nbt::string(name)),
        ("count", Nbt::Int(stack.count)),
    ]))
}

/// Parse an `ItemStack.MAP_CODEC` compound (`{ id, count }`). Missing `count`
/// defaults to `1` (`optionalAlwaysPresentFieldOf(.., "count", 1)`); an
/// unregistered id, or an empty/non-positive stack, yields `None`.
fn parse_item_stack(tag: &Nbt) -> Option<ItemStack> {
    let id = item::id_of(tag.get("id")?.as_str()?)?;
    let count = match tag.get("count") {
        Some(Nbt::Int(c)) => *c,
        _ => 1,
    };
    let stack = ItemStack { id, count };
    if stack.is_empty() {
        None
    } else {
        Some(stack)
    }
}

/// `Inventory.save` + the `"equipment"` split: return the `"Inventory"` list
/// entries (storage slots `0..36`, empties skipped) and, when anything is worn,
/// the `"equipment"` compound.
pub fn inventory_to_nbt(
    slots: &[Option<ItemStack>; PLAYER_INVENTORY_SLOTS],
) -> (Vec<Nbt>, Option<Nbt>) {
    let mut list = Vec::new();
    for vanilla in 0..INVENTORY_SIZE {
        if let Some(stack) = slots[menu_index_for_storage(vanilla)] {
            if !stack.is_empty() {
                if let Some(entry) = item_stack_with_slot_nbt(vanilla, &stack) {
                    list.push(entry);
                }
            }
        }
    }

    let mut equipment: Vec<(String, Nbt)> = Vec::new();
    for (menu, name) in EQUIPMENT_SLOTS {
        if let Some(stack) = slots[menu] {
            if !stack.is_empty() {
                if let Some(item) = item_stack_nbt(&stack) {
                    equipment.push((name.to_string(), item));
                }
            }
        }
    }
    let equipment = (!equipment.is_empty()).then_some(Nbt::Compound(equipment));

    (list, equipment)
}

/// `Inventory.load` + `EntityEquipment` read: rebuild the 46-slot menu store from
/// the `"Inventory"` list and optional `"equipment"` compound. Starts from all-
/// empty (vanilla `load` clears `items` first); unknown ids and out-of-range
/// slots are skipped, matching `isValidInContainer`.
pub fn inventory_from_nbt(
    inventory_list: Option<&Vec<Nbt>>,
    equipment: Option<&Nbt>,
) -> [Option<ItemStack>; PLAYER_INVENTORY_SLOTS] {
    let mut slots = [None; PLAYER_INVENTORY_SLOTS];

    for entry in inventory_list.into_iter().flatten() {
        let slot = match entry.get("Slot") {
            Some(Nbt::Byte(b)) => (*b as u8) as usize,
            _ => continue,
        };
        if slot >= INVENTORY_SIZE {
            continue; // isValidInContainer(items.size() == 36)
        }
        if let Some(stack) = parse_item_stack(entry) {
            slots[menu_index_for_storage(slot)] = Some(stack);
        }
    }

    if let Some(equipment) = equipment {
        for (menu, name) in EQUIPMENT_SLOTS {
            if let Some(tag) = equipment.get(name) {
                if let Some(stack) = parse_item_stack(tag) {
                    slots[menu] = Some(stack);
                }
            }
        }
    }

    slots
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> [Option<ItemStack>; PLAYER_INVENTORY_SLOTS] {
        [None; PLAYER_INVENTORY_SLOTS]
    }

    #[test]
    fn hotbar_maps_to_vanilla_slots_0_to_8() {
        let mut slots = empty();
        slots[36] = Some(ItemStack::new(1, 5)); // menu hotbar 0 -> vanilla slot 0
        slots[44] = Some(ItemStack::new(1, 2)); // menu hotbar 8 -> vanilla slot 8
        let (list, equip) = inventory_to_nbt(&slots);
        assert!(equip.is_none());
        let slot_of = |n: &Nbt| match n.get("Slot") {
            Some(Nbt::Byte(b)) => *b,
            _ => panic!("no Slot"),
        };
        assert_eq!(list.len(), 2);
        assert_eq!(slot_of(&list[0]), 0);
        assert_eq!(slot_of(&list[1]), 8);
    }

    #[test]
    fn main_inventory_is_identity_mapped() {
        let mut slots = empty();
        slots[9] = Some(ItemStack::new(1, 1)); // menu main first -> vanilla slot 9
        let (list, _) = inventory_to_nbt(&slots);
        assert_eq!(list.len(), 1);
        assert!(matches!(list[0].get("Slot"), Some(Nbt::Byte(9))));
        assert_eq!(list[0].get("id").and_then(Nbt::as_str), Some("minecraft:stone"));
        assert!(matches!(list[0].get("count"), Some(Nbt::Int(1))));
    }

    #[test]
    fn armor_and_offhand_go_to_equipment_tag() {
        let mut slots = empty();
        slots[5] = Some(ItemStack::new(1, 1)); // head
        slots[45] = Some(ItemStack::new(1, 1)); // offhand
        let (list, equip) = inventory_to_nbt(&slots);
        assert!(list.is_empty(), "armor/offhand must not land in the Inventory list");
        let equip = equip.expect("equipment tag present");
        assert!(equip.get("head").is_some());
        assert!(equip.get("offhand").is_some());
    }

    #[test]
    fn round_trips_through_nbt() {
        let mut slots = empty();
        slots[36] = Some(ItemStack::new(1, 64)); // hotbar
        slots[9] = Some(ItemStack::new(55, 10)); // main
        slots[6] = Some(ItemStack::new(1, 1)); // chest armor
        let (list, equip) = inventory_to_nbt(&slots);
        let restored = inventory_from_nbt(Some(&list), equip.as_ref());
        assert_eq!(restored, slots);
    }

    #[test]
    fn empty_inventory_writes_no_entries() {
        let (list, equip) = inventory_to_nbt(&empty());
        assert!(list.is_empty());
        assert!(equip.is_none());
    }

    #[test]
    fn out_of_range_and_unknown_slots_are_skipped() {
        // A Slot >= 36 (armor range in the Inventory list) is rejected by the
        // container-size guard, exactly as vanilla `isValidInContainer` does.
        let bogus = Nbt::compound([
            ("Slot", Nbt::Byte(36)),
            ("id", Nbt::string("minecraft:stone")),
            ("count", Nbt::Int(1)),
        ]);
        let restored = inventory_from_nbt(Some(&vec![bogus]), None);
        assert_eq!(restored, empty());
    }
}
