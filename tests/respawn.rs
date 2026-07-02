//! End-to-end respawn tests: a player who dies far from spawn must respawn onto
//! solid ground *fast*, without the server pre-loading (pinning) the spawn region
//! between deaths, and must not free-fall into the void while the terrain streams.
//!
//! Both behaviours come from the same fix pair:
//!   * `respawn_player` warms the 3×3 columns under the respawn point synchronously,
//!     so they ship in the very first post-respawn chunk batch even when the spawn
//!     region was evicted while the player was away (no preloading required);
//!   * the `ClientLoaded` gate keeps the player invulnerable to the void until the
//!     client confirms it has loaded that terrain (`ServerboundPlayerLoaded`), so a
//!     slow stream can never drop them through not-yet-received ground.

use std::net::TcpStream;
use std::time::{Duration, Instant};

mod common;
use common::*;

/// `Attributes.MAX_HEALTH` for a player — the full-health value a `SetHealth` carries.
const MAX_HEALTH: f32 = 20.0;

// Clientbound Play packet ids used here (see `sim::packets`).
const CB_CHUNK_BATCH_FINISHED: i32 = 11;
const CB_CHUNK_BATCH_START: i32 = 12;
const CB_LEVEL_CHUNK: i32 = 45;
const CB_PLAYER_COMBAT_KILL: i32 = 68; // death screen — the "you died" signal
const CB_PLAYER_POSITION: i32 = 72;
const CB_RESPAWN: i32 = 82;
const CB_SET_HEALTH: i32 = 104;

// Serverbound Play packet ids.
const SB_MOVE_PLAYER_POS: i32 = 30;
const SB_CLIENT_COMMAND: i32 = 12; // 0 = PERFORM_RESPAWN
const SB_PLAYER_LOADED: i32 = 44;

/// Send a `ServerboundMovePlayerPos` (id 30): x/y/z doubles then a flags byte
/// (bit 0 = on-ground). We always report airborne so the huge dive below doesn't
/// trip the landing-fall-damage path — the void floor is what should kill.
fn send_move(s: &mut TcpStream, x: f64, y: f64, z: f64) {
    let mut body = Vec::new();
    body.extend_from_slice(&x.to_be_bytes());
    body.extend_from_slice(&y.to_be_bytes());
    body.extend_from_slice(&z.to_be_bytes());
    body.push(0); // flags: not on ground
    send(s, SB_MOVE_PLAYER_POS, &body);
}

fn read_f64(b: &[u8], pos: &mut usize) -> f64 {
    let v = f64::from_be_bytes(b[*pos..*pos + 8].try_into().unwrap());
    *pos += 8;
    v
}

