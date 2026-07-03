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
//! * **Physics** models collision at *block granularity* against
//!   [`crate::world::block_state_at`] (the world is effectively full cubes, so
//!   "solid" = "not air" is the only shape test): the `Entity.move`/`collide`
//!   axis-separated clip is ported — Y first, then the larger horizontal axis
//!   (`Direction.axisStepOrder`) — sweeping each box face against the block cells it
//!   would enter, zeroing the collided velocity component (vanilla's restitution-0
//!   `restituteMovementAfterCollisions`). There is still no per-block friction and no
//!   fluids. **Step-up** (`maxUpStep` 0.6) is *moot* here: with full cubes and no
//!   slabs a 0.6 step can never climb a 1.0 block, so a bumped ledge is cleared by a
//!   jump, never a step.
//! * **Jumping** ports both vanilla triggers (`LivingEntity.aiStep` jump block →
//!   `jumpFromGround`, `vy = max(JUMP_STRENGTH·0.42, vy)`, `noJumpDelay = 10`): the
//!   `MoveControl.tick` "wantedY is a step above" trigger (`yd > maxUpStep && dx²+dz²
//!   < max(1, bbWidth)`, with the stroll target's Y as `wantedY`), and a
//!   navigator-stand-in trigger that fires when the mob bumped a wall last tick
//!   (`horizontalCollision`), is on the ground, and the blocking cell is a 1-high
//!   ledge (air directly above it). The latter replaces the ground `PathNavigation`
//!   producing a node one block up that `MoveControl` would jump toward.
//! * **Speed** is derived from the vanilla `MOVEMENT_SPEED` attribute default per
//!   kind fed through the `MoveControl` + `LivingEntity.travelInAir` pipeline
//!   (acceleration + `blockFriction·0.91` drag), matching the vanilla steady-state
//!   ground walk. Straight-line steering (no `PathNavigation` A*) is the only gap.
//! * **Movement broadcast** ports `ServerEntity.sendChanges` 1:1 (see
//!   [`broadcast_movement`]): the 3-tick `updateInterval` gate, the
//!   `positionChanged` tolerance, the 60-tick forced resend, the 400-tick forced
//!   teleport, the Pos/PosRot/Rot decision tree, the motion packet, and the
//!   trailing head-rotation check.
//! * **Health/damage**: health is modelled and synced, and [`damage`] applies the
//!   vanilla `LivingEntity.hurtServer` i-frame gate (`invulnerableTime`/`lastHurt`).
//!   There is still no full damage *source* (knockback, attacker-relative hurt
//!   direction, armour/absorption). Death removes the entity.

use bevy_ecs::prelude::*;
use bytes::Bytes;
use rand::seq::SliceRandom;
use rand::Rng;

use super::components::{Conn, LoadedChunks, Pos, Tick};
use super::entity::syncher::{DataValue, EntityData};
use super::entity::{
    remove_entity, spawn_item_entity, spawn_net_entity, spawn_xp_orb, EntityMeta, NetEntity,
    AGEABLE_MOB_DATA_BABY, ENTITY_DATA_SHARED_FLAGS, LIVING_ENTITY_DATA_HEALTH, SHEEP_DATA_WOOL,
};
use super::packets;
use crate::inventory::ItemStack;
use crate::registry::builtin::{ENTITY_TYPE, SOUND_EVENT};

// --- Movement / AI constants (mirroring the animal goals) --------------------
/// `Entity.getDefaultGravity` for a `LivingEntity` — 0.08 blocks/tick² down.
const LIVING_GRAVITY: f64 = 0.08;
/// The tolerance used when turning a box's max face into a block-cell index: a face
/// resting exactly on an integer boundary (`maxY == 65.0`) must not count the block
/// it merely touches (`Shapes.collide` treats flush contact as non-overlapping).
const COLLISION_EPS: f64 = 1.0e-7;
/// `Attributes.STEP_HEIGHT` default (0.6) -> `Entity.maxUpStep()`. Only read for the
/// `MoveControl.tick` jump trigger's `yd > maxUpStep` gate; no step-up is performed
/// (moot with full cubes -- see the module header).
const MAX_UP_STEP: f64 = 0.6;
/// `Attributes.JUMP_STRENGTH` default (0.42) -> `LivingEntity.getJumpPower` on normal
/// blocks with no jump-boost (`0.42 * 1.0 * blockJumpFactor(1.0) + 0`). These mobs
/// never sprint, so the sprint-boost term of `jumpFromGround` never applies.
const JUMP_POWER: f64 = 0.42;
/// `LivingEntity.aiStep`: `noJumpDelay = 10` after a jump, decremented each tick; a
/// new jump is gated on `noJumpDelay == 0`.
const NO_JUMP_DELAY: i32 = 10;
/// `LivingEntity.travelInAir` vertical friction (`computeModifiedFriction(0.98F, 1)`)
/// applied to the post-move `deltaMovement.y` after gravity is subtracted.
const VERTICAL_AIR_DRAG: f64 = 0.98;
/// Default block friction (`BlockBehaviour` `friction = 0.6F`). Vela models no
/// per-block friction, so every solid surface uses this. Applied as the ground
/// `blockFriction` in `LivingEntity.travelInAir`; airborne uses `1.0`.
const BLOCK_FRICTION: f64 = 0.6;
/// `LivingEntity.travelInAir` horizontal air-drag factor
/// (`computeModifiedFriction(0.91F, 1)`). Horizontal `deltaMovement` is scaled by
/// `blockFriction * HORIZONTAL_AIR_DRAG` each tick after `move`.
const HORIZONTAL_AIR_DRAG: f64 = 0.91;
/// `LivingEntity.getFlyingSpeed` for a non-player-controlled mob — the tiny
/// airborne acceleration factor `moveRelative` uses while `!onGround()`.
const FLYING_SPEED: f64 = 0.02;
/// `MoveControl.MAX_TURN` — the yaw slews at most 90°/tick toward the target
/// (`rotlerp(current, target, 90.0F)`).
const MAX_TURN: f32 = 90.0;
/// `MoveControl.MIN_SPEED_SQR` — below this squared distance to the wanted point
/// the move controller stops feeding a forward input (`setZza(0)`).
const MIN_SPEED_SQR: f64 = 2.500_000_3e-7;
/// `RandomStrollGoal.DEFAULT_INTERVAL` (120). `GoalSelector` runs `canUse` only on
/// the "full tick" (`Mob.serverAiStep`: `(tickCount+id) % 2 == 0`), and the roll is
/// `nextInt(reducedTickDelay(120))` where `reducedTickDelay = ceilDiv(120, 2) = 60`.
/// Net cadence: every 2nd tick, `nextInt(60) == 0` — an average of one stroll every
/// 120 ticks. Vela has no per-entity id offset, so tick parity is the gate stand-in.
const STROLL_ROLL: u32 = 60;
/// `Mob.checkDespawn` resets `noActionTime` to 0 while a player is within
/// `MobCategory.noDespawnDistance` (32) blocks; `RandomStrollGoal.canUse` refuses to
/// stroll once `getNoActionTime() >= 100` — i.e. mobs only wander near a player.
const NO_DESPAWN_DISTANCE: f64 = 32.0;
const NO_ACTION_STROLL_LIMIT: i32 = 100;
/// `LandRandomPos.getPos(mob, 10, 7)` search extents (all four animals use
/// `WaterAvoidingRandomStrollGoal`, which 99.9% of the time calls this): the random
/// direction is `nextInt(2*10+1)-10` per horizontal axis and `nextInt(2*7+1)-7`
/// vertical.
const STROLL_H_DIST: i32 = 10;
const STROLL_V_DIST: i32 = 7;
/// Max ticks to pursue one target before giving up (stand-in for the navigation
/// timing out / arriving — Vela has no `PathNavigation`).
const STROLL_MAX_TICKS: u32 = 100;
/// Distance at which the mob is considered to have arrived at its target. Stand-in
/// for `PathNavigation.isDone()`; Vela has no waypoint acceptance radius to mirror.
const ARRIVE_DIST: f64 = 0.7;
/// `Chicken.aiStep`: while airborne and descending, `deltaMovement.y` is scaled by
/// this each tick (`movement.multiply(1.0, 0.6, 1.0)`), giving the wing-flap slow
/// descent — a decaying multiplier, not a hard fall-speed cap.
const CHICKEN_FALL_MULTIPLIER: f64 = 0.6;
/// `LivingEntity.hurtServer` invulnerability window: a fresh hit sets
/// `invulnerableTime` to 20; while it is `> INVULNERABLE_GATE` (the first 10 ticks)
/// only the excess over `lastHurt` lands. Both are the vanilla constants.
const INVULNERABLE_TIME: i32 = 20;
const INVULNERABLE_GATE: i32 = 10;
/// `LivingEntity.hurtDuration` — the red hurt-flash timer set on a full hit.
const HURT_DURATION: i32 = 10;
/// `LivingEntity.tickDeath`: a dead mob stays as a corpse playing the death
/// animation and is removed once `deathTime >= 20`.
const DEATH_TICKS: i32 = 20;
/// The `ClientboundEntityEventPacket` event `LivingEntity.tickDeath` broadcasts
/// when the corpse is removed — `(byte)60` (`makePoofParticles`).
const ENTITY_EVENT_POOF: u8 = 60;
/// `LivingEntity.dealDefaultKnockback` strength (`knockback(0.4F, …)`). Farm
/// animals have `Attributes.KNOCKBACK_RESISTANCE` 0, so the scaled power stays 0.4.
const KNOCKBACK_POWER: f64 = 0.4;
/// `LivingEntity.knockback`'s vertical clamp — `min(0.4, deltaMovement.y/2 + power)`
/// while on the ground.
const KNOCKBACK_VERTICAL_CAP: f64 = 0.4;
/// `LivingEntity.knockback`'s minimum-direction guard: below this squared
/// horizontal length a tiny random direction is substituted so the push is never
/// degenerate (`xd*xd + zd*zd < 1.0E-5`).
const KNOCKBACK_MIN_DIR_SQ: f64 = 1.0e-5;
/// `resolvePlayerResponsibleForDamage`'s `setLastHurtByPlayer(player, 100)` memory
/// window: loot/XP gate on `lastHurtByPlayerMemoryTime > 0` at death.
const PLAYER_HURT_MEMORY_TIME: i32 = 100;
/// `SoundSource.NEUTRAL` enum ordinal — the category `Entity.getSoundSource`
/// returns for animals (MASTER 0, MUSIC 1, RECORDS 2, WEATHER 3, BLOCKS 4,
/// HOSTILE 5, NEUTRAL 6, …). Sent as the `ClientboundSoundPacket` source.
const SOUND_SOURCE_NEUTRAL: i32 = 6;
/// `LivingEntity.getSoundVolume` for these animals — the base 1.0.
const SOUND_VOLUME: f32 = 1.0;
/// `minecraft:player_attack` — the melee damage type carried by the
/// `ClientboundDamageEventPacket`.
const DAMAGE_TYPE_PLAYER_ATTACK: &str = "minecraft:player_attack";

// --- Movement-broadcast constants (mirroring `ServerEntity`) ------------------
/// `EntityType.Builder`'s default `updateInterval` (3). Pig/cow/sheep/chicken all
/// keep the builder default (no `.updateInterval(...)` override in their
/// `EntityTypes` registrations), so `ServerEntity.sendChanges` evaluates their
/// movement broadcast only every 3rd tick.
const UPDATE_INTERVAL: u32 = 3;
/// `ServerEntity.FORCED_POS_UPDATE_PERIOD` — a position packet goes out when
/// `tickCount % 60 == 0` even without movement.
const FORCED_POS_UPDATE_PERIOD: u32 = 60;
/// `ServerEntity.FORCED_TELEPORT_PERIOD` — an absolute position sync is forced
/// once `teleportDelay > 400`.
const FORCED_TELEPORT_PERIOD: u32 = 400;
/// `ServerEntity.TOLERANCE_LEVEL_POSITION` (`7.6293945E-6F` widened to double):
/// the squared block-unit distance from the codec base under which the position
/// counts as unchanged.
const TOLERANCE_LEVEL_POSITION: f64 = 7.6293945e-6_f32 as f64;
/// `ServerEntity.sendChanges`' velocity-drift gate: a motion packet is sent when
/// the squared distance from `lastSentMovement` exceeds `1.0E-7` (or on any
/// nonzero drift that lands exactly on zero velocity).
const MOTION_TOLERANCE_SQ: f64 = 1.0e-7;

/// The passive-mob kinds Vela spawns. A deliberately small set (the task's
/// "pig, cow, sheep, chicken at minimum"); more kinds slot in by extending this
/// enum and the tables below.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MobKind {
    Pig,
    Cow,
    Sheep,
    Chicken,
}

impl MobKind {
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

    /// `Attributes.MOVEMENT_SPEED` default from each kind's `createAttributes`:
    /// Pig 0.25, Chicken 0.25, Sheep 0.23, Cow 0.2. Vela has no attribute system, so
    /// this per-kind constant stands in for the attribute value. Fed through the
    /// vanilla `MoveControl`/`travelInAir` pipeline (see [`step_mob`]) — *not* used as
    /// a direct blocks/tick velocity — so the steady-state ground walk speed is the
    /// much smaller `speed² / (1 - 0.6·0.91)` (≈0.138 b/t for Pig/Chicken).
    fn movement_speed(self) -> f64 {
        match self {
            MobKind::Pig | MobKind::Chicken => 0.25,
            MobKind::Sheep => 0.23,
            MobKind::Cow => 0.2,
        }
    }

    /// `EntityType.Builder.sized(width, height)` from each kind's `EntityTypes`
    /// registration: Pig 0.9×0.9, Cow 0.9×1.4, Sheep 0.9×1.3, Chicken 0.4×0.7. The
    /// bounding box is horizontally centred on the position (`AABB` half-width
    /// `width/2`) and spans `[y, y + height]` vertically.
    fn bb_width(self) -> f64 {
        match self {
            MobKind::Pig | MobKind::Cow | MobKind::Sheep => 0.9,
            MobKind::Chicken => 0.4,
        }
    }

    fn bb_height(self) -> f64 {
        match self {
            MobKind::Pig => 0.9,
            MobKind::Cow => 1.4,
            MobKind::Sheep => 1.3,
            MobKind::Chicken => 0.7,
        }
    }

    /// Whether this kind flutters (slow-falls) rather than dropping at full
    /// gravity — chickens only.
    fn flutters(self) -> bool {
        matches!(self, MobKind::Chicken)
    }

    /// `LivingEntity.getHurtSound` for this kind — the `sound_event` registry name.
    /// Pig/Cow/Chicken resolve their default (temperate) variant sound set, whose
    /// `hurtSound` is `entity.<kind>.hurt`; Sheep is `SoundEvents.SHEEP_HURT`.
    fn hurt_sound(self) -> &'static str {
        match self {
            MobKind::Pig => "minecraft:entity.pig.hurt",
            MobKind::Cow => "minecraft:entity.cow.hurt",
            MobKind::Sheep => "minecraft:entity.sheep.hurt",
            MobKind::Chicken => "minecraft:entity.chicken.hurt",
        }
    }

    /// `LivingEntity.getDeathSound` for this kind (`entity.<kind>.death`).
    fn death_sound(self) -> &'static str {
        match self {
            MobKind::Pig => "minecraft:entity.pig.death",
            MobKind::Cow => "minecraft:entity.cow.death",
            MobKind::Sheep => "minecraft:entity.sheep.death",
            MobKind::Chicken => "minecraft:entity.chicken.death",
        }
    }

    /// `Animal.getBaseExperienceReward` = `1 + random.nextInt(3)` (1..=3). Vela has
    /// no enchantments, so `getExperienceReward` (which would apply
    /// `EnchantmentHelper.processMobExperience`) reduces to this base value.
    fn base_experience_reward(self, rng: &mut impl Rng) -> i32 {
        1 + rng.gen_range(0..3)
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
    /// Last-broadcast position (`ServerEntity.positionCodec` base — the raw
    /// coordinates at the last *sent* position packet, updated only then).
    base_x: f64,
    base_y: f64,
    base_z: f64,
    /// `ServerEntity.lastSentYRot` / `lastSentXRot` — packed body rotation at the
    /// last packet that carried rotation, updated only then.
    base_yaw: i8,
    base_pitch: i8,
    /// `ServerEntity.lastSentYHeadRot` — packed head yaw at the last
    /// `ClientboundRotateHeadPacket` (Vela's mobs keep head == body yaw).
    last_sent_head_yaw: i8,
    /// `ServerEntity.wasOnGround` — a flip forces an absolute position sync.
    base_on_ground: bool,
    /// `ServerEntity.lastSentMovement` — velocity at the last motion packet.
    last_sent_vx: f64,
    last_sent_vy: f64,
    last_sent_vz: f64,
    /// `ServerEntity.tickCount` — per-entity; increments every server tick, and
    /// the broadcast body runs only when `tick_count % UPDATE_INTERVAL == 0`.
    tick_count: u32,
    /// `ServerEntity.teleportDelay` — incremented each broadcast evaluation,
    /// reset when an absolute sync is sent; `> 400` forces one.
    teleport_delay: u32,
    /// `LivingEntity.invulnerableTime` — the i-frame counter, set to 20 on a full
    /// hit and decremented each tick. While `> INVULNERABLE_GATE`, only damage
    /// exceeding `last_hurt` lands (see [`damage`]).
    invulnerable_time: i32,
    /// `LivingEntity.hurtTime` — the hurt-flash timer, set to `HURT_DURATION` on a
    /// full hit and decremented each tick. Tracked for parity; not otherwise read.
    hurt_time: i32,
    /// `LivingEntity.lastHurt` — the damage of the most recent hit within the
    /// i-frame window, used to compute the marginal damage of a re-hit.
    last_hurt: f32,
    /// `LivingEntity.noActionTime` — increments each `serverAiStep` and is reset to
    /// 0 by `Mob.checkDespawn` while a player is within 32 blocks. `RandomStrollGoal`
    /// refuses to start once this reaches 100.
    no_action_time: i32,
    /// `LivingEntity.dead` — set on the killing blow (`handleKillingBlow`). A dead
    /// mob stops its wander AI and just falls as a corpse until removal.
    dead: bool,
    /// `LivingEntity.deathTime` — increments each tick while dead
    /// (`tickDeath`); at [`DEATH_TICKS`] the corpse is removed.
    death_time: i32,
    /// `LivingEntity.lastHurtByPlayerMemoryTime` — set to [`PLAYER_HURT_MEMORY_TIME`]
    /// when a player deals damage and decremented each tick. Loot/XP at death gate
    /// on this being `> 0` (`dropAllDeathLoot`/`dropExperience`).
    last_hurt_by_player_time: i32,
    /// `Entity.horizontalCollision` from the *previous* tick's `move` — the
    /// navigator-stand-in jump trigger reads it (a mob that bumped a wall last tick
    /// jumps this tick, mirroring the path node one block up that would fire
    /// `MoveControl`'s jump the following tick).
    horizontal_collision: bool,
    /// `LivingEntity.noJumpDelay` — set to 10 on a jump, decremented each tick; a new
    /// jump only fires at 0. Reset to 0 on any tick the mob is not requesting a jump
    /// (vanilla's `else { this.noJumpDelay = 0; }`).
    no_jump_delay: i32,
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
            base_pitch: 0, // mobs spawn with pitch 0 (packDegrees(0) == 0)
            last_sent_head_yaw: packets::pack_angle(yaw),
            base_on_ground: false,
            last_sent_vx: 0.0,
            last_sent_vy: 0.0,
            last_sent_vz: 0.0,
            tick_count: 0,
            teleport_delay: 0,
            invulnerable_time: 0,
            hurt_time: 0,
            last_hurt: 0.0,
            no_action_time: 0,
            dead: false,
            death_time: 0,
            last_hurt_by_player_time: 0,
            horizontal_collision: false,
            no_jump_delay: 0,
        }
    }
}

