//! The command-suggestion engine: answers `ServerboundCommandSuggestionPacket`
//! for the argument nodes we flagged `minecraft:ask_server` in [`tree`](super::tree).

use bevy_ecs::prelude::*;

use crate::sim::components::Profile;

/// Answer a `ServerboundCommandSuggestionPacket`: compute completions for the
/// partial `command` line (including its leading `/`). Returns the `StringRange`
/// (`start`, `length`) the suggestions replace and the matching strings.
///
/// We complete player names for the player-target argument of `/tell`, `/msg`,
/// `/w`, and `/gamemode` (the nodes we flagged `ask_server`), and game-mode
/// keywords for `/gamemode`'s first argument — matched case-insensitively by
/// prefix, as `SharedSuggestionProvider.suggest` does.
pub fn suggest(world: &mut World, command: &str) -> (i32, i32, Vec<String>) {
    // The word under the cursor is the suffix after the last whitespace.
    let trimmed_end = command.trim_end_matches(char::is_whitespace);
    let has_trailing_space = trimmed_end.len() != command.len();
    let partial = if has_trailing_space {
        ""
    } else {
        command.rsplit(char::is_whitespace).next().unwrap_or("")
    };
    // Brigadier `StringRange` indices are string (char) positions, not UTF-8
    // byte offsets, so count chars — a multi-byte char earlier in the line must
    // not shift the replacement range the client applies.
    let length = partial.chars().count();
    let start = command.chars().count() - length;

    let parts: Vec<&str> = command.split_whitespace().collect();
    let root = parts.first().copied().unwrap_or("").trim_start_matches('/');
    // Argument index 0 is the command word itself.
    let arg_index = if has_trailing_space {
        parts.len()
    } else {
        parts.len().saturating_sub(1)
    };

    let matches = |candidates: Vec<String>| -> Vec<String> {
        let needle = partial.to_lowercase();
        candidates
            .into_iter()
            .filter(|c| c.to_lowercase().starts_with(&needle))
            .collect()
    };

    let suggestions = match (root, arg_index) {
        ("tell" | "msg" | "w", 1) => matches(online_names(world)),
        ("gamemode", 1) => matches(
            ["survival", "creative", "adventure", "spectator"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        ),
        ("gamemode", 2) => matches(online_names(world)),
        _ => Vec::new(),
    };

    (start as i32, length as i32, suggestions)
}

/// The names of all online players.
fn online_names(world: &mut World) -> Vec<String> {
    let mut q = world.query::<&Profile>();
    q.iter(world).map(|p| p.name.clone()).collect()
}
