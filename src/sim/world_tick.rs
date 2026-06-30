//! World clock: day/night time, weather, and game rules.
//!
//! This is the server-side "level tick" for global world state, mirroring the
//! relevant slices of `net.minecraft.server.level.ServerLevel`,
//! `net.minecraft.world.clock.ServerClockManager`, and
//! `net.minecraft.world.level.gamerules.GameRules` (MC 26.2). It is deliberately
//! self-contained in its own module so the parallel edits to the shared `sim`
//! files stay minimal: only the schedule wiring (in `mod.rs`), the resource
//! inserts (in `mod.rs`), and the join-time sync (in `systems.rs`) touch shared
//! code.
//!
//! ## Time model (reworked in 26.2)
//!
//! Two independent counters:
//!   * `game_time` — the world age. Always advances one tick per server tick
//!     (vanilla `ServerLevel.tickTime` / `levelData.getGameTime() + 1`).
//!   * the overworld **clock** (`day_time`) — the time of day. Advances only when
//!     the `advance_time` game rule (formerly `doDaylightCycle`) is true, driven
//!     by `ServerClockManager.ClockInstance.tick`: `partial += rate;
//!     full = floor(partial); partial -= full; total += full`. With the default
//!     `rate == 1.0` this is a plain `+1` per tick. `day_time` is *monotonic* on
//!     the wire (the client takes `day_time % 24000` for the celestial angle);
//!     [`WorldTime::time_of_day`] exposes that wrapped value.
//!
//! The packet (`ClientboundSetTimePacket`) carries `game_time` plus a map of
//! per-clock `ClockNetworkState(totalTicks, partialTick, rate)`. A paused clock
//! reports `rate == 0.0` — that is how 26.2 signals frozen daylight (older
//! clients negated `dayTime` instead). See [`crate::sim::packets::set_time`].

use bevy_ecs::prelude::*;
use bytes::Bytes;

use super::bridge::Outbound;
use super::components::Conn;
use super::packets::{self, ClockUpdate};

/// Length of a Minecraft day in ticks; the time of day is `day_time % DAY_LENGTH`.
/// Used by [`WorldTime::time_of_day`] (and the tests); not referenced by the live
/// broadcast path, which sends the monotonic `day_time` and lets the client wrap.
#[allow(dead_code)]
pub const DAY_LENGTH: i64 = 24_000;

/// Default starting time of day (1000 ticks ≈ just after sunrise) so a fresh
/// world spawns in daylight rather than at midnight.
const DEFAULT_DAY_TIME: i64 = 1_000;

/// How often the gameTime-only resync is broadcast, matching vanilla's
/// `MinecraftServer.tickChildren` `tickCount % 20 == 0` (`forceGameTimeSynchronization`).
const TIME_SYNC_INTERVAL: i64 = 20;

/// World game rules. Models the subset the world tick reads plus a few common
/// ones, with the vanilla 26.2 defaults from `GameRules.java`. Names follow the
/// 26.2 ids (e.g. `advance_time` was `doDaylightCycle`, `advance_weather` was
/// `doWeatherCycle`). Add fields here as more rules are needed — the struct is a
/// plain record so a future `/gamerule` command and `ClientboundSetGameRule`
/// sync can read and mutate it directly.
///
/// `keep_inventory`, `spawn_monsters`, and `random_tick_speed` are not consumed
/// yet (no death/spawn/random-tick systems exist) but are modelled now so the
/// rule set — and its vanilla defaults — is in one place for those systems and a
/// `/gamerule` command to read.
#[derive(Resource, Clone, Copy)]
#[allow(dead_code)]
pub struct GameRules {
    /// `advance_time` (was `doDaylightCycle`) — the day/night clock advances.
    pub advance_time: bool,
    /// `advance_weather` (was `doWeatherCycle`) — weather timers tick.
    pub advance_weather: bool,
    /// `keep_inventory` — keep items on death.
    pub keep_inventory: bool,
    /// `spawn_monsters` — hostile mob spawning (was part of `doMobSpawning`).
    pub spawn_monsters: bool,
    /// `random_tick_speed` — random ticks per chunk section per tick.
    pub random_tick_speed: i32,
}

impl Default for GameRules {
    fn default() -> Self {
        // Vanilla 26.2 defaults (GameRules.java).
        Self {
            advance_time: true,
            advance_weather: true,
            keep_inventory: false,
            spawn_monsters: true,
            random_tick_speed: 3,
        }
    }
}

