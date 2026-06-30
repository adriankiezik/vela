//! Inventory & item-registry scaffolding.
//!
//! A self-contained data model for items and slots, its own packet
//! builders/decoders, and the player-inventory container. This is greenfield: it
//! owns the `ItemStack` network codec, a small curated item registry, and the
//! four inventory packets we currently speak (two clientbound, two serverbound).
//!
//! Wire formats are taken 1:1 from the decompiled 26.2 reference — see the
//! per-item citations below. Nothing here copies Mojang code; the layouts are
//! transcribed from the `StreamCodec` definitions.
//!
//! **Packet placement.** Unlike the player/movement/chat builders in
//! `sim::packets`, this module owns *its own* inventory packet builders and ids.
//! That is deliberate: inventory/containers are a self-contained domain that
//! will keep growing (menu types, clicks, recipes), so the `ItemStack` codec,
//! the registry, and the packets that carry them live together rather than
//! leaking into the shared packet module. `sim::packets` notes the same split.

use bevy_ecs::prelude::*;
use bytes::Bytes;

use crate::protocol::buffer::{PacketReader, PacketWriter};
use crate::protocol::framing::frame;

// ---------------------------------------------------------------------------
// Packet ids (registration order in the decompiled 26.2 `GameProtocols`).
// ---------------------------------------------------------------------------

/// `ClientboundContainerSetContentPacket` — index 18 in the clientbound PLAY
/// flow. The flow's first entry is the bundle delimiter (id 0), so the visible
/// `ADD_ENTITY` sits at id 1; `CONTAINER_SET_CONTENT` follows at 18. Verified
/// against `GameProtocols.CLIENTBOUND_TEMPLATE`.
const CB_PLAY_CONTAINER_SET_CONTENT: i32 = 18;

/// `ClientboundSetHeldSlotPacket` — index 105 in the clientbound PLAY flow
/// (`GameProtocols.CLIENTBOUND_TEMPLATE`). Tells the client which hotbar slot is
/// selected.
const CB_PLAY_SET_HELD_SLOT: i32 = 105;

// ---------------------------------------------------------------------------
// Item registry — a hand-curated subset.
// ---------------------------------------------------------------------------

/// `namespace:path` → numeric item id.
///
/// **Id source:** the registration (= field-declaration) order in the decompiled
/// 26.2 `net/minecraft/world/item/Items.java`, where `AIR` is index 0 and every
/// subsequent `public static final Item …` declaration advances the id by one.
/// `BuiltInRegistries.ITEM.getId(item)` returns exactly this index, and that is
/// the value the `Item` `StreamCodec` (a `holderRegistry` over `Registries.ITEM`)
/// writes on the wire as a plain VarInt. The ids below were read off that
/// declaration order — e.g. `grass_block` is the 55th declaration → id 54.
///
/// This is a representative scaffold, not the full ~1500-item registry; extend it
/// by adding rows (keep them in ascending-id order for readability).
#[rustfmt::skip]
static ITEMS: &[(&str, i32)] = &[
    ("minecraft:air",             0),
    ("minecraft:stone",           1),
    ("minecraft:granite",         2),
    ("minecraft:diorite",         4),
    ("minecraft:andesite",        6),
    ("minecraft:cobbled_deepslate", 9),
    ("minecraft:grass_block",     54),
    ("minecraft:dirt",            55),
    ("minecraft:cobblestone",     62),
    ("minecraft:oak_planks",      63),
    ("minecraft:oak_sapling",     76),
    ("minecraft:bedrock",         85),
    ("minecraft:sand",            86),
    ("minecraft:gravel",          90),
    ("minecraft:coal_ore",        91),
    ("minecraft:iron_ore",        93),
    ("minecraft:gold_ore",        97),
    ("minecraft:diamond_ore",     105),
    ("minecraft:oak_log",         121),
    ("minecraft:oak_leaves",      169),
    ("minecraft:glass",           182),
    ("minecraft:obsidian",        293),
    ("minecraft:torch",           294),
    ("minecraft:chest",           303),
    ("minecraft:crafting_table",  304),
    ("minecraft:furnace",         306),
    ("minecraft:cobblestone_stairs", 308),
    ("minecraft:glowstone",       339),
    ("minecraft:oak_door",        600),
    ("minecraft:apple",           681),
    ("minecraft:bow",             682),
    ("minecraft:arrow",           683),
    ("minecraft:coal",            684),
    ("minecraft:diamond",         686),
    ("minecraft:emerald",         687),
    ("minecraft:iron_ingot",      692),
    ("minecraft:gold_ingot",      696),
    ("minecraft:iron_sword",      719),
    ("minecraft:iron_pickaxe",    721),
    ("minecraft:diamond_sword",   724),
    ("minecraft:diamond_shovel",  725),
    ("minecraft:diamond_pickaxe", 726),
    ("minecraft:diamond_axe",     727),
    ("minecraft:stick",           734),
    ("minecraft:string",          736),
    ("minecraft:feather",         737),
    ("minecraft:wheat",           740),
    ("minecraft:bread",           741),
    ("minecraft:flint",           770),
    ("minecraft:golden_apple",    774),
    ("minecraft:bucket",          800),
    ("minecraft:water_bucket",    801),
];

