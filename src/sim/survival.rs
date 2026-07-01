//! The player survival loop: health, hunger (`FoodData`), damage sources, death,
//! and respawn. A 1:1 port of the relevant slices of `world/food/FoodData`,
//! `world/entity/LivingEntity`, `world/entity/player/Player`, and
//! `server/level/ServerPlayer` (MC 26.2), minus the parts that need systems Vela
//! does not model yet (armor/absorption/status effects/knockback — noted below).
//!
//! ## Damage entry point (the mob-combat seam)
//!
//! [`hurt`] is the *single* place damage is applied. Environmental sources call it
//! today ([`survival_tick`] for void/starvation, [`handle_move`] for fall); the
//! entities milestone wires mob attacks through the same function with
//! [`DamageKind::MobAttack`], so combat merges here without touching the health/
//! death pipeline. It ports `Player.hurtServer` → `LivingEntity.hurtServer` →
//! `actuallyHurt`: game-mode invulnerability, difficulty scaling, the 20-tick
//! i-frame cooldown, food-exhaustion on hit, the health subtraction, and death.
//!
//! ## Not modelled yet (clean gaps, faithful where present)
//!
//! Armor/toughness, absorption hearts, status effects (Resistance/Regeneration/
//! Fire Resistance), enchantment protection, item blocking, and knockback are all
//! absent — `actuallyHurt` here is the no-mitigation path. Drowning/suffocation
//! are exposed as [`DamageKind`]s for callers but have no environment probe wired
//! (no fluid/again-block model); void and fall are fully driven.

use bevy_ecs::prelude::*;
use tracing::info;

use super::bridge::Outbound;
use super::components::*;
use super::packets;
use super::world_tick::GameRules;

/// Default and maximum player health — `Attributes.MAX_HEALTH` = 20 (10 hearts).
pub const MAX_HEALTH: f32 = 20.0;
/// `Attributes.SAFE_FALL_DISTANCE` — blocks of a fall absorbed before it hurts.
const SAFE_FALL_DISTANCE: f64 = 3.0;
/// The i-frame window a full hit opens (`LivingEntity.invulnerableTime = 20`).
/// Damage within the first half (`> 10`) only applies the amount above the last
/// hit; after it decays past 10 a fresh full hit lands.
const INVULNERABLE_TICKS: i32 = 20;
/// Damage dealt per applicable tick below the world floor (`Entity` fell-out-of-
/// world hits for 4.0 — half a heart shy of two hearts).
const VOID_DAMAGE: f32 = 4.0;

/// World difficulty (`net.minecraft.world.Difficulty`), ordinals matching
/// `Difficulty.byId` (peaceful=0 … hard=3). Drives food-drain, starvation floor,
/// and per-source difficulty scaling.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Difficulty {
    Peaceful,
    Easy,
    Normal,
    Hard,
}

impl Difficulty {
    /// Resolve a wire/ordinal id to a difficulty (`Difficulty.byId`, out-of-range
    /// clamps to peaceful).
    pub fn from_id(id: u8) -> Self {
        match id {
            1 => Difficulty::Easy,
            2 => Difficulty::Normal,
            3 => Difficulty::Hard,
            _ => Difficulty::Peaceful,
        }
    }
}

/// A source of damage, carrying the two parity-relevant bits of a vanilla
/// `DamageType`: the death-message key suffix (`msgId`) and the food exhaustion
/// added to the victim when the hit lands (`DamageType.exhaustion`). This is the
/// enum the mob-combat milestone extends (`MobAttack` is already here as the
/// seam).
// Drown/Suffocation/Generic/MobAttack are the exposed seam for callers not yet
// wired (mob combat lands in the entities milestone; drowning/suffocation need a
// fluid/again-block probe). Kept deliberately so the entry point is complete.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DamageKind {
    /// `minecraft:fall` — falling too far.
    Fall,
    /// `minecraft:out_of_world` — below the world floor (the void).
    Void,
    /// `minecraft:starve` — an empty food bar.
    Starve,
    /// `minecraft:drown` — out of air underwater.
    Drown,
    /// `minecraft:in_wall` — suffocating inside a block.
    Suffocation,
    /// `minecraft:generic` — an unattributed hit.
    Generic,
    /// `minecraft:mob_attack` — a mob's melee. The seam for the entities milestone.
    MobAttack,
}

