//! Live end-to-end natural-spawning test: a real server process, a real (fake)
//! client connection, and the 400-tick CREATURE cadence — asserting a passive
//! mob's `AddEntity` actually reaches the client. This is the wire-level
//! counterpart to the in-process spawner unit tests: it exercises the full
//! production path (Tick clock → `mob_spawn` → pack placement → tracking →
//! pairing packets through the connection).

mod common;
use common::*;

use std::io::Write as _;
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// Start a server with a view distance large enough for a meaningful CREATURE
/// mobcap: the global cap is `10 * loadedChunks / 289`, so the harness default
/// of view-distance 2 (5×5 = 25 columns) caps at **0** and can never spawn.
/// Radius 6 → 13×13 = 169 columns (the global cap is `10 * loadedColumns / 289`, so
/// this comfortably clears the view-distance-2 default's cap of 0). Radius 6 is the
/// smallest view whose generate+light burst still finishes in ~2 s *and* is large
/// enough that the whole spawn area is inside the client's tracking range — smaller
/// views leave an outer ring of spawnable columns the client can't see, where the
/// never-despawning CREATURE cap fills with invisible mobs and the test deadlocks.
///
/// `VELA_SPAWN_INTERVAL_TICKS=10` is the lever that cuts the wall time from ~40 s to
/// a couple of seconds: it fires the persistent CREATURE cadence every 10 ticks
/// (0.5 s at the normal 20 TPS) instead of vanilla's 400 (20 s), so a spawn attempt
/// lands twice a second and the RNG-driven spawn reliably succeeds within a couple of
/// passes of the terrain streaming — without changing *what* the spawner does. The
/// tick rate is deliberately left at the vanilla 50 ms: speeding the whole sim up
/// floods this synchronous fake client's bounded outbox (the server then drops it as
/// "too far behind"), whereas a shorter *spawn* cadence adds no extra packet volume.
fn start_server_fast(addr: &str) -> Server {
    start_server_configured(
        addr,
        "network-compression-threshold=-1\nonline-mode=false\nview-distance=6\nsimulation-distance=6\n",
        &[("VELA_SPAWN_INTERVAL_TICKS", "10")],
    )
}

/// A pig/cow/sheep/chicken must naturally spawn near a connected player and be
/// paired to them (`AddEntity` with a non-player, non-item type). With the
/// shortened spawn cadence (see `start_server_fast`) a pass fires twice a second,
/// so the pairing normally arrives within a couple of seconds; the loop breaks the
/// moment it does. The deadline is only a generous backstop against a pathologically
/// slow chunk-stream, not the expected runtime.
#[test]
fn passive_mob_naturally_spawns_and_pairs_to_client() {
    let addr = "127.0.0.1:25611";
    let _server = start_server_fast(addr);

    let mut c = TcpStream::connect(addr).expect("connect");
    // View distance 6 matches the server's, so the whole spawn area (the player's
    // loaded/spawnable columns) is inside the client's tracking range. If the client
    // view were smaller than the spawn radius, mobs could fill the never-despawning
    // CREATURE cap in columns the client can't see and the test would deadlock.
    drive_into_play_with_view_distance(&mut c, addr, "Watcher", [7u8; 16], 6);
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

    const PLAYER_TYPE: i32 = 156;
    const ITEM_TYPE: i32 = 71;
    const XP_ORB_TYPE: i32 = 49;

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut mob_type = None;
    while Instant::now() < deadline {
        let Some((id, body)) = recv_opt(&mut c) else {
            continue; // read timeout — keep waiting out the spawn cadence
        };
        match id {
            // ChunkBatchFinished: acknowledge with a generous sustainable rate so
            // the chunk pacer keeps streaming (a real client always does this;
            // without the ack the pacer stalls and every column stays "pending",
            // which blocks entity visibility).
            11 => {
                let mut ack = Vec::new();
                ack.extend_from_slice(&64.0f32.to_be_bytes());
                send(&mut c, 11, &ack); // ServerboundChunkBatchReceived
            }
            // KeepAlive: echo it back so the 200-tick liveness check passes.
            44 => {
                let mut p = 0usize;
                let mut ka = [0u8; 8];
                ka.copy_from_slice(&body[p..p + 8]);
                p += 8;
                let _ = p;
                let mut echo = Vec::new();
                echo.extend_from_slice(&ka);
                send(&mut c, 28, &echo);
                // Flush eagerly; `send` already writes the frame.
                let _ = c.flush();
            }
            // AddEntity: VarInt id, uuid (16), VarInt type, ...
            1 => {
                let mut p = 0usize;
                let _eid = read_varint_slice(&body, &mut p);
                p += 16;
                let ty = read_varint_slice(&body, &mut p);
                if ty != PLAYER_TYPE && ty != ITEM_TYPE && ty != XP_ORB_TYPE {
                    mob_type = Some(ty);
                    break;
                }
            }
            _ => {}
        }
    }

    assert!(
        mob_type.is_some(),
        "no naturally-spawned mob was paired to the client within 30 s \
         (many spawn passes at view-distance 6, shortened spawn cadence)"
    );
}
