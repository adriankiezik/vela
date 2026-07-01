//! Non-player entities: the ECS components, the spawn API, and the tracking that
//! makes them appear on and disappear from nearby clients.
//!
//! Players are still their own thing (they carry `Profile`/`Tracking` and a
//! connection). Everything else in the world — dropped items, XP orbs, and the
//! mobs/projectiles that will follow — is a *net entity*: a numeric id, a UUID, a
//! type, a position, and a [`syncher::EntityData`] metadata set. This module owns
//! that model and the wire path to viewers, generalizing the previously
//! player-only spawn/track code.
//!
//! Tracking mirrors vanilla's `ChunkMap.TrackedEntity` / `ServerEntity`: each net
//! entity carries a [`Tracked`] set of the player entities that currently have it
//! paired (vanilla `TrackedEntity.seenBy`). Every tick [`update_entity_tracking`]
//! reconciles that set against each player's position and view — a player entering
//! range is *paired* (`ServerEntity.addPairing`: `ClientboundAddEntityPacket` then
//! `ClientboundSetEntityDataPacket`, see `sendPairingData`) and one leaving range
//! is *unpaired* (`ServerEntity.removePairing`: `ClientboundRemoveEntitiesPacket`).
//! This closes the pre-fix leak where an entity was sent only to the viewers
//! present at spawn time and never removed as a player travelled away.
//!
//! The visibility predicate is vanilla's `TrackedEntity.updatePlayer`: the
//! horizontal (x/z) distance-squared to the player is compared to
//! `min(clientTrackingRange*16, playerViewDistance*16)²`, *and* the player must
//! have the entity's column tracked. Vela mirrors `ChunkMap.isChunkTracked`
//! (`getChunkTrackingView().contains(x,z) && !chunkSender.isPending(...)`) with
//! [`LoadedChunks`] membership minus the [`ChunkSender`]'s still-pending columns,
//! so a loaded-but-not-yet-streamed column does not spawn entities early. All
//! per-entity clientbound fan-out — spawn,
//! movement deltas, metadata, hurt-animation, despawn — is routed through exactly
//! this [`Tracked::seen_by`] set (see [`fan_to_seen`]), so a player is fed an
//! entity iff it is spawned on their client (no spawned-but-unfed or fed-but-
//! unspawned desync).
//!
//! Documented deviation from vanilla: Vela has no per-entity `ServerEntity`
//! position-codec base shared across viewers — the delta base lives in each mob's
//! `MobState` / item's `ItemPhysics`. A viewer entering range therefore receives an
//! `AddEntity` at the entity's *current* `Pos` rather than the tracker base; for a
//! moving entity the two differ by at most one tick of sub-block motion, which the
//! next delta/absolute-sync corrects. Player↔player tracking is still handled by
//! the player-specific path (`movement::broadcast_movement` and the join/leave
//! pairing in `player_lifecycle`), not this system — see that code for why (player
//! count is bounded, so the unbounded-leak concern this fix addresses does not
//! apply; bringing players into the unified tracker is a documented follow-up).

pub mod packets;
pub mod syncher;

use std::collections::{HashMap, HashSet};

use bevy_ecs::prelude::*;
use bytes::Bytes;
use rand::RngCore;
use uuid::Uuid;

use super::bridge::{Outbound, OutboxTx};
use super::components::*;
use crate::inventory::ItemStack;
use syncher::{DataValue, EntityData};

// --- Entity-type registry ids (see `registry::builtin::ENTITY_TYPE`) ----------
// Hard-coded (like the player's id in `sim::packets`) with a test below pinning
// them to the registry so a reordering is caught.
//
// The item path is now live: breaking a block in survival spawns a
// `minecraft:item` via `spawn_item_entity` (see `sim::packet_handlers`). The XP
// orb path still has no in-game trigger (mob/ore XP lands later), so its spawn
// helper and constant read as dead in a non-test build.
/// `minecraft:item` — the dropped-item entity type.
const ENTITY_TYPE_ITEM: i32 = 71;
/// `minecraft:experience_orb`.
#[allow(dead_code)]
const ENTITY_TYPE_EXPERIENCE_ORB: i32 = 49;