impl DamageKind {
    /// The `DamageType.msgId` — the `death.attack.<id>` key suffix.
    fn msg_id(self) -> &'static str {
        match self {
            DamageKind::Fall => "fall",
            DamageKind::Void => "outOfWorld",
            DamageKind::Starve => "starve",
            DamageKind::Drown => "drown",
            DamageKind::Suffocation => "inWall",
            DamageKind::Generic => "generic",
            DamageKind::MobAttack => "mob",
        }
    }

    /// The food exhaustion added when this source hurts the victim
    /// (`DamageType.exhaustion`, applied in `Player.actuallyHurt`).
    fn food_exhaustion(self) -> f32 {
        match self {
            DamageKind::MobAttack => 0.1,
            // fall / void / starve / drown / suffocation / generic all 0.0.
            _ => 0.0,
        }
    }

    /// Whether creative players still take this source. Vanilla gates on the
    /// `BYPASSES_INVULNERABILITY` damage-type tag; only the void (fell-out-of-
    /// world) member is relevant here — creative players still die in the void.
    fn bypasses_invulnerability(self) -> bool {
        matches!(self, DamageKind::Void)
    }

    /// Whether the amount scales with difficulty in `Player.hurtServer`. Keyed off
    /// the `DamageType.scaling` enum; the environmental sources here never scale
    /// (no causing entity), while `MobAttack` scales `WHEN_CAUSED_BY_LIVING_NON_
    /// PLAYER` — true once real mobs drive it.
    fn scales_with_difficulty(self) -> bool {
        matches!(self, DamageKind::MobAttack)
    }
}

/// A player's health and hurt state, mirroring the `LivingEntity` health fields.
#[derive(Component, Clone, Copy)]
pub struct Health {
    /// Current health (`DATA_HEALTH_ID`), clamped to `0..=max`.
    pub current: f32,
    /// Maximum health (`Attributes.MAX_HEALTH`, 20 for a player).
    pub max: f32,
    /// Accumulated fall distance since last on-ground, in blocks (`Entity.fallDistance`).
    pub fall_distance: f64,
    /// Remaining i-frames (`LivingEntity.invulnerableTime`), decremented per tick.
    pub invulnerable_time: i32,
    /// The damage of the hit that opened the current i-frame window (`lastHurt`).
    pub last_hurt: f32,
    /// Set once health hits 0 and the death screen is sent, cleared on respawn.
    pub dead: bool,
    // --- HUD sync tracking (`ServerPlayer.lastSentHealth/Food/FoodSaturationZero`) ---
    last_sent_health: f32,
    last_sent_food: i32,
    last_sent_saturation_zero: bool,
}

impl Health {
    /// Fresh health at `current` (max 20), seeded so the first [`sync_health`]
    /// always emits a `SetHealth` (vanilla seeds `lastSentHealth = -1`).
    pub fn new(current: f32) -> Self {
        Self {
            current,
            max: MAX_HEALTH,
            fall_distance: 0.0,
            invulnerable_time: 0,
            last_hurt: 0.0,
            dead: false,
            last_sent_health: -1.0,
            last_sent_food: -1,
            last_sent_saturation_zero: false,
        }
    }

    /// `LivingEntity.setHealth`: clamp into `0..=max`.
    fn set_health(&mut self, health: f32) {
        self.current = health.clamp(0.0, self.max);
    }

    /// `LivingEntity.heal`: only a living entity heals.
    fn heal(&mut self, amount: f32) {
        if self.current > 0.0 {
            self.set_health(self.current + amount);
        }
    }

    /// `Player.isHurt`: alive and below max — the regen precondition.
    fn is_hurt(&self) -> bool {
        self.current > 0.0 && self.current < self.max
    }

    /// `LivingEntity.isDeadOrDying`.
    fn is_dead_or_dying(&self) -> bool {
        self.current <= 0.0 || self.dead
    }
}

impl Default for Health {
    fn default() -> Self {
        Health::new(MAX_HEALTH)
    }
}

/// A player's hunger state, a 1:1 port of `world/food/FoodData.java`.
#[derive(Component, Clone, Copy)]
pub struct FoodData {
    /// `foodLevel` (0..=20).
    pub food_level: i32,
    /// `saturationLevel` (0..=food_level).
    pub saturation_level: f32,
    /// `exhaustionLevel` (0..=40); drains saturation/food past 4.0.
    pub exhaustion_level: f32,
    /// `tickTimer` gating the regen/starve cadence.
    pub tick_timer: i32,
}

impl Default for FoodData {
    fn default() -> Self {
        // FoodData field defaults: foodLevel 20, saturationLevel 5.0, rest 0.
        Self {
            food_level: 20,
            saturation_level: 5.0,
            exhaustion_level: 0.0,
            tick_timer: 0,
        }
    }
}

impl FoodData {
    /// `FoodData.addExhaustion`: accumulate, capped at 40.
    pub fn add_exhaustion(&mut self, amount: f32) {
        self.exhaustion_level = (self.exhaustion_level + amount).min(40.0);
    }

    /// `FoodData.needsFood`.
    #[allow(dead_code)]
    pub fn needs_food(&self) -> bool {
        self.food_level < 20
    }

