//! Dropped-item behavior: the per-tick physics, pickup, merge and despawn for
//! `minecraft:item` entities, ported from vanilla `ItemEntity.tick()`
//! (`net.minecraft.world.entity.item.ItemEntity`, MC 26.2) with the shared
//! movement integration from `net.minecraft.world.entity.Entity`.
//!
//! This is the gameplay half of the item-entity scaffolding in
//! [`super::entity`]: that module owns the spawn/track/remove wire path and the
//! `ItemDrop` marker; this module drives the entities once they exist. It runs as
//! an exclusive ECS system registered in [`super::run`]'s schedule.
//!
//! Parity notes / simplifications (Vela has no full voxel-shape collision, fluid
//! volumes, or per-block friction table yet):
//!
//! * **Ground collision** is a single vertical probe against
//!   [`crate::world::block_state_at`]: an item falling into a non-air block is
//!   clamped to that block's top face and reported on-ground. Horizontal
//!   collision is not modelled (items may slide into a wall's cell). Vanilla runs
//!   the full `Entity.move` AABB sweep.
//! * **Fluids** are absent, so the water/lava movement branches of `tick()` are
//!   omitted — every item takes the dry `applyGravity` path.
//! * **Block friction** uses the default `0.6` for every on-ground block
//!   (`AbstractBlock.getFriction` default); ice/slime/honey are not special-cased.
//! * The `(tickCount + id) % 4` idle-skip optimization is not reproduced — an
//!   item resting on the ground re-clamps to the same integer surface each tick,
//!   so it stays put and emits no movement packet regardless. This is a
//!   CPU-only optimization in vanilla, not a behavioral one.
//! * Item entities use `EntityType.updateInterval(20)` in vanilla; we instead
//!   emit a movement packet on any tick the encoded position changed, which is
//!   smoother than vanilla and never spams for a resting item.

use std::collections::HashSet;

use bevy_ecs::prelude::*;
use bytes::Bytes;

use super::bridge::{Outbound, OutboxTx};
use super::components::{Conn, LoadedChunks, Pos, Profile};
use super::entity::syncher::DataValue;
use super::entity::{remove_entity, EntityMeta, ItemDrop, NetEntity};
use crate::ids::ItemId;
use crate::inventory::{container_set_content, Inventory, ItemStack};
use crate::protocol::buffer::PacketWriter;
use crate::protocol::framing::frame;

// --- Vanilla `ItemEntity` / `Entity` constants -------------------------------
/// `Entity.getDefaultGravity` for `ItemEntity` — 0.04 blocks/tick² downward.
const ITEM_GRAVITY: f64 = 0.04;
/// `Entity.getAirDrag` — the 0.98 velocity retention applied each tick.
const AIR_DRAG: f64 = 0.98;
/// Default `BlockBehaviour.getFriction` (the friction of most blocks); ice and
/// friends are not modelled, so every on-ground block uses this.
const DEFAULT_BLOCK_FRICTION: f64 = 0.6;
/// `ItemEntity.LIFETIME` — age at which a normal item despawns (6000 ticks).
const LIFETIME: i32 = 6000;
/// `ItemEntity.INFINITE_PICKUP_DELAY` — a `pickupDelay` that never counts down
/// and blocks pickup entirely.
const INFINITE_PICKUP_DELAY: i32 = 32767;
/// `ItemEntity.INFINITE_LIFETIME` — an `age` sentinel that never ages out.
const INFINITE_LIFETIME: i32 = -32768;
/// `ItemEntity.setDefaultPickUpDelay` — the 10-tick (0.5 s) delay a freshly
/// dropped item carries before it can be collected. Spawned items have no
/// physics state until this system attaches it, so this is the default we seed.
const DEFAULT_PICKUP_DELAY: i32 = 10;

/// `ItemEntity.DATA_ITEM` accessor index (`SynchedEntityData.defineId` order:
/// `Entity` occupies 0..=7, so the first `ItemEntity` field is 8).
const ITEM_ENTITY_DATA_ITEM: u8 = 8;

