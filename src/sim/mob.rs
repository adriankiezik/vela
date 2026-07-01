//! Living passive mobs: the spawn API, per-tick wander AI, movement broadcasting,
//! health, and a simple natural spawner.
//!
//! This is the mob counterpart to the dropped-item scaffolding: [`super::entity`]
//! owns the generic net-entity spawn/track/remove wire path (`AddEntity` +
//! `SetEntityData` on spawn, `RemoveEntities` on despawn), and this module drives
//! the mobs once they exist. It runs as an exclusive ECS system registered in
//! [`super::run`]'s schedule, alongside `item_tick`.
//!
//! Reference (MC 26.2, decompiled server): `world/entity/LivingEntity`,
//! `world/entity/Mob`, `world/entity/animal/*` (Pig/Cow/Sheep/Chicken),
//! `entity/ai/goal/RandomStrollGoal`, and `NaturalSpawner`. The metadata accessor
//! indices are transcribed from the `SynchedEntityData.defineId` order (see the
//! constants in [`super::entity`]); nothing here copies Mojang code.
//!
//! Parity notes / simplifications (a *reasonable first cut*, not full parity):
//!
//! * **AI** is a single simplified `RandomStrollGoal`: occasionally pick a random
//!   nearby target and walk to it. There is no real pathfinding/navigation graph,
//!   no goal selector priority, no panic/breed/tempt/follow-parent goals, and no
//!   water avoidance. Horizontal velocity is driven straight at the target rather
//!   than solved by `PathNavigation`.
//! * **Physics** reuses the item-entity's single vertical ground probe against
//!   [`crate::world::block_state_at`] (no full AABB sweep, no per-block friction,
//!   no fluids). Horizontal collision is not modelled.
//! * **Speed** is a per-kind blocks/tick constant chosen for a plausible stroll,
//!   *not* derived from the `MOVEMENT_SPEED` attribute × navigation math (Vela has
//!   no attribute system yet).
//! * **Health/damage**: health is modelled and synced, but there is no damage
//!   source wired in — [`damage`] is a ready seam for the survival/combat
//!   milestone. Death removes the entity.

use bevy_ecs::prelude::*;
use bytes::Bytes;
use rand::Rng;

use super::bridge::{Outbound, OutboxTx};
use super::components::{Conn, LoadedChunks, Pos, Tick};
use super::entity::syncher::{DataValue, EntityData};
use super::entity::{
    remove_entity, spawn_net_entity, EntityMeta, NetEntity, ENTITY_DATA_SHARED_FLAGS,
    LIVING_ENTITY_DATA_HEALTH, SHEEP_DATA_WOOL,
};
use super::packets;
use crate::registry::builtin::ENTITY_TYPE;

// --- Movement / AI constants (mirroring the animal goals) --------------------
/// `Entity.getDefaultGravity` for a `LivingEntity` — 0.08 blocks/tick² down.
const LIVING_GRAVITY: f64 = 0.08;
/// `Entity.getAirDrag` vertical retention (0.98) applied to falling velocity.
const AIR_DRAG: f64 = 0.98;
/// `RandomStrollGoal.DEFAULT_INTERVAL` — the goal rolls `nextInt(interval)==0`
/// each tick, so a fresh stroll target is picked on average every 120 ticks.
const STROLL_INTERVAL: u32 = 120;
/// Horizontal search radius for a stroll target (`LandRandomPos` uses 10; we keep
/// a slightly tighter 8 so mobs don't wander off their loaded columns as fast).
const STROLL_RADIUS: f64 = 8.0;
/// Max ticks to pursue one target before giving up (stand-in for the navigation
/// timing out / arriving). Roughly `STROLL_RADIUS / min-speed` with slack.
const STROLL_MAX_TICKS: u32 = 100;
/// Distance at which the mob is considered to have arrived at its target.
const ARRIVE_DIST: f64 = 0.7;
/// A fluttering mob (chicken) never falls faster than this — its wing-flap slow
/// descent, an approximation of `Chicken`'s reduced fall speed.
const FLUTTER_MAX_FALL: f64 = 0.1;