    /// One `FoodData.tick`: drain exhaustion→saturation→food, then run the regen /
    /// starvation cadence, healing `health` in place. Returns `true` when a 1.0
    /// starvation hit should be applied by the caller (kept out of here so the
    /// full [`hurt`] pipeline — i-frames, death — runs for it).
    ///
    /// 1:1 with `FoodData.tick` including the 26.2 *fast* saturation-heal path
    /// (`saturation > 0 && food >= 20`, every 10 ticks) that precedes the classic
    /// slow path (`food >= 18`, every 80 ticks).
    pub fn tick(&mut self, health: &mut Health, difficulty: Difficulty, natural_regen: bool) -> bool {
        if self.exhaustion_level > 4.0 {
            self.exhaustion_level -= 4.0;
            if self.saturation_level > 0.0 {
                self.saturation_level = (self.saturation_level - 1.0).max(0.0);
            } else if difficulty != Difficulty::Peaceful {
                self.food_level = (self.food_level - 1).max(0);
            }
        }

        let mut starve = false;
        if natural_regen && self.saturation_level > 0.0 && health.is_hurt() && self.food_level >= 20 {
            // Fast regen: up to 1.0 HP per 10 ticks, spending saturation.
            self.tick_timer += 1;
            if self.tick_timer >= 10 {
                let saturation_spent = self.saturation_level.min(6.0);
                health.heal(saturation_spent / 6.0);
                self.add_exhaustion(saturation_spent);
                self.tick_timer = 0;
            }
        } else if natural_regen && self.food_level >= 18 && health.is_hurt() {
            // Slow regen: 1.0 HP per 80 ticks.
            self.tick_timer += 1;
            if self.tick_timer >= 80 {
                health.heal(1.0);
                self.add_exhaustion(6.0);
                self.tick_timer = 0;
            }
        } else if self.food_level <= 0 {
            // Starvation, gated by difficulty floor: EASY stops at 10 HP, NORMAL
            // at 1 HP, HARD kills outright.
            self.tick_timer += 1;
            if self.tick_timer >= 80 {
                if health.current > 10.0
                    || difficulty == Difficulty::Hard
                    || (health.current > 1.0 && difficulty == Difficulty::Normal)
                {
                    starve = true;
                }
                self.tick_timer = 0;
            }
        } else {
            self.tick_timer = 0;
        }
        starve
    }
}

/// The per-tick survival system: decay i-frames, apply void damage, run
/// [`FoodData::tick`] (food drain / regen / starvation) for every player, then
/// sync any changed HUD state via `SetHealth`. Exclusive because damage/death
/// fan packets across connections.
pub fn survival_tick(world: &mut World) {
    let difficulty = Difficulty::from_id(world.resource::<Config>().0.properties.difficulty());
    let (natural_regen, show_death) = {
        let rules = world.resource::<GameRules>();
        (rules.natural_regeneration, rules.show_death_messages)
    };
    // Void floor: vanilla hurts below `level.getMinY() - 64`.
    let void_y = (crate::world::MIN_Y - 64) as f64;

    let entities: Vec<Entity> = {
        let mut q = world.query_filtered::<Entity, With<Health>>();
        q.iter(world).collect()
    };

    for entity in entities {
        // Decay i-frames (for a ServerPlayer this happens in the base entity tick,
        // not aiStep, so it runs every tick regardless of hit state).
        if let Some(mut h) = world.get_mut::<Health>(entity) {
            if h.invulnerable_time > 0 {
                h.invulnerable_time -= 1;
            }
        }
        // A dead player is inert until it respawns.
        if world.get::<Health>(entity).is_none_or(|h| h.dead) {
            continue;
        }

        // Void damage.
        if world.get::<Pos>(entity).is_some_and(|p| p.y < void_y) {
            hurt(world, entity, DamageKind::Void, VOID_DAMAGE, difficulty, show_death);
            if world.get::<Health>(entity).is_none_or(|h| h.dead) {
                continue;
            }
        }

        // Food / regen / starvation. Copy both components out, run the tick, write
        // back (bevy hands out one `&mut` at a time).
        let (Some(mut food), Some(mut health)) = (
            world.get::<FoodData>(entity).copied(),
            world.get::<Health>(entity).copied(),
        ) else {
            continue;
        };
        let starve = food.tick(&mut health, difficulty, natural_regen);
        *world.get_mut::<FoodData>(entity).unwrap() = food;
        *world.get_mut::<Health>(entity).unwrap() = health;
        if starve {
            hurt(world, entity, DamageKind::Starve, 1.0, difficulty, show_death);
        }
    }

    sync_health(world);
}