// --- Entity bounding-box dimensions ------------------------------------------
/// `EntityTypes.ITEM` size — a 0.25×0.25 cube.
const ITEM_SIZE: f64 = 0.25;
/// Standing-player bounding box (`EntityTypes.PLAYER`): 0.6 wide, 1.8 tall.
const PLAYER_WIDTH: f64 = 0.6;
const PLAYER_HEIGHT: f64 = 1.8;
/// `Player.aiStep` pickup-area inflation of the player box: `inflate(1.0, 0.5, 1.0)`.
const PICKUP_INFLATE_XZ: f64 = 1.0;
const PICKUP_INFLATE_Y: f64 = 0.5;

/// `ClientboundTakeItemEntityPacket` — index 124 in the clientbound PLAY flow
/// (`GameProtocols.CLIENTBOUND_TEMPLATE`, past the bundle delimiter at 0).
const CB_PLAY_TAKE_ITEM_ENTITY: i32 = 124;

/// Per-item physics/lifecycle state, mirroring the `ItemEntity` fields
/// (`deltaMovement`, `age`, `pickupDelay`) plus the last-broadcast position base
/// used to build movement deltas (vanilla `ServerEntity.positionCodec`).
///
/// Attached lazily by [`item_tick`] to any `ItemDrop` that lacks it, so an item
/// spawned by the (separately owned) drop/spawn path picks up behavior on its
/// first tick without that path needing to know this component exists.
#[derive(Component)]
pub struct ItemPhysics {
    /// `deltaMovement` (velocity), blocks/tick.
    pub vx: f64,
    pub vy: f64,
    pub vz: f64,
    /// `ItemEntity.age`; `-32768` never ages out.
    pub age: i32,
    /// `ItemEntity.pickupDelay`; `32767` never allows pickup.
    pub pickup_delay: i32,
    /// Per-entity `tickCount`, gating the merge cadence.
    pub tick_count: u32,
    /// Last-broadcast position (the `move`-packet delta base).
    pub base_x: f64,
    pub base_y: f64,
    pub base_z: f64,
}

impl ItemPhysics {
    /// A freshly spawned item's state: at rest, default pickup delay, position
    /// base seeded to its spawn position so the first movement delta is measured
    /// from where the client already placed it.
    fn new(x: f64, y: f64, z: f64) -> Self {
        Self {
            vx: 0.0,
            vy: 0.0,
            vz: 0.0,
            age: 0,
            pickup_delay: DEFAULT_PICKUP_DELAY,
            tick_count: 0,
            base_x: x,
            base_y: y,
            base_z: z,
        }
    }
}

/// `ClientboundTakeItemEntityPacket` — plays the pickup animation (the item flies
/// into the collector) on every viewer. Layout
/// (`ClientboundTakeItemEntityPacket.write`): `itemId` (VarInt), `playerId`
/// (VarInt), `amount` (VarInt).
pub fn take_item_entity(item_id: i32, player_id: i32, amount: i32) -> Bytes {
    let mut p = PacketWriter::new();
    p.write_varint(item_id);
    p.write_varint(player_id);
    p.write_varint(amount);
    frame(CB_PLAY_TAKE_ITEM_ENTITY, &p.buf)
}

/// The chunk column `(cx, cz)` a world position sits in — the per-viewer culling
/// key, matching [`super::entity`]'s private helper.
fn chunk_of(x: f64, z: f64) -> (i32, i32) {
    ((x.floor() as i32) >> 4, (z.floor() as i32) >> 4)
}

/// Whether the world block at `(x, y, z)` is a movement obstacle. Vela has no
/// per-block collision shapes yet, so "solid" is simply "not air".
fn is_solid(x: i32, y: i32, z: i32) -> bool {
    crate::world::block_state_at(x, y, z) != crate::world::AIR_STATE
}

/// A single item's kinematic state for one integration step.
struct Motion {
    x: f64,
    y: f64,
    z: f64,
    vx: f64,
    vy: f64,
    vz: f64,
    on_ground: bool,
}

