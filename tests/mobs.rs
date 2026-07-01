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
/// Radius 8 → 17×17 = 289 columns → cap 10.
fn start_server_vd8(addr: &str) -> Server {
    start_server_with_properties(
        addr,
        "network-compression-threshold=-1\nonline-mode=false\nview-distance=8\nsimulation-distance=8\n",
    )
}

/// A pig/cow/sheep/chicken must naturally spawn near a connected player and be
/// paired to them (`AddEntity` with a non-player, non-item type) within a few
/// 400-tick spawn passes (~20 s each at 20 TPS; we allow 3).
#[test]
fn passive_mob_naturally_spawns_and_pairs_to_client() {
    let addr = "127.0.0.1:25611";
    let _server = start_server_vd8(addr);

    let mut c = TcpStream::connect(addr).expect("connect");
    drive_into_play_with_view_distance(&mut c, addr, "Watcher", [7u8; 16], 8);
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

    const PLAYER_TYPE: i32 = 156;
    const ITEM_TYPE: i32 = 71;
    const XP_ORB_TYPE: i32 = 49;

    let deadline = Instant::now() + Duration::from_secs(70);
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
        "no naturally-spawned mob was paired to the client within 70 s \
         (3+ spawn passes at view-distance 8, cap 10)"
    );
    eprintln!("DIAG: paired mob entity type {}", mob_type.unwrap());
}