/// The single damage entry point (the mob-combat seam — see the module note).
/// Ports `Player.hurtServer` → `LivingEntity.hurtServer` → `actuallyHurt` (minus
/// armor/absorption/effects/knockback), driving death when health reaches 0.
/// Returns whether the hit landed.
pub fn hurt(
    world: &mut World,
    entity: Entity,
    kind: DamageKind,
    amount: f32,
    difficulty: Difficulty,
    show_death: bool,
) -> bool {
    // Game-mode invulnerability (`Player.abilities.invulnerable`): spectators are
    // untouchable; creative takes only sources that bypass invulnerability (void).
    match world.get::<GameMode>(entity).copied() {
        Some(GameMode::Spectator) => return false,
        Some(GameMode::Creative) if !kind.bypasses_invulnerability() => return false,
        _ => {}
    }

    let mut amount = amount.max(0.0);
    // Player.hurtServer difficulty scaling, for sources that scale with it.
    if kind.scales_with_difficulty() {
        amount = match difficulty {
            Difficulty::Peaceful => 0.0,
            Difficulty::Easy => (amount / 2.0 + 1.0).min(amount),
            Difficulty::Normal => amount,
            Difficulty::Hard => amount * 3.0 / 2.0,
        };
    }
    if amount == 0.0 {
        return false;
    }

    // LivingEntity.hurtServer: dead-check + the 20-tick i-frame cooldown. None of
    // our sources bypass the cooldown, so a hit within the window only applies the
    // amount above the last hit.
    let landed = {
        let Some(mut h) = world.get_mut::<Health>(entity) else {
            return false;
        };
        if h.is_dead_or_dying() {
            return false;
        }
        if h.invulnerable_time > 10 {
            if amount <= h.last_hurt {
                return false;
            }
            let extra = amount - h.last_hurt;
            h.last_hurt = amount;
            extra
        } else {
            h.last_hurt = amount;
            h.invulnerable_time = INVULNERABLE_TICKS;
            amount
        }
    };

    // actuallyHurt: food exhaustion from the hit, then subtract from health.
    if let Some(mut food) = world.get_mut::<FoodData>(entity) {
        food.add_exhaustion(kind.food_exhaustion());
    }
    let dead = {
        let mut h = world.get_mut::<Health>(entity).expect("health present");
        let new = h.current - landed;
        h.set_health(new);
        h.is_dead_or_dying()
    };
    if dead {
        die(world, entity, kind, show_death);
    }
    true
}

/// Fall-distance tracking and sprint exhaustion, driven from each applied
/// movement (`ServerGamePacketListenerImpl` → `Entity.checkFallDamage` +
/// `ServerPlayer.checkMovementStatistics`). Accumulate downward travel while
/// airborne and, on landing, apply `floor(distance + 1e-6 - 3.0)` fall damage.
pub fn handle_move(
    world: &mut World,
    entity: Entity,
    old: (f64, f64, f64),
    new: (f64, f64, f64),
    on_ground: bool,
    sprinting: bool,
) {
    let (_ox, oy, _oz) = old;
    let (nx, ny, nz) = new;

    // Fall accumulation / landing (Entity.checkFallDamage).
    let fall_damage = {
        let Some(mut h) = world.get_mut::<Health>(entity) else {
            return;
        };
        if h.dead {
            return;
        }
        if on_ground {
            let distance = h.fall_distance;
            h.fall_distance = 0.0;
            if distance > 0.0 {
                // calculateFallDamage: floor(power * modifier(1.0) * multiplier(1.0)),
                // power = distance + 1e-6 - SAFE_FALL_DISTANCE.
                (distance + 1.0e-6 - SAFE_FALL_DISTANCE).floor() as i32
            } else {
                0
            }
        } else {
            if ny < oy {
                h.fall_distance += oy - ny;
            }
            0
        }
    };

    // Sprint exhaustion (checkMovementStatistics, on-ground sprinting branch):
    // 0.1 * horizontalDistanceCm * 0.01, horizontalDistanceCm = round(dist*100).
    if on_ground && sprinting {
        let (dx, dz) = (nx - old.0, nz - old.2);
        let horizontal_cm = ((dx * dx + dz * dz).sqrt() * 100.0).round() as i32;
        if horizontal_cm > 0 {
            if let Some(mut food) = world.get_mut::<FoodData>(entity) {
                food.add_exhaustion(0.1 * horizontal_cm as f32 * 0.01);
            }
        }
    }

    if fall_damage > 0 {
        let difficulty = Difficulty::from_id(world.resource::<Config>().0.properties.difficulty());
        let show_death = world.resource::<GameRules>().show_death_messages;
        hurt(world, entity, DamageKind::Fall, fall_damage as f32, difficulty, show_death);
    }
}

/// Death: mark the player dead, open the death screen (`PlayerCombatKill`),
/// broadcast the death message, and drop inventory (unless `keepInventory`).
/// Mirrors `ServerPlayer.die`.
fn die(world: &mut World, entity: Entity, kind: DamageKind, show_death: bool) {
    {
        let mut h = world.get_mut::<Health>(entity).expect("health present");
        h.dead = true;
        h.current = 0.0;
    }
    let Some((name, entity_id)) = world.get::<Profile>(entity).map(|p| (p.name.clone(), p.entity_id))
    else {
        return;
    };

    // death.attack.<msgId> with the victim's name — the common single-arg form of
    // CombatTracker.getDeathMessage / DamageSource.getLocalizedDeathMessage.
    let message = super::text::translatable(
        &format!("death.attack.{}", kind.msg_id()),
        vec![super::text::text(name.clone())],
    );

    // Death screen to the victim.
    send_to(world, entity, packets::player_combat_kill(entity_id, &message));
    // Broadcast the message to everyone (incl. the victim) as system chat.
    if show_death {
        let bytes = packets::system_chat_component(&message);
        broadcast_all(world, bytes);
    }
    info!(%name, cause = kind.msg_id(), "died");

    drop_inventory_on_death(world, entity);
}