// --- Metadata accessor indices ------------------------------------------------
// `SynchedEntityData.defineId` assigns indices in class-hierarchy order; `Entity`
// itself occupies 0..=7, so the first field of a direct `Entity` subclass is 8.
/// `ItemEntity.DATA_ITEM` — the carried `ItemStack` (accessor index 8).
const ITEM_ENTITY_DATA_ITEM: u8 = 8;
/// `ExperienceOrb.DATA_VALUE` — the orb's XP amount (accessor index 8).
#[allow(dead_code)]
const EXPERIENCE_ORB_DATA_VALUE: u8 = 8;

// --- LivingEntity / Mob metadata accessor indices ----------------------------
// `SynchedEntityData.defineId` continues numbering down the class hierarchy:
// `Entity` occupies 0..=7 (shared flags, air, custom-name, silent, no-gravity,
// pose, ticks-frozen), so `LivingEntity`'s first field is 8 and `Mob`'s is 15.
// Only the indices the mob spawn path actually emits are named here.
/// `LivingEntity.DATA_LIVING_ENTITY_FLAGS` — the living-entity flags byte (index 8).
#[allow(dead_code)]
pub const LIVING_ENTITY_DATA_FLAGS: u8 = 8;
/// `LivingEntity.DATA_HEALTH_ID` — the current-health float (index 9). The client
/// registers a default of `1.0`, so this must be sent for a full-health render.
pub const LIVING_ENTITY_DATA_HEALTH: u8 = 9;
/// `Mob.DATA_MOB_FLAGS_ID` — the mob flags byte (index 15); bit 0 is the "no AI"
/// flag (`Mob.setNoAi`).
#[allow(dead_code)]
pub const MOB_DATA_FLAGS: u8 = 15;
/// `AgeableMob.DATA_BABY_ID` — the baby flag (index 16, BOOLEAN). The client
/// registers a default of `false`, so it is only emitted for an actual baby.
pub const AGEABLE_MOB_DATA_BABY: u8 = 16;
/// `Sheep.DATA_WOOL_ID` — the wool byte (index 18): low nibble is the `DyeColor`
/// id, bit 0x10 is the sheared flag. `Entity` 0..=7, `LivingEntity` 8..=14,
/// `Mob` 15, `AgeableMob` 16..=17 (baby, age-locked), then `Sheep`'s first field.
pub const SHEEP_DATA_WOOL: u8 = 18;
/// `Entity.DATA_SHARED_FLAGS_ID` — the shared flags byte (index 0), shared by
/// every entity. Sent as `0` for a plain standing mob.
pub const ENTITY_DATA_SHARED_FLAGS: u8 = 0;

/// A world entity that is not a player: the identity the client needs to spawn
/// and address it. Players carry `Profile` instead (their numeric id lives there).
#[derive(Component)]
pub struct NetEntity {
    /// Network entity id (shared id space with players; from [`NextEntityId`]).
    pub id: i32,
    /// Stable UUID sent in `AddEntity`.
    pub uuid: Uuid,
    /// `entity_type` registry id.
    pub type_id: i32,
}

/// The entity's synchronized metadata (`SynchedEntityData`), sent after its
/// `AddEntity` and resent on change.
#[derive(Component)]
pub struct EntityMeta(pub EntityData);

/// Per-entity tracking state, mirroring `ChunkMap.TrackedEntity`: the set of
/// player ECS entities that currently have this entity paired on their client
/// (`TrackedEntity.seenBy`), and the un-clamped tracking range in blocks
/// (`EntityType.clientTrackingRange() * 16`, vanilla `TrackedEntity.range`).
///
/// All per-entity clientbound fan-out targets exactly `seen_by`; the set is
/// reconciled each tick by [`update_entity_tracking`].
#[derive(Component)]
pub struct Tracked {
    /// `ChunkMap.TrackedEntity.seenBy` — player entities this is spawned to.
    pub seen_by: HashSet<Entity>,
    /// `EntityType.clientTrackingRange() * 16` in blocks (vanilla `range`).
    pub range: f64,
}

/// `EntityType.clientTrackingRange()` in **chunks** for the types Vela spawns,
/// transcribed 1:1 from `EntityTypes` (MC 26.2): pig/cow/sheep/chicken `10`,
/// item/experience_orb `6`, player `32`. Anything else falls back to the
/// `EntityType.Builder` default of `5`.
fn client_tracking_range_chunks(type_id: i32) -> i32 {
    use crate::registry::builtin::ENTITY_TYPE;
    let is = |name: &str| ENTITY_TYPE.id_of(name) == Some(type_id);
    if is("minecraft:player") {
        32
    } else if is("minecraft:pig")
        || is("minecraft:cow")
        || is("minecraft:sheep")
        || is("minecraft:chicken")
    {
        10
    } else if is("minecraft:item") || is("minecraft:experience_orb") {
        6
    } else {
        5
    }
}