/// One tick of item movement, matching the dry (non-fluid) path of
/// `ItemEntity.tick()` + `Entity.move`: apply gravity, integrate, clamp onto a
/// solid block below (our stand-in for the vertical `move` sweep), then apply air
/// drag — scaled by block friction on the ground — and the on-ground downward
/// damping. `solid(x, y, z)` reports whether a block cell obstructs movement.
fn step_motion(m: &mut Motion, solid: impl Fn(i32, i32, i32) -> bool) {
    // applyGravity(): deltaMovement.y -= gravity.
    m.vy -= ITEM_GRAVITY;

    // move(SELF, deltaMovement): integrate, then resolve a downward collision
    // against the block the item's feet would enter.
    let nx = m.x + m.vx;
    let mut ny = m.y + m.vy;
    let nz = m.z + m.vz;

    let mut on_ground = false;
    if m.vy < 0.0 {
        let (bx, bz) = (nx.floor() as i32, nz.floor() as i32);
        let by = ny.floor() as i32;
        if solid(bx, by, bz) {
            // Land on the block's top face.
            ny = (by + 1) as f64;
            on_ground = true;
            m.vy = 0.0;
        }
    }

    m.x = nx;
    m.y = ny;
    m.z = nz;
    m.on_ground = on_ground;

    // airDrag / groundFriction: horizontal is scaled by block friction on the
    // ground, vertical by the air drag alone.
    let horizontal = if on_ground {
        AIR_DRAG * DEFAULT_BLOCK_FRICTION
    } else {
        AIR_DRAG
    };
    m.vx *= horizontal;
    m.vz *= horizontal;
    m.vy *= AIR_DRAG;

    // On the ground, a leftover downward velocity is halved and reversed
    // (vanilla `movement.multiply(1.0, -0.5, 1.0)`).
    if on_ground && m.vy < 0.0 {
        m.vy *= -0.5;
    }
}

/// Read an item entity's carried stack out of its `DATA_ITEM` metadata. Returns
/// [`ItemStack::EMPTY`] when the accessor is absent or empty.
fn item_of(meta: &EntityMeta) -> ItemStack {
    for it in meta.0.items() {
        if it.index == ITEM_ENTITY_DATA_ITEM {
            if let DataValue::ItemStack(Some(s)) = &it.value {
                return *s;
            }
        }
    }
    ItemStack::EMPTY
}

/// A per-item working record collected during the physics pass and consumed by
/// the merge / pickup / despawn passes.
struct ItemRecord {
    entity: Entity,
    net_id: i32,
    x: f64,
    y: f64,
    z: f64,
    chunk: (i32, i32),
    item_id: ItemId,
    count: i32,
    max: i32,
    age: i32,
    pickup_delay: i32,
    tick_count: u32,
    /// Whether the item's integer block position changed this tick (drives the
    /// vanilla merge cadence: every 2 ticks while moving, every 40 while at rest).
    moved: bool,
}

impl ItemRecord {
    /// `ItemEntity.isMergable`: alive, collectable, not aged out, and not full.
    fn is_mergable(&self) -> bool {
        self.pickup_delay != INFINITE_PICKUP_DELAY
            && self.age != INFINITE_LIFETIME
            && self.age < LIFETIME
            && self.count < self.max
    }
}