/// Whether the world block at `(x, y, z)` obstructs movement. Vela has no
/// per-block collision shapes, so "solid" is "not air" (as in `item_tick`).
///
/// Non-generating: a cold (non-resident) column reads as **solid**. This runs on
/// the tick thread (mob movement/collision every tick), so it must never
/// generate a chunk. Treating an unloaded cell as an obstacle is the conservative
/// parity choice — vanilla's `Entity.move` never advances into an unloaded chunk,
/// so a mob at the loaded edge simply comes to rest rather than the tick thread
/// stalling on a worldgen build. A ticking mob's own column is always resident,
/// so this only bites a probe that reaches a cold neighbour.
fn is_solid(x: i32, y: i32, z: i32) -> bool {
    crate::world::try_block_state_at(x, y, z)
        .map(|s| s != crate::world::AIR_STATE)
        .unwrap_or(true)
}

/// The inclusive block-index range a `[lo, hi]` box interval overlaps. The trailing
/// face is nudged in by [`COLLISION_EPS`] so a box whose max face lands exactly on an
/// integer (`hi == 65.0`) does not count the block it merely touches — matching
/// `Shapes.collide`, which treats flush contact as non-overlapping.
fn cell_span(lo: f64, hi: f64) -> std::ops::RangeInclusive<i32> {
    (lo.floor() as i32)..=((hi - COLLISION_EPS).floor() as i32)
}

/// `Shapes.collide(axis, box, shapes, d)` for a full-cube world: clip the scalar
/// movement `d` of a box face so the box comes to rest flush against the nearest
/// solid block layer it would otherwise enter. `min`/`max` are the box's extent on
/// the moving axis; `solid(i)` reports whether block layer `i` (spanning `[i, i+1)`
/// on the moving axis) is solid anywhere across the box's footprint on the other two
/// axes. Returns the (possibly shortened) movement, preserving sign.
fn collide_axis(min: f64, max: f64, d: f64, solid: impl Fn(i32) -> bool) -> f64 {
    if d == 0.0 {
        return 0.0;
    }
    if d > 0.0 {
        // Leading face `max` sweeps toward `max + d`. The first block layer it can
        // enter is the one whose near (min) face is at the next integer >= max.
        let target = max + d;
        let mut c = (max - COLLISION_EPS).ceil() as i32;
        while (c as f64) < target {
            if solid(c) {
                // Rest flush: `max + d == c`.
                return c as f64 - max;
            }
            c += 1;
        }
        d
    } else {
        // Leading face `min` sweeps toward `min + d`. The first block layer it can
        // enter is the one whose far (max) face is at the next integer <= min.
        let target = min + d;
        let mut f = (min + COLLISION_EPS).floor() as i32;
        while (f as f64) > target {
            // Block layer `[f - 1, f)` has its far face at `f`.
            if solid(f - 1) {
                // Rest flush: `min + d == f`.
                return f as f64 - min;
            }
            f -= 1;
        }
        d
    }
}

/// The result of `Entity.move`'s collision resolution for a full-cube world: the
/// clipped per-axis movement plus the collision flags vanilla derives from it.
struct MoveResult {
    dx: f64,
    dy: f64,
    dz: f64,
    horizontal_collision: bool,
    /// `verticalCollisionBelow` — a downward clip → `onGround`.
    on_ground: bool,
    /// An upward clip (head hit a ceiling) → the jump arc's `vy` is zeroed.
    ceiling: bool,
}

/// Port of `Entity.move` → `collide` → `collideBoundingBox` → `collideWithShapes` at
/// block granularity. `(x, y, z)` are the box's feet-centre (position); `hw` the
/// half-width, `h` the height. Axes resolve in vanilla's `Direction.axisStepOrder`:
/// Y first, then the larger-magnitude horizontal axis (X before Z unless
/// `|dz| > |dx|`), each against a box translated by the axes resolved so far.
fn move_and_collide(x: f64, y: f64, z: f64, hw: f64, h: f64, dx: f64, dy: f64, dz: f64) -> MoveResult {
    // Static box extents on each axis (untranslated).
    let (min_x, max_x) = (x - hw, x + hw);
    let (min_y, max_y) = (y, y + h);
    let (min_z, max_z) = (z - hw, z + hw);

    let (mut rx, mut rz) = (0.0_f64, 0.0_f64);

    // `Direction.axisStepOrder`: Y always first; then X,Z unless |dx| < |dz|.
    let horizontal_x_first = dx.abs() >= dz.abs();

    // Y axis, box translated by (rx, rz) resolved so far (both still 0 here, but
    // written generally to mirror `boundingBox.move(resolved)`).
    let resolve_y = |rx: f64, rz: f64| {
        let (xa, xb) = (min_x + rx, max_x + rx);
        let (za, zb) = (min_z + rz, max_z + rz);
        collide_axis(min_y, max_y, dy, |iy| {
            cell_span(xa, xb).any(|ix| cell_span(za, zb).any(|iz| is_solid(ix, iy, iz)))
        })
    };
    let resolve_x = |ry: f64, rz: f64| {
        let (ya, yb) = (min_y + ry, max_y + ry);
        let (za, zb) = (min_z + rz, max_z + rz);
        collide_axis(min_x, max_x, dx, |ix| {
            cell_span(ya, yb).any(|iy| cell_span(za, zb).any(|iz| is_solid(ix, iy, iz)))
        })
    };
    let resolve_z = |rx: f64, ry: f64| {
        let (xa, xb) = (min_x + rx, max_x + rx);
        let (ya, yb) = (min_y + ry, max_y + ry);
        collide_axis(min_z, max_z, dz, |iz| {
            cell_span(xa, xb).any(|ix| cell_span(ya, yb).any(|iy| is_solid(ix, iy, iz)))
        })
    };

    let ry = resolve_y(rx, rz);
    if horizontal_x_first {
        rx = resolve_x(ry, rz);
        rz = resolve_z(rx, ry);
    } else {
        rz = resolve_z(rx, ry);
        rx = resolve_x(ry, rz);
    }

    // `Entity.move`: `xCollision = !equal(dx, movement.x)`, etc.
    let x_collision = rx != dx;
    let z_collision = rz != dz;
    let vertical_collision = ry != dy;
    MoveResult {
        dx: rx,
        dy: ry,
        dz: rz,
        horizontal_collision: x_collision || z_collision,
        on_ground: vertical_collision && dy < 0.0,
        ceiling: vertical_collision && dy > 0.0,
    }
}

/// Build a freshly-spawned mob's [`EntityData`]: the shared-flags byte (0), the
/// living-entity health float, the baby flag when `baby`, and — for a sheep — a
/// biome-appropriate wool colour. Other accessors (variant holders, …) are left
/// to the client's registered defaults, which keeps the metadata stream small and
/// avoids the variant-holder serializers Vela does not model yet.
fn spawn_metadata(
    kind: MobKind,
    health: f32,
    pos: (f64, f64, f64),
    baby: bool,
    rng: &mut impl Rng,
) -> EntityData {
    let mut meta = EntityData::new();
    meta.set(ENTITY_DATA_SHARED_FLAGS, DataValue::Byte(0));
    meta.set(LIVING_ENTITY_DATA_HEALTH, DataValue::Float(health));
    // `AgeableMob.DATA_BABY_ID` defaults to `false` client-side, so emit it only
    // for an actual baby (`AgeableMob.finalizeSpawn` → `setAge(getBabyStartAge())`
    // sets the flag). Vela models no age counter / growth: a baby stays a baby.
    if baby {
        meta.set(AGEABLE_MOB_DATA_BABY, DataValue::Boolean(true));
    }
    if kind == MobKind::Sheep {
        // Low nibble is the DyeColor id (0..=15); high bits (incl. 0x10 sheared)
        // stay clear.
        meta.set(
            SHEEP_DATA_WOOL,
            DataValue::Byte(sheep_color(pos.0.floor() as i32, pos.2.floor() as i32, rng)),
        );
    }
    meta
}

/// The climate class the sheep-colour table keys off — the `getSheepColorConfiguration`
/// branch selected by the biome tags.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SheepClimate {
    Temperate,
    Warm,
    Cold,
}

/// `SheepColorSpawnRules.getSheepColorConfiguration` for the biome at `(wx, wz)`:
/// `SPAWNS_WARM_VARIANT_FARM_ANIMALS` → warm, `SPAWNS_COLD_VARIANT_FARM_ANIMALS`
/// → cold, else temperate. Only the tagged biomes Vela actually generates are
/// listed; the warm tag also covers warm/lukewarm oceans, mangrove swamp and
/// badlands, and the cold tag many peak/frozen/old-growth biomes Vela has no
/// generator for — those simply never occur here.
fn sheep_climate(wx: i32, wz: i32) -> SheepClimate {
    match crate::world::biome_at(wx, wz).name() {
        // warm tag: desert, #is_jungle, #is_savanna (of Vela's set).
        "minecraft:desert" | "minecraft:jungle" | "minecraft:savanna" => SheepClimate::Warm,
        // cold tag: snowy_plains, snowy_taiga, taiga, windswept_hills (of Vela's set).
        "minecraft:snowy_plains"
        | "minecraft:snowy_taiga"
        | "minecraft:taiga"
        | "minecraft:windswept_hills" => SheepClimate::Cold,
        _ => SheepClimate::Temperate,
    }
}

/// A naturally-spawned sheep's wool colour, as a `DyeColor` id nibble, mirroring
/// `SheepColorSpawnRules.getSheepColor`.
///
/// Each of the three `SheepColorSpawnConfiguration`s is a weighted outer list of
/// the same shape — three `single` colours at weight 5, one at weight 3, and an
/// 82-weight `commonColors(default)` block — where `commonColors(d)` is itself
/// `d` 499 / PINK 1 (total 500). Vanilla selects the outer weighted list, then the
/// inner one, so the two draws are kept. DyeColor ids: WHITE=0, PINK=6, GRAY=7,
/// LIGHT_GRAY=8, BROWN=12, BLACK=15.
fn sheep_color(wx: i32, wz: i32, rng: &mut impl Rng) -> u8 {
    const WHITE: u8 = 0;
    const PINK: u8 = 6;
    const GRAY: u8 = 7;
    const LIGHT_GRAY: u8 = 8;
    const BROWN: u8 = 12;
    const BLACK: u8 = 15;
    // The four `single`s (weights 5,5,5,3) then the `commonColors` default (82),
    // in each configuration's builder insertion order.
    let (w5a, w5b, w5c, w3, common) = match sheep_climate(wx, wz) {
        SheepClimate::Temperate => (BLACK, GRAY, LIGHT_GRAY, BROWN, WHITE),
        SheepClimate::Warm => (GRAY, LIGHT_GRAY, WHITE, BLACK, BROWN),
        SheepClimate::Cold => (LIGHT_GRAY, GRAY, WHITE, BROWN, BLACK),
    };
    match rng.gen_range(0u32..100) {
        0..=4 => w5a,   // weight 5
        5..=9 => w5b,   // weight 5
        10..=14 => w5c, // weight 5
        15..=17 => w3,  // weight 3
        // weight 82: the commonColors block — default 499 / PINK 1 (total 500).
        _ => {
            if rng.gen_range(0u32..500) == 499 {
                PINK
            } else {
                common
            }
        }
    }
}

/// Spawn a passive mob of `kind` at `pos` facing a random yaw, pairing it to every
/// tracking viewer. Returns its network entity id. Public so a command / spawn-egg
/// path can reuse it (spawns an adult); the natural spawner ([`mob_spawn`]) goes
/// through [`spawn_mob_with`] so it can request babies.
#[allow(dead_code)] // public spawn-egg/command seam; the live caller is `spawn_mob_with`.
pub fn spawn_mob(world: &mut World, kind: MobKind, pos: (f64, f64, f64)) -> i32 {
    spawn_mob_with(world, kind, pos, false)
}