/// A viewer snapshot for the tracking pass: a player's position, tracked columns,
/// still-pending columns, view-distance cap, and outbox. Only players match
/// `(&Pos, &LoadedChunks, &Conn, &ChunkSender, &RequestedViewDistance)` — net
/// entities carry `Pos` but none of the others.
struct Viewer {
    entity: Entity,
    x: f64,
    z: f64,
    loaded: HashSet<(i32, i32)>,
    /// Columns in `LoadedChunks` that are still queued for streaming and have not
    /// yet been sent to the client (`ChunkSender.pending`); a column here is
    /// loaded-but-not-yet-tracked and must not count for entity visibility.
    pending: HashSet<(i32, i32)>,
    /// This viewer's `getPlayerViewDistance` cap on the visible range, in blocks
    /// (`clamp(requestedViewDistance, 2, serverViewDistance) * 16`). Per player, so
    /// a client with a smaller render distance sees a correspondingly smaller entity
    /// range. Unbounded in unit-test worlds with no [`Config`].
    view: f64,
    outbox: OutboxTx,
}

fn snapshot_viewers(world: &mut World) -> Vec<Viewer> {
    // `ChunkMap.getPlayerViewDistance` clamps *per player*, so each viewer carries its
    // own view range. Read the server distance once (unbounded when no `Config`, i.e.
    // in unit-test worlds — column membership still gates visibility there).
    let server_view_distance = world.get_resource::<Config>().map(|c| c.0.properties.view_distance());
    // `RequestedViewDistance` is queried optionally: a real player always carries it
    // (attached at join), but tracking fixtures in sibling modules spawn bare viewers.
    // An absent request falls back to the vanilla default (which then clamps to 2).
    let mut q = world.query::<(
        Entity,
        &Pos,
        &LoadedChunks,
        &Conn,
        &ChunkSender,
        Option<&RequestedViewDistance>,
    )>();
    q.iter(world)
        .map(|(entity, p, l, c, sender, requested)| Viewer {
            entity,
            x: p.x,
            z: p.z,
            loaded: l.loaded.clone(),
            pending: sender.pending.iter().copied().collect(),
            view: match server_view_distance {
                Some(svd) => {
                    let r = requested.copied().unwrap_or(RequestedViewDistance(RequestedViewDistance::DEFAULT));
                    (r.clamped(svd) as f64) * 16.0
                }
                None => f64::INFINITY,
            },
            outbox: c.outbox.clone(),
        })
        .collect()
}

/// `ChunkMap.TrackedEntity.updatePlayer`'s visibility test: horizontal
/// distance-squared within `min(range, viewDistance)²` **and** the entity's column
/// tracked by the player. The `viewDistance` cap is the viewer's own
/// `getPlayerViewDistance` (`Viewer::view`). Vanilla `isChunkTracked` is
/// `getChunkTrackingView().contains(x, z) && !chunkSender.isPending(pack(x, z))`, so a
/// column that is loaded but still *pending* (queued, bytes not yet sent) does not count.
fn is_visible(v: &Viewer, ex: f64, ez: f64, chunk: (i32, i32), range: f64) -> bool {
    let visible_range = range.min(v.view);
    let dx = v.x - ex;
    let dz = v.z - ez;
    dx * dx + dz * dz <= visible_range * visible_range
        && v.loaded.contains(&chunk)
        && !v.pending.contains(&chunk)
}

/// Marker for dropped-item entities (`minecraft:item`). Inserted by
/// [`spawn_item_entity`] on every survival block break; the item-pickup system
/// (another module) queries `(&NetEntity, &Pos, &ItemDrop, &EntityMeta)` to find
/// stacks to hand to a nearby player.
#[derive(Component)]
pub struct ItemDrop;

/// Marker for XP orbs (`minecraft:experience_orb`).
#[derive(Component)]
#[allow(dead_code)] // classifies spawned orbs; queried once gameplay (pickup) exists.
pub struct XpOrb;