/// The item-entity behavior system: physics, merge, pickup, and despawn for every
/// `ItemDrop`. Registered after `world_tick` in the schedule.
pub fn item_tick(world: &mut World) {
    // Phase 0: attach physics state to any newly spawned item that lacks it.
    let uninit: Vec<Entity> = world
        .query_filtered::<Entity, (With<ItemDrop>, Without<ItemPhysics>)>()
        .iter(world)
        .collect();
    for e in uninit {
        let (x, y, z) = {
            let p = world.get::<Pos>(e).expect("item has Pos");
            (p.x, p.y, p.z)
        };
        world.entity_mut(e).insert(ItemPhysics::new(x, y, z));
    }

    // Packets queued for per-chunk viewer fan-out at the end of the tick.
    let mut emissions: Vec<((i32, i32), Bytes)> = Vec::new();
    let mut records: Vec<ItemRecord> = Vec::new();
    let mut removed: HashSet<Entity> = HashSet::new();

    // Phase 1: physics + movement broadcast, gathering a record per live item.
    {
        let mut q = world
            .query_filtered::<(Entity, &NetEntity, &mut Pos, &mut ItemPhysics, &EntityMeta), With<ItemDrop>>();
        for (entity, net, mut pos, mut phys, meta) in q.iter_mut(world) {
            let stack = item_of(meta);
            if stack.is_empty() {
                // ItemEntity.tick(): an empty stack discards the entity.
                removed.insert(entity);
                continue;
            }

            // Count down the pickup delay (32767 == "never", left untouched).
            if phys.pickup_delay > 0 && phys.pickup_delay != INFINITE_PICKUP_DELAY {
                phys.pickup_delay -= 1;
            }

            let (ox, oy, oz) = (pos.x, pos.y, pos.z);
            let mut m = Motion {
                x: pos.x,
                y: pos.y,
                z: pos.z,
                vx: phys.vx,
                vy: phys.vy,
                vz: phys.vz,
                on_ground: false,
            };
            step_motion(&mut m, is_solid);
            pos.x = m.x;
            pos.y = m.y;
            pos.z = m.z;
            pos.on_ground = m.on_ground;
            phys.vx = m.vx;
            phys.vy = m.vy;
            phys.vz = m.vz;

            // Age the item (unless it is flagged never-despawn).
            if phys.age != INFINITE_LIFETIME {
                phys.age += 1;
            }
            phys.tick_count = phys.tick_count.wrapping_add(1);

            let moved = (ox.floor() as i64 != pos.x.floor() as i64)
                || (oy.floor() as i64 != pos.y.floor() as i64)
                || (oz.floor() as i64 != pos.z.floor() as i64);

            // Movement packet: a relative delta from the last-sent base, or an
            // absolute resync when the delta can't fit a short.
            let dx = super::packets::enc(pos.x) - super::packets::enc(phys.base_x);
            let dy = super::packets::enc(pos.y) - super::packets::enc(phys.base_y);
            let dz = super::packets::enc(pos.z) - super::packets::enc(phys.base_z);
            let chunk = chunk_of(pos.x, pos.z);
            if dx != 0 || dy != 0 || dz != 0 {
                let fits = (-32768..=32767).contains(&dx)
                    && (-32768..=32767).contains(&dy)
                    && (-32768..=32767).contains(&dz);
                let pkt = if fits {
                    super::packets::move_entity_pos(
                        net.id,
                        dx as i16,
                        dy as i16,
                        dz as i16,
                        pos.on_ground,
                    )
                } else {
                    super::packets::entity_position_sync(
                        net.id, pos.x, pos.y, pos.z, pos.yaw, pos.pitch, pos.on_ground,
                    )
                };
                emissions.push((chunk, pkt));
                phys.base_x = pos.x;
                phys.base_y = pos.y;
                phys.base_z = pos.z;
            }

            records.push(ItemRecord {
                entity,
                net_id: net.id,
                x: pos.x,
                y: pos.y,
                z: pos.z,
                chunk,
                item_id: stack.id,
                count: stack.count,
                max: stack.max_stack_size(),
                age: phys.age,
                pickup_delay: phys.pickup_delay,
                tick_count: phys.tick_count,
                moved,
            });
        }
    }

    // Phase 2: merge nearby same-item stacks (ItemEntity.mergeWithNeighbours).
    merge_pass(world, &mut records, &mut removed, &mut emissions);

    // Phase 3: pickup by nearby players (ItemEntity.playerTouch / Player.aiStep).
    pickup_pass(world, &mut records, &mut removed, &mut emissions);

    // Phase 4: despawn aged-out items (age >= LIFETIME).
    for rec in &records {
        if removed.contains(&rec.entity) {
            continue;
        }
        if rec.age != INFINITE_LIFETIME && rec.age >= LIFETIME {
            remove_entity(world, rec.entity);
            removed.insert(rec.entity);
        }
    }

    // Flush queued movement / metadata / take packets to their chunk viewers.
    // (Removals were already broadcast by `remove_entity`.)
    if !emissions.is_empty() {
        let conns: Vec<(HashSet<(i32, i32)>, OutboxTx)> = {
            let mut q = world.query::<(&LoadedChunks, &Conn)>();
            q.iter(world)
                .map(|(l, c)| (l.loaded.clone(), c.outbox.clone()))
                .collect()
        };
        for (chunk, pkt) in &emissions {
            for (loaded, outbox) in &conns {
                if loaded.contains(chunk) {
                    let _ = outbox.try_send(Outbound::Packet(pkt.clone()));
                }
            }
        }
    }
}

