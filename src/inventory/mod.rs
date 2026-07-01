//! Inventory & item-registry scaffolding.
//!
//! A self-contained data model for items and slots, split into focused modules:
//!
//! * [`item_stack`] — the `ItemStack` value type and its 26.2 network codec;
//! * [`container`] — the player-inventory container (`Inventory` component);
//! * [`packets`] — the clientbound inventory packet builders.
//!
//! The numeric item-id table lives one level up in [`crate::registry::item`]
//! alongside the other registries, rather than here.
//!
//! This is greenfield: it owns its own `ItemStack` codec and the inventory
//! packets we currently speak. Wire formats are taken 1:1 from the decompiled
//! 26.2 reference — see the per-module citations. Nothing here copies Mojang
//! code; the layouts are transcribed from the `StreamCodec` definitions.
//!
//! **Packet placement.** Unlike the player/movement/chat builders in
//! `sim::packets`, this module owns *its own* inventory packet builders and ids
//! (in [`packets`]). That is deliberate: inventory/containers are a
//! self-contained domain that will keep growing (menu types, clicks, recipes), so
//! the `ItemStack` codec, the registry, and the packets that carry them live
//! together rather than leaking into the shared packet module. `sim::packets`
//! notes the same split.

mod container;
mod item_stack;
mod menu;
mod packets;
mod persistence;

pub use container::{Inventory, HOTBAR_START, PLAYER_INVENTORY_SLOTS};
pub use item_stack::{read_item_stack, ItemStack};
pub use menu::{ClickType, Menu, OpenContainer};
pub use packets::container_set_content;
pub use persistence::{inventory_from_nbt, inventory_to_nbt};

// Scaffolding surface: re-exported for a complete public API but not yet consumed
// elsewhere in the crate. The underlying items carry their own `dead_code` allows;
// silence the matching unused-re-export warnings here.
#[allow(unused_imports)]
pub use item_stack::write_item_stack;
#[allow(unused_imports)]
pub use packets::{container_close, container_set_slot, open_screen, set_held_slot};