/// The world clock. `game_time` is the always-advancing world age; the remaining
/// fields are the overworld clock's `ClockNetworkState` (`day_time` == totalTicks).
#[derive(Resource)]
pub struct WorldTime {
    pub game_time: i64,
    pub day_time: i64,
    pub partial_tick: f32,
    pub rate: f32,
    /// The `advance_time` value last reflected in a clock sync sent to clients.
    /// A change forces a full clock resync (vanilla broadcasts the full sync when
    /// the `ADVANCE_TIME` rule changes, so the client learns the new rate).
    synced_advance_time: bool,
}

impl Default for WorldTime {
    fn default() -> Self {
        Self {
            game_time: 0,
            day_time: DEFAULT_DAY_TIME,
            partial_tick: 0.0,
            rate: 1.0,
            synced_advance_time: true,
        }
    }
}

impl WorldTime {
    /// Advance the day clock one tick. Mirrors `ServerClockManager.ClockInstance.tick`:
    /// accumulate `rate` into `partial_tick`, move whole ticks into `day_time`.
    pub fn step_clock(&mut self) {
        self.partial_tick += self.rate;
        let full = self.partial_tick.floor() as i64; // Mth.floor
        self.partial_tick -= full as f32;
        self.day_time += full;
    }

    /// The time of day in `0..DAY_LENGTH`, the value the client derives from the
    /// monotonic `day_time` for the celestial angle. Exposed for server-side
    /// time-of-day checks (e.g. sleep, mob spawning) and covered by tests; the
    /// wire path sends the raw `day_time`.
    #[allow(dead_code)]
    pub fn time_of_day(&self) -> i64 {
        self.day_time.rem_euclid(DAY_LENGTH)
    }

    /// The overworld [`ClockUpdate`] for a full clock sync. A paused clock
    /// (`advance_time == false`) reports `rate == 0.0`, matching
    /// `ServerClockManager.ClockInstance.packNetworkState`.
    pub fn clock_update(&self, advance_time: bool) -> ClockUpdate {
        ClockUpdate {
            clock_id: packets::WORLD_CLOCK_OVERWORLD,
            total_ticks: self.day_time,
            partial_tick: self.partial_tick,
            rate: if advance_time { self.rate } else { 0.0 },
        }
    }
}

/// Weather state, mirroring `ServerLevel`'s rain/thunder fields. `raining` /
/// `thundering` are the discrete targets; `rain_level` / `thunder_level` ramp
/// toward them by 0.01 per tick (vanilla `advanceWeatherCycle`). The `o*`
/// shadows are last tick's levels, used to detect a change worth broadcasting.
#[derive(Resource, Default)]
pub struct Weather {
    pub raining: bool,
    pub thundering: bool,
    pub rain_level: f32,
    pub thunder_level: f32,
    o_rain_level: f32,
    o_thunder_level: f32,
}

impl Weather {
    /// Step the weather one tick and return the `GameEvent` packets that should
    /// be broadcast for any change, mirroring the broadcasting tail of
    /// `ServerLevel.advanceWeatherCycle`.
    ///
    /// NOTE: the *random* weather cycle (the `RAIN_DELAY`/`RAIN_DURATION` timers
    /// that flip `raining`/`thundering` on their own) is **deferred** — the repo
    /// has no RNG dependency yet and no `/weather` command to drive it, so weather
    /// stays clear until something toggles `raining`/`thundering`. The level
    /// interpolation and the GameEvent broadcasts below are the vanilla-faithful
    /// part and fire correctly the moment the state is toggled. `advance_weather`
    /// is threaded through for when the timer cycle lands.
    pub fn advance(&mut self, _advance_weather: bool) -> Vec<Bytes> {
        let was_raining = self.is_raining();

        self.o_thunder_level = self.thunder_level;
        self.thunder_level += if self.thundering { 0.01 } else { -0.01 };
        self.thunder_level = self.thunder_level.clamp(0.0, 1.0);

        self.o_rain_level = self.rain_level;
        self.rain_level += if self.raining { 0.01 } else { -0.01 };
        self.rain_level = self.rain_level.clamp(0.0, 1.0);

        let mut events = Vec::new();
        if self.o_rain_level != self.rain_level {
            events.push(packets::game_event(
                packets::GAME_EVENT_RAIN_LEVEL_CHANGE,
                self.rain_level,
            ));
        }
        if self.o_thunder_level != self.thunder_level {
            events.push(packets::game_event(
                packets::GAME_EVENT_THUNDER_LEVEL_CHANGE,
                self.thunder_level,
            ));
        }
        if was_raining != self.is_raining() {
            let start_stop = if was_raining {
                packets::GAME_EVENT_STOP_RAINING
            } else {
                packets::GAME_EVENT_START_RAINING
            };
            events.push(packets::game_event(start_stop, 0.0));
            events.push(packets::game_event(
                packets::GAME_EVENT_RAIN_LEVEL_CHANGE,
                self.rain_level,
            ));
            events.push(packets::game_event(
                packets::GAME_EVENT_THUNDER_LEVEL_CHANGE,
                self.thunder_level,
            ));
        }
        events
    }