/// The passive-mob kinds Vela spawns. A deliberately small set (the task's
/// "pig, cow, sheep, chicken at minimum"); more kinds slot in by extending this
/// enum and the tables below.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum MobKind {
    Pig,
    Cow,
    Sheep,
    Chicken,
}

impl MobKind {
    /// Every kind, for random selection by the natural spawner.
    const ALL: [MobKind; 4] = [MobKind::Pig, MobKind::Cow, MobKind::Sheep, MobKind::Chicken];

    /// The `entity_type` registry name.
    fn type_name(self) -> &'static str {
        match self {
            MobKind::Pig => "minecraft:pig",
            MobKind::Cow => "minecraft:cow",
            MobKind::Sheep => "minecraft:sheep",
            MobKind::Chicken => "minecraft:chicken",
        }
    }

    /// The `entity_type` registry id (`ClientboundAddEntityPacket` type field).
    fn type_id(self) -> i32 {
        ENTITY_TYPE
            .id_of(self.type_name())
            .expect("passive mob type is a registered entity type")
    }

    /// `Attributes.MAX_HEALTH` for this kind (Pig/Cow 10, Sheep 8, Chicken 4).
    fn max_health(self) -> f32 {
        match self {
            MobKind::Pig | MobKind::Cow => 10.0,
            MobKind::Sheep => 8.0,
            MobKind::Chicken => 4.0,
        }
    }

    /// Per-tick stroll speed in blocks. Chosen for a plausible walk (~2 blocks/s),
    /// *not* derived from the `MOVEMENT_SPEED` attribute — see the module note.
    fn walk_speed(self) -> f64 {
        match self {
            MobKind::Pig | MobKind::Chicken => 0.12,
            MobKind::Sheep => 0.11,
            MobKind::Cow => 0.10,
        }
    }

    /// Whether this kind flutters (slow-falls) rather than dropping at full
    /// gravity — chickens only.
    fn flutters(self) -> bool {
        matches!(self, MobKind::Chicken)
    }
}

/// The living-mob marker + kind. Its metadata/health are separate components so a
/// future combat pass can query health without touching the AI state.
#[derive(Component)]
pub struct Mob {
    pub kind: MobKind,
}

/// A mob's health (`LivingEntity.DATA_HEALTH_ID`), current and max. `max` is the
/// spawn attribute value; it is stored for the combat milestone (heal clamps,
/// regen) which does not exist yet, so it currently has no reader.
#[derive(Component)]
pub struct Health {
    pub current: f32,
    #[allow(dead_code)]
    pub max: f32,
}

/// Per-mob AI + kinematic state: the stroll target and how long to pursue it,
/// velocity, the last-broadcast position/rotation base (the movement-packet delta
/// base, vanilla `ServerEntity.positionCodec`), and a tick counter.
#[derive(Component)]
pub struct MobState {
    /// Current stroll target `(x, y, z)`, or `None` when idle.
    target: Option<(f64, f64, f64)>,
    /// Ticks left to pursue the current target before giving up.
    pursue_ticks: u32,
    /// `deltaMovement` (velocity), blocks/tick.
    vx: f64,
    vy: f64,
    vz: f64,
    /// Last-broadcast position (movement-delta base).
    base_x: f64,
    base_y: f64,
    base_z: f64,
    /// Last-broadcast packed yaw / on-ground, so a change forces the right packet.
    base_yaw: i8,
    base_on_ground: bool,
}

impl MobState {
    fn new(x: f64, y: f64, z: f64, yaw: f32) -> Self {
        Self {
            target: None,
            pursue_ticks: 0,
            vx: 0.0,
            vy: 0.0,
            vz: 0.0,
            base_x: x,
            base_y: y,
            base_z: z,
            base_yaw: packets::pack_angle(yaw),
            base_on_ground: false,
        }
    }
}

