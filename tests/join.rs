//! End-to-end framing test: drives a byte-level fake client through the full
//! handshake -> status and login -> configuration -> play pipeline against the
//! real server binary, asserting the expected clientbound packet ids appear.
//!
//! This does not validate that a *real* Minecraft client renders the world
//! (that needs the actual 26.2 client) — it validates our framing, state
//! transitions, and packet ids, which are the parts most likely to regress.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command};
use std::time::Duration;

/// Spawns the built server bound to a test port; killed on drop.
struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_server(addr: &str) -> Server {
    // Run each server in its own temp directory so the generated config files
    // (server.properties, ops.json, …) don't litter the checkout, and set the
    // IDE bypass so the EULA gate auto-agrees — mirroring how vanilla's own
    // tests run with `IS_RUNNING_IN_IDE`.
    let workdir = std::env::temp_dir().join(format!("vela-it-{}", addr.replace(':', "_")));
    std::fs::create_dir_all(&workdir).expect("create server workdir");
    // This byte-level fake client speaks only the plain `VarInt(len)|id|body`
    // framing, so disable the mid-Login compression negotiation (default 256) for
    // it: `-1` is vanilla's "compression off". The compressed framing itself is
    // covered by the round-trip unit tests in `protocol::framing`.
    std::fs::write(
        workdir.join("server.properties"),
        "network-compression-threshold=-1\n",
    )
    .expect("write server.properties");
    let child = Command::new(env!("CARGO_BIN_EXE_vela"))
        .arg(addr)
        .current_dir(&workdir)
        .env("VELA_RUNNING_IN_IDE", "1")
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

fn write_varint(buf: &mut Vec<u8>, mut v: i32) {
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

fn read_varint(s: &mut TcpStream) -> i32 {
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

fn write_utf(buf: &mut Vec<u8>, s: &str) {
    write_varint(buf, s.len() as i32);
    buf.extend_from_slice(s.as_bytes());
}

/// Send a framed packet: VarInt(len) | VarInt(id) | body.
fn send(s: &mut TcpStream, id: i32, body: &[u8]) {
    let mut idbuf = Vec::new();
    write_varint(&mut idbuf, id);
    let mut frame = Vec::new();
    write_varint(&mut frame, (idbuf.len() + body.len()) as i32);
    frame.extend_from_slice(&idbuf);
    frame.extend_from_slice(body);
    s.write_all(&frame).expect("send packet");
}

/// Read one frame, returning (packet_id, body).
fn recv(s: &mut TcpStream) -> (i32, Vec<u8>) {
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
fn recv_opt(s: &mut TcpStream) -> Option<(i32, Vec<u8>)> {
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
fn read_varint_slice(b: &[u8], pos: &mut usize) -> i32 {
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

fn handshake(s: &mut TcpStream, addr: &str, intent: i32) {
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
fn drive_into_play(s: &mut TcpStream, addr: &str, name: &str, uuid: [u8; 16]) {
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

/// Two players must see each other join (player list + entity spawn) and see
/// each other move (relative movement + head rotation broadcasts). Drives two
/// real connections against the server binary.
#[test]
fn two_players_see_each_other() {
    let addr = "127.0.0.1:25593";
    let _server = start_server(addr);

    let mut alice = TcpStream::connect(addr).expect("connect alice");
    drive_into_play(&mut alice, addr, "Alice", [1u8; 16]);

    // Bob joins second; his entity id is 2 (Alice took 1).
    let mut bob = TcpStream::connect(addr).expect("connect bob");
    drive_into_play(&mut bob, addr, "Bob", [2u8; 16]);

    // Bob (the newcomer) must be told about Alice (already online): a
    // PlayerInfoUpdate followed by an AddEntity for Alice's entity id 1.
    let mut bob_saw_alice_info = false;
    for _ in 0..600 {
        let (id, body) = recv(&mut bob);
        if id == 70 {
            bob_saw_alice_info = true;
        }
        if id == 1 {
            let mut pos = 0;
            assert_eq!(read_varint_slice(&body, &mut pos), 1, "Alice's entity id");
            pos += 16; // skip uuid
            assert_eq!(read_varint_slice(&body, &mut pos), 156, "player entity type");
            break;
        }
    }
    assert!(bob_saw_alice_info, "Bob received PlayerInfoUpdate for Alice");

    // Alice (already online) must be told about Bob joining: PlayerInfoUpdate
    // then AddEntity for Bob's entity id 2.
    let mut alice_saw_bob_info = false;
    let mut alice_saw_bob_spawn = false;
    for _ in 0..600 {
        let (id, body) = recv(&mut alice);
        if id == 70 {
            alice_saw_bob_info = true;
        }
        if id == 1 {
            let mut pos = 0;
            assert_eq!(read_varint_slice(&body, &mut pos), 2, "Bob's entity id");
            pos += 16;
            assert_eq!(read_varint_slice(&body, &mut pos), 156, "player entity type");
            alice_saw_bob_spawn = true;
            break;
        }
    }
    assert!(alice_saw_bob_info, "Alice received PlayerInfoUpdate for Bob");
    assert!(alice_saw_bob_spawn, "Alice received AddEntity for Bob");

    // Bob moves and turns. Alice should receive a position-carrying packet and a
    // head-rotation packet, both for Bob's entity id 2.
    let mut move_body = Vec::new();
    move_body.extend_from_slice(&1.5f64.to_be_bytes()); // x
    move_body.extend_from_slice(&64.0f64.to_be_bytes()); // y
    move_body.extend_from_slice(&(-3.5f64).to_be_bytes()); // z
    move_body.extend_from_slice(&90.0f32.to_be_bytes()); // yaw
    move_body.extend_from_slice(&12.0f32.to_be_bytes()); // pitch
    move_body.push(1); // flags: on ground
    send(&mut bob, 31, &move_body); // ServerboundMovePlayerPosRot

    // Give the simulation a few ticks to broadcast.
    std::thread::sleep(Duration::from_millis(300));

    let mut saw_position = false;
    let mut saw_head = false;
    for _ in 0..1000 {
        let (id, body) = recv(&mut alice);
        match id {
            // MoveEntityPosRot or EntityPositionSync for Bob.
            54 | 35 => {
                let mut pos = 0;
                if read_varint_slice(&body, &mut pos) == 2 {
                    saw_position = true;
                }
            }
            // RotateHead for Bob — proves the body/head yaw propagated.
            83 => {
                let mut pos = 0;
                if read_varint_slice(&body, &mut pos) == 2 {
                    saw_head = true;
                }
            }
            _ => {}
        }
        if saw_position && saw_head {
            break;
        }
    }
    assert!(saw_position, "Alice received Bob's position update");
    assert!(saw_head, "Alice received Bob's head rotation");
}

#[test]
fn status_ping_roundtrip() {
    let addr = "127.0.0.1:25591";
    let _server = start_server(addr);
    let mut s = TcpStream::connect(addr).expect("connect");

    handshake(&mut s, addr, 1); // status
    send(&mut s, 0x00, &[]); // status request
    let (id, body) = recv(&mut s);
    assert_eq!(id, 0x00, "status response id");
    assert!(
        String::from_utf8_lossy(&body).contains("\"protocol\":776"),
        "status JSON advertises protocol 776"
    );

    send(&mut s, 0x01, &1234567890i64.to_be_bytes()); // ping
    let (id, body) = recv(&mut s);
    assert_eq!(id, 0x01, "pong id");
    assert_eq!(
        i64::from_be_bytes(body.try_into().unwrap()),
        1234567890,
        "pong echoes the ping payload"
    );
}

#[test]
fn login_through_configuration_into_play() {
    let addr = "127.0.0.1:25592";
    let _server = start_server(addr);
    let mut s = TcpStream::connect(addr).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

    handshake(&mut s, addr, 2); // login

    // ServerboundHello(name, uuid)
    let mut hello = Vec::new();
    write_utf(&mut hello, "TestPlayer");
    hello.extend_from_slice(&[0u8; 16]); // client uuid
    send(&mut s, 0x00, &hello);

    // Expect ClientboundLoginFinished (id 2).
    let (id, _) = recv(&mut s);
    assert_eq!(id, 2, "ClientboundLoginFinished");

    // Acknowledge login -> server enters configuration.
    send(&mut s, 3, &[]); // ServerboundLoginAcknowledged

    // A real client sends its brand (ServerboundCustomPayload, id 2) early in
    // configuration, before the known-packs response. The server must tolerate
    // it rather than disconnect.
    let mut brand = Vec::new();
    write_utf(&mut brand, "minecraft:brand");
    write_utf(&mut brand, "vanilla");
    send(&mut s, 2, &brand);

    // The server should now push UpdateEnabledFeatures (12) and
    // SelectKnownPacks (14). Read until we see the known-packs request.
    let mut saw_features = false;
    loop {
        let (id, _) = recv(&mut s);
        match id {
            1 => {} // ClientboundCustomPayload (server brand "Vela")
            12 => saw_features = true,
            14 => break, // ClientboundSelectKnownPacks
            other => panic!("unexpected config packet before known packs: {other}"),
        }
    }
    assert!(saw_features, "UpdateEnabledFeatures precedes known packs");

    // Echo the core pack back: ServerboundSelectKnownPacks(minecraft:core/26.2).
    let mut packs = Vec::new();
    write_varint(&mut packs, 1);
    write_utf(&mut packs, "minecraft");
    write_utf(&mut packs, "core");
    write_utf(&mut packs, "26.2");
    send(&mut s, 7, &packs);

    // Now expect a run of RegistryData packets (7), an UpdateTags packet (13),
    // terminated by FinishConfiguration (3).
    let mut registry_packets = 0;
    let mut saw_tags = false;
    loop {
        let (id, _) = recv(&mut s);
        match id {
            7 => registry_packets += 1,
            13 => saw_tags = true, // ClientboundUpdateTags
            3 => break,            // ClientboundFinishConfiguration
            other => panic!("unexpected packet during registry sync: {other}"),
        }
    }
    assert_eq!(
        registry_packets, 29,
        "one RegistryData packet per synced registry"
    );
    assert!(saw_tags, "UpdateTags sent before FinishConfiguration");

    // Acknowledge finish -> server enters play and sends the join sequence.
    send(&mut s, 3, &[]); // ServerboundFinishConfiguration

    // First play packet must be ClientboundLogin (id 49).
    let (id, body) = recv(&mut s);
    assert_eq!(id, 49, "ClientboundLogin (play)");
    // entity id is the first field: int 1.
    assert_eq!(&body[0..4], &1i32.to_be_bytes(), "entity id");

    // The join sequence also includes a level chunk (45). Confirm at least one
    // arrives without the connection dropping.
    let mut saw_chunk = false;
    for _ in 0..60 {
        let (id, _) = recv(&mut s);
        if id == 45 {
            saw_chunk = true;
            break;
        }
    }
    assert!(saw_chunk, "received at least one LevelChunkWithLight");

    // Confirm the spawn teleport (teleport id 1) so the server treats us as a
    // settled player, then report a movement. ServerboundMovePlayerPosRot (id
    // 31): double x/y/z, float yaw/pitch, byte flags (bit 0 = on ground).
    let mut accept = Vec::new();
    write_varint(&mut accept, 1); // teleport id
    send(&mut s, 0, &accept); // ServerboundAcceptTeleportation

    let mut move_body = Vec::new();
    move_body.extend_from_slice(&1.5f64.to_be_bytes()); // x
    move_body.extend_from_slice(&72.0f64.to_be_bytes()); // y
    move_body.extend_from_slice(&(-3.5f64).to_be_bytes()); // z
    move_body.extend_from_slice(&90.0f32.to_be_bytes()); // yaw
    move_body.extend_from_slice(&12.0f32.to_be_bytes()); // pitch
    move_body.push(1); // flags: on ground
    send(&mut s, 31, &move_body); // ServerboundMovePlayerPosRot

    // The server should accept the movement and keep the connection open: it
    // still drives keep-alives, so we expect at least one more clientbound
    // packet (a KeepAlive, id 44) rather than an EOF/disconnect.
    assert!(
        recv_opt(&mut s).is_some(),
        "server stays connected after accepting a movement packet"
    );

    // Send a chat message and expect it broadcast back as ClientboundSystemChat
    // (id 121). ServerboundChatPacket (id 9): message string, then
    // timestamp/salt/signature/last-seen fields the server ignores.
    let mut chat = Vec::new();
    write_utf(&mut chat, "hello vela");
    chat.extend_from_slice(&0i64.to_be_bytes()); // timestamp
    chat.extend_from_slice(&0i64.to_be_bytes()); // salt
    chat.push(0); // signature: absent
    write_varint(&mut chat, 0); // last-seen offset
    chat.extend_from_slice(&[0, 0, 0]); // last-seen acknowledged: 20-bit bitset
    chat.push(0); // last-seen checksum
    send(&mut s, 9, &chat);

    // A SystemChat (121) carrying the rendered "<TestPlayer> hello vela" line
    // should come back (we're subscribed to our own broadcast). It trails the
    // large backlog of join-sequence chunk packets (id 45) still in the stream,
    // plus the odd keep-alive (44), so drain a generous run looking for it. The
    // backlog scales with the configured view distance (default 10 -> a 21x21
    // chunk burst, ~441 packets), so the bound must comfortably exceed that.
    let mut chat_body = None;
    for _ in 0..1200 {
        let (id, body) = recv(&mut s);
        if id == 121 {
            chat_body = Some(body);
            break;
        }
    }
    let chat_body = chat_body.expect("received a ClientboundSystemChat after sending chat");
    // The content is a network-NBT `{text:"…"}` compound; rather than decode it,
    // confirm the rendered line appears verbatim in the bytes, and that the
    // trailing overlay flag is 0 (chat box, not action bar).
    let needle = b"<TestPlayer> hello vela";
    assert!(
        chat_body.windows(needle.len()).any(|w| w == needle),
        "system chat carries the formatted line"
    );
    assert_eq!(
        *chat_body.last().unwrap(),
        0,
        "overlay flag is false (renders in the chat box)"
    );

    // Run a command. ServerboundChatCommand (id 7): just the command string with
    // no leading slash. `/list` replies to the issuer with a SystemChat (121)
    // carrying the translatable `commands.list.players` component.
    let mut list_cmd = Vec::new();
    write_utf(&mut list_cmd, "list");
    send(&mut s, 7, &list_cmd);

    let mut cmd_reply = None;
    for _ in 0..400 {
        let (id, body) = recv(&mut s);
        if id == 121 {
            cmd_reply = Some(body);
            break;
        }
    }
    let cmd_reply = cmd_reply.expect("received a SystemChat reply to /list");
    // The reply is a translatable component; its key appears verbatim in the
    // network-NBT bytes, as does the only online player's name in the `with` arg.
    for needle in [b"commands.list.players".as_slice(), b"TestPlayer".as_slice()] {
        assert!(
            cmd_reply.windows(needle.len()).any(|w| w == needle),
            "command reply carries {}",
            String::from_utf8_lossy(needle)
        );
    }
}
