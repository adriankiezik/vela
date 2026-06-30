//! Item registry — a hand-curated subset of `BuiltInRegistries.ITEM`.

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