fn read_f32(b: &[u8], pos: &mut usize) -> f32 {
    let v = f32::from_be_bytes(b[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    v
}

fn chunk_coord(body: &[u8]) -> (i32, i32) {
    let cx = i32::from_be_bytes(body[0..4].try_into().unwrap());
    let cz = i32::from_be_bytes(body[4..8].try_into().unwrap());
    (cx, cz)
}

/// A player who dies far from spawn respawns onto ground delivered in the first
/// chunk batch — proving the respawn point's own columns are generated on demand
/// and shipped immediately, without the server keeping the spawn region loaded.
#[test]
fn respawn_far_away_delivers_ground_chunk_in_first_batch() {
    let addr = "127.0.0.1:25620";
    // Parity worldgen: cold chunks cost ~40 ms each, so a respawn region that was
    // evicted while the player roamed regenerates slowly. That's the scenario where
    // a readiness-gated stream would leave the client on "Loading terrain" — and
    // where the synchronous warm under the respawn point has to earn its keep.
    let _server = start_server_parity(addr);
    let mut s = TcpStream::connect(addr).expect("connect");
    drive_into_play(&mut s, addr, "faller", [7u8; 16]);
    // Announce we've loaded the join terrain, exactly as a real client does, so the
    // void damage below isn't suppressed by the join-time invulnerability window.
    send(&mut s, SB_PLAYER_LOADED, &[]);

    // Teleport far from spawn *and* into the void in one move: the jump to chunk
    // (100, 100) makes the streamer evict the spawn columns (this player is their
    // only viewer), and y = -2000 is well below the void floor, so the player dies
    // there — with the spawn region now cold, exactly the reported scenario.
    send_move(&mut s, 1600.0, -2000.0, 1600.0);

    // Drain (auto-acking chunk batches) until the death screen arrives.
    let died = drain_until(&mut s, |id, _| id == CB_PLAYER_COMBAT_KILL);
    assert!(died, "player should die in the void far from spawn");

    // Click "Respawn" (ClientCommand action 0 = PERFORM_RESPAWN).
    let respawn_sent = Instant::now();
    send(&mut s, SB_CLIENT_COMMAND, &[0]);

    // After the Respawn packet, capture the respawn coordinates (the teleport that
    // follows) and the first chunk batch, then assert the column the player lands
    // in is delivered inside that first batch.
    let mut saw_respawn = false;
    let mut spawn_chunk: Option<(i32, i32)> = None;
    let mut in_batch = false;
    let mut first_batch: Vec<(i32, i32)> = Vec::new();
    let mut batch_closed = false;

    let ok = drain_until(&mut s, |id, body| {
        if id == CB_RESPAWN {
            saw_respawn = true;
        } else if saw_respawn && id == CB_PLAYER_POSITION && spawn_chunk.is_none() {
            // VarInt teleport id, then position doubles (x, y, z).
            let mut pos = 0;
            let _teleport_id = read_varint_slice(body, &mut pos);
            let x = read_f64(body, &mut pos);
            let _y = read_f64(body, &mut pos);
            let z = read_f64(body, &mut pos);
            spawn_chunk = Some(((x.floor() as i32) >> 4, (z.floor() as i32) >> 4));
        } else if saw_respawn && !batch_closed {
            match id {
                CB_CHUNK_BATCH_START => in_batch = true,
                CB_LEVEL_CHUNK if in_batch => first_batch.push(chunk_coord(body)),
                CB_CHUNK_BATCH_FINISHED if in_batch => batch_closed = true,
                _ => {}
            }
        }
        // Stop once we know the respawn column and have the full first batch.
        batch_closed && spawn_chunk.is_some()
    });
    assert!(ok, "did not observe the respawn chunk batch");
    let elapsed = respawn_sent.elapsed();

    let spawn_chunk = spawn_chunk.expect("respawn teleport carried a position");
    assert!(
        first_batch.contains(&spawn_chunk),
        "the column under the respawned player {spawn_chunk:?} must arrive in the \
         first post-respawn batch (got {first_batch:?}) — otherwise the client sits \
         on 'Loading terrain' and falls into the void",
    );
    // The reported symptom was a *long* "Loading terrain" (up to the vanilla client's
    // 30 s load timeout) before the fall. The respawn point's terrain is generated on
    // demand — not preloaded — yet must arrive promptly. A generous bound well under
    // 30 s still fails hard if the re-stream stalls behind cold generation.
    assert!(
        elapsed < Duration::from_secs(8),
        "respawn terrain took {elapsed:?} to reach the client — far too close to the \
         client's 30 s load timeout (which drops the player into the void)",
    );
}

/// While the client hasn't confirmed it loaded the level around the player, the
/// void must not hurt them (`ServerPlayer.isInvulnerableTo` while `!hasClientLoaded`).
/// A player that dives into the void *without* sending `PlayerLoaded` takes no void
/// damage at all — the invulnerability window that stops a slow respawn stream from
/// turning into a void death.
///
/// Keyed on the first health *drop* (a `SetHealth` below full), not on death: a
/// vulnerable player is hit within a tick or two of the dive, so a leaked gate shows
/// up almost immediately — comfortably before the 60-tick (3 s) backstop opens, which
/// keeps the assertion robust even when a sibling test is loading the CPU.
#[test]
fn void_damage_suppressed_until_client_confirms_load() {
    let addr = "127.0.0.1:25621";
    let _server = start_server(addr);
    let mut s = TcpStream::connect(addr).expect("connect");
    drive_into_play(&mut s, addr, "waiter", [9u8; 16]);
    // Deliberately do NOT send ServerboundPlayerLoaded: the load gate stays shut, so
    // the player must remain invulnerable to the void.

    // Drain the join backlog, including the initial full-health SetHealth, so the
    // watch below only sees state changes caused by the dive.
    drain_until(&mut s, |id, body| id == CB_SET_HEALTH && read_f32(body, &mut 0) >= MAX_HEALTH);

    // Dive into the void.
    send_move(&mut s, 8.0, -2000.0, 8.0);

    // Watch for ~2 s (well under the 3 s backstop). While invulnerable the server
    // sends no SetHealth — health never changes — so any SetHealth reporting less
    // than full health means the gate leaked and the void bit through.
    s.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
    let deadline = Instant::now() + Duration::from_millis(2000);
    while Instant::now() < deadline {
        match recv_opt(&mut s) {
            Some((CB_SET_HEALTH, body)) => {
                let health = read_f32(&body, &mut 0);
                assert!(
                    health >= MAX_HEALTH,
                    "health dropped to {health} in the void while the client-load gate \
                     was shut — the player should have been invulnerable",
                );
            }
            Some((CB_PLAYER_COMBAT_KILL, _)) => {
                panic!("player took a void death while the client-load gate was shut");
            }
            Some((CB_CHUNK_BATCH_FINISHED, _)) => send(&mut s, 11, &64.0f32.to_be_bytes()),
            Some(_) => {}
            None => {} // idle read timeout — no packets is exactly what we expect
        }
    }
}