/// Drop the dead player's inventory as item entities and clear it, unless the
/// `keepInventory` game rule is set (`ServerPlayer.dropAllDeathLoot`, loosely — no
/// XP orbs / armor-slot distinction). A clean seam: gated on the rule so a
/// keep-inventory world leaves the stacks in place for respawn to re-sync.
fn drop_inventory_on_death(world: &mut World, entity: Entity) {
    if world.resource::<GameRules>().keep_inventory {
        return;
    }
    let Some(pos) = world.get::<Pos>(entity).map(|p| (p.x, p.y, p.z)) else {
        return;
    };
    let stacks: Vec<crate::inventory::ItemStack> = {
        let Some(mut inv) = world.get_mut::<crate::inventory::Inventory>(entity) else {
            return;
        };
        let mut out = Vec::new();
        for slot in inv.slots.iter_mut() {
            if let Some(stack) = slot.take() {
                out.push(stack);
            }
        }
        if let Some(carried) = inv.carried.take() {
            out.push(carried);
        }
        out
    };
    for stack in stacks {
        super::entity::spawn_item_entity(world, pos, stack);
    }
}

/// Handle a `PERFORM_RESPAWN` request: reset health/food, teleport to spawn, and
/// re-create the player's client level (`Respawn`). Mirrors the death-respawn path
/// of `PlayerList.respawn` / `ServerPlayer.restoreFrom(_, false)`: health → max,
/// `FoodData` → fresh defaults, inventory already cleared on death.
pub fn respawn_player(world: &mut World, entity: Entity) {
    // Ignore respawn requests from a player who isn't dead.
    if world.get::<Health>(entity).is_none_or(|h| !h.dead) {
        return;
    }

    // Reset survival state to fresh defaults (a death respawn keeps nothing).
    *world.get_mut::<Health>(entity).expect("health present") = Health::new(MAX_HEALTH);
    if let Some(mut food) = world.get_mut::<FoodData>(entity) {
        *food = FoodData::default();
    }

    // Spawn is the top of the origin column, matching the join sequence.
    let (sx, sz) = (0.0f64, 0.0f64);
    let sy = (crate::world::surface_height(0, 0) + 1) as f64;
    if let Some(mut pos) = world.get_mut::<Pos>(entity) {
        pos.x = sx;
        pos.y = sy;
        pos.z = sz;
        pos.yaw = 0.0;
        pos.pitch = 0.0;
        pos.on_ground = false;
    }
    // Reset the broadcast base so the next movement tick sends deltas from spawn,
    // and immediately teleport the entity for other viewers.
    let entity_id = world.get::<Profile>(entity).map(|p| p.entity_id);
    if let Some(mut t) = world.get_mut::<Tracking>(entity) {
        t.base_x = sx;
        t.base_y = sy;
        t.base_z = sz;
        t.on_ground = false;
        t.teleport_delay = 0;
    }

    let game_type = world
        .get::<GameMode>(entity)
        .map(|gm| *gm as u8)
        .unwrap_or(0);

    // Re-create the client level. The Respawn packet makes the client drop its
    // loaded level, so re-stream the spawn region and reseed the loaded-chunk set
    // (mirroring the fresh chunk send in `PlayerList.respawn`) — otherwise
    // `stream_chunks`, believing the columns are still loaded, sends nothing and
    // the player respawns into an empty world.
    send_to(world, entity, packets::respawn(game_type, 0));
    // `getPlayerViewDistance` clamp, same as the join/streaming paths: honour the
    // player's requested distance, capped by the server's. The component is attached
    // at join, so the fallback default is only defensive.
    let server_view_distance = world.resource::<Config>().0.properties.view_distance();
    let radius = world
        .get::<RequestedViewDistance>(entity)
        .copied()
        .unwrap_or(RequestedViewDistance(RequestedViewDistance::DEFAULT))
        .clamped(server_view_distance);
    let center = ((sx.floor() as i32) >> 4, (sz.floor() as i32) >> 4);
    send_to(
        world,
        entity,
        packets::game_event(packets::GAME_EVENT_LEVEL_CHUNKS_LOAD_START, 0.0),
    );
    // Ordering-critical on the respawn re-stream, same as `stream_chunks`: a
    // dropped SetChunkCacheCenter here strands the whole re-sent spawn region.
    if let Some(conn) = world.get::<Conn>(entity) {
        conn.send_reliable(packets::set_chunk_center(center.0, center.1));
    }
    // Collect the new spawn region nearest-first — it is *queued* through the
    // player's chunk sender (like the join path), not blasted, so the respawn re-
    // stream is paced by the same batch/ack throttle. The `Respawn` packet made the
    // client drop its level, so every column is re-sent.
    let mut ordered: Vec<(i32, i32)> = Vec::new();
    for cx in (center.0 - radius - 1)..=(center.0 + radius + 1) {
        for cz in (center.1 - radius - 1)..=(center.1 + radius + 1) {
            if super::chunking::in_view(center, cx, cz, radius) {
                ordered.push((cx, cz));
            }
        }
    }
    ordered.sort_by_key(|&(cx, cz)| {
        let dx = (cx - center.0) as i64;
        let dz = (cz - center.1) as i64;
        dx * dx + dz * dz
    });
    let loaded: std::collections::HashSet<(i32, i32)> = ordered.iter().copied().collect();
    // Reset the chunk sender to a fresh `PlayerChunkSender` (vanilla creates a new
    // `ServerPlayer`, hence a new sender, on respawn) and re-queue the spawn region.
    if let Some(mut sender) = world.get_mut::<ChunkSender>(entity) {
        *sender = ChunkSender::new();
        for &c in &ordered {
            sender.mark_pending(c);
        }
    }
    // Rebalance chunk references for the swapped view: acquire the new spawn
    // region *first*, then release the old set, so a column present in both never
    // transiently falls to zero (and so never needlessly unloads+regenerates).
    // Columns only the old set held drop to zero here and unload this tick.
    let old: Vec<(i32, i32)> = world
        .get::<LoadedChunks>(entity)
        .map(|lc| lc.loaded.iter().copied().collect())
        .unwrap_or_default();
    let game_time = world.resource::<super::world_tick::WorldTime>().game_time;
    {
        let mut refs = world.resource_mut::<super::chunking::ChunkRefs>();
        for &c in &loaded {
            refs.acquire(c);
        }
        for c in old {
            refs.release(c, game_time);
        }
    }
    if let Some(mut lc) = world.get_mut::<LoadedChunks>(entity) {
        lc.center = center;
        lc.loaded = loaded;
    }
    send_to(world, entity, packets::player_position(RESPAWN_TELEPORT_ID, sx, sy, sz, 0.0, 0.0));

    // Re-sync the (now empty) inventory + held slot, then the reset HUD.
    if let Some(inv) = world.get::<crate::inventory::Inventory>(entity) {
        let slots = inv.slots;
        let selected = inv.selected as i32;
        let state_id = world
            .get_mut::<crate::inventory::Inventory>(entity)
            .unwrap()
            .next_state_id();
        send_to(
            world,
            entity,
            crate::inventory::container_set_content(0, state_id, &slots, None),
        );
        send_to(world, entity, crate::inventory::set_held_slot(selected));
    }
    send_to(world, entity, packets::set_health(MAX_HEALTH, 20, 5.0));

    // Teleport the respawned entity for every other viewer.
    if let Some(eid) = entity_id {
        let sync = packets::entity_position_sync(eid, sx, sy, sz, 0.0, 0.0, false);
        broadcast_others(world, entity, sync);
    }
}