/// The chunk column `(cx, cz)` a world position sits in — the per-viewer culling
/// key, matching [`super::entity`]'s and `item_tick`'s helpers.
fn chunk_of(x: f64, z: f64) -> (i32, i32) {
    ((x.floor() as i32) >> 4, (z.floor() as i32) >> 4)
}

/// Whether the world block at `(x, y, z)` obstructs movement. Vela has no
/// per-block collision shapes, so "solid" is "not air" (as in `item_tick`).
fn is_solid(x: i32, y: i32, z: i32) -> bool {
    crate::world::block_state_at(x, y, z) != crate::world::AIR_STATE
}

/// Build a freshly-spawned mob's [`EntityData`]: the shared-flags byte (0), the
/// living-entity health float, and — for a sheep — a random wool colour. Other
/// accessors (variant holders, baby flag, …) are left to the client's registered
/// defaults, which keeps the metadata stream small and avoids the variant-holder
/// serializers Vela does not model yet.
fn spawn_metadata(kind: MobKind, health: f32, rng: &mut impl Rng) -> EntityData {
    let mut meta = EntityData::new();
    meta.set(ENTITY_DATA_SHARED_FLAGS, DataValue::Byte(0));
    meta.set(LIVING_ENTITY_DATA_HEALTH, DataValue::Float(health));
    if kind == MobKind::Sheep {
        // Low nibble is the DyeColor id (0..=15); high bits (incl. 0x10 sheared)
        // stay clear. A random natural colour is a nice type-specific touch.
        let colour = rng.gen_range(0u8..16);
        meta.set(SHEEP_DATA_WOOL, DataValue::Byte(colour));
    }
    meta
}

/// Spawn a passive mob of `kind` at `pos` facing a random yaw, pairing it to every
/// tracking viewer. Returns its network entity id. Public so a future command /
/// spawn-egg path can reuse it; the natural spawner ([`mob_spawn`]) is the live
/// caller today.
pub fn spawn_mob(world: &mut World, kind: MobKind, pos: (f64, f64, f64)) -> i32 {
    let mut rng = rand::thread_rng();
    let yaw = rng.gen_range(0.0f32..360.0);
    let health = kind.max_health();
    let meta = spawn_metadata(kind, health, &mut rng);
    let (x, y, z) = pos;

    let (id, _entity) = spawn_net_entity(world, kind.type_id(), pos, yaw, meta, |ec| {
        ec.insert((
            Mob { kind },
            Health { current: health, max: health },
            MobState::new(x, y, z, yaw),
        ));
    });
    id
}