/// `ItemEntity.mergeWithNeighbours` / `tryToMerge`: fold same-item stacks that sit
/// within `inflate(0.5, 0.0, 0.5)` of each other into one, up to the max stack
/// size, respecting the per-item merge cadence (`moved ? 2 : 40` ticks).
fn merge_pass(
    world: &mut World,
    records: &mut [ItemRecord],
    removed: &mut HashSet<Entity>,
    emissions: &mut Vec<((i32, i32), Bytes)>,
) {
    let n = records.len();
    for i in 0..n {
        if removed.contains(&records[i].entity) {
            continue;
        }
        let rate = if records[i].moved { 2 } else { 40 };
        if !records[i].tick_count.is_multiple_of(rate) || !records[i].is_mergable() {
            continue;
        }
        for j in 0..n {
            if i == j {
                continue;
            }
            if removed.contains(&records[i].entity) {
                break; // `this` was consumed into another stack.
            }
            if removed.contains(&records[j].entity) || !records[j].is_mergable() {
                continue;
            }
            if records[i].item_id != records[j].item_id
                || !merge_in_range(&records[i], &records[j])
            {
                continue;
            }
            // areMergable: the combined count must fit one stack.
            if records[i].count + records[j].count > records[i].max {
                continue;
            }
            // The larger stack absorbs the smaller (vanilla picks `to` as the one
            // with the greater count).
            let (to, from) = if records[j].count < records[i].count {
                (i, j)
            } else {
                (j, i)
            };
            let delta =
                (records[to].max.min(64) - records[to].count).min(records[from].count);
            records[to].count += delta;
            records[from].count -= delta;
            commit_count(world, records, to, removed, emissions);
            commit_count(world, records, from, removed, emissions);
        }
    }
}

/// Whether `a` inflated by `(0.5, 0, 0.5)` overlaps `b`'s bounding box — the
/// vanilla merge search volume.
fn merge_in_range(a: &ItemRecord, b: &ItemRecord) -> bool {
    // Horizontal centres within (item width + 0.5 inflation); vertical boxes
    // overlap (no vertical inflation).
    let overlap_xz = |ac: f64, bc: f64| (ac - bc).abs() < ITEM_SIZE + 0.5;
    overlap_xz(a.x, b.x)
        && overlap_xz(a.z, b.z)
        && a.y < b.y + ITEM_SIZE
        && a.y + ITEM_SIZE > b.y
}

/// Push an item's new count into its metadata and queue the metadata broadcast,
/// or despawn it when the count has dropped to zero.
fn commit_count(
    world: &mut World,
    records: &mut [ItemRecord],
    idx: usize,
    removed: &mut HashSet<Entity>,
    emissions: &mut Vec<((i32, i32), Bytes)>,
) {
    let entity = records[idx].entity;
    if records[idx].count <= 0 {
        remove_entity(world, entity);
        removed.insert(entity);
        return;
    }
    let stack = ItemStack {
        id: records[idx].item_id,
        count: records[idx].count,
    };
    if let Some(mut meta) = world.get_mut::<EntityMeta>(entity) {
        meta.0
            .set(ITEM_ENTITY_DATA_ITEM, DataValue::ItemStack(Some(stack)));
    }
    let data = {
        let meta = world.get::<EntityMeta>(entity).expect("item has EntityMeta");
        super::entity::packets::set_entity_data(records[idx].net_id, &meta.0)
    };
    emissions.push((records[idx].chunk, data));
}

