//! Dynamic chunk streaming: keep each player's loaded-chunk set following them,
//! mirroring vanilla's `ChunkMap`/`PlayerChunkSender`. Runs as a system after
//! movement is applied. Also exposes the `ChunkTrackingView` membership predicate
//! used by the join path to seed a newcomer's loaded set.

use bevy_ecs::prelude::*;

use super::bridge::Outbound;
use super::components::*;
use super::packets;

/// A chunk column coordinate `(cx, cz)`, used by the chunk-streaming diff.
type ChunkCoord = (i32, i32);

/// Dynamic chunk streaming: keep each player's loaded-chunk set following them,
/// mirroring vanilla's `ChunkMap`/`PlayerChunkSender`. Runs after
/// `broadcast_movement` so it sees the position `drain_ingress` applied this
/// tick. Per-player (each player streams to its *own* outbox), so a single
/// mutable `Query` suffices — no exclusive-`World` access needed.
///
/// Each tick, compute the player's chunk center `(floor(x)>>4, floor(z)>>4)`. If
/// it is unchanged, do nothing. Otherwise send `SetChunkCacheCenter`, then diff
/// the new view-distance square against the old one: stream `level_chunk` for
/// newly-in-range columns (nearest-first, like vanilla's distance ordering) and
/// `forget_chunk` for columns that left range, updating the `LoadedChunks` set.
pub fn stream_chunks(config: Res<Config>, mut players: Query<(&Pos, &Conn, &mut LoadedChunks)>) {
    let radius = config.0.properties.view_distance();
    for (pos, conn, mut loaded) in players.iter_mut() {
        let center = ((pos.x.floor() as i32) >> 4, (pos.z.floor() as i32) >> 4);
        if center == loaded.center {
            continue;
        }
        let (added, removed) = chunk_diff(loaded.center, center, radius);

        let _ = conn
            .outbox
            .try_send(Outbound::Packet(packets::set_chunk_center(
                center.0, center.1,
            )));
        for &(cx, cz) in &added {
            let _ = conn
                .outbox
                .try_send(Outbound::Packet(packets::level_chunk(cx, cz)));
            loaded.loaded.insert((cx, cz));
        }
        for &(cx, cz) in &removed {
            let _ = conn
                .outbox
                .try_send(Outbound::Packet(packets::forget_chunk(cx, cz)));
            loaded.loaded.remove(&(cx, cz));
        }
        loaded.center = center;
    }
}

/// Vanilla `ChunkTrackingView` membership with `includeNeighbors = true`
/// (`bufferRange = 2`, `ChunkTrackingView.isWithinDistance`): a chunk `(x, z)` is
/// tracked by a player centered at `center` with server view-distance `radius`
/// iff `max(0, |dx|-2)² + max(0, |dz|-2)² < radius²`. This reaches `radius+1` on
/// the axes and rounds the far corners off — the exact shape vanilla streams,
/// which is neither the plain `|dx|≤R ∧ |dz|≤R` square (it misses the `R+1` ring
/// and over-sends corners) nor a circle. The enclosing bounding box is
/// `center ± (radius+1)`.
pub(super) fn in_view(center: ChunkCoord, x: i32, z: i32, radius: i32) -> bool {
    let dx = ((x - center.0).abs() - 2).max(0) as i64;
    let dz = ((z - center.1).abs() - 2).max(0) as i64;
    dx * dx + dz * dz < (radius as i64) * (radius as i64)
}