/// Send a `SetHealth` to any player whose HUD state changed since it was last
/// sent (`ServerPlayer.doTick`: resend when health, food, or the
/// saturation-is-zero flag changes). Also seeds the sent-state so the initial
/// send from [`send_initial_health`] isn't duplicated.
fn sync_health(world: &mut World) {
    let updates: Vec<(Entity, f32, i32, f32)> = {
        let mut q = world.query::<(Entity, &mut Health, &FoodData)>();
        q.iter_mut(world)
            .filter_map(|(e, mut h, food)| {
                let saturation_zero = food.saturation_level == 0.0;
                if h.current != h.last_sent_health
                    || h.last_sent_food != food.food_level
                    || saturation_zero != h.last_sent_saturation_zero
                {
                    h.last_sent_health = h.current;
                    h.last_sent_food = food.food_level;
                    h.last_sent_saturation_zero = saturation_zero;
                    Some((e, h.current, food.food_level, food.saturation_level))
                } else {
                    None
                }
            })
            .collect()
    };
    for (entity, health, food, saturation) in updates {
        send_to(world, entity, packets::set_health(health, food, saturation));
    }
}

/// Send the joining player their current HUD state and seed the sent-state so
/// [`sync_health`] won't immediately resend it. Called from the join sequence.
pub fn send_initial_health(world: &mut World, entity: Entity) {
    let Some((health, food, saturation)) = world
        .get::<Health>(entity)
        .map(|h| h.current)
        .zip(world.get::<FoodData>(entity).map(|f| (f.food_level, f.saturation_level)))
        .map(|(h, (fl, sat))| (h, fl, sat))
    else {
        return;
    };
    send_to(world, entity, packets::set_health(health, food, saturation));
    if let Some(mut h) = world.get_mut::<Health>(entity) {
        h.last_sent_health = health;
        h.last_sent_food = food;
        h.last_sent_saturation_zero = saturation == 0.0;
    }
}