/// [`spawn_mob`] with the natural-spawner baby flag. `mob.snapTo(x, y, z,
/// random.nextFloat() * 360.0F, 0.0F)` — a uniform random yaw, pitch 0.
fn spawn_mob_with(world: &mut World, kind: MobKind, pos: (f64, f64, f64), baby: bool) -> i32 {
    let mut rng = rand::thread_rng();
    let yaw = rng.gen_range(0.0f32..360.0);
    let health = kind.max_health();
    let meta = spawn_metadata(kind, health, pos, baby, &mut rng);
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

/// `Mth.wrapDegrees` — normalise an angle to `[-180, 180)`.
fn wrap_degrees(value: f32) -> f32 {
    let mut v = value % 360.0;
    if v >= 180.0 {
        v -= 360.0;
    }
    if v < -180.0 {
        v += 360.0;
    }
    v
}

/// `MoveControl.rotlerp` — turn `a` toward `b` by at most `max` degrees, keeping the
/// result in `[0, 360)`.
fn rotlerp(a: f32, b: f32, max: f32) -> f32 {
    let diff = wrap_degrees(b - a).clamp(-max, max);
    let mut result = a + diff;
    if result < 0.0 {
        result += 360.0;
    } else if result > 360.0 {
        result -= 360.0;
    }
    result
}

/// `WaterAvoidingRandomStrollGoal.getPosition` → `LandRandomPos.getPos(mob, 10, 7)`:
/// a random direction offset `nextInt(2*d+1)-d` per axis from the mob's block, then
/// `movePosUpOutOfSolid` (walk up while the block is solid). Vanilla draws 10
/// candidates and keeps the highest `getWalkTargetValue`, but that weight is 0 for
/// plain animals, so the *first* valid candidate always wins — and Vela rejects
/// none (no water/malus model), so a single draw reproduces the outcome. The result
/// is `Vec3.atBottomCenterOf(pos)` = block centre at its bottom face.
fn stroll_target(pos: &Pos, rng: &mut impl Rng) -> Option<(f64, f64, f64)> {
    let xt = rng.gen_range(0..(2 * STROLL_H_DIST + 1)) - STROLL_H_DIST;
    let yt = rng.gen_range(0..(2 * STROLL_V_DIST + 1)) - STROLL_V_DIST;
    let zt = rng.gen_range(0..(2 * STROLL_H_DIST + 1)) - STROLL_H_DIST;
    let bx = pos.x.floor() as i32 + xt;
    let mut by = pos.y.floor() as i32 + yt;
    let bz = pos.z.floor() as i32 + zt;

    // `RandomPos.moveUpOutOfSolid` — rise until the block is not solid.
    let max_y = crate::world::MIN_Y + crate::world::SECTION_COUNT * 16;
    while by <= max_y && is_solid(bx, by, bz) {
        by += 1;
    }
    Some((bx as f64 + 0.5, by as f64, bz as f64 + 0.5))
}

/// One integration step for a mob: run the wander goal, steer via the `MoveControl`
/// + `LivingEntity.travelInAir` pipeline (acceleration + friction drag), integrate,
/// and clamp onto a solid block below. `players` carries live player positions for
/// the `noActionTime` reset. Mutates `st`/`pos` in place.
///
/// Ordering mirrors `LivingEntity.aiStep`: the AI (`serverAiStep` → `MoveControl`)
/// sets the forward input and turns the yaw, then `travel` runs
/// `moveRelative` (add acceleration) → `move` (integrate, using the velocity that
/// already includes this tick's horizontal acceleration but *last* tick's vertical
/// velocity) → subtract gravity → apply friction. Chicken's flutter is applied by
/// `Chicken.aiStep` *after* `super.aiStep()`, i.e. after the whole travel step.
fn step_mob(kind: MobKind, st: &mut MobState, pos: &mut Pos, players: &[(f64, f64, f64)], rng: &mut impl Rng) {
    // --- LivingEntity.tick: decrement the hit timers (mobs are never ServerPlayer,
    // so invulnerableTime always ticks down here). ---
    if st.hurt_time > 0 {
        st.hurt_time -= 1;
    }
    if st.invulnerable_time > 0 {
        st.invulnerable_time -= 1;
    }
    // --- LivingEntity.baseTick: the lastHurtByPlayer memory counts down. ---
    if st.last_hurt_by_player_time > 0 {
        st.last_hurt_by_player_time -= 1;
    }

    // --- LivingEntity.baseTick → tickDeath: a dead mob is a corpse. It runs no AI
    // (isImmobile() is true while dead), but travel still applies, so it keeps
    // falling; its deathTime advances toward removal (handled in mob_tick). ---
    if st.dead {
        st.death_time += 1;
        travel_and_integrate(kind, st, pos, 0.0);
        return;
    }

    // --- Mob.serverAiStep / Mob.checkDespawn: noActionTime increments each tick and
    // is reset to 0 while a player is within noDespawnDistance (32) blocks. ---
    st.no_action_time += 1;
    let near_player = players.iter().any(|&(px, py, pz)| {
        let (dx, dy, dz) = (px - pos.x, py - pos.y, pz - pos.z);
        dx * dx + dy * dy + dz * dz < NO_DESPAWN_DISTANCE * NO_DESPAWN_DISTANCE
    });
    if near_player {
        st.no_action_time = 0;
    }

    // --- RandomStrollGoal.canUse via GoalSelector: evaluated on the "full tick"
    // (every 2nd tick), refused while noActionTime >= 100, else roll nextInt(60)==0
    // and pick a LandRandomPos target. ---
    if st.target.is_none()
        && st.tick_count % 2 == 0
        && st.no_action_time < NO_ACTION_STROLL_LIMIT
        && rng.gen_ratio(1, STROLL_ROLL)
    {
        if let Some(t) = stroll_target(pos, rng) {
            st.target = Some(t);
            st.pursue_ticks = STROLL_MAX_TICKS;
        }
    }

    // --- MoveControl.tick (MOVE_TO): turn the yaw toward the target (≤90°/tick) and
    // derive the forward input `zza = speedModifier(1.0) * MOVEMENT_SPEED`. ---
    // `speed` is the attribute value; `zza`/the forward input equal it (Mob.setSpeed
    // → setZza), so the local forward acceleration below is `zza * fricSpeed`.
    let mut forward_input = 0.0_f64;
    let mut jump_requested = false;
    if let Some((tx, ty, tz)) = st.target {
        let dx = tx - pos.x;
        let dy = ty - pos.y;
        let dz = tz - pos.z;
        let dist_h = (dx * dx + dz * dz).sqrt();
        // Arrival / give-up stand-in for `PathNavigation.isDone()`.
        if dist_h < ARRIVE_DIST || st.pursue_ticks == 0 {
            st.target = None;
        } else {
            st.pursue_ticks -= 1;
            // `MoveControl.tick`: skip the turn/input when essentially on the point.
            if dx * dx + dy * dy + dz * dz >= MIN_SPEED_SQR {
                // `yRotD = atan2(zd, xd) * 180/PI - 90`, then rotlerp toward it.
                let target_yaw = dz.atan2(dx).to_degrees() as f32 - 90.0;
                pos.yaw = rotlerp(pos.yaw, target_yaw, MAX_TURN);
                forward_input = kind.movement_speed(); // zza == speedModifier·speed
                // `MoveControl.tick` jump trigger (a): the wanted point is more than a
                // step above and horizontally within `max(1, bbWidth)` blocks (== 1
                // for these mobs — all narrower than a block) → `getJumpControl().jump()`.
                if dy > MAX_UP_STEP && dx * dx + dz * dz < 1.0_f64.max(kind.bb_width()) {
                    jump_requested = true;
                }
            }
        }
    }

    // Navigator stand-in for `MoveControl`'s wall-climb (trigger b): with no
    // `PathNavigation`, the mob never gets a path node one block up to steer toward.
    // Instead, when it bumped a wall on the previous tick (`horizontalCollision`), is
    // on the ground, and is facing a 1-high ledge (solid ahead with air directly
    // above), request a jump — reproducing the observable "walking mob hops a
    // 1-block ledge" behaviour.
    if st.horizontal_collision && pos.on_ground && facing_one_high_ledge(kind, pos) {
        jump_requested = true;
    }

    // `LivingEntity.aiStep` jump block, run *before* `travel`: decrement the cooldown,
    // then — if a jump was requested and the mob is grounded with the cooldown
    // elapsed — `jumpFromGround` (`vy = max(getJumpPower(), vy)`) and re-arm the
    // 10-tick `noJumpDelay`. A tick with no jump requested resets the delay to 0
    // (vanilla's `else { this.noJumpDelay = 0; }`).
    if st.no_jump_delay > 0 {
        st.no_jump_delay -= 1;
    }
    if jump_requested {
        if pos.on_ground && st.no_jump_delay == 0 {
            st.vy = JUMP_POWER.max(st.vy);
            st.no_jump_delay = NO_JUMP_DELAY;
        }
    } else {
        st.no_jump_delay = 0;
    }

    travel_and_integrate(kind, st, pos, forward_input);
}

/// The navigator-stand-in ledge probe: is the block the mob is facing — just beyond
/// its bounding box at foot level — a solid 1-high ledge (solid with air directly
/// above)? Uses the body yaw as the facing direction, since `MoveControl` steers the
/// body toward the target and thus into the wall it bumped.
fn facing_one_high_ledge(kind: MobKind, pos: &Pos) -> bool {
    let hw = kind.bb_width() / 2.0;
    let yaw_rad = (pos.yaw as f64).to_radians();
    // Minecraft yaw forward vector: (-sin(yaw), cos(yaw)) on (x, z).
    let (dirx, dirz) = (-yaw_rad.sin(), yaw_rad.cos());
    // Probe 0.1 blocks past the leading face, into the neighbouring column.
    let fx = (pos.x + dirx * (hw + 0.1)).floor() as i32;
    let fz = (pos.z + dirz * (hw + 0.1)).floor() as i32;
    let fy = pos.y.floor() as i32;
    is_solid(fx, fy, fz) && !is_solid(fx, fy + 1, fz)
}

/// The `LivingEntity.travelInAir` → `move` → gravity/friction pipeline, split out
/// so both a live mob (with a `forward_input` from its `MoveControl`) and a dead
/// corpse (zero input, but still falling — vanilla dead mobs are `isImmobile()`
/// yet `travel` still runs) share the exact integration. Mutates `st`/`pos`.
fn travel_and_integrate(kind: MobKind, st: &mut MobState, pos: &mut Pos, forward_input: f64) {
    // --- LivingEntity.travelInAir + handleRelativeFrictionAndCalculateMovement ---
    // blockFriction reflects last tick's onGround (vanilla reads this.onGround()
    // before move updates it); airborne uses 1.0.
    let block_friction = if pos.on_ground { BLOCK_FRICTION } else { 1.0 };
    // `getFrictionInfluencedSpeed`: on ground with default friction 0.6 the
    // `blockFriction > 0.6` branch is false, so it returns getSpeed() (== the
    // attribute speed); airborne it returns getFlyingSpeed() (0.02).
    let fric_influenced_speed = if pos.on_ground {
        kind.movement_speed()
    } else {
        FLYING_SPEED
    };

    // `moveRelative(fricSpeed, input=(0,0,zza))`: the local forward is
    // `zza * fricSpeed` (input length < 1, so it is not normalised), rotated by yaw.
    let accel_fwd = forward_input * fric_influenced_speed;
    if accel_fwd != 0.0 {
        let yaw_rad = (pos.yaw as f64).to_radians();
        st.vx += -accel_fwd * yaw_rad.sin();
        st.vz += accel_fwd * yaw_rad.cos();
    }

    // `move(deltaMovement)`: integrate the velocity (this tick's horizontal
    // acceleration + this tick's vertical velocity, including any jump just applied)
    // through the block-granularity axis-separated collision clip.
    let mv = move_and_collide(
        pos.x,
        pos.y,
        pos.z,
        kind.bb_width() / 2.0,
        kind.bb_height(),
        st.vx,
        st.vy,
        st.vz,
    );
    let on_ground = mv.on_ground;
    pos.x += mv.dx;
    pos.y += mv.dy;
    pos.z += mv.dz;
    pos.on_ground = on_ground;
    st.horizontal_collision = mv.horizontal_collision;

    // `restituteMovementAfterCollisions` with restitution 0: a collided axis zeroes
    // its velocity component. A clipped horizontal face rests flush; a vertical clip —
    // landing (below) or a head hitting a ceiling (above) — zeroes `vy`.
    if mv.dx != st.vx {
        st.vx = 0.0;
    }
    if mv.dz != st.vz {
        st.vz = 0.0;
    }
    if mv.on_ground || mv.ceiling {
        st.vy = 0.0;
    }

    // Post-move: `movementY -= gravity; setDeltaMovement(x*friction, y*vFric, z*friction)`.
    st.vy = (st.vy - LIVING_GRAVITY) * VERTICAL_AIR_DRAG;
    let friction = block_friction * HORIZONTAL_AIR_DRAG;
    st.vx *= friction;
    st.vz *= friction;

    // --- Chicken.aiStep: after super.aiStep()'s travel, `if (!onGround &&
    // deltaMovement.y < 0.0)` scale the downward velocity by 0.6 — a decaying
    // flutter, not a hard cap. ---
    if kind.flutters() && !on_ground && st.vy < 0.0 {
        st.vy *= CHICKEN_FALL_MULTIPLIER;
    }
}

/// One `ServerEntity.sendChanges` movement-broadcast evaluation for a mob,
/// pushing any packets onto `emissions`. Ports the non-passenger, non-minecart
/// branch of the vanilla method 1:1 (MC 26.2 `ServerEntity.sendChanges`, the
/// `tickCount % updateInterval` gate through the trailing head-rotation check):
///
/// * The body runs only when `tick_count % updateInterval == 0` (vanilla also
///   runs it on `needsSync`/dirty entity data — Vela has neither: teleports
///   don't exist for mobs yet and metadata is flushed immediately elsewhere).
/// * `positionChanged` is `positionCodec.delta(pos).lengthSqr() >= 7.6293945E-6`
///   in raw block units, and a position packet is due when it moved **or**
///   `tickCount % 60 == 0` (the forced resend).
/// * An absolute `ClientboundEntityPositionSyncPacket` replaces the relative
///   packet when the encoded delta overflows a short, `teleportDelay > 400`, or
///   the on-ground flag flipped (`wasOnGround != onGround()`).
/// * Otherwise the vanilla decision tree picks `Pos` (moved only), `Rot`
///   (turned only, on packed bytes), or `PosRot` (both).
/// * The motion packet goes out first (mobs have `trackDeltas()` true) when the
///   velocity drifted from `lastSentMovement` by more than `1.0E-7` squared.
/// * The position base updates only when a position was actually sent, and the
///   last-sent rotation only when rotation was sent.
/// * Head yaw is handled separately at the end: `ClientboundRotateHeadPacket`
///   iff the packed head yaw differs from `lastSentYHeadRot` (Vela's mobs keep
///   head == body yaw, but the two last-sent values are tracked independently).
fn broadcast_movement(
    entity: Entity,
    net_id: i32,
    pos: &Pos,
    st: &mut MobState,
    emissions: &mut Vec<(Entity, Bytes)>,
) {
    // `if (this.tickCount % this.updateInterval == 0 || ...)`.
    if st.tick_count % UPDATE_INTERVAL == 0 {
        let yaw_n = packets::pack_angle(pos.yaw);
        let pitch_n = packets::pack_angle(pos.pitch);
        // `Math.abs(yRotn - lastSentYRot) >= 1 || Math.abs(xRotn - lastSentXRot)
        // >= 1` — on int-promoted packed bytes, which is exactly `!=`.
        let should_send_rotation = yaw_n != st.base_yaw || pitch_n != st.base_pitch;

        st.teleport_delay += 1;
        // `positionCodec.delta(currentPosition).lengthSqr() >= TOLERANCE`.
        let ddx = pos.x - st.base_x;
        let ddy = pos.y - st.base_y;
        let ddz = pos.z - st.base_z;
        let position_changed =
            ddx * ddx + ddy * ddy + ddz * ddz >= TOLERANCE_LEVEL_POSITION;
        // `boolean pos = positionChanged || this.tickCount % 60 == 0`.
        let send_pos = position_changed || st.tick_count % FORCED_POS_UPDATE_PERIOD == 0;

        // Encoded 1/4096-block delta from the codec base (`VecDeltaCodec.encodeX/Y/Z`).
        let xa = packets::enc(pos.x) - packets::enc(st.base_x);
        let ya = packets::enc(pos.y) - packets::enc(st.base_y);
        let za = packets::enc(pos.z) - packets::enc(st.base_z);
        let delta_too_big = !((-32768..=32767).contains(&xa)
            && (-32768..=32767).contains(&ya)
            && (-32768..=32767).contains(&za));

        let mut packet: Option<Bytes> = None;
        let mut sent_position = false;
        let mut sent_rotation = false;
        if delta_too_big || st.teleport_delay > FORCED_TELEPORT_PERIOD || pos.on_ground != st.base_on_ground {
            // Absolute resync (vanilla also triggers on requiresPrecisePosition /
            // wasRiding, which Vela's mobs never set).
            st.base_on_ground = pos.on_ground;
            st.teleport_delay = 0;
            packet = Some(packets::entity_position_sync(
                net_id, pos.x, pos.y, pos.z, pos.yaw, pos.pitch, pos.on_ground,
            ));
            sent_position = true;
            sent_rotation = true;
        } else if !(send_pos && should_send_rotation) {
            if send_pos {
                packet = Some(packets::move_entity_pos(
                    net_id, xa as i16, ya as i16, za as i16, pos.on_ground,
                ));
                sent_position = true;
            } else if should_send_rotation {
                packet = Some(packets::move_entity_rot(net_id, yaw_n, pitch_n, pos.on_ground));
                sent_rotation = true;
            }
        } else {
            packet = Some(packets::move_entity_pos_rot(
                net_id, xa as i16, ya as i16, za as i16, yaw_n, pitch_n, pos.on_ground,
            ));
            sent_position = true;
            sent_rotation = true;
        }

        // Motion broadcast — animals have `EntityType.trackDeltas()` true, so
        // this always runs for them; vanilla sends it *before* the move packet.
        let mvx = st.vx - st.last_sent_vx;
        let mvy = st.vy - st.last_sent_vy;
        let mvz = st.vz - st.last_sent_vz;
        let diff = mvx * mvx + mvy * mvy + mvz * mvz;
        let len_sqr = st.vx * st.vx + st.vy * st.vy + st.vz * st.vz;
        if diff > MOTION_TOLERANCE_SQ || (diff > 0.0 && len_sqr == 0.0) {
            st.last_sent_vx = st.vx;
            st.last_sent_vy = st.vy;
            st.last_sent_vz = st.vz;
            emissions.push((entity, packets::set_entity_motion(net_id, st.vx, st.vy, st.vz)));
        }

        if let Some(pkt) = packet {
            emissions.push((entity, pkt));
        }
        // `if (sentPosition) positionCodec.setBase(currentPosition)` — the raw
        // position, even when the packet carried an unchanged rotation.
        if sent_position {
            st.base_x = pos.x;
            st.base_y = pos.y;
            st.base_z = pos.z;
        }
        if sent_rotation {
            st.base_yaw = yaw_n;
            st.base_pitch = pitch_n;
        }

        // Head rotation, unconditionally at the end of the vanilla body:
        // `if (Math.abs(yHeadRot - lastSentYHeadRot) >= 1)` — `!=` on the bytes.
        let head_n = yaw_n; // Vela's mobs keep head yaw == body yaw
        if head_n != st.last_sent_head_yaw {
            emissions.push((entity, packets::rotate_head(net_id, head_n)));
            st.last_sent_head_yaw = head_n;
        }
    }

    // `this.tickCount++` — every tick, gated or not.
    st.tick_count += 1;
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

    let mut emissions: Vec<(Entity, Bytes)> = Vec::new();

    // Snapshot player positions for the `noActionTime` reset (a mob only wanders
    // while a player is within 32 blocks). Players carry a `Conn`; mobs do not.
    let players: Vec<(f64, f64, f64)> = {
        let mut q = world.query_filtered::<&Pos, With<Conn>>();
        q.iter(world).map(|p| (p.x, p.y, p.z)).collect()
    };

    // Corpses whose death animation has run its course (`tickDeath` reached
    // `deathTime >= 20`): collected here so removal happens after the borrow of the
    // movement query is released.
    let mut expired: Vec<(Entity, i32)> = Vec::new();

    {
        let mut rng = rand::thread_rng();
        let mut q = world.query::<(Entity, &NetEntity, &Mob, &mut Pos, &mut MobState)>();
        for (entity, net, mob, mut pos, mut st) in q.iter_mut(world) {
            step_mob(mob.kind, &mut st, &mut pos, &players, &mut rng);
            broadcast_movement(entity, net.id, &pos, &mut st, &mut emissions);
            if st.dead && st.death_time >= DEATH_TICKS {
                expired.push((entity, net.id));
            }
        }
    }

    flush_emissions(world, &emissions);

    // `LivingEntity.tickDeath`: broadcast the poof `EntityEvent` (byte 60) to the
    // corpse's trackers, then `remove(KILLED)` (which fans a RemoveEntities). The
    // poof must reach viewers before the removal drops them from the tracking set.
    for (entity, net_id) in expired {
        let poof = super::entity::packets::entity_event(net_id, ENTITY_EVENT_POOF);
        flush_emissions(world, &[(entity, poof)]);
        remove_entity(world, entity);
    }
}

/// Fan queued per-entity packets out to each source entity's tracking set,
/// mirroring `ServerEntity.sendToTrackingPlayers` — the same delivery the generic
/// entity path uses (see [`super::entity::fan_to_seen`]). Keying by the source ECS
/// entity (not its chunk) is what keeps movement fan-out identical to the spawn
/// pairing: a player is fed a mob iff the mob is spawned on their client.
fn flush_emissions(world: &mut World, emissions: &[(Entity, Bytes)]) {
    super::entity::fan_to_seen(world, emissions);
}

// --- Natural spawning --------------------------------------------------------
// A 1:1 port of vanilla's CREATURE-category natural spawning: the `gameTime % 400`
// persistent-category gate (`ServerChunkCache.tickChunks`), the global mob-cap math
// (`NaturalSpawner.SpawnState.canSpawnForCategoryGlobal`), the per-player local cap
// (`LocalMobCapCalculator`), and the per-chunk pack loop
// (`NaturalSpawner.spawnCategoryForChunk` / `spawnCategoryForPosition`).
//
// Vela stand-ins (see the per-item docs below): "spawnableChunkCount" is the union
// of every player's loaded columns rather than the distance-manager's entity-ticking
// count; a "redstone conductor" full block is stood in for by "solid" (not air); and
// the biome spawn list is the hard-coded `BiomeDefaultFeatures.farmAnimals` table
// (Vela's biomes carry no `MobSpawnSettings`), which is the every-overworld-biome
// list anyway.

/// `ServerChunkCache.tickChunks`: CREATURE is a *persistent* category
/// (`MobCategory.CREATURE.isPersistent()`), so it only joins the spawning
/// categories when `spawnPersistent = gameTime % 400 == 0`. Vela spawns only
/// CREATURE mobs today, so the whole spawn pass runs on that cadence.
const SPAWN_PERSISTENT_INTERVAL: u64 = 400;

/// The live persistent-spawn cadence. Vanilla's 400, unless `VELA_SPAWN_INTERVAL_TICKS`
/// overrides it — a test-only lever the natural-spawn integration test uses to fire
/// many spawn passes per second (so it doesn't have to wait out real 20-second
/// cadences) without touching *what* gets spawned. Unset/unparsable → 400. Read once.
fn spawn_persistent_interval() -> u64 {
    static INTERVAL: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *INTERVAL.get_or_init(|| {
        std::env::var("VELA_SPAWN_INTERVAL_TICKS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&t| t > 0)
            .unwrap_or(SPAWN_PERSISTENT_INTERVAL)
    })
}
/// `MobCategory.CREATURE.getMaxInstancesPerChunk()`.
const CREATURE_MAX_PER_CHUNK: i32 = 10;
/// `NaturalSpawner.MAGIC_NUMBER` — `17²`, the divisor in the global cap formula.
const MAGIC_NUMBER: i32 = 289;
/// `MobCategory.CREATURE.getDespawnDistance()` (128) squared: a farm animal
/// (`canSpawnFarFromPlayer() == false`) is rejected past this from every player.
const CREATURE_DESPAWN_DISTANCE_SQR: f64 = 128.0 * 128.0;
/// `NaturalSpawner.MIN_SPAWN_DISTANCE` (24) squared — `isRightDistanceToPlayerAndSpawnPoint`
/// rejects a member whose nearest player is within this (`<= 576.0`).
const MIN_SPAWN_DISTANCE_SQR: f64 = 576.0;
/// `ChunkMap.getPlayersCloseForSpawning` / `euclideanDistanceSquared` bound
/// (`< 16384.0`) — 128 blocks from a chunk's centre, the "player near a chunk"
/// test the local mob cap keys off.
const SPAWN_RANGE_SQR: f64 = 16384.0;
/// The per-member scatter of a pack (`x += nextInt(6) - nextInt(6)`) in
/// `spawnCategoryForPosition`.
const PACK_SPREAD: i32 = 6;
/// The outer group loop count in `spawnCategoryForPosition` (`groupCount < 3`).
const GROUP_ATTEMPTS: i32 = 3;
/// `Mob.getMaxSpawnClusterSize()` (4) — the pack loop returns once this many mobs
/// have spawned from one start position.
const MAX_SPAWN_CLUSTER: i32 = 4;
/// Farm-animal `SpawnerData` `minCount == maxCount == 4` (see [`FARM_ANIMALS`]),
/// so the pack size `minCount + nextInt(1 + maxCount - minCount)` is always 4.
const PACK_MIN: i32 = 4;
const PACK_MAX: i32 = 4;
/// `AgeableMob.AgeableMobGroupData` default `babySpawnChance` (0.05).
const BABY_SPAWN_CHANCE: f32 = 0.05;

/// `BiomeDefaultFeatures.farmAnimals` — the CREATURE spawn list every overworld
/// biome carries, in builder insertion order, as `(kind, weight)`. All four use
/// `minCount == maxCount == 4`. Vela's biomes carry no `MobSpawnSettings`, so this
/// hard-coded table stands in for the biome-at-position weighted list (which, for
/// the farm animals, is this exact list in every overworld biome regardless).
const FARM_ANIMALS: [(MobKind, u32); 4] = [
    (MobKind::Sheep, 12),
    (MobKind::Pig, 10),
    (MobKind::Chicken, 10),
    (MobKind::Cow, 8),
];

/// `WeightedList.getRandom` over [`FARM_ANIMALS`]: `nextInt(totalWeight)` then walk
/// the list subtracting each weight (cumulative selection).
fn pick_farm_animal(rng: &mut impl Rng) -> MobKind {
    let total: u32 = FARM_ANIMALS.iter().map(|(_, w)| *w).sum();
    let mut roll = rng.gen_range(0..total);
    for (kind, w) in FARM_ANIMALS {
        if roll < w {
            return kind;
        }
        roll -= w;
    }
    FARM_ANIMALS[FARM_ANIMALS.len() - 1].0 // unreachable: roll < total
}

/// A player snapshot for the spawner: position and the columns they have loaded
/// (so a spawn is only placed where a viewer will actually see it).
struct PlayerSnap {
    x: f64,
    y: f64,
    z: f64,
    loaded: std::collections::HashSet<(i32, i32)>,
}

/// `AgeableMob.AgeableMobGroupData` — the running group size drives the baby roll:
/// `finalizeSpawn` sets a baby when `shouldSpawnBaby && groupSize > 0 && random <=
/// babySpawnChance`, so the *first* member of a group is never a baby.
struct AgeableGroupData {
    group_size: i32,
    should_spawn_baby: bool,
    baby_chance: f32,
}

impl AgeableGroupData {
    /// A fresh `new AgeableMob.AgeableMobGroupData(true)` — `shouldSpawnBaby` true,
    /// default 0.05 chance, group size 0. Re-created per group attempt (vanilla
    /// declares `groupData` inside the `groupCount` loop).
    fn new() -> Self {
        Self {
            group_size: 0,
            should_spawn_baby: true,
            baby_chance: BABY_SPAWN_CHANCE,
        }
    }

    /// `AgeableMob.finalizeSpawn`: decide this member's baby-ness against the
    /// current group size, then `increaseGroupSizeByOne`.
    fn take_baby(&mut self, rng: &mut impl Rng) -> bool {
        let baby = self.should_spawn_baby
            && self.group_size > 0
            && rng.gen::<f32>() <= self.baby_chance;
        self.group_size += 1;
        baby
    }
}

/// `LocalMobCapCalculator` for the CREATURE category. For each online player it
/// holds a running count of CREATURE mobs within the spawn range (128 blocks of a
/// chunk centre) of that player; a chunk may spawn while *some* player near it is
/// under `CREATURE.getMaxInstancesPerChunk()` (10). Seeded from the existing mobs,
/// then bumped on each spawn (`SpawnState.afterSpawn` → `addMob`).
struct LocalMobCap {
    /// Per-player (parallel to the players snapshot) CREATURE tally.
    counts: Vec<i32>,
    /// Player horizontal positions, for the "near a chunk" test.
    player_xz: Vec<(f64, f64)>,
}

impl LocalMobCap {
    /// Seed each player's tally by walking the existing CREATURE mobs and, for
    /// every player within spawn range of a mob's chunk, incrementing that player's
    /// count — exactly `NaturalSpawner.createState`'s `localMobCapCalculator.addMob`
    /// loop.
    fn new(players: &[PlayerSnap], mobs: &[(f64, f64, f64)]) -> Self {
        let player_xz: Vec<(f64, f64)> = players.iter().map(|p| (p.x, p.z)).collect();
        let mut counts = vec![0i32; players.len()];
        for &(mx, _my, mz) in mobs {
            let chunk = ((mx.floor() as i32) >> 4, (mz.floor() as i32) >> 4);
            for (i, &pxz) in player_xz.iter().enumerate() {
                if chunk_near_player(chunk, pxz) {
                    counts[i] += 1;
                }
            }
        }
        Self { counts, player_xz }
    }

    /// `SpawnState.canSpawnForCategoryLocal` → `LocalMobCapCalculator.canSpawn`:
    /// true iff some player near `chunk` is under the per-chunk cap. A chunk with no
    /// player near it (empty `getPlayersCloseForSpawning`) cannot spawn. Our
    /// per-player seed starts at the real count, so a near player with no nearby
    /// mobs (`count == 0`) matches vanilla's `mobCounts == null` → allowed.
    fn can_spawn(&self, chunk: (i32, i32)) -> bool {
        self.player_xz
            .iter()
            .enumerate()
            .any(|(i, &pxz)| chunk_near_player(chunk, pxz) && self.counts[i] < CREATURE_MAX_PER_CHUNK)
    }

    /// `LocalMobCapCalculator.addMob` — bump every player near the spawned mob's
    /// chunk.
    fn add_mob(&mut self, chunk: (i32, i32)) {
        for (i, &pxz) in self.player_xz.iter().enumerate() {
            if chunk_near_player(chunk, pxz) {
                self.counts[i] += 1;
            }
        }
    }
}

/// `ChunkMap.euclideanDistanceSquared(chunkPos, playerPos) < 16384.0` — the chunk's
/// centre block `(cx*16 + 8, cz*16 + 8)` is within 128 blocks of the player
/// (horizontal only, as vanilla compares against the player's x/z).
fn chunk_near_player(chunk: (i32, i32), player_xz: (f64, f64)) -> bool {
    let cx = (chunk.0 * 16 + 8) as f64;
    let cz = (chunk.1 * 16 + 8) as f64;
    let dx = cx - player_xz.0;
    let dz = cz - player_xz.1;
    dx * dx + dz * dz < SPAWN_RANGE_SQR
}

/// The default `minecraft:grass_block` state — the only block in the MC 26.2
/// `ANIMALS_SPAWNABLE_ON` tag (`data/minecraft/tags/block/animals_spawnable_on.json`).
fn grass_block_state() -> crate::ids::BlockState {
    crate::registry::block_state::default_state_of("minecraft:grass_block")
        .map(crate::ids::BlockState)
        .expect("grass_block is a registered block")
}

/// `NaturalSpawner.getFilteredSpawningCategories` global-cap gate for CREATURE:
/// `getMaxInstancesPerChunk() (10) * spawnableChunkCount / MAGIC_NUMBER (289)`.
fn creature_global_cap(spawnable_chunk_count: i32) -> i32 {
    CREATURE_MAX_PER_CHUNK * spawnable_chunk_count / MAGIC_NUMBER
}

/// The natural spawner: on the `gameTime % 400 == 0` persistent-category cadence,
/// evaluate every eligible chunk (the union of players' loaded columns) for a
/// CREATURE pack — `ServerChunkCache.tickChunks` → `NaturalSpawner.spawnForChunk`
/// → `spawnCategoryForChunk`. Registered before `mob_tick` in the schedule.
pub fn mob_spawn(world: &mut World) {
    let tick = world.resource::<Tick>().0;
    // CREATURE only participates on the persistent cadence.
    if !tick.is_multiple_of(spawn_persistent_interval()) {
        return;
    }

    // Snapshot online players (position + their loaded columns).
    let players: Vec<PlayerSnap> = {
        let mut q = world.query::<(&Pos, &LoadedChunks)>();
        q.iter(world)
            .map(|(p, l)| PlayerSnap { x: p.x, y: p.y, z: p.z, loaded: l.loaded.clone() })
            .collect()
    };
    if players.is_empty() {
        return;
    }

    // "spawnableChunkCount" stand-in: the union of every player's loaded columns
    // (deduped), standing in for `DistanceManager.getNaturalSpawnChunkCount`
    // (vanilla's count of chunks in entity-ticking range of any player).
    let spawnable: std::collections::HashSet<(i32, i32)> =
        players.iter().flat_map(|p| p.loaded.iter().copied()).collect();
    let global_cap = creature_global_cap(spawnable.len() as i32);

    // Existing CREATURE mobs (all Vela mobs are CREATURE): their count feeds the
    // global cap, their positions seed the local cap.
    let mob_positions: Vec<(f64, f64, f64)> = {
        let mut q = world.query_filtered::<&Pos, With<Mob>>();
        q.iter(world).map(|p| (p.x, p.y, p.z)).collect()
    };
    let mut creature_count = mob_positions.len() as i32;
    // `canSpawnForCategoryGlobal`: at/over the cap, the category is absent from the
    // spawning list, so nothing spawns this pass.
    if creature_count >= global_cap {
        return;
    }

    let mut local = LocalMobCap::new(&players, &mob_positions);
    let player_pos: Vec<(f64, f64, f64)> = players.iter().map(|p| (p.x, p.y, p.z)).collect();

    let mut rng = rand::thread_rng();
    // `Util.shuffle(spawningChunks, level.getRandom())` before iterating.
    let mut chunks: Vec<(i32, i32)> = spawnable.iter().copied().collect();
    chunks.shuffle(&mut rng);

    for chunk in chunks {
        // Deviation from vanilla (which gates the global cap only once per tick and
        // may overshoot within a tick, relying on despawn to self-correct): Vela's
        // passive mobs never despawn, so we re-check the global cap before each
        // chunk and stop the pass at the cap to keep it meaningful.
        if creature_count >= global_cap {
            break;
        }
        // `spawnForChunk`: `canSpawnForCategoryLocal`.
        if !local.can_spawn(chunk) {
            continue;
        }
        // `spawnCategoryForChunk` → `getRandomPosWithin`: random column in the
        // chunk, then a random y from the world floor up to WORLD_SURFACE + 1.
        let sx = chunk.0 * 16 + rng.gen_range(0..16);
        let sz = chunk.1 * 16 + rng.gen_range(0..16);
        // Non-generating surface peek: `mob_spawn` runs on the tick thread every
        // persistent-cadence tick, so a cold column here is skipped (vanilla's
        // spawner only walks loaded chunks) rather than forced through a ~40 ms
        // parity build. `spawnable` is the players' loaded set, so a resident hit
        // is the norm; the `None` guard is defensive against a mid-tick eviction.
        let Some(surface) = crate::world::resident_surface_height(sx, sz) else {
            continue;
        };
        let top = surface + 1;
        let sy = rng.gen_range(crate::world::MIN_Y..=top);
        // `if (start.getY() >= level.getMinY() + 1)`.
        if sy < crate::world::MIN_Y + 1 {
            continue;
        }
        spawn_category_for_position(
            world,
            chunk,
            (sx, sy, sz),
            &player_pos,
            &spawnable,
            &mut rng,
            &mut creature_count,
            global_cap,
            &mut local,
        );
    }
}

/// `NaturalSpawner.spawnCategoryForPosition` for the CREATURE category: three group
/// attempts, each an inner pack loop that scatters members off the start position,
/// gates them by player distance, draws a weighted farm animal, validates the
/// spot, and spawns — up to the cluster cap.
#[allow(clippy::too_many_arguments)]
fn spawn_category_for_position(
    world: &mut World,
    chunk: (i32, i32),
    start: (i32, i32, i32),
    players: &[(f64, f64, f64)],
    loaded: &std::collections::HashSet<(i32, i32)>,
    rng: &mut impl Rng,
    creature_count: &mut i32,
    global_cap: i32,
    local: &mut LocalMobCap,
) {
    let (start_x, y_start, start_z) = start;
    // `if (!state.isRedstoneConductor(chunk, start))` — Vela stand-in: proceed only
    // if the start block is not a full solid block (`is_solid` == not air). A start
    // buried in terrain (the common case for a random underground y) is solid and
    // skipped here.
    if is_solid(start_x, y_start, start_z) {
        return;
    }

    let mut cluster_size = 0i32;
    for _group in 0..GROUP_ATTEMPTS {
        let mut x = start_x;
        let mut z = start_z;
        let mut current: Option<MobKind> = None;
        let mut group_data = AgeableGroupData::new();
        // `int max = Mth.ceil(level.random.nextFloat() * 4.0F);` — reset to the pack
        // size once a `SpawnerData` is chosen for this group.
        let mut max = (rng.gen::<f32>() * 4.0).ceil() as i32;
        let mut ll = 0;
        while ll < max {
            // `x += nextInt(6) - nextInt(6); z += nextInt(6) - nextInt(6)`.
            x += rng.gen_range(0..PACK_SPREAD) - rng.gen_range(0..PACK_SPREAD);
            z += rng.gen_range(0..PACK_SPREAD) - rng.gen_range(0..PACK_SPREAD);
            ll += 1;

            let xx = x as f64 + 0.5;
            let zz = z as f64 + 0.5;
            // `Player nearestPlayer = level.getNearestPlayer(xx, yStart, zz, -1, false)`.
            let Some(dist_sqr) = nearest_player_dist_sqr(players, xx, y_start as f64, zz) else {
                continue;
            };
            if !is_right_distance_to_player(dist_sqr, (x, z), chunk, loaded) {
                continue;
            }

            if current.is_none() {
                // `getRandomSpawnMobAt` → the biome weighted list.
                current = Some(pick_farm_animal(rng));
                // `max = minCount + nextInt(1 + maxCount - minCount)` = 4.
                max = PACK_MIN + rng.gen_range(0..(1 + PACK_MAX - PACK_MIN));
            }
            let kind = current.expect("spawn data set above");

            // `isValidSpawnPostitionForType` + `isValidPositionForMob`, collapsed to
            // one check in Vela (both re-run `checkSpawnRules`/`checkSpawnObstruction`).
            if !is_valid_spawn_position(x, y_start, z, dist_sqr) {
                continue;
            }

            // `finalizeSpawn` baby roll, then the spawn + accounting.
            let baby = group_data.take_baby(rng);
            let pos = (xx, y_start as f64, zz);
            let id = spawn_mob_with(world, kind, pos, baby);
            *creature_count += 1;
            local.add_mob(((x >> 4), (z >> 4)));
            cluster_size += 1;
            tracing::debug!(?kind, id, baby, x = pos.0, y = pos.1, z = pos.2, "spawned passive mob");

            // `if (clusterSize >= mob.getMaxSpawnClusterSize()) return;` — plus the
            // Vela global-cap recheck (see [`mob_spawn`]).
            if cluster_size >= MAX_SPAWN_CLUSTER || *creature_count >= global_cap {
                return;
            }
            // `mob.isMaxGroupSizeReached(groupSize)` is always false for these mobs,
            // so there is no per-group early break.
        }
    }
}

/// `Level.getNearestPlayer(x, y, z, -1, false)` → the minimum 3-D squared distance
/// from `(x, y, z)` to any player (`Entity.distanceToSqr`), or `None` when no
/// players are online.
fn nearest_player_dist_sqr(players: &[(f64, f64, f64)], x: f64, y: f64, z: f64) -> Option<f64> {
    players
        .iter()
        .map(|&(px, py, pz)| {
            let (dx, dy, dz) = (px - x, py - y, pz - z);
            dx * dx + dy * dy + dz * dz
        })
        .reduce(f64::min)
}

/// `NaturalSpawner.isRightDistanceToPlayerAndSpawnPoint`: reject members closer than
/// 24 blocks (`<= 576`) to the nearest player, then require the member's chunk to be
/// the start chunk or otherwise spawnable (loaded). The world-spawn-point proximity
/// clause is omitted — Vela models no per-world respawn point at this site.
fn is_right_distance_to_player(
    dist_sqr: f64,
    member: (i32, i32),
    chunk: (i32, i32),
    loaded: &std::collections::HashSet<(i32, i32)>,
) -> bool {
    if dist_sqr <= MIN_SPAWN_DISTANCE_SQR {
        return false;
    }
    let member_chunk = ((member.0 >> 4), (member.1 >> 4));
    member_chunk == chunk || loaded.contains(&member_chunk)
}

/// The combined `isValidSpawnPostitionForType` + `isValidPositionForMob` gate for a
/// farm animal at `(x, y, z)` with nearest-player squared distance `dist_sqr`:
///
/// * `!canSpawnFarFromPlayer() && dist > despawnDistance²` → reject (farm animals
///   never spawn far from a player);
/// * `SpawnPlacements.ON_GROUND.isSpawnPositionOk`: the block below is a valid
///   spawn (Vela: `grass_block`, the whole `ANIMALS_SPAWNABLE_ON` tag) and the spawn
///   block plus the one above are empty (air);
/// * `Animal.checkAnimalSpawnRules`: block below in `ANIMALS_SPAWNABLE_ON` **and**
///   `getRawBrightness(pos, 0) > 8` (Vela's [`crate::world::raw_brightness`]).
///
/// `checkSpawnObstruction`/`noCollision` reduce to "the spawn block is air", already
/// covered. Structure/nether-fortress and biome-membership checks are moot for the
/// hard-coded farm-animal list.
fn is_valid_spawn_position(x: i32, y: i32, z: i32, dist_sqr: f64) -> bool {
    if dist_sqr > CREATURE_DESPAWN_DISTANCE_SQR {
        return false;
    }
    // Non-generating reads on the tick thread: a cold (non-resident) neighbour
    // makes the position invalid — vanilla only spawns in loaded chunks, so a
    // read that would generate one is a skip. `None != Some(grass)` / `None !=
    // Some(AIR)` naturally reject a missing column.
    if crate::world::try_block_state_at(x, y - 1, z) != Some(grass_block_state()) {
        return false;
    }
    if crate::world::try_block_state_at(x, y, z) != Some(crate::world::AIR_STATE) {
        return false;
    }
    if crate::world::try_block_state_at(x, y + 1, z) != Some(crate::world::AIR_STATE) {
        return false;
    }
    // Brightness gate: an unlit column (wire/light not yet built) is also a skip —
    // building light on the tick thread is exactly what we avoid. `None` (cold or
    // unlit) fails `is_some_and`, so the candidate is rejected.
    crate::world::try_raw_brightness(x, y, z).is_some_and(|b| b > 8)
}

// --- Damage / death path (LivingEntity.hurtServer + die) ---------------------

/// Who/what dealt the damage — the bits `LivingEntity.hurtServer` reads off its
/// `DamageSource`. Vela's only live source today is a player melee hit
/// (`packet_handlers::on_attack`), but the fields mirror the vanilla source so the
/// packet/knockback/loot logic stays a faithful port.
#[derive(Clone, Copy)]
pub struct DamageContext {
    /// `source.getEntity()`/`getDirectEntity()` network id — the attacker, carried
    /// as both the cause and direct id of the `ClientboundDamageEventPacket`. `None`
    /// for an unattributed/environmental source (encodes as `-1`).
    pub attacker_id: Option<i32>,
    /// The attacker's horizontal position (`source.getSourcePosition().x()/z()`) —
    /// the knockback direction origin. `None` leaves the direction to the random
    /// fallback in `LivingEntity.knockback`.
    pub attacker_xz: Option<(f64, f64)>,
    /// Whether `source.getEntity()` is a `Player` — arms `lastHurtByPlayer`
    /// (`resolvePlayerResponsibleForDamage`), which gates XP (and, with `!isBaby`,
    /// mirrors the "killed by player" loot memory).
    pub by_player: bool,
    /// `source.typeHolder()` — the `damage_type` registry name for the packet.
    pub damage_type: &'static str,
}

impl DamageContext {
    /// The player-melee source `packet_handlers::on_attack` passes: attacker id +
    /// position, `by_player`, and the `player_attack` damage type.
    pub fn player_attack(attacker_id: i32, attacker_xz: (f64, f64)) -> Self {
        Self {
            attacker_id: Some(attacker_id),
            attacker_xz: Some(attacker_xz),
            by_player: true,
            damage_type: DAMAGE_TYPE_PLAYER_ATTACK,
        }
    }
}

/// Apply `amount` damage to a mob from `ctx`, porting `LivingEntity.hurtServer`
/// (and, on a fatal blow, `die`). Returns `true` if the hit was fatal.
///
/// The i-frame gate is unchanged (a fresh hit sets `invulnerableTime` 20 /
/// `lastHurt` / `hurtTime` 10; while `invulnerableTime > 10` a re-hit lands only
/// its excess over `lastHurt`, and `amount <= lastHurt` cancels). On top of it this
/// now mirrors the rest of `hurtServer`:
///
/// * **Full hits** (`tookFullDamage`) broadcast a `ClientboundDamageEventPacket`
///   (`broadcastDamageEvent`) carrying the damage type + attacker, from which the
///   client derives the red flash *and* the attacker-relative hurt lean — this
///   replaces the old attacker-less `hurt_animation`. They also apply
///   `dealDefaultKnockback` (0.4, away from the attacker) to the mob's velocity and
///   emit an immediate `SetEntityMotion` (vanilla's `needsSync`).
/// * A player source arms `lastHurtByPlayer` (100 ticks).
/// * On a **fatal** blow the mob enters the `Dying` corpse state (health-0 metadata
///   still synced so the client plays the fall-over), `dropAllDeathLoot` drops the
///   loot table + (player-killed, non-baby) XP *immediately*, and the death sound
///   plays; the corpse is removed 20 ticks later by [`mob_tick`]. A surviving full
///   hit plays the hurt sound. A partial re-hit is silent.
pub fn damage(world: &mut World, entity: Entity, amount: f32, ctx: &DamageContext) -> bool {
    // `if (damage < 0.0F) damage = 0.0F;`
    let amount = amount.max(0.0);
    let mut rng = rand::thread_rng();

    // i-frame gate (LivingEntity.hurtServer). Returns the damage that actually
    // lands and whether this was a full hit (flash + timer reset). Mobs are seeded
    // with MobState by `spawn_mob`; a missing one is treated as a fresh full hit.
    let (applied, full_hit) = match world.get_mut::<MobState>(entity) {
        Some(mut st) => {
            // `if (this.isDeadOrDying()) return false;` — a corpse takes no damage.
            if st.dead {
                return false;
            }
            if st.invulnerable_time > INVULNERABLE_GATE {
                if amount <= st.last_hurt {
                    return false; // within i-frames and no stronger than the last hit
                }
                let marginal = amount - st.last_hurt;
                st.last_hurt = amount;
                (marginal, false)
            } else {
                st.last_hurt = amount;
                st.invulnerable_time = INVULNERABLE_TIME;
                st.hurt_time = HURT_DURATION;
                (amount, true)
            }
        }
        None => (amount, true),
    };

    // `resolvePlayerResponsibleForDamage`: a player source arms the memory that
    // gates loot/XP at death (runs for both full and partial hits).
    if ctx.by_player {
        if let Some(mut st) = world.get_mut::<MobState>(entity) {
            st.last_hurt_by_player_time = PLAYER_HURT_MEMORY_TIME;
        }
    }

    // `actuallyHurt`: subtract health.
    let (net_id, kind, dead, new_health) = {
        let Some(mut health) = world.get_mut::<Health>(entity) else {
            return false;
        };
        health.current = (health.current - applied).max(0.0);
        let dead = health.current <= 0.0;
        let new_health = health.current;
        let net_id = match world.get::<NetEntity>(entity).map(|n| n.id) {
            Some(id) => id,
            None => return false,
        };
        let kind = world.get::<Mob>(entity).map(|m| m.kind);
        (net_id, kind, dead, new_health)
    };
    let Some(kind) = kind else { return false };

    // Sync the updated health metadata to viewers (the killing blow syncs health 0,
    // which is what makes the client play the death fall-over).
    if let Some(mut meta) = world.get_mut::<EntityMeta>(entity) {
        meta.0
            .set(LIVING_ENTITY_DATA_HEALTH, DataValue::Float(new_health));
    }
    let mut emissions: Vec<(Entity, Bytes)> = {
        let meta = world.get::<EntityMeta>(entity).expect("mob has EntityMeta");
        vec![(entity, super::entity::packets::set_entity_data(net_id, &meta.0))]
    };

    if full_hit {
        // `broadcastDamageEvent` → ClientboundDamageEventPacket (supersedes the
        // hurt-animation packet: the client derives the flash *and* the lean from it).
        let dtype = crate::registry::synced_id("minecraft:damage_type", ctx.damage_type)
            .expect("damage type is a synced registry entry");
        let cause = ctx.attacker_id.unwrap_or(-1);
        emissions.push((
            entity,
            super::entity::packets::damage_event(net_id, dtype, cause, cause, None),
        ));

        // `dealDefaultKnockback(0.4, …)` → `knockback`: push the mob away from the
        // attacker and emit an immediate motion packet (vanilla `needsSync`).
        if let Some(motion) = apply_knockback(world, entity, net_id, ctx, &mut rng) {
            emissions.push((entity, motion));
        }
    }

    let baby = is_baby(world, entity);
    if dead {
        // `die`: enter the corpse state (removal handled by mob_tick at deathTime 20)
        // and drop loot + XP *now* (dropAllDeathLoot runs immediately, not after the
        // 20-tick animation). Health-0 metadata was already queued above.
        if let Some(mut st) = world.get_mut::<MobState>(entity) {
            st.dead = true;
            st.death_time = 0;
        }
        // `makeSound(getDeathSound())` — only on a full hit (tookFullDamage).
        if full_hit {
            emissions.push((entity, entity_sound(world, entity, kind.death_sound(), baby, &mut rng)));
        }
        flush_emissions(world, &emissions);
        drop_death_loot(world, entity, kind, baby, &mut rng);
    } else {
        // `playHurtSound(getHurtSound())` — a surviving full hit.
        if full_hit {
            emissions.push((entity, entity_sound(world, entity, kind.hurt_sound(), baby, &mut rng)));
        }
        flush_emissions(world, &emissions);
    }
    dead
}

/// `LivingEntity.knockback(0.4, xd, zd)` from `dealDefaultKnockback`: push the mob
/// away from the attacker. Returns the `SetEntityMotion` packet to broadcast (the
/// `needsSync` immediate send), or `None` when the power is non-positive. Mutates
/// the mob's velocity and latches `last_sent_*` so the movement broadcast doesn't
/// re-send the same value.
fn apply_knockback(
    world: &mut World,
    entity: Entity,
    net_id: i32,
    ctx: &DamageContext,
    rng: &mut impl Rng,
) -> Option<Bytes> {
    // KNOCKBACK_RESISTANCE is 0 for these animals, so `power` stays 0.4.
    let power = KNOCKBACK_POWER;
    if power <= 0.0 {
        return None;
    }
    let (mx, mz, on_ground) = {
        let p = world.get::<Pos>(entity)?;
        (p.x, p.z, p.on_ground)
    };
    // `xd = sourcePosition.x - getX(); zd = sourcePosition.z - getZ()`.
    let (mut xd, mut zd) = match ctx.attacker_xz {
        Some((ax, az)) => (ax - mx, az - mz),
        None => (0.0, 0.0),
    };
    // `while (xd*xd + zd*zd < 1.0E-5) { xd = (r - r)*0.01; zd = (r - r)*0.01; }`.
    while xd * xd + zd * zd < KNOCKBACK_MIN_DIR_SQ {
        xd = (rng.gen::<f64>() - rng.gen::<f64>()) * 0.01;
        zd = (rng.gen::<f64>() - rng.gen::<f64>()) * 0.01;
    }
    // `deltaVector = new Vec3(xd, 0, zd).normalize().scale(power)`.
    let len = (xd * xd + zd * zd).sqrt();
    let (dvx, dvz) = (xd / len * power, zd / len * power);

    let mut st = world.get_mut::<MobState>(entity)?;
    // `setDeltaMovement(dm.x/2 - dv.x, onGround ? min(0.4, dm.y/2 + power) : dm.y, dm.z/2 - dv.z)`.
    st.vx = st.vx / 2.0 - dvx;
    st.vy = if on_ground {
        (st.vy / 2.0 + power).min(KNOCKBACK_VERTICAL_CAP)
    } else {
        st.vy
    };
    st.vz = st.vz / 2.0 - dvz;
    st.last_sent_vx = st.vx;
    st.last_sent_vy = st.vy;
    st.last_sent_vz = st.vz;
    Some(packets::set_entity_motion(net_id, st.vx, st.vy, st.vz))
}

/// `LivingEntity.isBaby` for a Vela mob — the `AgeableMob.DATA_BABY_ID` metadata
/// flag (absent ⇒ adult; Vela models no growth, so a baby stays a baby).
fn is_baby(world: &World, entity: Entity) -> bool {
    world
        .get::<EntityMeta>(entity)
        .map(|m| {
            m.0.items()
                .iter()
                .any(|it| it.index == AGEABLE_MOB_DATA_BABY && it.value == DataValue::Boolean(true))
        })
        .unwrap_or(false)
}

/// Build the `ClientboundSoundPacket` for `LivingEntity.makeSound(sound)`:
/// `playSound(sound, getSoundVolume()=1.0, getVoicePitch())` at the mob's position
/// in the `NEUTRAL` category. `getVoicePitch` = `(r - r)*0.2 + (baby ? 1.5 : 1.0)`.
fn entity_sound(
    world: &World,
    entity: Entity,
    sound_name: &str,
    baby: bool,
    rng: &mut impl Rng,
) -> Bytes {
    let (x, y, z) = world
        .get::<Pos>(entity)
        .map(|p| (p.x, p.y, p.z))
        .unwrap_or((0.0, 0.0, 0.0));
    let base = if baby { 1.5 } else { 1.0 };
    let pitch = (rng.gen::<f32>() - rng.gen::<f32>()) * 0.2 + base;
    let sound_id = SOUND_EVENT
        .id_of(sound_name)
        .expect("mob hurt/death sound is a registered sound_event");
    super::entity::packets::play_sound(
        sound_id,
        SOUND_SOURCE_NEUTRAL,
        x,
        y,
        z,
        SOUND_VOLUME,
        pitch,
        rng.gen::<i64>(),
    )
}

/// `LivingEntity.dropAllDeathLoot` + `dropExperience` for a farm animal: drop the
/// kind's loot table (adults only — `shouldDropLoot` is `!isBaby`) as item entities
/// at the mob, then award XP when player-killed and adult (`dropExperience`'s
/// `lastHurtByPlayerMemoryTime > 0 && !isBaby` gate). Vela has no fire/looting, so
/// the cooked variants and looting bonuses never apply.
fn drop_death_loot(world: &mut World, entity: Entity, kind: MobKind, baby: bool, rng: &mut impl Rng) {
    // Babies drop no loot and no XP.
    if baby {
        return;
    }
    let (px, py, pz) = world
        .get::<Pos>(entity)
        .map(|p| (p.x, p.y, p.z))
        .unwrap_or((0.0, 0.0, 0.0));
    let wool = sheep_wool_color(world, entity);
    for stack in loot_for(kind, wool, rng) {
        // `Entity.spawnAtLocation(stack, 0.0F)` — the item entity spawns at the mob.
        spawn_item_entity(world, (px, py, pz), stack);
    }

    // `dropExperience`: player-killed (memory > 0) adults only.
    let player_killed = world
        .get::<MobState>(entity)
        .map(|st| st.last_hurt_by_player_time > 0)
        .unwrap_or(false);
    if player_killed {
        award_experience(world, (px, py, pz), kind.base_experience_reward(rng));
    }
}

/// `ExperienceOrb.award(level, pos, amount)`: split `amount` into orbs by the
/// vanilla denomination ladder (`getExperienceValue`) and spawn each.
fn award_experience(world: &mut World, pos: (f64, f64, f64), mut amount: i32) {
    while amount > 0 {
        let value = experience_value(amount);
        amount -= value;
        spawn_xp_orb(world, pos, value);
    }
}

/// `ExperienceOrb.getExperienceValue` — the largest orb denomination `<= max`
/// (only the low rungs matter for a 1..=3 animal reward, but the full ladder is
/// kept 1:1).
fn experience_value(max: i32) -> i32 {
    const LADDER: [i32; 11] = [2477, 1237, 617, 307, 149, 73, 37, 17, 7, 3, 1];
    for v in LADDER {
        if max >= v {
            return v;
        }
    }
    1
}

/// The sheep's wool colour nibble from its `Sheep.DATA_WOOL_ID` metadata (low 4
/// bits; the 0x10 sheared flag is never set in Vela), used to pick the wool item
/// from its loot dispatch pool. `None` for non-sheep.
fn sheep_wool_color(world: &World, entity: Entity) -> Option<u8> {
    let meta = world.get::<EntityMeta>(entity)?;
    meta.0.items().iter().find_map(|it| {
        if it.index == SHEEP_DATA_WOOL {
            if let DataValue::Byte(b) = it.value {
                return Some(b & 0x0F);
            }
        }
        None
    })
}

/// `Mth.nextInt(random, min, max)` — a uniform integer in `[min, max]`, the shape
/// `UniformGenerator.between(min, max).getInt` takes for these loot counts.
fn uniform(rng: &mut impl Rng, min: i32, max: i32) -> i32 {
    min + rng.gen_range(0..=(max - min))
}

/// The item id for `namespace:path`, as a `ItemStack`-ready numeric id.
fn item_id(name: &str) -> i32 {
    crate::registry::item::id_of(name)
        .expect("loot item is a registered item")
        .get()
}

/// The `DyeColor` id (0..=15) → `<color>_wool` item name, matching the sheep loot
/// dispatch table (`ColorCollection.zipApply(SHEEP, Blocks.WOOL, …)`).
fn wool_item_name(color: u8) -> &'static str {
    match color {
        0 => "minecraft:white_wool",
        1 => "minecraft:orange_wool",
        2 => "minecraft:magenta_wool",
        3 => "minecraft:light_blue_wool",
        4 => "minecraft:yellow_wool",
        5 => "minecraft:lime_wool",
        6 => "minecraft:pink_wool",
        7 => "minecraft:gray_wool",
        8 => "minecraft:light_gray_wool",
        9 => "minecraft:cyan_wool",
        10 => "minecraft:purple_wool",
        11 => "minecraft:blue_wool",
        12 => "minecraft:brown_wool",
        13 => "minecraft:green_wool",
        14 => "minecraft:red_wool",
        _ => "minecraft:black_wool",
    }
}

/// The `VanillaEntityLoot` table for `kind` (MC 26.2), in pool order. Counts use
/// `UniformGenerator.between` (→ [`uniform`]); a count of 0 (leather/feather) drops
/// nothing. Looting/smelting bonuses are omitted (no enchantments / no fire in
/// Vela). Sheep also drops one wool of its `color`.
fn loot_for(kind: MobKind, color: Option<u8>, rng: &mut impl Rng) -> Vec<ItemStack> {
    let mut drops = Vec::new();
    let mut push = |name: &str, count: i32| {
        if count > 0 {
            drops.push(ItemStack::new(item_id(name), count));
        }
    };
    match kind {
        // porkchop 1..=3.
        MobKind::Pig => push("minecraft:porkchop", uniform(rng, 1, 3)),
        // leather 0..=2 pool, then beef 1..=3 pool.
        MobKind::Cow => {
            push("minecraft:leather", uniform(rng, 0, 2));
            push("minecraft:beef", uniform(rng, 1, 3));
        }
        // feather 0..=2 pool, then raw chicken (count 1, no SetItemCount) pool.
        MobKind::Chicken => {
            push("minecraft:feather", uniform(rng, 0, 2));
            push("minecraft:chicken", 1);
        }
        // mutton 1..=2 pool, then one wool of the sheep's colour.
        MobKind::Sheep => {
            push("minecraft:mutton", uniform(rng, 1, 2));
            push(wool_item_name(color.unwrap_or(0)), 1);
        }
    }
    drops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::buffer::PacketReader;
    use crate::sim::bridge::Outbound;
    use crate::sim::components::{ChunkSender, Conn, NextEntityId};
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
            Conn::new(tx),
            LoadedChunks {
                center: (0, 0),
                loaded: chunks.iter().copied().collect::<HashSet<_>>(),
            },
            // Real players always carry a ChunkSender (see `stream_chunks`); the
            // entity-tracking snapshot now requires it. Empty `pending` means every
            // loaded column counts as tracked, matching this fixture's intent.
            ChunkSender::new(),
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
        let meta = spawn_metadata(MobKind::Cow, 10.0, (0.5, 64.0, 0.5), false, &mut rng);
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
        let meta = spawn_metadata(MobKind::Sheep, 8.0, (0.5, 64.0, 0.5), false, &mut rng);
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
        // The collision `is_solid` is now non-generating: it treats a cold column
        // as solid, so a mob only falls through a *resident* one. In production the
        // mob's chunk is loaded; warm it here so the fall reaches the surface
        // instead of the pig resting in mid-air over an ungenerated column.
        let _ = crate::world::chunk_columns(0, 0);
        // The landing floor is the topmost non-air block in the column — `is_solid`
        // treats every non-air state as an obstacle, so a tree canopy over the
        // terrain (the default seed grows mangroves near the origin) is what the
        // pig actually comes to rest on, not the terrain surface below it.
        let sy = (crate::world::MIN_Y..crate::world::MIN_Y + crate::world::SECTION_COUNT * 16)
            .rev()
            .find(|&y| crate::world::block_state_at(0, y, 0) != crate::world::AIR_STATE)
            .expect("column has ground");
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

    /// Drive one wander/physics step in isolation (no broadcast), with a pinned
    /// player so `noActionTime` never blocks and an odd `tick_count` so no fresh
    /// stroll target is rolled mid-test.
    fn step(kind: MobKind, st: &mut MobState, pos: &mut Pos) {
        let players = [(pos.x, pos.y, pos.z)];
        let mut rng = rand::thread_rng();
        step_mob(kind, st, pos, &players, &mut rng);
    }

    #[test]
    fn mob_walks_into_two_high_wall_and_stops() {
        // A pig (0.9 wide) walking +x into a 2-high wall is clipped flush against it,
        // never clipping through, with its x-velocity zeroed.
        let _lock = crate::world::WORLD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let solid = grass_block_state();
        let (bx, bz) = (600_000, 0);
        // Floor at y=99 (feet stand at y=100) across the walk, wall 2-high at x=bx+3.
        for x in 0..7 {
            for z in -2..=2 {
                crate::world::set_block(bx + x, 99, bz + z, solid);
            }
        }
        for z in -2..=2 {
            crate::world::set_block(bx + 3, 100, bz + z, solid);
            crate::world::set_block(bx + 3, 101, bz + z, solid);
        }

        let mut pos = Pos { x: bx as f64 + 0.5, y: 100.0, z: bz as f64 + 0.5, yaw: 0.0, pitch: 0.0, on_ground: true };
        let mut st = MobState::new(pos.x, pos.y, pos.z, 0.0);
        st.tick_count = 1;
        // A target straight through the wall, at the same height (no genuine step-up).
        st.target = Some((bx as f64 + 6.5, 100.0, bz as f64 + 0.5));
        st.pursue_ticks = STROLL_MAX_TICKS;

        for _ in 0..80 {
            step(MobKind::Pig, &mut st, &mut pos);
            // The box's leading face (x + 0.45) must never enter the wall column (x=3).
            assert!(pos.x + 0.45 <= (bx + 3) as f64 + 1e-6, "pig clipped into/through the wall at x={}", pos.x);
        }
        // It has come to rest flush against the wall with no residual x-velocity.
        assert!(pos.x + 0.45 > (bx + 3) as f64 - 0.05, "pig should be pressed against the wall, x={}", pos.x);
        assert!(st.vx.abs() < 1e-9, "x-velocity zeroed on collision, vx={}", st.vx);
        assert!(st.horizontal_collision, "horizontal collision flagged");
    }

    #[test]
    fn mob_jumps_a_one_high_ledge_and_lands_on_top() {
        // A pig bumping a 1-high ledge (solid ahead, air above) jumps and ends up
        // standing on top within a bounded number of ticks.
        let _lock = crate::world::WORLD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let solid = grass_block_state();
        let (bx, bz) = (601_000, 0);
        // Live parity worldgen is on by default, so this far-away column is real
        // (possibly tall) terrain rather than the old value-noise flatland. Clear
        // the airspace the pig walks and jumps through first, so generated blocks
        // can't bury the controlled platform, then (re)place the floors on top.
        for x in 0..9 {
            for z in -2..=2 {
                for y in 100..=104 {
                    crate::world::set_block(bx + x, y, bz + z, crate::world::AIR_STATE);
                }
            }
        }
        // Low floor at y=99 (top y=100) for x in 0..2, raised floor (extra block at
        // y=100, top y=101) for x in 3..8 — a 1-block ledge whose face is at x=3.
        for x in 0..9 {
            for z in -2..=2 {
                crate::world::set_block(bx + x, 99, bz + z, solid);
            }
        }
        for x in 3..9 {
            for z in -2..=2 {
                crate::world::set_block(bx + x, 100, bz + z, solid);
            }
        }

        let mut pos = Pos { x: bx as f64 + 0.5, y: 100.0, z: bz as f64 + 0.5, yaw: 0.0, pitch: 0.0, on_ground: true };
        let mut st = MobState::new(pos.x, pos.y, pos.z, 0.0);
        st.tick_count = 1;
        // Target on top of the ledge, beyond the face.
        st.target = Some((bx as f64 + 6.5, 101.0, bz as f64 + 0.5));
        st.pursue_ticks = 300;

        let mut on_ledge = false;
        for _ in 0..200 {
            // Keep pursuing (this test only exercises movement, not the stroll roll).
            if st.target.is_none() {
                st.target = Some((bx as f64 + 6.5, 101.0, bz as f64 + 0.5));
                st.pursue_ticks = 300;
            }
            step(MobKind::Pig, &mut st, &mut pos);
            if pos.on_ground && (pos.y - 101.0).abs() < 0.05 && pos.x > (bx + 3) as f64 {
                on_ledge = true;
                break;
            }
        }
        assert!(on_ledge, "pig should jump the ledge and stand on top, ended at ({}, {})", pos.x, pos.y);
    }

    #[test]
    fn jump_cooldown_allows_one_jump_per_ten_ticks() {
        // With a jump requested every tick and the mob pinned grounded, `noJumpDelay`
        // gates jumping to once per 10 ticks (no jump spam). A jump fired this tick is
        // detectable as `no_jump_delay` having just been (re)armed to 10.
        let _lock = crate::world::WORLD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let solid = grass_block_state();
        let (bx, bz) = (602_000, 0);
        for z in -2..=2 {
            for x in -2..=2 {
                crate::world::set_block(bx + x, 99, bz + z, solid);
            }
        }
        let x0 = bx as f64 + 0.5;
        let z0 = bz as f64 + 0.5;
        let mut st = MobState::new(x0, 100.0, z0, 0.0);
        st.tick_count = 1;
        let mut pos = Pos { x: x0, y: 100.0, z: z0, yaw: 0.0, pitch: 0.0, on_ground: true };

        let mut jumps = 0;
        for _ in 0..30 {
            // Re-pin to a grounded, fixed state and re-request a jump via trigger (a):
            // a target 2 up and 0.8 away horizontally (dist_h ≥ ARRIVE_DIST, dist² < 1).
            pos.x = x0;
            pos.z = z0;
            pos.y = 100.0;
            pos.on_ground = true;
            st.vy = 0.0;
            st.target = Some((x0 + 0.8, 102.0, z0));
            st.pursue_ticks = STROLL_MAX_TICKS;
            step(MobKind::Pig, &mut st, &mut pos);
            if st.no_jump_delay == NO_JUMP_DELAY {
                jumps += 1;
            }
        }
        assert_eq!(jumps, 3, "expected one jump per 10 ticks over 30 ticks, got {jumps}");
    }

    #[test]
    fn ceiling_clip_zeroes_upward_velocity() {
        // A pig with upward velocity under a low ceiling is clipped: it does not pass
        // through, and its upward velocity is zeroed (then gravity turns it negative).
        let _lock = crate::world::WORLD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let solid = grass_block_state();
        let (bx, bz) = (603_000, 0);
        for z in -1..=1 {
            for x in -1..=1 {
                crate::world::set_block(bx + x, 99, bz + z, solid); // floor (top y=100)
                crate::world::set_block(bx + x, 101, bz + z, solid); // ceiling (bottom y=101)
            }
        }
        // Direct collision check: box [100, 100.9] moving up 0.42 clips at the ceiling.
        let mv = move_and_collide(bx as f64 + 0.5, 100.0, bz as f64 + 0.5, 0.45, 0.9, 0.0, 0.42, 0.0);
        assert!(mv.ceiling, "upward clip flagged");
        assert!((mv.dy - 0.1).abs() < 1e-9, "clipped to the 0.1 gap under the ceiling, dy={}", mv.dy);

        // Through a full step, the upward velocity is zeroed then made negative by
        // gravity, and the pig never rises past the ceiling gap.
        let mut pos = Pos { x: bx as f64 + 0.5, y: 100.0, z: bz as f64 + 0.5, yaw: 0.0, pitch: 0.0, on_ground: true };
        let mut st = MobState::new(pos.x, pos.y, pos.z, 0.0);
        st.tick_count = 1;
        st.vy = 0.42; // as if it had just jumped
        step(MobKind::Pig, &mut st, &mut pos);
        assert!(pos.y <= 100.1 + 1e-9, "pig rose past the ceiling gap, y={}", pos.y);
        assert!(st.vy < 0.0, "upward vy zeroed then pulled down by gravity, vy={}", st.vy);
    }

    #[test]
    fn cow_footprint_stays_supported_over_a_block_edge() {
        // A cow (0.9 wide) whose centre column is air but whose box overlaps a
        // neighbouring solid column stays supported — vanilla stands on any block
        // under the box, not just the one under its centre.
        let _lock = crate::world::WORLD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let solid = grass_block_state();
        let (bx, bz) = (604_000, 0);
        // Support block only under column x=bx+2; the centre column (x=bx+3) is air.
        crate::world::set_block(bx + 2, 99, bz, solid);
        // Cow centred at x=bx+3.05 → box x ∈ [bx+2.6, bx+3.5], overlapping columns 2 and 3.
        let cx = bx as f64 + 3.05;
        assert_eq!(cx.floor() as i32, bx + 3, "centre column is the air column");
        assert_eq!(crate::world::block_state_at(bx + 3, 99, bz), crate::world::AIR_STATE);

        let mv = move_and_collide(cx, 100.0, bz as f64 + 0.5, 0.45, 1.4, 0.0, -0.1, 0.0);
        assert!(mv.on_ground, "cow is supported by the neighbouring column under its box");
        assert!((mv.dy - 0.0).abs() < 1e-9, "downward move clipped to rest on the block top, dy={}", mv.dy);
    }

    #[test]
    fn yaw_turn_is_capped_at_90_degrees_per_tick() {
        // `MoveControl.rotlerp(current, target, 90.0F)` — a target directly behind
        // the mob (180° away) can only be approached by 90° in one tick.
        let mut st = MobState::new(0.5, 65.0, 0.5, 0.0); // yaw 0 → faces +Z (south)
        let mut pos = Pos { x: 0.5, y: 65.0, z: 0.5, yaw: 0.0, pitch: 0.0, on_ground: true };
        // Target due north (−Z): yRotD = atan2(0,-10)*180/PI - 90 = 180 - 90 = 90...
        // atan2(dz=-10, dx=0) = -90°, minus 90 = -180 → wrapped, capped to a 90° turn.
        st.target = Some((0.5, 65.0, -9.5));
        st.pursue_ticks = STROLL_MAX_TICKS;
        st.tick_count = 1; // odd → skip the stroll roll, just steer
        let players = [(0.5f64, 65.0, 0.5)]; // keep noActionTime pinned at 0
        let mut rng = rand::thread_rng();
        step_mob(MobKind::Pig, &mut st, &mut pos, &players, &mut rng);
        // Turned exactly 90° (0° → 270°, the −90° branch), never snapped to 180°.
        assert!(
            (pos.yaw - 270.0).abs() < 1e-3,
            "yaw should turn 90°/tick, got {}",
            pos.yaw
        );
    }

    #[test]
    fn stroll_target_stays_within_land_random_pos_bounds() {
        // `LandRandomPos.getPos(mob, 10, 7)`: horizontal offset is `nextInt(21)-10`
        // per axis, so the block centre lands within ±10 blocks (+0.5) of the mob's
        // column. Sample many draws and assert the horizontal bound holds.
        let pos = Pos { x: 0.5, y: 65.0, z: 0.5, yaw: 0.0, pitch: 0.0, on_ground: true };
        let mut rng = rand::thread_rng();
        for _ in 0..1000 {
            let (tx, _ty, tz) = stroll_target(&pos, &mut rng).expect("target");
            // floor(0.5)=0, so tx = xt + 0.5 with xt ∈ [-10, 10] → tx ∈ [-9.5, 10.5].
            assert!((tx - 0.5).abs() <= 10.0 + 1e-9, "x offset {} exceeds ±10", tx - 0.5);
            assert!((tz - 0.5).abs() <= 10.0 + 1e-9, "z offset {} exceeds ±10", tz - 0.5);
        }
    }

    // --- broadcast_movement unit tests (vanilla ServerEntity.sendChanges) -----

    /// A settled mob fixture: `Pos` and a `MobState` whose last-sent bases all
    /// match the current state (as right after a sent position packet), standing
    /// on ground at yaw 90°.
    fn resting_fixture(world: &mut World) -> (Entity, Pos, MobState) {
        let e = world.spawn_empty().id();
        let pos = Pos { x: 0.5, y: 65.0, z: 0.5, yaw: 90.0, pitch: 0.0, on_ground: true };
        let mut st = MobState::new(0.5, 65.0, 0.5, 90.0);
        st.base_on_ground = true;
        (e, pos, st)
    }

    fn emission_ids(emissions: &[(Entity, Bytes)]) -> Vec<i32> {
        emissions.iter().map(|(_, b)| frame_id(b)).collect()
    }

    fn id_move_pos() -> i32 {
        frame_id(&packets::move_entity_pos(0, 0, 0, 0, false))
    }
    fn id_move_pos_rot() -> i32 {
        frame_id(&packets::move_entity_pos_rot(0, 0, 0, 0, 0, 0, false))
    }
    fn id_move_rot() -> i32 {
        frame_id(&packets::move_entity_rot(0, 0, 0, false))
    }
    fn id_rotate_head() -> i32 {
        frame_id(&packets::rotate_head(0, 0))
    }
    fn id_position_sync() -> i32 {
        frame_id(&packets::entity_position_sync(0, 0.0, 0.0, 0.0, 0.0, 0.0, false))
    }
    fn id_set_motion() -> i32 {
        frame_id(&packets::set_entity_motion(0, 1.0, 0.0, 0.0))
    }

    #[test]
    fn update_interval_gates_broadcast_to_every_third_tick() {
        // `tickCount % updateInterval == 0` — animals use the builder default 3,
        // so a move made on tick 1 is only broadcast on tick 3.
        let mut world = World::new();
        let (e, mut pos, mut st) = resting_fixture(&mut world);
        st.tick_count = 1;
        pos.x += 0.5;
        let mut emissions = Vec::new();
        broadcast_movement(e, 1, &pos, &mut st, &mut emissions); // tick 1
        broadcast_movement(e, 1, &pos, &mut st, &mut emissions); // tick 2
        assert!(emissions.is_empty(), "off-interval ticks are silent");
        broadcast_movement(e, 1, &pos, &mut st, &mut emissions); // tick 3
        assert_eq!(emission_ids(&emissions), vec![id_move_pos()]);
        // The codec base advanced to the sent position.
        assert_eq!(st.base_x, pos.x);
    }

    #[test]
    fn resting_mob_silent_between_forced_resends() {
        // A settled, idle mob is silent from one forced resend to the next,
        // then `tickCount % 60 == 0` forces a position packet without movement.
        let mut world = World::new();
        let (e, pos, mut st) = resting_fixture(&mut world);
        st.tick_count = 1;
        let mut emissions = Vec::new();
        for _ in 1..60 {
            broadcast_movement(e, 1, &pos, &mut st, &mut emissions); // ticks 1..=59
        }
        assert!(emissions.is_empty(), "no packets between forced resends");
        broadcast_movement(e, 1, &pos, &mut st, &mut emissions); // tick 60
        assert_eq!(emission_ids(&emissions), vec![id_move_pos()], "forced zero-delta resend");
    }

    #[test]
    fn packet_selection_pos_vs_posrot_vs_rot() {
        let mut world = World::new();

        // Moved but not turned → MoveEntityPacket.Pos, no head rotation.
        let (e, mut pos, mut st) = resting_fixture(&mut world);
        st.tick_count = 3;
        pos.x += 0.5;
        let mut emissions = Vec::new();
        broadcast_movement(e, 1, &pos, &mut st, &mut emissions);
        assert_eq!(emission_ids(&emissions), vec![id_move_pos()]);

        // Moved and turned → PosRot, then the head follows.
        let (e, mut pos, mut st) = resting_fixture(&mut world);
        st.tick_count = 3;
        pos.x += 0.5;
        pos.yaw = 135.0;
        let mut emissions = Vec::new();
        broadcast_movement(e, 1, &pos, &mut st, &mut emissions);
        assert_eq!(emission_ids(&emissions), vec![id_move_pos_rot(), id_rotate_head()]);
        assert_eq!(st.base_yaw, packets::pack_angle(135.0));

        // Turned only → Rot, then the head follows.
        let (e, mut pos, mut st) = resting_fixture(&mut world);
        st.tick_count = 3;
        pos.yaw = 135.0;
        let mut emissions = Vec::new();
        broadcast_movement(e, 1, &pos, &mut st, &mut emissions);
        assert_eq!(emission_ids(&emissions), vec![id_move_rot(), id_rotate_head()]);
        // Rotation-only send must NOT move the codec base semantics: position
        // base is untouched (nothing to test — it never changed), but the next
        // evaluation with the same yaw is silent (lastSent latched).
        st.tick_count = 6;
        let mut emissions = Vec::new();
        broadcast_movement(e, 1, &pos, &mut st, &mut emissions);
        assert!(emissions.is_empty());
    }

    #[test]
    fn forced_pos_resend_with_turn_sends_posrot_and_updates_base() {
        // At a 60-tick boundary `pos` is forced true; combined with a turn that
        // is PosRot — and the position base must update even though the delta
        // was zero (vanilla `if (sentPosition) setBase`).
        let mut world = World::new();
        let (e, mut pos, mut st) = resting_fixture(&mut world);
        st.tick_count = 60;
        pos.yaw = 135.0;
        let mut emissions = Vec::new();
        broadcast_movement(e, 1, &pos, &mut st, &mut emissions);
        assert_eq!(emission_ids(&emissions), vec![id_move_pos_rot(), id_rotate_head()]);
        assert_eq!((st.base_x, st.base_yaw), (pos.x, packets::pack_angle(135.0)));
    }

    #[test]
    fn ground_flip_forces_absolute_position_sync() {
        // `wasOnGround != onGround()` → ClientboundEntityPositionSyncPacket; no
        // head packet when the yaw is unchanged.
        let mut world = World::new();
        let (e, mut pos, mut st) = resting_fixture(&mut world);
        st.tick_count = 3;
        pos.on_ground = false;
        let mut emissions = Vec::new();
        broadcast_movement(e, 1, &pos, &mut st, &mut emissions);
        assert_eq!(emission_ids(&emissions), vec![id_position_sync()]);
        assert!(!st.base_on_ground, "wasOnGround latched to the new state");
    }

    #[test]
    fn teleport_delay_over_400_forces_absolute_sync_and_resets() {
        // `teleportDelay > 400` (FORCED_TELEPORT_PERIOD) → absolute sync even at
        // rest; the delay resets to 0 afterwards.
        let mut world = World::new();
        let (e, pos, mut st) = resting_fixture(&mut world);
        st.tick_count = 3;
        st.teleport_delay = FORCED_TELEPORT_PERIOD; // +1 this evaluation → 401
        let mut emissions = Vec::new();
        broadcast_movement(e, 1, &pos, &mut st, &mut emissions);
        assert_eq!(emission_ids(&emissions), vec![id_position_sync()]);
        assert_eq!(st.teleport_delay, 0);
    }

    #[test]
    fn head_rotation_sent_only_when_packed_head_yaw_changes() {
        // Sub-packing-resolution yaw wobble (< 360/256 °) does not resend the
        // head; a real turn does, exactly once.
        let mut world = World::new();
        let (e, mut pos, mut st) = resting_fixture(&mut world);
        st.tick_count = 3;
        pos.yaw = 90.5; // packs to the same byte as 90.0
        let mut emissions = Vec::new();
        broadcast_movement(e, 1, &pos, &mut st, &mut emissions);
        assert!(emissions.is_empty(), "sub-resolution turn is invisible on the wire");

        pos.yaw = 135.0;
        st.tick_count = 6;
        let mut emissions = Vec::new();
        broadcast_movement(e, 1, &pos, &mut st, &mut emissions);
        assert_eq!(emission_ids(&emissions), vec![id_move_rot(), id_rotate_head()]);
    }

    #[test]
    fn velocity_drift_sends_motion_before_move_packet() {
        // A velocity change past the 1.0E-7 gate emits SetEntityMotion, and it
        // precedes the move packet (vanilla sends motion first).
        let mut world = World::new();
        let (e, mut pos, mut st) = resting_fixture(&mut world);
        st.tick_count = 3;
        st.vx = 0.12;
        pos.x += 0.12;
        let mut emissions = Vec::new();
        broadcast_movement(e, 1, &pos, &mut st, &mut emissions);
        assert_eq!(emission_ids(&emissions), vec![id_set_motion(), id_move_pos()]);
        assert_eq!(st.last_sent_vx, 0.12);

        // Unchanged velocity on the next evaluation stays silent (motion-wise).
        st.tick_count = 6;
        pos.x += 0.12;
        let mut emissions = Vec::new();
        broadcast_movement(e, 1, &pos, &mut st, &mut emissions);
        assert_eq!(emission_ids(&emissions), vec![id_move_pos()]);
    }

    #[test]
    fn sub_tolerance_position_drift_is_not_broadcast() {
        // `positionCodec.delta(pos).lengthSqr() >= 7.6293945E-6` — a drift below
        // ~2.76e-3 blocks is not movement, even though it changes the 1/4096 grid.
        let mut world = World::new();
        let (e, mut pos, mut st) = resting_fixture(&mut world);
        st.tick_count = 3;
        pos.x += 0.002; // 2e-3² = 4e-6 < 7.63e-6, but ~8 encoded steps
        let mut emissions = Vec::new();
        broadcast_movement(e, 1, &pos, &mut st, &mut emissions);
        assert!(emissions.is_empty(), "sub-tolerance drift stays local");

        pos.x += 0.002; // cumulative 0.004 from the base → 1.6e-5 ≥ tolerance
        st.tick_count = 6;
        let mut emissions = Vec::new();
        broadcast_movement(e, 1, &pos, &mut st, &mut emissions);
        assert_eq!(emission_ids(&emissions), vec![id_move_pos()]);
    }

    // --- Natural spawner (vanilla NaturalSpawner CREATURE port) ---------------

    #[test]
    fn global_cap_matches_vanilla_formula() {
        // `CREATURE.getMaxInstancesPerChunk() (10) * spawnableChunkCount / 289`.
        assert_eq!(creature_global_cap(289), 10);
        assert_eq!(creature_global_cap(28), 0); // 280/289 = 0 → no spawns
        assert_eq!(creature_global_cap(29), 1); // 290/289 = 1 → first slot opens
        assert_eq!(creature_global_cap(441), 15); // a ~10-chunk view distance
    }

    #[test]
    fn farm_animal_weights_favour_sheep_and_cover_all_kinds() {
        // Weighted list sheep 12 / pig 10 / chicken 10 / cow 8: every kind appears
        // over many draws and sheep is the most frequent.
        let mut rng = rand::thread_rng();
        let mut counts = std::collections::HashMap::new();
        for _ in 0..5000 {
            *counts.entry(pick_farm_animal(&mut rng)).or_insert(0u32) += 1;
        }
        for k in [MobKind::Pig, MobKind::Cow, MobKind::Sheep, MobKind::Chicken] {
            assert!(counts.get(&k).copied().unwrap_or(0) > 0, "{k:?} never drawn");
        }
        let sheep = counts[&MobKind::Sheep];
        for k in [MobKind::Pig, MobKind::Cow, MobKind::Chicken] {
            assert!(sheep >= counts[&k], "sheep (12) should be at least as common as {k:?}");
        }
        // Cow (weight 8) should be the rarest over a large sample (loose bound).
        assert!(counts[&MobKind::Cow] < sheep, "cow (8) rarer than sheep (12)");
    }

    #[test]
    fn chunk_near_player_uses_128_block_range() {
        // Chunk (0,0) centre is (8,8); a player at the origin is within 128 blocks.
        assert!(chunk_near_player((0, 0), (0.0, 0.0)));
        // A player exactly 128 blocks from the centre is *not* within (`< 16384`).
        assert!(!chunk_near_player((0, 0), (8.0 + 128.0, 8.0)));
        // Far away → not near.
        assert!(!chunk_near_player((0, 0), (400.0, 0.0)));
    }

    #[test]
    fn local_cap_gates_by_nearest_player_tally() {
        // One player at the origin, ten CREATURE mobs stacked near it → that player
        // is at the per-chunk cap, so a chunk it is near cannot spawn.
        let player = PlayerSnap { x: 0.0, y: 64.0, z: 0.0, loaded: HashSet::new() };
        let mobs: Vec<(f64, f64, f64)> = (0..CREATURE_MAX_PER_CHUNK)
            .map(|_| (4.0, 64.0, 4.0))
            .collect();
        let cap = LocalMobCap::new(std::slice::from_ref(&player), &mobs);
        assert!(!cap.can_spawn((0, 0)), "player at the cap blocks a nearby chunk");
        // A chunk with no player near it also can't spawn (empty near-player list).
        assert!(!cap.can_spawn((100, 100)), "no player near → no spawn");

        // With nine mobs the player is under the cap and the chunk opens up; the
        // tenth `add_mob` closes it again.
        let mut cap = LocalMobCap::new(std::slice::from_ref(&player), &mobs[..9]);
        assert!(cap.can_spawn((0, 0)));
        cap.add_mob((0, 0));
        assert!(!cap.can_spawn((0, 0)), "the tenth mob reaches the local cap");
    }

    #[test]
    fn is_right_distance_rejects_within_24_blocks() {
        let loaded: HashSet<(i32, i32)> = [(0, 0)].into_iter().collect();
        // <= 576 (24²) is rejected; just past it is accepted when the chunk matches.
        assert!(!is_right_distance_to_player(576.0, (0, 0), (0, 0), &loaded));
        assert!(is_right_distance_to_player(577.0, (0, 0), (0, 0), &loaded));
        // A member scattered into an unloaded chunk that isn't the start chunk is
        // rejected even when far enough from the player.
        assert!(!is_right_distance_to_player(2000.0, (500, 500), (0, 0), &loaded));
    }

    #[test]
    fn baby_roll_skips_first_member_then_rolls_five_percent() {
        let mut rng = rand::thread_rng();
        // A fresh group: the first member is never a baby (`groupSize > 0` false).
        let mut g = AgeableGroupData::new();
        assert!(!g.take_baby(&mut rng), "first pack member is never a baby");
        assert_eq!(g.group_size, 1);

        // Subsequent members roll ~5% babies; over many draws some are babies but
        // the clear majority are not (loose bounds around 0.05).
        let mut babies = 0u32;
        let trials = 20_000;
        for _ in 0..trials {
            let mut g = AgeableGroupData::new();
            let _ = g.take_baby(&mut rng); // burn the first (never baby)
            if g.take_baby(&mut rng) {
                babies += 1;
            }
        }
        assert!(babies > 0, "no babies over {trials} rolls");
        assert!(babies < trials / 5, "far too many babies ({babies}/{trials})");
    }

    #[test]
    fn baby_metadata_flag_emitted_only_for_babies() {
        let mut rng = rand::thread_rng();
        let adult = spawn_metadata(MobKind::Pig, 10.0, (0.5, 64.0, 0.5), false, &mut rng);
        assert!(
            !adult.items().iter().any(|it| it.index == AGEABLE_MOB_DATA_BABY),
            "an adult must not carry the baby flag"
        );
        let baby = spawn_metadata(MobKind::Pig, 10.0, (0.5, 64.0, 0.5), true, &mut rng);
        let flag = baby
            .items()
            .iter()
            .find(|it| it.index == AGEABLE_MOB_DATA_BABY)
            .expect("a baby carries DATA_BABY_ID");
        assert_eq!(flag.value, DataValue::Boolean(true));
    }

    #[test]
    fn mob_spawn_populates_real_terrain_end_to_end() {
        // Production-like conditions: a player on the generated surface with a
        // radius-4 loaded set (9×9 = 81 chunks → global cap 2). Many passes on the
        // 400-tick cadence must actually place mobs — this covers the full
        // `mob_spawn` path (random chunk/Y picks, grass/air/light gates) on real
        // worldgen, not a hand-built platform. (Radius 4 rather than a wider set
        // keeps the up-front warm — a full parity generate+light per column, ~40 ms
        // each — bounded: 81 columns instead of 289 still yields a non-zero cap and
        // ample real grass for the spawner to land on, at a fraction of the cost.)
        let _lock = crate::world::WORLD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let (px, pz) = (0, 0);
        let py = crate::world::surface_height(px, pz) + 1;
        let loaded: Vec<(i32, i32)> = (-4..=4)
            .flat_map(|cx| (-4..=4).map(move |cz| (cx, cz)))
            .collect();
        // The spawner now only touches *resident, wire-built* chunks (it never
        // generates or lights on the tick thread). In production the loaded set is
        // exactly the streamed-and-lit columns, so warm them here to model that —
        // otherwise every candidate would be skipped as "not loaded".
        for &(cx, cz) in &loaded {
            let _ = crate::world::chunk_columns(cx, cz);
        }

        let mut world = world_with_id();
        let _rx = spawn_viewer(&mut world, &loaded);
        // Move the viewer onto the surface.
        {
            let mut q = world.query_filtered::<&mut Pos, Without<Mob>>();
            let mut p = q.iter_mut(&mut world).next().unwrap();
            p.x = px as f64 + 0.5;
            p.y = py as f64;
            p.z = pz as f64 + 0.5;
        }

        for pass in 1..=50u64 {
            world.resource_mut::<Tick>().0 = pass * 400;
            mob_spawn(&mut world);
        }
        let count = world.query::<&Mob>().iter(&world).count();
        assert!(count > 0, "no mobs spawned in 50 end-to-end passes");
        assert!(
            count as i32 <= creature_global_cap(loaded.len() as i32),
            "global cap respected"
        );
    }

    #[test]
    fn mob_spawn_respects_cadence_and_global_cap() {
        let mut world = world_with_id();
        // A single loaded column → spawnableChunkCount 1 → global cap 0.
        let _rx = spawn_viewer(&mut world, &[(0, 0)]);

        // Off the 400-tick cadence: nothing runs.
        world.resource_mut::<Tick>().0 = 401;
        mob_spawn(&mut world);
        assert_eq!(world.query::<&Mob>().iter(&world).count(), 0);

        // On cadence but under the cap-1 chunk count: the global gate blocks it.
        world.resource_mut::<Tick>().0 = 400;
        mob_spawn(&mut world);
        assert_eq!(
            world.query::<&Mob>().iter(&world).count(),
            0,
            "cap 0 from a 1-chunk spawnable count blocks all spawns"
        );
    }

    #[test]
    fn spawn_category_scatters_pack_beyond_24_on_grass_with_babies() {
        // A flat grass platform lifted above the terrain (so every scattered member
        // stands on grass under open sky), a player 40 blocks from the start anchor,
        // and many `spawnCategoryForPosition` runs. Assert: mobs spawn, all land
        // beyond 24 blocks of the player, within the ±20 pack scatter of the start,
        // standing on grass — and at least one baby is produced.
        let _lock = crate::world::WORLD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        // Far-away region to avoid colliding with other tests' chunk edits.
        let (base_x, base_z) = (500_000, 0);
        let platform_y = 249; // grass; well above any generated terrain
        let start_y = platform_y + 1;
        let grass = grass_block_state();
        for dx in -20..=20 {
            for dz in -20..=20 {
                crate::world::set_block(base_x + dx, platform_y, base_z + dz, grass);
            }
        }

        let mut world = world_with_id();
        let start = (base_x, start_y, base_z);
        // Player 40 blocks east of the start anchor, at platform height.
        let player = (base_x as f64 + 40.0, start_y as f64, base_z as f64);
        let players = [player];
        // Every column of the platform (and the start chunk) is spawnable/loaded.
        let loaded: HashSet<(i32, i32)> = (-2..=2)
            .flat_map(|cx| (-2..=2).map(move |cz| ((base_x >> 4) + cx, (base_z >> 4) + cz)))
            .collect();
        // Rebuild the wire/light the spawner's brightness gate (`try_raw_brightness`)
        // reads: the platform `set_block`s above invalidated it, and the gate is
        // non-generating, so an unlit column would skip every candidate. In
        // production a re-stream rebuilds this off-thread.
        for &(cx, cz) in &loaded {
            let _ = crate::world::chunk_columns(cx, cz);
        }

        let mut rng = rand::thread_rng();
        let mut cap = LocalMobCap::new(&[], &[]);
        let mut count = 0i32;
        for _ in 0..120 {
            spawn_category_for_position(
                &mut world,
                (base_x >> 4, base_z >> 4),
                start,
                &players,
                &loaded,
                &mut rng,
                &mut count,
                i32::MAX, // no global cap for this fixture
                &mut cap,
            );
        }

        let spawned: Vec<(f64, f64, f64)> = world
            .query_filtered::<&Pos, With<Mob>>()
            .iter(&world)
            .map(|p| (p.x, p.y, p.z))
            .collect();
        assert!(!spawned.is_empty(), "the pack loop should place mobs on the platform");

        for (x, y, z) in &spawned {
            // Beyond 24 blocks of the player (horizontal; y is equal here).
            let (ddx, ddz) = (x - player.0, z - player.2);
            assert!(ddx * ddx + ddz * ddz > MIN_SPAWN_DISTANCE_SQR, "spawned within 24 of player");
            // Within the ±20 pack scatter of the start anchor centre.
            assert!((x - (base_x as f64 + 0.5)).abs() <= 20.5, "x scatter out of bounds: {x}");
            assert!((z - (base_z as f64 + 0.5)).abs() <= 20.5, "z scatter out of bounds: {z}");
            // Standing on grass: the block below the feet is the platform.
            assert_eq!(
                crate::world::block_state_at(x.floor() as i32, *y as i32 - 1, z.floor() as i32),
                grass,
                "mob is not standing on grass"
            );
        }

        // At least one baby over the whole run (members after the first, 5% each).
        let any_baby = world
            .query_filtered::<&EntityMeta, With<Mob>>()
            .iter(&world)
            .any(|m| m.0.items().iter().any(|it| it.index == AGEABLE_MOB_DATA_BABY));
        assert!(any_baby, "expected at least one baby across many packs");
    }

    // --- Damage / death path (LivingEntity.hurtServer + die) ------------------

    use super::super::entity::{ItemDrop, XpOrb};

    /// A player-attack context from an attacker at `(ax, az)`; attacker id 99.
    fn attack_from(ax: f64, az: f64) -> DamageContext {
        DamageContext::player_attack(99, (ax, az))
    }

    fn item_id_of(name: &str) -> i32 {
        crate::registry::item::id_of(name).expect("known item").get()
    }

    /// Every dropped-item stack currently in the world (from `spawn_item_entity`).
    fn item_drops(world: &mut World) -> Vec<ItemStack> {
        let mut q = world.query_filtered::<&EntityMeta, With<ItemDrop>>();
        q.iter(world)
            .filter_map(|m| {
                m.0.items().iter().find_map(|it| match it.value {
                    DataValue::ItemStack(Some(s)) => Some(s),
                    _ => None,
                })
            })
            .collect()
    }

    /// Every XP-orb value currently in the world (from `spawn_xp_orb`).
    fn xp_values(world: &mut World) -> Vec<i32> {
        let mut q = world.query_filtered::<&EntityMeta, With<XpOrb>>();
        q.iter(world)
            .filter_map(|m| {
                m.0.items().iter().find_map(|it| match it.value {
                    DataValue::Int(v) => Some(v),
                    _ => None,
                })
            })
            .collect()
    }

    fn only_mob(world: &mut World) -> Entity {
        world.query_filtered::<Entity, With<Mob>>().iter(world).next().unwrap()
    }

    #[test]
    fn full_hit_reduces_health_and_broadcasts_damage_event() {
        let mut world = world_with_id();
        let mut rx = spawn_viewer(&mut world, &[(0, 0)]);
        spawn_mob(&mut world, MobKind::Chicken, (0.5, 64.0, 0.5)); // 4 hp
        let _ = drain(&mut rx);
        let e = only_mob(&mut world);

        assert!(!damage(&mut world, e, 1.0, &attack_from(2.0, 0.5))); // 4 -> 3, survives
        assert_eq!(world.get::<Health>(e).unwrap().current, 3.0);

        // The metadata (health) and a DamageEvent both reached the viewer; the
        // legacy hurt_animation packet is NOT sent for the mob path.
        let ids = drain(&mut rx);
        let data_id = frame_id(&epackets::set_entity_data(0, &EntityData::new()));
        let dmg_id = frame_id(&epackets::damage_event(0, 0, -1, -1, None));
        let hurt_id = frame_id(&epackets::hurt_animation(0, 0.0));
        assert!(ids.contains(&data_id), "health metadata synced");
        assert!(ids.contains(&dmg_id), "DamageEvent broadcast on a full hit");
        assert!(!ids.contains(&hurt_id), "no legacy hurt_animation on the mob path");
    }

    #[test]
    fn partial_hit_within_iframes_sends_no_damage_event() {
        let mut world = world_with_id();
        let mut rx = spawn_viewer(&mut world, &[(0, 0)]);
        spawn_mob(&mut world, MobKind::Cow, (0.5, 64.0, 0.5)); // 10 hp
        let _ = drain(&mut rx);
        let e = only_mob(&mut world);

        // First (full) hit sets the i-frame window and lastHurt = 3.
        assert!(!damage(&mut world, e, 3.0, &attack_from(2.0, 0.5)));
        let _ = drain(&mut rx);
        // A weaker re-hit within i-frames is fully absorbed (amount <= lastHurt):
        // no health change, no packets at all.
        assert!(!damage(&mut world, e, 2.0, &attack_from(2.0, 0.5)));
        assert_eq!(world.get::<Health>(e).unwrap().current, 7.0);
        assert!(drain(&mut rx).is_empty(), "absorbed re-hit is silent");

        // A stronger re-hit lands only its excess (5 - 3 = 2) and is a *partial*
        // hit: health drops, metadata syncs, but NO DamageEvent (tookFullDamage false).
        assert!(!damage(&mut world, e, 5.0, &attack_from(2.0, 0.5)));
        assert_eq!(world.get::<Health>(e).unwrap().current, 5.0); // 7 - 2
        let ids = drain(&mut rx);
        let data_id = frame_id(&epackets::set_entity_data(0, &EntityData::new()));
        let dmg_id = frame_id(&epackets::damage_event(0, 0, -1, -1, None));
        assert!(ids.contains(&data_id), "partial hit still syncs health");
        assert!(!ids.contains(&dmg_id), "partial hit sends no DamageEvent");
    }

    #[test]
    fn killing_blow_syncs_health_zero_and_keeps_corpse() {
        let mut world = world_with_id();
        let mut rx = spawn_viewer(&mut world, &[(0, 0)]);
        spawn_mob(&mut world, MobKind::Chicken, (0.5, 64.0, 0.5)); // 4 hp
        let _ = drain(&mut rx);
        let e = only_mob(&mut world);

        assert!(damage(&mut world, e, 10.0, &attack_from(2.0, 0.5)), "fatal");
        // The corpse is NOT removed yet (death animation plays first).
        assert_eq!(world.query::<&Mob>().iter(&world).count(), 1, "corpse persists");
        assert!(world.get::<MobState>(e).unwrap().dead, "entered the dying state");
        assert_eq!(world.get::<Health>(e).unwrap().current, 0.0, "health synced to 0");
        // The health-0 metadata (fall-over trigger) reached the viewer.
        let data_id = frame_id(&epackets::set_entity_data(0, &EntityData::new()));
        assert!(drain(&mut rx).contains(&data_id));
    }

    #[test]
    fn corpse_is_removed_after_20_ticks_with_poof() {
        let mut world = world_with_id();
        let mut rx = spawn_viewer(&mut world, &[(0, 0)]);
        let sy = crate::world::surface_height(0, 0);
        spawn_mob(&mut world, MobKind::Chicken, (0.5, (sy + 1) as f64, 0.5));
        let e = only_mob(&mut world);
        assert!(damage(&mut world, e, 10.0, &attack_from(2.0, 0.5)));
        let _ = drain(&mut rx);

        // 19 ticks: still a corpse.
        for _ in 0..(DEATH_TICKS - 1) {
            mob_tick(&mut world);
        }
        assert_eq!(world.query::<&Mob>().iter(&world).count(), 1, "corpse persists 19 ticks");

        // 20th tick: poof EntityEvent then RemoveEntities, and the mob is gone.
        mob_tick(&mut world);
        assert_eq!(world.query::<&Mob>().iter(&world).count(), 0, "removed at deathTime 20");
        let ids = drain(&mut rx);
        let poof_id = frame_id(&epackets::entity_event(0, ENTITY_EVENT_POOF));
        let remove_id = frame_id(&crate::sim::packets::remove_entities(&[0]));
        assert!(ids.contains(&poof_id), "poof EntityEvent broadcast");
        assert!(ids.contains(&remove_id), "RemoveEntities broadcast");
    }

    #[test]
    fn knockback_pushes_velocity_away_from_attacker() {
        let mut world = world_with_id();
        let _rx = spawn_viewer(&mut world, &[(0, 0)]);
        spawn_mob(&mut world, MobKind::Cow, (0.5, 64.0, 0.5)); // 10 hp, survives a 1-dmg hit
        let e = only_mob(&mut world);
        // Put the corpse-to-be on the ground so the vertical knockback lands.
        world.get_mut::<Pos>(e).unwrap().on_ground = true;

        // Attacker to the +x side → the mob is pushed toward −x.
        assert!(!damage(&mut world, e, 1.0, &attack_from(5.0, 0.5)));
        let st = world.get::<MobState>(e).unwrap();
        assert!(st.vx < 0.0, "knocked away from +x attacker, got vx={}", st.vx);
        assert!(st.vy > 0.0, "grounded knockback adds upward velocity, got vy={}", st.vy);
        assert!(st.vz.abs() < 1e-9, "attacker on the x axis → no z knockback");
    }

    #[test]
    fn adult_pig_drops_porkchop_and_xp_on_player_kill() {
        let mut world = world_with_id();
        let _rx = spawn_viewer(&mut world, &[(0, 0)]);
        spawn_mob(&mut world, MobKind::Pig, (0.5, 64.0, 0.5)); // 10 hp
        let e = only_mob(&mut world);
        assert!(damage(&mut world, e, 20.0, &attack_from(2.0, 0.5)));

        // Porkchop 1..=3 dropped as an item entity.
        let drops = item_drops(&mut world);
        let porkchop = item_id_of("minecraft:porkchop");
        assert_eq!(drops.len(), 1, "pig drops exactly one porkchop stack");
        assert_eq!(drops[0].id.get(), porkchop);
        assert!((1..=3).contains(&drops[0].count), "porkchop 1..=3, got {}", drops[0].count);

        // XP orb(s) worth a 1..=3 reward.
        let xp: i32 = xp_values(&mut world).iter().sum();
        assert!((1..=3).contains(&xp), "XP reward 1..=3, got {xp}");
    }

    #[test]
    fn cow_drops_beef_and_optional_leather() {
        // Over many kills, cow always drops beef 1..=3 and sometimes leather 0..=2.
        let beef = item_id_of("minecraft:beef");
        let leather = item_id_of("minecraft:leather");
        let mut saw_leather = false;
        for _ in 0..64 {
            let mut world = world_with_id();
            let _rx = spawn_viewer(&mut world, &[(0, 0)]);
            spawn_mob(&mut world, MobKind::Cow, (0.5, 64.0, 0.5));
            let e = only_mob(&mut world);
            damage(&mut world, e, 20.0, &attack_from(2.0, 0.5));
            let drops = item_drops(&mut world);
            let beef_stack = drops.iter().find(|s| s.id.get() == beef).expect("beef always drops");
            assert!((1..=3).contains(&beef_stack.count));
            if let Some(l) = drops.iter().find(|s| s.id.get() == leather) {
                assert!((1..=2).contains(&l.count));
                saw_leather = true;
            }
        }
        assert!(saw_leather, "leather should drop on at least one of 64 cow kills");
    }

    #[test]
    fn sheep_drops_mutton_and_one_wool_of_its_colour() {
        let mut world = world_with_id();
        let _rx = spawn_viewer(&mut world, &[(0, 0)]);
        spawn_mob(&mut world, MobKind::Sheep, (0.5, 64.0, 0.5));
        let e = only_mob(&mut world);
        // Its rolled wool colour, as an item name.
        let wool = sheep_wool_color(&world, e).expect("sheep has a wool colour");
        let wool_item = item_id_of(wool_item_name(wool));

        damage(&mut world, e, 20.0, &attack_from(2.0, 0.5));
        let drops = item_drops(&mut world);
        let mutton = item_id_of("minecraft:mutton");
        let mutton_stack = drops.iter().find(|s| s.id.get() == mutton).expect("mutton drops");
        assert!((1..=2).contains(&mutton_stack.count));
        let wool_stack = drops.iter().find(|s| s.id.get() == wool_item).expect("wool of its colour");
        assert_eq!(wool_stack.count, 1, "exactly one wool");
    }

    #[test]
    fn baby_drops_nothing_and_no_xp() {
        let mut world = world_with_id();
        let _rx = spawn_viewer(&mut world, &[(0, 0)]);
        // Spawn a baby pig directly (spawn_mob_with with baby = true).
        spawn_mob_with(&mut world, MobKind::Pig, (0.5, 64.0, 0.5), true);
        let e = only_mob(&mut world);
        assert!(is_baby(&world, e), "spawned a baby");
        assert!(damage(&mut world, e, 20.0, &attack_from(2.0, 0.5)));
        assert!(item_drops(&mut world).is_empty(), "babies drop no loot");
        assert!(xp_values(&mut world).is_empty(), "babies drop no XP");
    }

    #[test]
    fn experience_value_splits_like_vanilla() {
        assert_eq!(experience_value(1), 1);
        assert_eq!(experience_value(2), 1); // 2 → orb 1, then orb 1
        assert_eq!(experience_value(3), 3);
        assert_eq!(experience_value(7), 7);
    }
}