/// `minecraft:air` — the empty/sentinel item id. An `ItemStack` with this id (or
/// a count of zero) is treated as empty by the network codec.
#[allow(dead_code)] // scaffolding: the empty/sentinel id, for callers building stacks.
pub const AIR: i32 = 0;

/// Look up an item's numeric id by `namespace:path`. A bare `path` (no `:`) is
/// assumed to be in the `minecraft` namespace, matching `Identifier` parsing.
#[allow(dead_code)] // scaffolding: a lookup API for callers that build stacks by name.
pub fn id_of(name: &str) -> Option<i32> {
    let owned;
    let full = if name.contains(':') {
        name
    } else {
        owned = format!("minecraft:{name}");
        &owned
    };
    ITEMS.iter().find(|(n, _)| *n == full).map(|(_, id)| *id)
}

/// Reverse lookup: numeric id → `namespace:path`, if present in the scaffold.
#[allow(dead_code)] // scaffolding: paired reverse lookup for debugging/printing.
pub fn name_of(id: i32) -> Option<&'static str> {
    ITEMS.iter().find(|(_, i)| *i == id).map(|(n, _)| *n)
}

// ---------------------------------------------------------------------------
// ItemStack + the 26.2 network codec.
// ---------------------------------------------------------------------------

/// A stack of items. Data components are not modelled yet — on the wire we always
/// emit an empty `DataComponentPatch`. Emptiness is represented out-of-band as
/// `Option<ItemStack>` / `None` (the codec maps that to a zero count); a present
/// `ItemStack` always has `count >= 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemStack {
    /// Numeric item id (`BuiltInRegistries.ITEM` index).
    pub id: i32,
    /// Stack size (`>= 1` for a present stack).
    pub count: i32,
}

impl ItemStack {
    #[allow(dead_code)] // scaffolding: convenience constructor for future callers/tests.
    pub fn new(id: i32, count: i32) -> Self {
        Self { id, count }
    }
}

/// Write an `Option<ItemStack>` using the 26.2 `ItemStack.OPTIONAL_STREAM_CODEC`
/// layout (`createOptionalStreamCodec`):
///
/// * empty (`None`, or a count `<= 0`) → VarInt `0` and nothing else;
/// * otherwise → VarInt `count`, then the item id (VarInt, via the item
///   `holderRegistry` codec), then the `DataComponentPatch`. An empty patch is
///   VarInt `0` (components to add) + VarInt `0` (components to remove), per
///   `DataComponentPatch.createStreamCodec`.
pub fn write_item_stack(p: &mut PacketWriter, stack: Option<&ItemStack>) {
    match stack {
        Some(s) if s.count > 0 => {
            p.write_varint(s.count);
            p.write_varint(s.id);
            // Empty DataComponentPatch: zero added, zero removed.
            p.write_varint(0);
            p.write_varint(0);
        }
        // None, or a non-positive count: the empty encoding is a single VarInt 0.
        _ => p.write_varint(0),
    }
}

/// Read an `Option<ItemStack>`, the inverse of [`write_item_stack`]. A leading
/// count `<= 0` yields `None` (`ItemStack.EMPTY`); otherwise the item id and the
/// `DataComponentPatch` follow. We do not model components, so only the empty
/// patch (the form the vanilla creative client sends for a bare item) is
/// accepted — a non-empty patch is reported as a decode error rather than
/// silently desyncing the buffer.
pub fn read_item_stack(r: &mut PacketReader) -> std::io::Result<Option<ItemStack>> {
    let count = r.read_varint()?;
    if count <= 0 {
        return Ok(None);
    }
    let id = r.read_varint()?;
    let added = r.read_varint()?;
    let removed = r.read_varint()?;
    if added != 0 || removed != 0 {
        // We can't decode component bodies yet; refuse rather than desync. The
        // frame is its own buffer, so dropping this packet is safe.
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "item stack carries data components (unsupported)",
        ));
    }
    Ok(Some(ItemStack { id, count }))
}

// ---------------------------------------------------------------------------
// Player inventory container.
// ---------------------------------------------------------------------------

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
}