/// The respawn teleport id the client echoes back via `AcceptTeleportation`.
/// Distinct from the join teleport (1) purely for log clarity.
const RESPAWN_TELEPORT_ID: i32 = 2;

/// Send a framed packet to a single player's own connection.
fn send_to(world: &World, entity: Entity, bytes: bytes::Bytes) {
    if let Some(conn) = world.get::<Conn>(entity) {
        let _ = conn.outbox.try_send(Outbound::Packet(bytes));
    }
}

/// Fan a framed packet out to every connection (used for death messages).
fn broadcast_all(world: &mut World, bytes: bytes::Bytes) {
    let mut q = world.query::<&Conn>();
    for conn in q.iter(world) {
        let _ = conn.outbox.try_send(Outbound::Packet(bytes.clone()));
    }
}

/// Fan a framed packet out to every connection except `sender`.
fn broadcast_others(world: &mut World, sender: Entity, bytes: bytes::Bytes) {
    let mut q = world.query::<(Entity, &Conn)>();
    for (e, conn) in q.iter(world) {
        if e != sender {
            let _ = conn.outbox.try_send(Outbound::Packet(bytes.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fall_damage_curve_matches_vanilla() {
        // floor(distance + 1e-6 - 3.0): no damage up to a 3-block fall, then 1 per
        // block. A 4-block fall = 1 damage, 10 blocks = 7, 23 blocks = 20 (lethal).
        let dmg = |d: f64| (d + 1.0e-6 - SAFE_FALL_DISTANCE).floor().max(0.0) as i32;
        assert_eq!(dmg(1.0), 0);
        assert_eq!(dmg(3.0), 0);
        assert_eq!(dmg(3.5), 0); // 0.5 over the safe distance still floors to 0
        assert_eq!(dmg(4.0), 1);
        assert_eq!(dmg(10.0), 7);
        assert_eq!(dmg(23.0), 20);
    }

    #[test]
    fn health_clamps_and_heal_needs_life() {
        let mut h = Health::new(MAX_HEALTH);
        h.set_health(25.0);
        assert_eq!(h.current, 20.0); // clamped to max
        h.set_health(-5.0);
        assert_eq!(h.current, 0.0); // clamped to floor
        h.heal(5.0);
        assert_eq!(h.current, 0.0, "a dead entity does not heal");
        h.set_health(4.0);
        h.heal(3.0);
        assert_eq!(h.current, 7.0);
        assert!(h.is_hurt());
    }

    #[test]
    fn exhaustion_drains_saturation_then_food() {
        // exhaustion > 4 drains 4, taking a point of saturation first.
        let mut food = FoodData::default(); // food 20, sat 5.0
        let mut health = Health::new(MAX_HEALTH);
        food.exhaustion_level = 4.5;
        food.tick(&mut health, Difficulty::Normal, true);
        assert!((food.exhaustion_level - 0.5).abs() < 1e-6);
        assert_eq!(food.saturation_level, 4.0); // one saturation point spent
        assert_eq!(food.food_level, 20); // food untouched while saturation remains
    }

    #[test]
    fn food_drains_when_saturation_empty() {
        let mut food = FoodData {
            food_level: 20,
            saturation_level: 0.0,
            exhaustion_level: 5.0,
            tick_timer: 0,
        };
        let mut health = Health::new(MAX_HEALTH);
        food.tick(&mut health, Difficulty::Easy, true);
        assert_eq!(food.food_level, 19); // one food point spent
        assert!((food.exhaustion_level - 1.0).abs() < 1e-6); // 5.0 - 4.0
    }

    #[test]
    fn peaceful_never_drains_food() {
        let mut food = FoodData {
            food_level: 20,
            saturation_level: 0.0,
            exhaustion_level: 5.0,
            tick_timer: 0,
        };
        let mut health = Health::new(MAX_HEALTH);
        food.tick(&mut health, Difficulty::Peaceful, true);
        assert_eq!(food.food_level, 20); // peaceful skips the food-drain branch
    }

    #[test]
    fn fast_regen_heals_from_full_food_and_saturation() {
        // saturation > 0 && food >= 20 && hurt: heal min(sat,6)/6 every 10 ticks.
        let mut food = FoodData {
            food_level: 20,
            saturation_level: 6.0,
            exhaustion_level: 0.0,
            tick_timer: 9,
        };
        let mut health = Health::new(MAX_HEALTH);
        health.set_health(10.0);
        food.tick(&mut health, Difficulty::Normal, true);
        assert_eq!(food.tick_timer, 0);
        assert_eq!(health.current, 11.0); // min(6,6)/6 = 1.0 HP
        assert!((food.exhaustion_level - 6.0).abs() < 1e-6); // addExhaustion(6)
    }

    #[test]
    fn slow_regen_heals_when_food_high_but_not_full() {
        // food in [18,20) && hurt: heal 1 every 80 ticks (fast path needs food>=20).
        let mut food = FoodData {
            food_level: 18,
            saturation_level: 0.0,
            exhaustion_level: 0.0,
            tick_timer: 79,
        };
        let mut health = Health::new(MAX_HEALTH);
        health.set_health(10.0);
        food.tick(&mut health, Difficulty::Normal, true);
        assert_eq!(health.current, 11.0);
        assert!((food.exhaustion_level - 6.0).abs() < 1e-6);
    }

    #[test]
    fn regen_disabled_when_natural_regen_off() {
        let mut food = FoodData {
            food_level: 20,
            saturation_level: 6.0,
            exhaustion_level: 0.0,
            tick_timer: 9,
        };
        let mut health = Health::new(MAX_HEALTH);
        health.set_health(10.0);
        let starve = food.tick(&mut health, Difficulty::Normal, false);
        assert_eq!(health.current, 10.0); // no heal
        assert!(!starve);
    }

    /// A minimal world with one player carrying the survival + connection
    /// components, plus the `GameRules` resource the death path reads. Returns the
    /// entity and the outbox receiver so a test can inspect what was sent.
    fn one_player(health: f32) -> (World, Entity, tokio::sync::mpsc::Receiver<Outbound>) {
        use super::super::world_tick::GameRules;
        let mut world = World::new();
        world.insert_resource(GameRules {
            keep_inventory: true, // skip the inventory-drop path in these unit tests
            ..GameRules::default()
        });
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let entity = world
            .spawn((
                Profile { name: "tester".into(), entity_id: 7 },
                Conn::new(tx),
                Health::new(health),
                FoodData::default(),
            ))
            .id();
        (world, entity, rx)
    }

    #[test]
    fn lethal_hit_marks_dead_and_sends_death_screen() {
        let (mut world, e, mut rx) = one_player(4.0);
        // 10 damage onto 4 HP kills; death screen + broadcast message follow.
        let landed = hurt(&mut world, e, DamageKind::Fall, 10.0, Difficulty::Normal, true);
        assert!(landed);
        let h = world.get::<Health>(e).unwrap();
        assert!(h.dead);
        assert_eq!(h.current, 0.0);
        let mut sent = 0;
        while rx.try_recv().is_ok() {
            sent += 1;
        }
        assert!(sent >= 2, "expected a death-screen packet and a death message");
    }

    #[test]
    fn i_frames_absorb_equal_repeat_hits() {
        let (mut world, e, _rx) = one_player(20.0);
        // First hit lands fully (20 -> 17) and opens the 20-tick i-frame window.
        assert!(hurt(&mut world, e, DamageKind::Generic, 3.0, Difficulty::Normal, true));
        assert_eq!(world.get::<Health>(e).unwrap().current, 17.0);
        // A same-size hit inside the window (invulnerable_time 20 > 10) is absorbed.
        assert!(!hurt(&mut world, e, DamageKind::Generic, 3.0, Difficulty::Normal, true));
        assert_eq!(world.get::<Health>(e).unwrap().current, 17.0);
        // A bigger hit applies only the amount above the last (5 - 3 = 2 -> 15).
        assert!(hurt(&mut world, e, DamageKind::Generic, 5.0, Difficulty::Normal, true));
        assert_eq!(world.get::<Health>(e).unwrap().current, 15.0);
    }

    #[test]
    fn spectator_is_invulnerable_creative_only_to_void() {
        let (mut world, e, _rx) = one_player(20.0);
        world.entity_mut(e).insert(GameMode::Spectator);
        assert!(!hurt(&mut world, e, DamageKind::Void, 4.0, Difficulty::Normal, true));
        assert_eq!(world.get::<Health>(e).unwrap().current, 20.0);

        world.entity_mut(e).insert(GameMode::Creative);
        // Creative shrugs off fall damage...
        assert!(!hurt(&mut world, e, DamageKind::Fall, 5.0, Difficulty::Normal, true));
        assert_eq!(world.get::<Health>(e).unwrap().current, 20.0);
        // ...but still dies in the void.
        assert!(hurt(&mut world, e, DamageKind::Void, 4.0, Difficulty::Normal, true));
        assert_eq!(world.get::<Health>(e).unwrap().current, 16.0);
    }

    #[test]
    fn starvation_respects_difficulty_floor() {
        let starve_at = |health: f32, diff: Difficulty| {
            let mut food = FoodData {
                food_level: 0,
                saturation_level: 0.0,
                exhaustion_level: 0.0,
                tick_timer: 79,
            };
            let mut h = Health::new(MAX_HEALTH);
            h.set_health(health);
            food.tick(&mut h, diff, true)
        };
        // EASY floors at 10 HP.
        assert!(starve_at(11.0, Difficulty::Easy));
        assert!(!starve_at(10.0, Difficulty::Easy));
        // NORMAL floors at 1 HP.
        assert!(starve_at(2.0, Difficulty::Normal));
        assert!(!starve_at(1.0, Difficulty::Normal));
        // HARD kills outright.
        assert!(starve_at(1.0, Difficulty::Hard));
    }
}