/// The chunk column `(cx, cz)` a world position sits in
/// (`floor(x)>>4, floor(z)>>4`), the key used for per-viewer culling.
fn chunk_of(x: f64, z: f64) -> (i32, i32) {
    ((x.floor() as i32) >> 4, (z.floor() as i32) >> 4)
}

/// Allocate the next network entity id (shared with players).
fn alloc_id(world: &mut World) -> i32 {
    let mut next = world.resource_mut::<NextEntityId>();
    let v = next.0;
    next.0 += 1;
    v
}

/// A fresh random entity UUID. Non-player entities have no account identity, so
/// any unique value works (vanilla uses `Mth.createInsecureUUID`).
fn random_uuid() -> Uuid {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    Uuid::from_bytes(bytes)
}

/// Spawn a dropped-item entity carrying `stack` at `(x, y, z)` and show it to
/// every player tracking that column. Returns the network entity id.
///
/// The metadata mirrors `ItemEntity`: `DATA_ITEM` (index 8) holds the stack.
/// Pickup and physics are not modelled yet — the item renders and sits where it
/// spawned.
///
/// Live trigger: `sim::packet_handlers` calls this for each stack returned by
/// `world::block_drop::drops_for` when a survival player finishes breaking a block.
pub fn spawn_item_entity(world: &mut World, pos: (f64, f64, f64), stack: ItemStack) -> i32 {
    let id = alloc_id(world);
    let uuid = random_uuid();
    let mut meta = EntityData::new();
    meta.set(ITEM_ENTITY_DATA_ITEM, DataValue::ItemStack(Some(stack)));

    spawn_tracked(
        world,
        NetEntity { id, uuid, type_id: ENTITY_TYPE_ITEM },
        pos,
        0.0,
        meta,
        |ec| {
            ec.insert(ItemDrop);
        },
    );
    id
}

/// Spawn an arbitrary net entity of `type_id` at `pos` facing `yaw`, showing it to
/// every player tracking that column, and run `tag` to attach any type-specific
/// components (e.g. a mob's AI/health state) onto the new ECS entity. Returns the
/// network id and the ECS [`Entity`].
///
/// This is the generic spawn used by the mob module — the counterpart to the
/// fixed-type [`spawn_item_entity`]/[`spawn_xp_orb`] helpers. `meta` is the
/// initial [`EntityData`] sent right after the `AddEntity`.
pub fn spawn_net_entity(
    world: &mut World,
    type_id: i32,
    pos: (f64, f64, f64),
    yaw: f32,
    meta: EntityData,
    tag: impl FnOnce(&mut bevy_ecs::world::EntityWorldMut),
) -> (i32, Entity) {
    let id = alloc_id(world);
    let uuid = random_uuid();
    let entity = spawn_tracked(world, NetEntity { id, uuid, type_id }, pos, yaw, meta, tag);
    (id, entity)
}

/// Spawn an XP orb worth `value` at `(x, y, z)` and show it to every player
/// tracking that column. Returns the network entity id.
///
/// In 26.2 the orb's value is not carried in `AddEntity`'s `data` field but in
/// its `DATA_VALUE` metadata (index 8), so the spawn is a generic `AddEntity`
/// (data 0) followed by the metadata — same as any other entity.
#[allow(dead_code)] // public spawn API; no in-game trigger (mob/xp drop) yet.
pub fn spawn_xp_orb(world: &mut World, pos: (f64, f64, f64), value: i32) -> i32 {
    let id = alloc_id(world);
    let uuid = random_uuid();
    let mut meta = EntityData::new();
    meta.set(EXPERIENCE_ORB_DATA_VALUE, DataValue::Int(value));

    spawn_tracked(
        world,
        NetEntity { id, uuid, type_id: ENTITY_TYPE_EXPERIENCE_ORB },
        pos,
        0.0,
        meta,
        |ec| {
            ec.insert(XpOrb);
        },
    );
    id
}