    /// Whether it is currently raining (rain level non-zero), matching
    /// `Level.isRaining` (`getRainLevel(1.0) > 0.2` for gameplay, but the
    /// transition broadcast keys off the discrete `raining` flag via the level).
    pub fn is_raining(&self) -> bool {
        self.rain_level > 0.0
    }
}

/// The world tick: advance the clocks, step the weather, and broadcast time and
/// weather updates to every connected player. Runs as an ordinary system reading
/// the world-state resources and the connection query.
pub fn world_tick(
    mut time: ResMut<WorldTime>,
    rules: Res<GameRules>,
    mut weather: ResMut<Weather>,
    conns: Query<&Conn>,
) {
    // 1. World age always advances (ServerLevel.tickTime).
    time.game_time = time.game_time.wrapping_add(1);

    // 2. The day clock advances only under advance_time (ServerClockManager.tick).
    if rules.advance_time {
        time.step_clock();
    }

    // 3. Weather: interpolate levels and collect any GameEvent transitions.
    let mut out = weather.advance(rules.advance_weather);

    // 4. Time sync. A change to advance_time (the clock rate) forces a full clock
    //    resync; otherwise the periodic 1 s sync carries gameTime with an empty
    //    clock map (vanilla forceGameTimeSynchronization).
    if rules.advance_time != time.synced_advance_time {
        time.synced_advance_time = rules.advance_time;
        let update = time.clock_update(rules.advance_time);
        out.push(packets::set_time(time.game_time, &[update]));
    } else if time.game_time % TIME_SYNC_INTERVAL == 0 {
        out.push(packets::set_time(time.game_time, &[]));
    }

    if out.is_empty() {
        return;
    }
    for conn in &conns {
        for pkt in &out {
            let _ = conn.outbox.try_send(Outbound::Packet(pkt.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_rules_match_vanilla_defaults() {
        let r = GameRules::default();
        assert!(r.advance_time); // advance_time (doDaylightCycle)
        assert!(r.advance_weather); // advance_weather (doWeatherCycle)
        assert!(!r.keep_inventory); // keep_inventory
        assert!(r.spawn_monsters); // spawn_monsters
        assert_eq!(r.random_tick_speed, 3); // random_tick_speed
    }

    #[test]
    fn day_time_advances_and_wraps_at_24000() {
        let mut t = WorldTime {
            day_time: DAY_LENGTH - 2,
            partial_tick: 0.0,
            rate: 1.0,
            ..Default::default()
        };
        assert_eq!(t.time_of_day(), DAY_LENGTH - 2);
        t.step_clock(); // 23999
        t.step_clock(); // 24000 -> wraps to 0
        assert_eq!(t.day_time, DAY_LENGTH); // monotonic on the wire
        assert_eq!(t.time_of_day(), 0); // wrapped time of day
        t.step_clock();
        assert_eq!(t.time_of_day(), 1);
    }

    #[test]
    fn day_time_freezes_when_advance_time_false() {
        // The world_tick gate: step_clock is only called when advance_time. Model
        // that here — a frozen clock keeps its day_time across ticks.
        let mut t = WorldTime {
            day_time: 6_000,
            ..Default::default()
        };
        let advance_time = false;
        for _ in 0..100 {
            if advance_time {
                t.step_clock();
            }
        }
        assert_eq!(t.day_time, 6_000);
        // A frozen clock reports rate 0.0 (26.2 frozen-daylight signal).
        assert_eq!(t.clock_update(advance_time).rate, 0.0);
    }

    #[test]
    fn fractional_rate_accumulates_whole_ticks() {
        let mut t = WorldTime {
            day_time: 0,
            partial_tick: 0.0,
            rate: 0.5,
            ..Default::default()
        };
        t.step_clock(); // partial 0.5, no whole tick
        assert_eq!(t.day_time, 0);
        t.step_clock(); // partial 1.0 -> 1 whole tick
        assert_eq!(t.day_time, 1);
    }

    #[test]
    fn weather_clear_emits_no_events() {
        let mut w = Weather::default();
        assert!(w.advance(true).is_empty());
    }

    #[test]
    fn weather_start_rain_broadcasts_events() {
        let mut w = Weather {
            raining: true,
            ..Default::default()
        };
        // First tick lifts rain_level off 0.0 -> START_RAINING + level changes.
        let events = w.advance(true);
        assert!(!events.is_empty());
        assert!(w.is_raining());
    }
}
