//! Connection-establishment integration tests: the status/ping exchange and the
//! full login -> configuration -> play handshake (including the chat and command
//! round-trips a client makes right after spawning).

mod common;
use common::*;

use std::net::TcpStream;
use std::time::Duration;

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