/// Spawn a net entity into the world and pair it to every tracking viewer.
/// `tag` inserts any type-specific marker/state component onto the new entity.
/// Returns the ECS [`Entity`] so callers that need to address it further can.
fn spawn_tracked(
    world: &mut World,
    net: NetEntity,
    pos: (f64, f64, f64),
    yaw: f32,
    meta: EntityData,
    tag: impl FnOnce(&mut bevy_ecs::world::EntityWorldMut),
) -> Entity {
    let (x, y, z) = pos;
    let (id, uuid, type_id) = (net.id, net.uuid, net.type_id);
    let range = (client_tracking_range_chunks(type_id) * 16) as f64;

    // Build the pairing packets once, then pair to in-range viewers. The body yaw
    // is packed (`Mth.packDegrees`) and reused for the head yaw — spawned entities
    // face a single direction with head and body aligned.
    let packed_yaw = super::packets::pack_angle(yaw);
    let add = packets::add_entity(id, uuid, type_id, pos, packed_yaw, 0, packed_yaw, 0);
    let data = packets::set_entity_data(id, &meta);

    let mut ec = world.spawn((
        net,
        Pos { x, y, z, yaw, pitch: 0.0, on_ground: false },
        EntityMeta(meta),
        Tracked { seen_by: HashSet::new(), range },
    ));
    tag(&mut ec);
    let entity = ec.id();

    // Immediate pairing to viewers already in range — vanilla `ChunkMap.addEntity`
    // calls `TrackedEntity.updatePlayers` right after construction. `seen_by` is
    // seeded so [`update_entity_tracking`] only sends *changes* afterward. Pairing
    // is *reliable* (vanilla `ServerEntity.addPairing` uses `connection.send`): on
    // outbox overflow the send flags the player for a forced disconnect rather than
    // silently dropping the spawn, and `seen_by` is latched only when it succeeded —
    // otherwise later movement/metadata would reference an entity the client never
    // created.
    let chunk = chunk_of(x, z);
    let viewers = snapshot_viewers(world);
    let mut seen = HashSet::new();
    for v in &viewers {
        if is_visible(v, x, z, chunk, range) {
            let ok = world
                .get::<Conn>(v.entity)
                .map(|conn| conn.send_reliable(add.clone()) && conn.send_reliable(data.clone()))
                .unwrap_or(false);
            if ok {
                seen.insert(v.entity);
            }
        }
    }
    if let Some(mut t) = world.get_mut::<Tracked>(entity) {
        t.seen_by = seen;
    }
    entity
}

/// Fan per-source-entity clientbound packets to exactly each entity's tracking set
/// (`Tracked::seen_by`) — the movement/metadata/hurt counterpart to spawn pairing,
/// so a player is fed an entity iff it is spawned on their client. Best-effort per
/// the outbox contract: a momentarily-full outbox drops the (self-correcting) send.
pub fn fan_to_seen(world: &mut World, emissions: &[(Entity, Bytes)]) {
    if emissions.is_empty() {
        return;
    }
    let outboxes: HashMap<Entity, OutboxTx> = {
        let mut q = world.query::<(Entity, &Conn)>();
        q.iter(world).map(|(e, c)| (e, c.outbox.clone())).collect()
    };
    for (src, pkt) in emissions {
        if let Some(t) = world.get::<Tracked>(*src) {
            for viewer in &t.seen_by {
                if let Some(outbox) = outboxes.get(viewer) {
                    let _ = outbox.try_send(Outbound::Packet(pkt.clone()));
                }
            }
        }
    }
}