/// `ItemEntity.playerTouch` driven by `Player.aiStep`'s pickup sweep: for each
/// player, collect every item whose box intersects the player box inflated by
/// `(1.0, 0.5, 1.0)`, and — when the pickup delay has elapsed — add it to the
/// inventory, animate the take, and sync the container.
fn pickup_pass(
    world: &mut World,
    records: &mut [ItemRecord],
    removed: &mut HashSet<Entity>,
    emissions: &mut Vec<((i32, i32), Bytes)>,
) {
    // Note: we deliberately do NOT filter on `&Inventory` here. That component is
    // attached lazily (see `packet_handlers::inventory_mut`) on a player's first
    // inventory packet, so requiring it would silently exclude any player who
    // hasn't touched their inventory since joining — making pickup fail until they
    // happen to scroll the hotbar. We ensure the inventory exists at pickup time
    // instead.
    let players: Vec<(Entity, i32, f64, f64, f64, OutboxTx)> = {
        let mut q = world.query::<(Entity, &Profile, &Pos, &Conn)>();
        q.iter(world)
            .map(|(e, prof, pos, conn)| {
                (e, prof.entity_id, pos.x, pos.y, pos.z, conn.outbox.clone())
            })
            .collect()
    };

    for (p_entity, p_eid, px, py, pz, outbox) in &players {
        for rec in records.iter_mut() {
            if removed.contains(&rec.entity) || rec.pickup_delay != 0 {
                continue;
            }
            if !pickup_overlap(*px, *py, *pz, rec.x, rec.y, rec.z) {
                continue;
            }

            let org_count = rec.count;
            let mut stack = ItemStack {
                id: rec.item_id,
                count: org_count,
            };
            // Ensure the collector has an inventory (it is attached lazily), then
            // store the stack; `add` decrements `stack.count` in place.
            if world.get::<Inventory>(*p_entity).is_none() {
                world.entity_mut(*p_entity).insert(Inventory::new());
            }
            let added = world
                .get_mut::<Inventory>(*p_entity)
                .expect("inventory just ensured")
                .add(&mut stack);
            if added <= 0 {
                continue; // inventory full for this item — leave it on the ground.
            }

            // Broadcast the pickup animation to everyone tracking the item. Vanilla
            // reports the original (pre-pickup) count in the take packet.
            emissions.push((rec.chunk, take_item_entity(rec.net_id, *p_eid, org_count)));

            if stack.is_empty() {
                remove_entity(world, rec.entity);
                removed.insert(rec.entity);
            } else {
                // Partial pickup: the item lives on with the reduced count.
                rec.count = stack.count;
                commit_count(world, std::slice::from_mut(rec), 0, removed, emissions);
            }

            // Resync the collector's inventory so the new item shows up.
            let content = {
                let mut inv = world
                    .get_mut::<Inventory>(*p_entity)
                    .expect("player has Inventory");
                let sid = inv.next_state_id();
                container_set_content(0, sid, &inv.slots, inv.carried.as_ref())
            };
            let _ = outbox.try_send(Outbound::Packet(content));
        }
    }
}