impl Inventory {
    /// A fresh, empty inventory with the first hotbar slot selected.
    pub fn new() -> Self {
        Self {
            slots: [None; PLAYER_INVENTORY_SLOTS],
            selected: 0,
        }
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

// ---------------------------------------------------------------------------
// Clientbound packet builders.
// ---------------------------------------------------------------------------

/// `ClientboundContainerSetContentPacket` — overwrite a whole container's
/// contents. Layout (`StreamCodec.composite`):
///
/// * `containerId` — VarInt (`ByteBufCodecs.CONTAINER_ID` → `writeContainerId` →
///   `VarInt.write`; for the player inventory this is 0);
/// * `stateId` — VarInt;
/// * `items` — `ItemStack.OPTIONAL_LIST_STREAM_CODEC`: VarInt length then each
///   slot via the optional `ItemStack` codec;
/// * `carriedItem` — a single optional `ItemStack` (the cursor item).
#[allow(dead_code)] // scaffolding: a builder the sim will use once it pushes real contents.
pub fn container_set_content(
    window_id: i32,
    state_id: i32,
    items: &[Option<ItemStack>],
    carried: Option<&ItemStack>,
) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(window_id);
    p.write_varint(state_id);
    p.write_varint(items.len() as i32);
    for slot in items {
        write_item_stack(&mut p, slot.as_ref());
    }
    write_item_stack(&mut p, carried);
    frame(CB_PLAY_CONTAINER_SET_CONTENT, &p.buf)
}

/// `ClientboundSetHeldSlotPacket` — tell the client which hotbar slot (0..8) is
/// selected. A single VarInt (`ByteBufCodecs.VAR_INT`).
#[allow(dead_code)] // scaffolding: counterpart to serverbound SetCarriedItem.
pub fn set_held_slot(slot: i32) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(slot);
    frame(CB_PLAY_SET_HELD_SLOT, &p.buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strip the `len|id` frame header and return `(id, reader-at-body)`.
    fn unframe(bytes: Bytes) -> (i32, PacketReader) {
        let mut r = PacketReader::new(bytes);
        let _len = r.read_varint().unwrap();
        let id = r.read_varint().unwrap();
        (id, r)
    }

    #[test]
    fn registry_lookups() {
        assert_eq!(id_of("minecraft:air"), Some(0));
        assert_eq!(id_of("stone"), Some(1)); // bare path defaults to minecraft:
        assert_eq!(id_of("minecraft:grass_block"), Some(54));
        assert_eq!(id_of("minecraft:diamond_sword"), Some(724));
        assert_eq!(id_of("minecraft:not_a_real_item"), None);
        assert_eq!(name_of(0), Some("minecraft:air"));
        assert_eq!(name_of(686), Some("minecraft:diamond"));
        assert_eq!(name_of(1_000_000), None);
    }

    #[test]
    fn item_stack_round_trip_present() {
        let mut p = PacketWriter::new();
        let stack = ItemStack::new(724, 3); // diamond_sword x3
        write_item_stack(&mut p, Some(&stack));
        // count(1) + id(varint 724 = 2 bytes) + patch(0,0) = 1 + 2 + 2 = 5 bytes.
        assert_eq!(p.buf.len(), 5);
        let mut r = PacketReader::new(p.buf.freeze());
        assert_eq!(read_item_stack(&mut r).unwrap(), Some(stack));
    }

    #[test]
    fn item_stack_round_trip_empty() {
        let mut p = PacketWriter::new();
        write_item_stack(&mut p, None);
        assert_eq!(&p.buf[..], &[0u8]); // a single VarInt 0
        let mut r = PacketReader::new(p.buf.freeze());
        assert_eq!(read_item_stack(&mut r).unwrap(), None);
    }

    #[test]
    fn non_positive_count_encodes_empty() {
        let mut p = PacketWriter::new();
        write_item_stack(&mut p, Some(&ItemStack::new(1, 0)));
        assert_eq!(&p.buf[..], &[0u8]);
    }

    #[test]
    fn container_set_content_layout() {
        let items = [Some(ItemStack::new(1, 64)), None, Some(ItemStack::new(686, 1))];
        let carried = ItemStack::new(724, 1);
        let (id, mut r) = unframe(container_set_content(0, 5, &items, Some(&carried)));
        assert_eq!(id, CB_PLAY_CONTAINER_SET_CONTENT);
        assert_eq!(r.read_varint().unwrap(), 0); // window id
        assert_eq!(r.read_varint().unwrap(), 5); // state id
        assert_eq!(r.read_varint().unwrap(), 3); // item count
        assert_eq!(read_item_stack(&mut r).unwrap(), Some(ItemStack::new(1, 64)));
        assert_eq!(read_item_stack(&mut r).unwrap(), None);
        assert_eq!(read_item_stack(&mut r).unwrap(), Some(ItemStack::new(686, 1)));
        assert_eq!(read_item_stack(&mut r).unwrap(), Some(carried)); // carried/cursor
    }

    #[test]
    fn set_held_slot_layout() {
        let (id, mut r) = unframe(set_held_slot(4));
        assert_eq!(id, CB_PLAY_SET_HELD_SLOT);
        assert_eq!(r.read_varint().unwrap(), 4);
    }

    #[test]
    fn inventory_set_slot_bounds() {
        let mut inv = Inventory::new();
        assert!(inv.set_slot(36, Some(ItemStack::new(1, 1))));
        assert_eq!(inv.slots[36], Some(ItemStack::new(1, 1)));
        assert!(!inv.set_slot(46, Some(ItemStack::new(1, 1)))); // out of range
        assert!(!inv.set_slot(-1, None));
    }
}