/// Reconcile every net entity's [`Tracked::seen_by`] against the current players,
/// mirroring `ChunkMap.TrackedEntity.updatePlayers`: a player entering range is
/// paired (`addPairing`: `AddEntity` + `SetEntityData`), one leaving range is
/// unpaired (`removePairing`: `RemoveEntities`). A player that has disconnected is
/// dropped from `seen_by` with no packet (its client is gone) — this covers *every*
/// player-despawn path uniformly (clean `Left`, forced overflow disconnect, and
/// keep-alive timeout) since it prunes any `seen_by` id no longer present among
/// live viewers, without each despawn site needing to touch tracking state.
pub fn update_entity_tracking(world: &mut World) {
    let viewers = snapshot_viewers(world);
    let live: HashSet<Entity> = viewers.iter().map(|v| v.entity).collect();
    let outboxes: HashMap<Entity, OutboxTx> =
        viewers.iter().map(|v| (v.entity, v.outbox.clone())).collect();

    struct Ent {
        entity: Entity,
        x: f64,
        z: f64,
        chunk: (i32, i32),
        range: f64,
    }
    let ents: Vec<Ent> = {
        let mut q = world.query::<(Entity, &Pos, &Tracked)>();
        q.iter(world)
            .map(|(entity, p, t)| Ent {
                entity,
                x: p.x,
                z: p.z,
                chunk: chunk_of(p.x, p.z),
                range: t.range,
            })
            .collect()
    };

    for ent in &ents {
        let prev = world
            .get::<Tracked>(ent.entity)
            .map(|t| t.seen_by.clone())
            .unwrap_or_default();

        let mut enters: Vec<Entity> = Vec::new();
        let mut leaves: Vec<Entity> = Vec::new();
        for v in &viewers {
            let visible = is_visible(v, ent.x, ent.z, ent.chunk, ent.range);
            let seen = prev.contains(&v.entity);
            if visible && !seen {
                enters.push(v.entity);
            } else if !visible && seen {
                leaves.push(v.entity);
            }
        }
        // Viewers still in `seen_by` that are no longer live: disconnected — drop
        // silently (no removal packet; the client is gone).
        let gone: Vec<Entity> = prev.iter().copied().filter(|p| !live.contains(p)).collect();

        if enters.is_empty() && leaves.is_empty() && gone.is_empty() {
            continue;
        }

        // Enter-range pairing is *reliable*, same as the immediate spawn pairing
        // above (vanilla `ServerEntity.addPairing` uses `connection.send`): a viewer
        // is retained in `enters` (and later latched into `seen_by`) only if both
        // frames enqueued, so an overflow disconnects the player rather than leaving
        // a spawn silently dropped while later deltas reference a nonexistent entity.
        if !enters.is_empty() {
            let (add, data) = {
                let net = world.get::<NetEntity>(ent.entity).expect("net entity");
                let pos = world.get::<Pos>(ent.entity).expect("net entity Pos");
                let meta = world.get::<EntityMeta>(ent.entity).expect("net entity meta");
                let packed_yaw = super::packets::pack_angle(pos.yaw);
                (
                    packets::add_entity(
                        net.id,
                        net.uuid,
                        net.type_id,
                        (pos.x, pos.y, pos.z),
                        packed_yaw,
                        super::packets::pack_angle(pos.pitch),
                        packed_yaw,
                        0,
                    ),
                    packets::set_entity_data(net.id, &meta.0),
                )
            };
            enters.retain(|p| {
                world
                    .get::<Conn>(*p)
                    .map(|conn| conn.send_reliable(add.clone()) && conn.send_reliable(data.clone()))
                    .unwrap_or(false)
            });
        }

        if !leaves.is_empty() {
            let id = world.get::<NetEntity>(ent.entity).expect("net entity").id;
            let remove = super::packets::remove_entities(&[id]);
            for p in &leaves {
                if let Some(outbox) = outboxes.get(p) {
                    let _ = outbox.try_send(Outbound::Packet(remove.clone()));
                }
            }
        }

        if let Some(mut t) = world.get_mut::<Tracked>(ent.entity) {
            for p in leaves.iter().chain(gone.iter()) {
                t.seen_by.remove(p);
            }
            for p in enters {
                t.seen_by.insert(p);
            }
        }
    }
}