/// Whether an item at `(ix, iy, iz)` intersects the player pickup box: the
/// standing-player AABB inflated by `(1.0, 0.5, 1.0)` (`Player.aiStep`).
fn pickup_overlap(px: f64, py: f64, pz: f64, ix: f64, iy: f64, iz: f64) -> bool {
    let ph = PLAYER_WIDTH / 2.0;
    let p_min_x = px - ph - PICKUP_INFLATE_XZ;
    let p_max_x = px + ph + PICKUP_INFLATE_XZ;
    let p_min_y = py - PICKUP_INFLATE_Y;
    let p_max_y = py + PLAYER_HEIGHT + PICKUP_INFLATE_Y;
    let p_min_z = pz - ph - PICKUP_INFLATE_XZ;
    let p_max_z = pz + ph + PICKUP_INFLATE_XZ;

    let ih = ITEM_SIZE / 2.0;
    let i_min_x = ix - ih;
    let i_max_x = ix + ih;
    let i_min_y = iy;
    let i_max_y = iy + ITEM_SIZE;
    let i_min_z = iz - ih;
    let i_max_z = iz + ih;

    p_min_x < i_max_x
        && p_max_x > i_min_x
        && p_min_y < i_max_y
        && p_max_y > i_min_y
        && p_min_z < i_max_z
        && p_max_z > i_min_z
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat solid floor at y < 64 (so the top face is y = 64).
    fn floor_solid(_x: i32, y: i32, _z: i32) -> bool {
        y < 64
    }

    #[test]
    fn gravity_accelerates_a_falling_item() {
        let mut m = Motion {
            x: 0.0,
            y: 100.0,
            z: 0.0,
            vx: 0.0,
            vy: 0.0,
            vz: 0.0,
            on_ground: false,
        };
        // First step: vy = -0.04 (gravity), then *0.98 air drag => -0.0392.
        step_motion(&mut m, |_, _, _| false);
        assert!(!m.on_ground);
        assert!((m.y - 99.96).abs() < 1e-9); // 100 + (-0.04)
        assert!((m.vy - (-0.04 * AIR_DRAG)).abs() < 1e-12);
    }

    #[test]
    fn item_clamps_onto_the_ground() {
        // Start just above the floor top (y = 64) with a downward velocity; the
        // step must clamp the item to the surface and mark it grounded.
        let mut m = Motion {
            x: 0.5,
            y: 64.2,
            z: 0.5,
            vx: 0.0,
            vy: -0.5,
            vz: 0.0,
            on_ground: false,
        };
        step_motion(&mut m, floor_solid);
        assert!(m.on_ground);
        assert!((m.y - 64.0).abs() < 1e-9);
        assert_eq!(m.vy, 0.0);
    }

    #[test]
    fn resting_item_stays_on_surface() {
        // An item already resting at the surface re-clamps to the same y and does
        // not sink, so it emits no net position change.
        let mut m = Motion {
            x: 0.5,
            y: 64.0,
            z: 0.5,
            vx: 0.0,
            vy: 0.0,
            vz: 0.0,
            on_ground: false,
        };
        step_motion(&mut m, floor_solid);
        assert!(m.on_ground);
        assert!((m.y - 64.0).abs() < 1e-9);
    }

    #[test]
    fn take_item_entity_layout() {
        use crate::protocol::buffer::PacketReader;
        let bytes = take_item_entity(7, 42, 3);
        let mut r = PacketReader::new(bytes);
        r.read_varint().unwrap(); // frame length
        assert_eq!(r.read_varint().unwrap(), CB_PLAY_TAKE_ITEM_ENTITY);
        assert_eq!(r.read_varint().unwrap(), 7); // item entity id
        assert_eq!(r.read_varint().unwrap(), 42); // collector id
        assert_eq!(r.read_varint().unwrap(), 3); // amount
    }

    #[test]
    fn pickup_overlap_reaches_one_block_away() {
        // Player at origin; an item 1 block to the side is within the inflated box.
        assert!(pickup_overlap(0.0, 64.0, 0.0, 1.0, 64.0, 0.0));
        // Far away in Z is out of range.
        assert!(!pickup_overlap(0.0, 64.0, 0.0, 0.0, 64.0, 5.0));
    }

    #[test]
    fn merge_range_matches_inflate_half() {
        let base = |x: f64, z: f64| ItemRecord {
            entity: Entity::PLACEHOLDER,
            net_id: 0,
            x,
            y: 64.0,
            z,
            chunk: (0, 0),
            item_id: ItemId(1),
            count: 1,
            max: 64,
            age: 0,
            pickup_delay: 0,
            tick_count: 0,
            moved: false,
        };
        let a = base(0.0, 0.0);
        assert!(merge_in_range(&a, &base(0.6, 0.0))); // within 0.25 + 0.5
        assert!(!merge_in_range(&a, &base(0.9, 0.0))); // beyond
    }
}
