//! Domain newtypes for the two mutually-confusable integer ids that flow across
//! the registry, world, inventory and simulation layers.
//!
//! Item ids (`BuiltInRegistries.ITEM` indices) and block-state ids (global
//! `Block.BLOCK_STATE_REGISTRY` palette ids) are both bare integers, and there is
//! a real conversion between them ([`crate::world::block_state_for_item`]).
//! Passing an item id where a block-state is expected — or vice-versa — is a
//! plausible bug the compiler otherwise can't catch, since the two are
//! structurally identical ints flowing through `set_block`, `block_update`,
//! `ItemStack.id`, the section palette, and so on. Wrapping each in its own type
//! makes that conversion explicit and un-swappable at zero runtime cost
//! (`#[repr(transparent)]`; every method is trivial).
//!
//! These are deliberately the *only* two ids we newtype. Locally-scoped,
//! non-confusable ids — entity ids, container slots, sequences, the
//! `entity_type`/`menu`/`block` registry ids — stay bare integers: wrapping them
//! would add ceremony without preventing a real bug.

/// A numeric item id — an index into the `BuiltInRegistries.ITEM` table
/// (`registry::item`). This is what an `ItemStack` carries and what the item
/// `StreamCodec` writes on the wire as a VarInt.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ItemId(pub i32);

/// A global block-state palette id — the value the client decodes via
/// `Block.BLOCK_STATE_REGISTRY.byId`. Distinct from [`ItemId`]:
/// [`crate::world::block_state_for_item`] is the only bridge between the two.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BlockState(pub u32);

impl ItemId {
    /// The raw registry index, for the wire codec and table lookups.
    pub const fn get(self) -> i32 {
        self.0
    }
}

impl BlockState {
    /// The raw palette id, for the wire codec and the section bit-packer.
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl From<i32> for ItemId {
    fn from(id: i32) -> Self {
        ItemId(id)
    }
}

impl From<ItemId> for i32 {
    fn from(id: ItemId) -> Self {
        id.0
    }
}

impl From<u32> for BlockState {
    fn from(state: u32) -> Self {
        BlockState(state)
    }
}

impl From<BlockState> for u32 {
    fn from(state: BlockState) -> Self {
        state.0
    }
}