/// One integration step for a mob: steer toward its stroll target (if any), apply
/// gravity, integrate, and clamp onto a solid block below. Mutates `st`/`pos` in
/// place and returns whether the mob's facing changed enough to resend.
fn step_mob(kind: MobKind, st: &mut MobState, pos: &mut Pos, rng: &mut impl Rng) {
    // --- RandomStrollGoal: acquire a target ---
    if st.target.is_none() && rng.gen_ratio(1, STROLL_INTERVAL) {
        let tx = pos.x + rng.gen_range(-STROLL_RADIUS..STROLL_RADIUS);
        let tz = pos.z + rng.gen_range(-STROLL_RADIUS..STROLL_RADIUS);
        let ty = (crate::world::surface_height(tx.floor() as i32, tz.floor() as i32) + 1) as f64;
        st.target = Some((tx, ty, tz));
        st.pursue_ticks = STROLL_MAX_TICKS;
    }

    // --- Steer horizontally toward the target ---
    if let Some((tx, _ty, tz)) = st.target {
        let dx = tx - pos.x;
        let dz = tz - pos.z;
        let dist = (dx * dx + dz * dz).sqrt();
        if dist < ARRIVE_DIST || st.pursue_ticks == 0 {
            st.target = None;
            st.vx = 0.0;
            st.vz = 0.0;
        } else {
            let speed = kind.walk_speed();
            st.vx = dx / dist * speed;
            st.vz = dz / dist * speed;
            // Minecraft yaw: atan2(-dx, dz) — 0° faces +Z, increasing clockwise.
            pos.yaw = (-dx).atan2(dz).to_degrees() as f32;
            st.pursue_ticks -= 1;
        }
    } else {
        st.vx = 0.0;
        st.vz = 0.0;
    }

    // --- applyGravity ---
    st.vy -= LIVING_GRAVITY;
    if kind.flutters() && st.vy < -FLUTTER_MAX_FALL {
        st.vy = -FLUTTER_MAX_FALL;
    }

    // --- move(deltaMovement): integrate + resolve a downward collision ---
    let nx = pos.x + st.vx;
    let mut ny = pos.y + st.vy;
    let nz = pos.z + st.vz;

    let mut on_ground = false;
    if st.vy < 0.0 {
        let (bx, bz) = (nx.floor() as i32, nz.floor() as i32);
        let by = ny.floor() as i32;
        if is_solid(bx, by, bz) {
            ny = (by + 1) as f64; // land on the block's top face
            on_ground = true;
            st.vy = 0.0;
        }
    }

    pos.x = nx;
    pos.y = ny;
    pos.z = nz;
    pos.on_ground = on_ground;

    // Vertical air drag; horizontal velocity is re-derived from the target each
    // tick, so no residual horizontal drag is needed.
    st.vy *= AIR_DRAG;
}

/// The mob behavior system: wander AI, physics, and movement broadcast for every
/// [`Mob`]. Registered after `item_tick` in the schedule.
pub fn mob_tick(world: &mut World) {
    // Attach state to any mob spawned without it (defensive; `spawn_mob` seeds it).
    let uninit: Vec<Entity> = world
        .query_filtered::<Entity, (With<Mob>, Without<MobState>)>()
        .iter(world)
        .collect();
    for e in uninit {
        let (x, y, z, yaw) = {
            let p = world.get::<Pos>(e).expect("mob has Pos");
            (p.x, p.y, p.z, p.yaw)
        };
        world.entity_mut(e).insert(MobState::new(x, y, z, yaw));
    }

    let mut emissions: Vec<((i32, i32), Bytes)> = Vec::new();

    {
        let mut rng = rand::thread_rng();
        let mut q = world.query::<(&NetEntity, &Mob, &mut Pos, &mut MobState)>();
        for (net, mob, mut pos, mut st) in q.iter_mut(world) {
            step_mob(mob.kind, &mut st, &mut pos, &mut rng);

            let chunk = chunk_of(pos.x, pos.z);
            let yaw_n = packets::pack_angle(pos.yaw);

            // Position delta from the last-sent base (vanilla VecDeltaCodec).
            let dx = packets::enc(pos.x) - packets::enc(st.base_x);
            let dy = packets::enc(pos.y) - packets::enc(st.base_y);
            let dz = packets::enc(pos.z) - packets::enc(st.base_z);
            let moved = dx != 0 || dy != 0 || dz != 0;
            let turned = yaw_n != st.base_yaw;
            let ground_flip = pos.on_ground != st.base_on_ground;

            let fits = (-32768..=32767).contains(&dx)
                && (-32768..=32767).contains(&dz)
                && (-32768..=32767).contains(&dy);

            if (moved || turned || ground_flip) && (!fits || ground_flip) {
                // A relative delta won't do (too large, or the on-ground flag
                // flipped): resync absolutely.
                emissions.push((
                    chunk,
                    packets::entity_position_sync(
                        net.id, pos.x, pos.y, pos.z, pos.yaw, pos.pitch, pos.on_ground,
                    ),
                ));
                st.base_x = pos.x;
                st.base_y = pos.y;
                st.base_z = pos.z;
                st.base_yaw = yaw_n;
                st.base_on_ground = pos.on_ground;
                emissions.push((chunk, packets::rotate_head(net.id, yaw_n)));
            } else if moved {
                emissions.push((
                    chunk,
                    packets::move_entity_pos_rot(
                        net.id,
                        dx as i16,
                        dy as i16,
                        dz as i16,
                        yaw_n,
                        pos.pitch as i8, // pitch stays 0 for a strolling mob
                        pos.on_ground,
                    ),
                ));
                st.base_x = pos.x;
                st.base_y = pos.y;
                st.base_z = pos.z;
                if turned {
                    st.base_yaw = yaw_n;
                    emissions.push((chunk, packets::rotate_head(net.id, yaw_n)));
                }
            } else if turned {
                emissions.push((
                    chunk,
                    packets::move_entity_rot(net.id, yaw_n, pos.pitch as i8, pos.on_ground),
                ));
                emissions.push((chunk, packets::rotate_head(net.id, yaw_n)));
                st.base_yaw = yaw_n;
            }
        }
    }

    flush_emissions(world, &emissions);
}