/// Despawn a net entity and tell its current viewers (`Tracked::seen_by`) to
/// remove it — vanilla `TrackedEntity.broadcastRemoved` (`removePairing` for each
/// seen player). Returns the removed network id, or `None` if `entity` was not a
/// net entity.
pub fn remove_entity(world: &mut World, entity: Entity) -> Option<i32> {
    let (id, seen) = {
        let net = world.get::<NetEntity>(entity)?;
        let seen = world
            .get::<Tracked>(entity)
            .map(|t| t.seen_by.clone())
            .unwrap_or_default();
        (net.id, seen)
    };
    world.despawn(entity);
    if seen.is_empty() {
        return Some(id);
    }
    let remove = super::packets::remove_entities(&[id]);
    let outboxes: HashMap<Entity, OutboxTx> = {
        let mut q = world.query::<(Entity, &Conn)>();
        q.iter(world).map(|(e, c)| (e, c.outbox.clone())).collect()
    };
    for viewer in seen {
        if let Some(outbox) = outboxes.get(&viewer) {
            let _ = outbox.try_send(Outbound::Packet(remove.clone()));
        }
    }
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::buffer::PacketReader;
    use crate::sim::bridge::Outbound;
    use std::collections::HashSet;
    use tokio::sync::mpsc;

    fn frame_id(b: &bytes::Bytes) -> i32 {
        let mut r = PacketReader::new(b.clone());
        r.read_varint().unwrap(); // length
        r.read_varint().unwrap() // id
    }

    fn drain(rx: &mut mpsc::Receiver<Outbound>) -> Vec<i32> {
        let mut ids = Vec::new();
        while let Ok(Outbound::Packet(b)) = rx.try_recv() {
            ids.push(frame_id(&b));
        }
        ids
    }

    /// A minimal viewer at world origin whose loaded set covers `chunks`. The
    /// `Pos` is required now that visibility is distance-gated (mirroring a real
    /// player, which always carries `Pos` + `LoadedChunks` + `Conn`).
    fn spawn_viewer(world: &mut World, chunks: &[(i32, i32)]) -> mpsc::Receiver<Outbound> {
        spawn_viewer_at(world, chunks, 0.0, 0.0)
    }

    fn spawn_viewer_at(
        world: &mut World,
        chunks: &[(i32, i32)],
        x: f64,
        z: f64,
    ) -> mpsc::Receiver<Outbound> {
        let (tx, rx) = mpsc::channel(64);
        world.spawn((
            Conn::new(tx),
            LoadedChunks {
                center: (0, 0),
                loaded: chunks.iter().copied().collect::<HashSet<_>>(),
            },
            // Real players carry a `ChunkSender`; the tracking query now requires it
            // (a fresh sender has no pending columns, so all `loaded` count as tracked).
            ChunkSender::new(),
            // The tracking query also requires `RequestedViewDistance` now. These test
            // worlds have no `Config`, so the view range is unbounded regardless of the
            // value — column membership alone gates visibility here.
            RequestedViewDistance(RequestedViewDistance::DEFAULT),
            Pos { x, y: 64.0, z, yaw: 0.0, pitch: 0.0, on_ground: true },
        ));
        rx
    }

    /// Move the single viewer (the only entity carrying `LoadedChunks`) to `(x,z)`.
    fn move_viewer(world: &mut World, x: f64, z: f64) {
        let e = world
            .query_filtered::<Entity, With<LoadedChunks>>()
            .iter(world)
            .next()
            .unwrap();
        let mut pos = world.get_mut::<Pos>(e).unwrap();
        pos.x = x;
        pos.z = z;
    }

    fn expected_ids() -> (i32, i32) {
        (
            frame_id(&super::packets::add_entity(
                0,
                Uuid::nil(),
                0,
                (0.0, 0.0, 0.0),
                0,
                0,
                0,
                0,
            )),
            frame_id(&super::packets::set_entity_data(0, &EntityData::new())),
        )
    }

    fn world_with_id() -> World {
        let mut world = World::new();
        world.insert_resource(NextEntityId(1));
        world
    }

    #[test]
    fn item_spawn_pairs_add_then_data_to_in_range_viewer() {
        let mut world = world_with_id();
        let mut rx = spawn_viewer(&mut world, &[(0, 0)]);
        let id = spawn_item_entity(&mut world, (1.0, 64.0, 2.0), ItemStack::new(1, 1));
        assert_eq!(id, 1);
        let (add_id, data_id) = expected_ids();
        assert_eq!(drain(&mut rx), vec![add_id, data_id]);
    }

    #[test]
    fn out_of_range_viewer_receives_nothing() {
        let mut world = world_with_id();
        // Viewer only tracks chunk (10,10); the item spawns at chunk (0,0).
        let mut rx = spawn_viewer(&mut world, &[(10, 10)]);
        spawn_xp_orb(&mut world, (5.0, 64.0, 5.0), 7);
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn entity_enters_leaves_and_reenters_view() {
        // The core of Fix C: an entity spawned before a player arrives is paired
        // when the player enters range, unpaired when they recede, and re-paired
        // when they return — all driven by `update_entity_tracking`.
        let mut world = world_with_id();
        // Item spawns with no viewers present, so its `seen_by` starts empty.
        spawn_item_entity(&mut world, (0.5, 64.0, 0.5), ItemStack::new(1, 1));
        let mut rx = spawn_viewer(&mut world, &[(0, 0)]); // at origin, column loaded
        let (add_id, data_id) = expected_ids();
        let remove_id = frame_id(&super::super::packets::remove_entities(&[0]));

        // Enter: the approaching player receives the spawn pairing.
        update_entity_tracking(&mut world);
        assert_eq!(drain(&mut rx), vec![add_id, data_id]);
        // Idempotent: a second pass with nothing changed sends nothing.
        update_entity_tracking(&mut world);
        assert!(drain(&mut rx).is_empty());

        // Recede beyond the item's 6-chunk (96-block) range (column still loaded,
        // so only the distance predicate trips): the entity is removed.
        move_viewer(&mut world, 1000.0, 1000.0);
        update_entity_tracking(&mut world);
        assert_eq!(drain(&mut rx), vec![remove_id]);

        // Approach again: re-spawned.
        move_viewer(&mut world, 0.0, 0.0);
        update_entity_tracking(&mut world);
        assert_eq!(drain(&mut rx), vec![add_id, data_id]);
    }

    #[test]
    fn fan_to_seen_matches_tracked_set() {
        // Movement/metadata fan-out reaches exactly the tracked viewers: an in-range
        // player (in `seen_by`) receives it, an out-of-range one does not.
        let mut world = world_with_id();
        spawn_item_entity(&mut world, (0.5, 64.0, 0.5), ItemStack::new(1, 1));
        let mut near = spawn_viewer_at(&mut world, &[(0, 0)], 0.0, 0.0);
        let mut far = spawn_viewer_at(&mut world, &[(0, 0)], 1000.0, 1000.0);

        update_entity_tracking(&mut world);
        // near paired (add+data); far out of range, nothing.
        let (add_id, data_id) = expected_ids();
        assert_eq!(drain(&mut near), vec![add_id, data_id]);
        assert!(drain(&mut far).is_empty());

        // Fan one packet through the tracking set.
        let item = world
            .query::<(Entity, &NetEntity)>()
            .iter(&world)
            .next()
            .map(|(e, _)| e)
            .unwrap();
        let pkt = super::super::packets::remove_entities(&[42]);
        let pkt_id = frame_id(&pkt);
        fan_to_seen(&mut world, &[(item, pkt)]);
        assert_eq!(drain(&mut near), vec![pkt_id]);
        assert!(drain(&mut far).is_empty());
    }

    #[test]
    fn disconnected_viewer_is_pruned_from_seen_without_packet() {
        // A viewer that despawns (any disconnect path) is dropped from `seen_by`
        // silently on the next tracking pass — no removal packet, no stale id.
        let mut world = world_with_id();
        spawn_item_entity(&mut world, (0.5, 64.0, 0.5), ItemStack::new(1, 1));
        let _rx = spawn_viewer(&mut world, &[(0, 0)]);
        update_entity_tracking(&mut world);
        let item = world
            .query::<(Entity, &NetEntity)>()
            .iter(&world)
            .next()
            .map(|(e, _)| e)
            .unwrap();
        assert_eq!(world.get::<Tracked>(item).unwrap().seen_by.len(), 1);

        // Despawn the viewer (as a keep-alive timeout / overflow disconnect would).
        let viewer = world
            .query_filtered::<Entity, With<LoadedChunks>>()
            .iter(&world)
            .next()
            .unwrap();
        world.despawn(viewer);
        update_entity_tracking(&mut world);
        assert!(world.get::<Tracked>(item).unwrap().seen_by.is_empty());
    }

    #[test]
    fn remove_entity_broadcasts_removal_and_despawns() {
        let mut world = world_with_id();
        let mut rx = spawn_viewer(&mut world, &[(0, 0)]);
        let id = spawn_item_entity(&mut world, (1.0, 64.0, 2.0), ItemStack::new(1, 1));
        let _ = drain(&mut rx); // discard the spawn pair

        // Find the ECS entity for the spawned net id and remove it.
        let ecs = world
            .query::<(Entity, &NetEntity)>()
            .iter(&world)
            .find(|(_, n)| n.id == id)
            .map(|(e, _)| e)
            .unwrap();
        let removed = remove_entity(&mut world, ecs);
        assert_eq!(removed, Some(id));

        let remove_id = frame_id(&super::super::packets::remove_entities(&[0]));
        assert_eq!(drain(&mut rx), vec![remove_id]);
        assert_eq!(world.query::<&NetEntity>().iter(&world).count(), 0);
    }

    #[test]
    fn entity_type_ids_match_registry() {
        use crate::registry::builtin::ENTITY_TYPE;
        assert_eq!(ENTITY_TYPE.id_of("minecraft:item"), Some(ENTITY_TYPE_ITEM));
        assert_eq!(
            ENTITY_TYPE.id_of("minecraft:experience_orb"),
            Some(ENTITY_TYPE_EXPERIENCE_ORB)
        );
    }
}
