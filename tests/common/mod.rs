//! Shared byte-level fake-client harness for the end-to-end integration tests.
//!
//! Each file under `tests/` is its own crate, so this module is compiled into
//! every integration binary that declares `mod common;`. It drives a raw TCP
//! client through the real server binary using only the plain
//! `VarInt(len)|id|body` framing — validating framing, state transitions, and
//! packet ids (not that a *real* 26.2 client renders the world).
//!
//! Not every test uses every helper, and each crate compiles the whole module,
//! so unused-code warnings are expected and suppressed.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Spawns the built server bound to a test port; killed on drop.
pub struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn start_server(addr: &str) -> Server {
    // A small view/simulation distance keeps the per-join chunk burst tiny —
    // see the comment in `start_server_with_properties`.
    start_server_with_properties(
        addr,
        "network-compression-threshold=-1\nonline-mode=false\nview-distance=2\nsimulation-distance=2\n",
    )
}

/// Like [`start_server`] but with caller-supplied `server.properties` contents,
/// for tests that need a non-default config (e.g. the natural-spawner test needs
/// a view distance big enough for a non-zero CREATURE mobcap).
pub fn start_server_with_properties(addr: &str, properties: &str) -> Server {
    // Run each server in its own temp directory so the generated config files
    // (server.properties, ops.json, …) don't litter the checkout, and set the
    // IDE bypass so the EULA gate auto-agrees — mirroring how vanilla's own
    // tests run with `IS_RUNNING_IN_IDE`.
    let workdir = std::env::temp_dir().join(format!("vela-it-{}", addr.replace(':', "_")));
    std::fs::create_dir_all(&workdir).expect("create server workdir");
    // This byte-level fake client speaks only the plain `VarInt(len)|id|body`
    // framing, so disable the mid-Login compression negotiation (default 256) for
    // it: `-1` is vanilla's "compression off". The compressed framing itself is
    // covered by the round-trip unit tests in `protocol::framing`. It also cannot
    // perform the online-mode RSA/AES key exchange or Mojang `hasJoined` auth, so
    // run in offline mode (`online-mode=false`); the secure-transport math is
    // covered by the unit tests in `net::crypto`.
    // A small view/simulation distance keeps the per-join chunk burst tiny
    // (5x5 = 25 columns instead of the default 21x21 = 441). That slashes the
    // work each connection does at spawn, so the timing-sensitive broadcast
    // assertions stay reliable even when all the integration binaries run their
    // servers in parallel and contend for the CPU.
    std::fs::write(workdir.join("server.properties"), properties)
        .expect("write server.properties");
    // The server is a child process, so its `tracing` output would otherwise
    // inherit our stdout/stderr and interleave with the test harness. Silence it
    // by default; set VELA_TEST_LOGS=1 to pass the server logs through when
    // debugging a failing test.
    let (out, err) = if std::env::var_os("VELA_TEST_LOGS").is_some() {
        (Stdio::inherit(), Stdio::inherit())
    } else {
        (Stdio::null(), Stdio::null())
    };
    let child = Command::new(env!("CARGO_BIN_EXE_vela"))
        .arg(addr)
        .current_dir(&workdir)
        .env("VELA_RUNNING_IN_IDE", "1")
        .stdout(out)
        .stderr(err)
        .spawn()
        .expect("spawn vela");
    // Give it a moment to bind the listener.
    for _ in 0..50 {
        if TcpStream::connect(addr).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Server(child)
}

pub fn write_varint(buf: &mut Vec<u8>, mut v: i32) {
    loop {
        let mut b = (v as u32 & 0x7F) as u8;
        v = ((v as u32) >> 7) as i32;
        if v != 0 {
            b |= 0x80;
        }
        buf.push(b);
        if v == 0 {
            break;
        }
    }
}

pub fn read_varint(s: &mut TcpStream) -> i32 {
    let mut value = 0i32;
    let mut pos = 0;
    loop {
        let mut byte = [0u8; 1];
        s.read_exact(&mut byte).expect("read varint byte");
        value |= ((byte[0] & 0x7F) as i32) << pos;
        if byte[0] & 0x80 == 0 {
            break;
        }
        pos += 7;
    }
    value
}

pub fn write_utf(buf: &mut Vec<u8>, s: &str) {
    write_varint(buf, s.len() as i32);
    buf.extend_from_slice(s.as_bytes());
}

/// Send a framed packet: VarInt(len) | VarInt(id) | body.
pub fn send(s: &mut TcpStream, id: i32, body: &[u8]) {
    let mut idbuf = Vec::new();
    write_varint(&mut idbuf, id);
    let mut frame = Vec::new();
    write_varint(&mut frame, (idbuf.len() + body.len()) as i32);
    frame.extend_from_slice(&idbuf);
    frame.extend_from_slice(body);
    s.write_all(&frame).expect("send packet");
}

/// Read one frame, returning (packet_id, body).
pub fn recv(s: &mut TcpStream) -> (i32, Vec<u8>) {
    let len = read_varint(s) as usize;
    let mut frame = vec![0u8; len];
    s.read_exact(&mut frame).expect("read frame body");
    // Split the id varint off the front of the frame.
    let mut pos = 0usize;
    let mut id = 0i32;
    let mut shift = 0;
    loop {
        let b = frame[pos];
        id |= ((b & 0x7F) as i32) << shift;
        pos += 1;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    (id, frame[pos..].to_vec())
}

/// Like `recv`, but returns `None` on a read error (timeout or closed
/// connection) instead of panicking. Used to assert the server is still alive.
pub fn recv_opt(s: &mut TcpStream) -> Option<(i32, Vec<u8>)> {
    let mut first = [0u8; 1];
    if s.read_exact(&mut first).is_err() {
        return None;
    }
    // Re-read the length VarInt starting from the byte we just consumed.
    let mut len = (first[0] & 0x7F) as i32;
    let mut shift = 7;
    let mut last = first[0];
    while last & 0x80 != 0 {
        let mut byte = [0u8; 1];
        if s.read_exact(&mut byte).is_err() {
            return None;
        }
        len |= ((byte[0] & 0x7F) as i32) << shift;
        shift += 7;
        last = byte[0];
    }
    let mut frame = vec![0u8; len as usize];
    if s.read_exact(&mut frame).is_err() {
        return None;
    }
    let mut pos = 0usize;
    let mut id = 0i32;
    let mut shift = 0;
    loop {
        let b = frame[pos];
        id |= ((b & 0x7F) as i32) << shift;
        pos += 1;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    Some((id, frame[pos..].to_vec()))
}

/// Read a VarInt out of an in-memory body slice, advancing `pos`.
pub fn read_varint_slice(b: &[u8], pos: &mut usize) -> i32 {
    let mut value = 0i32;
    let mut shift = 0;
    loop {
        let byte = b[*pos];
        *pos += 1;
        value |= ((byte & 0x7F) as i32) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    value
}

/// Pack `(x, y, z)` into the vanilla `BlockPos.asLong` layout: 26-bit x at
/// bit 38, 26-bit z at bit 12, 12-bit y in the low bits.
pub fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    ((x as i64 & 0x3FF_FFFF) << 38) | ((z as i64 & 0x3FF_FFFF) << 12) | (y as i64 & 0xFFF)
}

/// Inverse of [`pack_block_pos`] (sign-extending each field), for parsing a
/// clientbound `BlockUpdate` position.
pub fn unpack_block_pos(n: i64) -> (i32, i32, i32) {
    let x = (n >> 38) as i32;
    let y = ((n << 52) >> 52) as i32;
    let z = ((n << 26) >> 38) as i32;
    (x, y, z)
}

/// Read one optional `ItemStack` from a body slice (the wire codec both
/// `ContainerSetContent` and the creative-slot packet use): a VarInt count, and
/// when positive an id VarInt plus the two (always-zero) component-patch VarInts.
/// Returns `(id, count)` or `None` for an empty slot.
pub fn read_item(b: &[u8], pos: &mut usize) -> Option<(i32, i32)> {
    let count = read_varint_slice(b, pos);
    if count <= 0 {
        return None;
    }
    let id = read_varint_slice(b, pos);
    let _added = read_varint_slice(b, pos);
    let _removed = read_varint_slice(b, pos);
    Some((id, count))
}

pub fn handshake(s: &mut TcpStream, addr: &str, intent: i32) {
    let (host, port) = addr.split_once(':').unwrap();
    let mut body = Vec::new();
    write_varint(&mut body, 776); // protocol version
    write_utf(&mut body, host);
    body.extend_from_slice(&port.parse::<u16>().unwrap().to_be_bytes());
    write_varint(&mut body, intent);
    send(s, 0x00, &body);
}

/// Drive a fresh connection all the way from handshake into Play, ending with
/// the spawn teleport acknowledged. Leaves the join-sequence packets (and any
/// later broadcasts) queued in the socket for the caller to drain.
pub fn drive_into_play(s: &mut TcpStream, addr: &str, name: &str, uuid: [u8; 16]) {
    drive_into_play_with_view_distance(s, addr, name, uuid, 0);
}

/// Like [`drive_into_play`], but declares a client view distance during
/// configuration (`ClientInformation`) when `view_distance > 0` — like a real
/// client always does. Without it the server keeps vanilla's not-yet-received
/// default (`ServerPlayer.requestedViewDistance` = 2), which caps entity
/// visibility at 32 blocks and hides anything spawning farther out.
pub fn drive_into_play_with_view_distance(
    s: &mut TcpStream,
    addr: &str,
    name: &str,
    uuid: [u8; 16],
    view_distance: u8,
) {
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    handshake(s, addr, 2); // login

    let mut hello = Vec::new();
    write_utf(&mut hello, name);
    hello.extend_from_slice(&uuid);
    send(s, 0x00, &hello);

    assert_eq!(recv(s).0, 2, "ClientboundLoginFinished");
    send(s, 3, &[]); // ServerboundLoginAcknowledged

    let mut brand = Vec::new();
    write_utf(&mut brand, "minecraft:brand");
    write_utf(&mut brand, "vanilla");
    send(s, 2, &brand);

    // ClientInformation (configuration id 0): language + viewDistance byte; the
    // server reads only these two fields.
    if view_distance > 0 {
        let mut info = Vec::new();
        write_utf(&mut info, "en_us");
        info.push(view_distance);
        send(s, 0, &info);
    }

    while recv(s).0 != 14 {} // read until ClientboundSelectKnownPacks

    let mut packs = Vec::new();
    write_varint(&mut packs, 1);
    write_utf(&mut packs, "minecraft");
    write_utf(&mut packs, "core");
    write_utf(&mut packs, "26.2");
    send(s, 7, &packs);

    while recv(s).0 != 3 {} // read until ClientboundFinishConfiguration
    send(s, 3, &[]); // ServerboundFinishConfiguration -> enter Play

    // Acknowledge the spawn teleport (id 1) so we count as a settled player.
    let mut accept = Vec::new();
    write_varint(&mut accept, 1);
    send(s, 0, &accept);
}

/// Drain frames from `s` (up to a generous bound that clears the join backlog)
/// until `f` returns `true`. Returns whether the condition was met.
pub fn drain_until(s: &mut TcpStream, mut f: impl FnMut(i32, &[u8]) -> bool) -> bool {
    for _ in 0..4000 {
        let Some((id, body)) = recv_opt(s) else {
            return false; // stream closed / timed out
        };
        if f(id, &body) {
            return true;
        }
    }
    false
}