/// Fan queued per-chunk packets out to every player whose loaded-chunk set covers
/// the source column — the same delivery the item/entity paths use.
fn flush_emissions(world: &mut World, emissions: &[((i32, i32), Bytes)]) {
    if emissions.is_empty() {
        return;
    }
    let conns: Vec<(std::collections::HashSet<(i32, i32)>, OutboxTx)> = {
        let mut q = world.query::<(&LoadedChunks, &Conn)>();
        q.iter(world)
            .map(|(l, c)| (l.loaded.clone(), c.outbox.clone()))
            .collect()
    };
    for (chunk, pkt) in emissions {
        for (loaded, outbox) in &conns {
            if loaded.contains(chunk) {
                let _ = outbox.try_send(Outbound::Packet(pkt.clone()));
            }
        }
    }
}

// --- Natural spawning --------------------------------------------------------
/// How often (in ticks) the natural spawner runs. Vanilla runs spawn attempts
/// every tick with per-mob-category caps; Vela keeps a coarser cadence with a
/// single global cap — a nice-to-have "keep the world populated" pass, not the
/// full `NaturalSpawner`.
const SPAWN_ATTEMPT_INTERVAL: u64 = 40;
/// Global passive-mob cap. Well under vanilla's per-chunk math, but enough to make
/// the area around players feel alive without unbounded growth.
const MOB_CAP: usize = 15;
/// Min/max horizontal distance from a player to place a natural spawn.
const SPAWN_MIN_DIST: f64 = 8.0;
const SPAWN_MAX_DIST: f64 = 24.0;

/// A player snapshot for the spawner: position and the columns they have loaded
/// (so a spawn is only placed where a viewer will actually see it).
struct PlayerSnap {
    x: f64,
    z: f64,
    loaded: std::collections::HashSet<(i32, i32)>,
}

/// A simple natural spawner: every [`SPAWN_ATTEMPT_INTERVAL`] ticks, if the world
/// holds fewer than [`MOB_CAP`] mobs and at least one player is online, place one
/// random passive mob on the surface a short distance from a random player — but
/// only where that column is already loaded (so the spawn has viewers and stands
/// on real terrain). Registered before `mob_tick` in the schedule.
pub fn mob_spawn(world: &mut World) {
    let tick = world.resource::<Tick>().0;
    if !tick.is_multiple_of(SPAWN_ATTEMPT_INTERVAL) {
        return;
    }
    let mob_count = world.query::<&Mob>().iter(world).count();
    if mob_count >= MOB_CAP {
        return;
    }

    // Snapshot online players (position + their loaded columns).
    let players: Vec<PlayerSnap> = {
        let mut q = world.query::<(&Pos, &LoadedChunks)>();
        q.iter(world)
            .map(|(p, l)| PlayerSnap { x: p.x, z: p.z, loaded: l.loaded.clone() })
            .collect()
    };
    if players.is_empty() {
        return;
    }

    let mut rng = rand::thread_rng();
    let anchor = &players[rng.gen_range(0..players.len())];
    // Random point in a ring around the player.
    let angle = rng.gen_range(0.0f64..std::f64::consts::TAU);
    let dist = rng.gen_range(SPAWN_MIN_DIST..SPAWN_MAX_DIST);
    let x = anchor.x + angle.cos() * dist;
    let z = anchor.z + angle.sin() * dist;
    if !anchor.loaded.contains(&chunk_of(x, z)) {
        return; // no viewer has this column loaded — skip this attempt.
    }
    let y = (crate::world::surface_height(x.floor() as i32, z.floor() as i32) + 1) as f64;

    // Centre the mob on the block it spawns over, like vanilla's `+ 0.5` offset.
    let pos = (x.floor() + 0.5, y, z.floor() + 0.5);
    let kind = MobKind::ALL[rng.gen_range(0..MobKind::ALL.len())];
    let id = spawn_mob(world, kind, pos);
    tracing::debug!(?kind, id, x = pos.0, y = pos.1, z = pos.2, "spawned passive mob");
}