/// Pure diff between two rounded `ChunkTrackingView` regions (see [`in_view`]).
/// Returns `(added, removed)`: columns tracked from `new` but not `old` (added),
/// and tracked from `old` but not `new` (removed). `added` is ordered
/// nearest-first by *squared Euclidean* chunk distance to `new`, matching
/// `PlayerChunkSender.collectChunksToSend`'s `playerPos.distanceSquared` sort.
fn chunk_diff(old: ChunkCoord, new: ChunkCoord, radius: i32) -> (Vec<ChunkCoord>, Vec<ChunkCoord>) {
    let mut added = Vec::new();
    for x in (new.0 - radius - 1)..=(new.0 + radius + 1) {
        for z in (new.1 - radius - 1)..=(new.1 + radius + 1) {
            if in_view(new, x, z, radius) && !in_view(old, x, z, radius) {
                added.push((x, z));
            }
        }
    }
    added.sort_by_key(|&(x, z)| {
        let dx = (x - new.0) as i64;
        let dz = (z - new.1) as i64;
        dx * dx + dz * dz
    });

    let mut removed = Vec::new();
    for x in (old.0 - radius - 1)..=(old.0 + radius + 1) {
        for z in (old.1 - radius - 1)..=(old.1 + radius + 1) {
            if in_view(old, x, z, radius) && !in_view(new, x, z, radius) {
                removed.push((x, z));
            }
        }
    }

    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::{chunk_diff, in_view};
    use std::collections::HashSet;

    /// The rounded `ChunkTrackingView` region around a center — the same
    /// predicate (`in_view`) the production diff uses, enumerated over its
    /// bounding box `center ± (radius+1)`.
    fn view_set(center: (i32, i32), radius: i32) -> HashSet<(i32, i32)> {
        let mut s = HashSet::new();
        for x in (center.0 - radius - 1)..=(center.0 + radius + 1) {
            for z in (center.1 - radius - 1)..=(center.1 + radius + 1) {
                if in_view(center, x, z, radius) {
                    s.insert((x, z));
                }
            }
        }
        s
    }

    #[test]
    fn view_reaches_axis_plus_one_and_rounds_corners() {
        // bufferRange=2: on-axis a chunk is in view out to radius+1, but the far
        // corner is rounded off. Use a realistic view distance where the rounding
        // is visible: max(0,7)²+max(0,7)² = 98 ≥ 64, so (9,9) is clipped at R=8.
        let radius = 8;
        assert!(in_view((0, 0), radius + 1, 0, radius)); // axis: reaches R+1
        assert!(!in_view((0, 0), radius + 2, 0, radius)); // but not R+2
        assert!(!in_view((0, 0), radius + 1, radius + 1, radius)); // corner clipped
    }

    #[test]
    fn diff_no_move_is_empty() {
        let (added, removed) = chunk_diff((0, 0), (0, 0), 3);
        assert!(added.is_empty());
        assert!(removed.is_empty());
    }

    #[test]
    fn diff_single_step_is_symmetric_and_consistent() {
        // Moving one chunk in +x: added are exactly the columns newly in view and
        // removed exactly those that left, and (by the shape's symmetry) the two
        // sets have equal size.
        let radius = 3;
        let (added, removed) = chunk_diff((0, 0), (1, 0), radius);
        assert!(!added.is_empty());
        assert_eq!(added.len(), removed.len());
        // Added are all leading-edge (x > 0 side), removed all trailing-edge.
        assert!(added.iter().all(|&(x, _)| x > 0));
        assert!(removed.iter().all(|&(x, _)| x <= 0));
    }

    #[test]
    fn diff_matches_set_difference() {
        // The diff must equal the set-theoretic difference of the two rounded
        // view regions, for an arbitrary jump that partially overlaps.
        let old = (2, -1);
        let new = (4, 1);
        let radius = 3;
        let (added, removed) = chunk_diff(old, new, radius);

        let old_v = view_set(old, radius);
        let new_v = view_set(new, radius);
        let expect_added: HashSet<_> = new_v.difference(&old_v).copied().collect();
        let expect_removed: HashSet<_> = old_v.difference(&new_v).copied().collect();

        assert_eq!(added.iter().copied().collect::<HashSet<_>>(), expect_added);
        assert_eq!(
            removed.iter().copied().collect::<HashSet<_>>(),
            expect_removed
        );
    }

    #[test]
    fn diff_disjoint_jump_swaps_whole_regions() {
        // A jump farther than the diameter shares no chunks: the whole old region
        // is forgotten and the whole new region is loaded.
        let radius = 2;
        let (added, removed) = chunk_diff((0, 0), (100, 100), radius);
        let area = view_set((0, 0), radius).len();
        assert_eq!(added.len(), area);
        assert_eq!(removed.len(), area);
    }

    #[test]
    fn diff_added_is_nearest_first() {
        // Added chunks are ordered by squared Euclidean distance to the new
        // center, matching PlayerChunkSender's distanceSquared sort.
        let (added, _) = chunk_diff((0, 0), (5, 0), 3);
        let dist = |&(x, z): &(i32, i32)| ((x - 5) * (x - 5) + z * z) as i64;
        for w in added.windows(2) {
            assert!(dist(&w[0]) <= dist(&w[1]));
        }
    }
}