// --- Damage seam (for the survival/combat milestone) -------------------------
/// Apply `amount` damage to a mob, syncing the new health to viewers and returning
/// `true` if the blow was fatal (the caller should then [`remove_entity`] it, which
/// this does). A clean seam for the combat milestone: there is no damage *source*
/// (attacker, knockback, i-frames, `HurtAnimation`) yet — only the health field
/// and its metadata sync, which is the part the mob model owns.
#[allow(dead_code)]
pub fn damage(world: &mut World, entity: Entity, amount: f32) -> bool {
    let (net_id, chunk, dead, new_health) = {
        let Some(mut health) = world.get_mut::<Health>(entity) else {
            return false;
        };
        health.current = (health.current - amount).max(0.0);
        let dead = health.current <= 0.0;
        let new_health = health.current;
        let net_id = world.get::<NetEntity>(entity).map(|n| n.id);
        let chunk = world.get::<Pos>(entity).map(|p| chunk_of(p.x, p.z));
        match (net_id, chunk) {
            (Some(id), Some(c)) => (id, c, dead, new_health),
            _ => return false,
        }
    };

    // Sync the updated health metadata to viewers.
    if let Some(mut meta) = world.get_mut::<EntityMeta>(entity) {
        meta.0
            .set(LIVING_ENTITY_DATA_HEALTH, DataValue::Float(new_health));
    }
    let data = {
        let meta = world.get::<EntityMeta>(entity).expect("mob has EntityMeta");
        super::entity::packets::set_entity_data(net_id, &meta.0)
    };
    flush_emissions(world, &[(chunk, data)]);

    if dead {
        remove_entity(world, entity);
    }
    dead
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::buffer::PacketReader;
    use crate::sim::components::NextEntityId;
    // The generic net-entity spawn/metadata packets live in the entity submodule
    // (8-arg `add_entity`, `EntityData`-taking `set_entity_data`), distinct from
    // the player-specific builders in `super::packets`.
    use crate::sim::entity::packets as epackets;
    use std::collections::HashSet;
    use tokio::sync::mpsc;

    fn frame_id(b: &Bytes) -> i32 {
        let mut r = PacketReader::new(b.clone());
        r.read_varint().unwrap(); // length
        r.read_varint().unwrap() // id
    }

    fn world_with_id() -> World {
        let mut world = World::new();
        world.insert_resource(NextEntityId(1));
        world.insert_resource(Tick(0));
        world
    }

    /// A viewer whose loaded set covers `chunks`.
    fn spawn_viewer(world: &mut World, chunks: &[(i32, i32)]) -> mpsc::Receiver<Outbound> {
        let (tx, rx) = mpsc::channel(256);
        world.spawn((
            Conn { outbox: tx },
            LoadedChunks {
                center: (0, 0),
                loaded: chunks.iter().copied().collect::<HashSet<_>>(),
            },
            // Give the viewer a position so the natural spawner can key off it.
            Pos { x: 0.0, y: 64.0, z: 0.0, yaw: 0.0, pitch: 0.0, on_ground: true },
        ));
        rx
    }

    fn drain(rx: &mut mpsc::Receiver<Outbound>) -> Vec<i32> {
        let mut ids = Vec::new();
        while let Ok(Outbound::Packet(b)) = rx.try_recv() {
            ids.push(frame_id(&b));
        }
        ids
    }

    #[test]
    fn mob_type_ids_match_registry() {
        assert_eq!(MobKind::Pig.type_id(), ENTITY_TYPE.id_of("minecraft:pig").unwrap());
        assert_eq!(MobKind::Cow.type_id(), ENTITY_TYPE.id_of("minecraft:cow").unwrap());
        assert_eq!(MobKind::Sheep.type_id(), ENTITY_TYPE.id_of("minecraft:sheep").unwrap());
        assert_eq!(
            MobKind::Chicken.type_id(),
            ENTITY_TYPE.id_of("minecraft:chicken").unwrap()
        );
    }

    #[test]
    fn spawn_pairs_add_then_data_and_seeds_components() {
        let mut world = world_with_id();
        let mut rx = spawn_viewer(&mut world, &[(0, 0)]);
        let id = spawn_mob(&mut world, MobKind::Pig, (1.0, 64.0, 2.0));
        assert_eq!(id, 1);

        // AddEntity then SetEntityData reach the in-range viewer.
        let ids = drain(&mut rx);
        let add_id =
            frame_id(&epackets::add_entity(0, uuid::Uuid::nil(), 0, (0.0, 0.0, 0.0), 0, 0, 0, 0));
        let data_id = frame_id(&epackets::set_entity_data(0, &EntityData::new()));
        assert_eq!(ids, vec![add_id, data_id]);

        // Components exist: Mob + Health (full) + MobState.
        let (mob, health) = world
            .query::<(&Mob, &Health)>()
            .iter(&world)
            .next()
            .map(|(m, h)| (m.kind, (h.current, h.max)))
            .unwrap();
        assert_eq!(mob, MobKind::Pig);
        assert_eq!(health, (10.0, 10.0));
        assert_eq!(world.query::<&MobState>().iter(&world).count(), 1);
    }

    #[test]
    fn spawn_metadata_carries_shared_flags_and_health() {
        let mut rng = rand::thread_rng();
        let meta = spawn_metadata(MobKind::Cow, 10.0, &mut rng);
        // Encode and verify shared-flags byte (0) + health float are present.
        let mut p = crate::protocol::buffer::PacketWriter::new();
        meta.write_packed(&mut p);
        let mut r = PacketReader::new(Bytes::from(p.buf.to_vec()));
        assert_eq!(r.read_u8().unwrap(), ENTITY_DATA_SHARED_FLAGS);
        assert_eq!(r.read_varint().unwrap(), 0); // BYTE serializer
        assert_eq!(r.read_u8().unwrap(), 0); // flags value
        assert_eq!(r.read_u8().unwrap(), LIVING_ENTITY_DATA_HEALTH);
        assert_eq!(r.read_varint().unwrap(), 3); // FLOAT serializer
        assert_eq!(r.read_f32().unwrap(), 10.0);
        assert_eq!(r.read_u8().unwrap(), 0xFF); // terminator (no sheep wool for a cow)
    }

    #[test]
    fn sheep_metadata_includes_wool_colour() {
        let mut rng = rand::thread_rng();
        let meta = spawn_metadata(MobKind::Sheep, 8.0, &mut rng);
        let wool = meta
            .items()
            .iter()
            .find(|it| it.index == SHEEP_DATA_WOOL)
            .expect("sheep carries a wool colour");
        match wool.value {
            DataValue::Byte(c) => assert!(c < 16, "colour is a 0..=15 nibble"),
            _ => panic!("wool colour must be a byte"),
        }
    }

    #[test]
    fn falling_mob_lands_on_ground_and_broadcasts() {
        // A pig spawned in the air over the generated surface falls, lands, and
        // its movement/landing is broadcast to a viewer.
        let mut world = world_with_id();
        let sy = crate::world::surface_height(0, 0);
        let mut rx = spawn_viewer(&mut world, &[(0, 0)]);
        spawn_mob(&mut world, MobKind::Pig, (0.5, (sy + 10) as f64, 0.5));
        let _ = drain(&mut rx); // discard the spawn pair

        // Tick until it settles or a bound is hit.
        let mut grounded = false;
        for _ in 0..200 {
            mob_tick(&mut world);
            let p = world.query::<&Pos>().iter(&world).next().unwrap();
            if p.on_ground {
                grounded = true;
                assert!((p.y - (sy + 1) as f64).abs() < 1.0);
                break;
            }
        }
        assert!(grounded, "mob should land on the surface");
        // The fall produced at least one movement packet for the viewer.
        assert!(!drain(&mut rx).is_empty(), "landing should broadcast movement");
    }

    #[test]
    fn resting_mob_emits_nothing() {
        // A pig already resting on the surface produces no movement packets.
        let mut world = world_with_id();
        let sy = crate::world::surface_height(0, 0);
        let mut rx = spawn_viewer(&mut world, &[(0, 0)]);
        // Spawn exactly on the surface; force it settled by ticking once first.
        spawn_mob(&mut world, MobKind::Pig, (0.5, (sy + 1) as f64, 0.5));
        let _ = drain(&mut rx);
        // First tick lands it (vy just below 0), a couple settle it.
        for _ in 0..3 {
            mob_tick(&mut world);
        }
        let _ = drain(&mut rx);
        // Clear any wander target so the next tick is truly idle.
        {
            let mut st = world.query::<&mut MobState>().iter_mut(&mut world).next().unwrap();
            st.target = None;
        }
        mob_tick(&mut world);
        assert!(drain(&mut rx).is_empty(), "a settled, idle mob is silent");
    }

    #[test]
    fn natural_spawner_populates_up_to_cap_then_stops() {
        let mut world = world_with_id();
        let _rx = spawn_viewer(&mut world, &[(0, 0)]);
        // Drive many attempts on the 40-tick cadence; the cap bounds the count.
        for t in 0..(SPAWN_ATTEMPT_INTERVAL * (MOB_CAP as u64 + 5)) {
            world.resource_mut::<Tick>().0 = t;
            mob_spawn(&mut world);
        }
        let count = world.query::<&Mob>().iter(&world).count();
        assert!(count > 0, "spawner should place at least one mob");
        assert!(count <= MOB_CAP, "spawner must respect the cap");
    }

    #[test]
    fn damage_reduces_health_and_kills() {
        let mut world = world_with_id();
        let mut rx = spawn_viewer(&mut world, &[(0, 0)]);
        spawn_mob(&mut world, MobKind::Chicken, (0.5, 64.0, 0.5)); // 4 hp
        let _ = drain(&mut rx);
        let e = world.query_filtered::<Entity, With<Mob>>().iter(&world).next().unwrap();

        assert!(!damage(&mut world, e, 1.0)); // 4 -> 3, survives
        assert_eq!(world.get::<Health>(e).unwrap().current, 3.0);
        // A metadata update was broadcast.
        let data_id = frame_id(&epackets::set_entity_data(0, &EntityData::new()));
        assert!(drain(&mut rx).contains(&data_id));

        assert!(damage(&mut world, e, 10.0)); // fatal
        assert_eq!(world.query::<&Mob>().iter(&world).count(), 0, "dead mob removed");
        // Removal broadcast reached the viewer.
        let remove_id = frame_id(&crate::sim::packets::remove_entities(&[0]));
        assert!(drain(&mut rx).contains(&remove_id));
    }
}
