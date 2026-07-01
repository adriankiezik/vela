//! Entity movement broadcasting: once per `UPDATE_INTERVAL` ticks, convey each
//! player's position/rotation change to every other viewer using the cheapest
//! packet that does the job, falling back to an absolute resync when a relative
//! delta won't do. Mirrors vanilla's `ServerEntity.sendChanges` for the player
//! case. Runs as an ordinary system in the schedule.

use bevy_ecs::prelude::*;

use super::bridge::{Outbound, OutboxTx};
use super::components::*;
use super::packets;

/// How often (in ticks) an entity's position/rotation is broadcast, matching
/// vanilla's player `EntityType.updateInterval` of 2.
const UPDATE_INTERVAL: u32 = 2;

/// Broadcast each player's movement to everyone else, following vanilla's
/// `ServerEntity.sendChanges` for the player case: every `UPDATE_INTERVAL` ticks
/// pick the cheapest packet that conveys the change (a position and/or rotation
/// delta), fall back to an absolute resync when a delta won't do, and send head
/// yaw separately. Deltas are relative to each entity's last-sent `Tracking`
/// base, which is advanced only for the fields actually sent.
pub fn broadcast_movement(world: &mut World) {
    // Phase 1: decide each player's packets and advance their tracking state.
    let mut pending: Vec<(Entity, Vec<bytes::Bytes>)> = Vec::new();
    {
        let mut q = world.query::<(Entity, &Profile, &Pos, &mut Tracking)>();
        for (entity, profile, pos, mut t) in q.iter_mut(world) {
            let mut packets = Vec::new();
            let eid = profile.entity_id;

            if t.tick_count % UPDATE_INTERVAL == 0 {
                t.teleport_delay += 1;

                let yaw_n = packets::pack_angle(pos.yaw);
                let pitch_n = packets::pack_angle(pos.pitch);
                let send_rotation = (yaw_n as i32 - t.yaw as i32).abs() >= 1
                    || (pitch_n as i32 - t.pitch as i32).abs() >= 1;

                let dx = pos.x - t.base_x;
                let dy = pos.y - t.base_y;
                let dz = pos.z - t.base_z;
                let position_changed = dx * dx + dy * dy + dz * dz >= 7.629_394_5e-6;
                // A forced position resend every 60 ticks corrects rounding drift
                // (vanilla `flag2 = flag1 || this.tickCount % 60 == 0`).
                let send_position = position_changed || t.tick_count % 60 == 0;

                let xa = packets::enc(pos.x) - packets::enc(t.base_x);
                let ya = packets::enc(pos.y) - packets::enc(t.base_y);
                let za = packets::enc(pos.z) - packets::enc(t.base_z);
                let delta_too_big = !(-32768..=32767).contains(&xa)
                    || !(-32768..=32767).contains(&ya)
                    || !(-32768..=32767).contains(&za);

                let mut sent_position = false;
                let mut sent_rotation = false;

                if delta_too_big || t.teleport_delay > 400 || t.on_ground != pos.on_ground {
                    // A relative delta won't do: resync absolutely.
                    t.on_ground = pos.on_ground;
                    t.teleport_delay = 0;
                    packets.push(packets::entity_position_sync(
                        eid, pos.x, pos.y, pos.z, pos.yaw, pos.pitch, pos.on_ground,
                    ));
                    sent_position = true;
                    sent_rotation = true;
                } else if !send_position || !send_rotation {
                    if send_position {
                        packets.push(packets::move_entity_pos(
                            eid,
                            xa as i16,
                            ya as i16,
                            za as i16,
                            pos.on_ground,
                        ));
                        sent_position = true;
                    } else if send_rotation {
                        packets.push(packets::move_entity_rot(
                            eid,
                            yaw_n,
                            pitch_n,
                            pos.on_ground,
                        ));
                        sent_rotation = true;
                    }
                } else {
                    packets.push(packets::move_entity_pos_rot(
                        eid,
                        xa as i16,
                        ya as i16,
                        za as i16,
                        yaw_n,
                        pitch_n,
                        pos.on_ground,
                    ));
                    sent_position = true;
                    sent_rotation = true;
                }

                if sent_position {
                    t.base_x = pos.x;
                    t.base_y = pos.y;
                    t.base_z = pos.z;
                }
                if sent_rotation {
                    t.yaw = yaw_n;
                    t.pitch = pitch_n;
                }

                // Head yaw is independent of the body yaw in general, but we
                // don't model a separate body yaw, so it reuses the packed look
                // yaw the move packets already carry.
                let head_n = yaw_n;
                if (head_n as i32 - t.head as i32).abs() >= 1 {
                    packets.push(packets::rotate_head(eid, head_n));
                    t.head = head_n;
                }
            }

            t.tick_count = t.tick_count.wrapping_add(1);

            if !packets.is_empty() {
                pending.push((entity, packets));
            }
        }
    }

    if pending.is_empty() {
        return;
    }

    // Phase 2: fan each player's packets out to every other connection.
    let conns: Vec<(Entity, OutboxTx)> = {
        let mut q = world.query::<(Entity, &Conn)>();
        q.iter(world).map(|(e, c)| (e, c.outbox.clone())).collect()
    };
    for (sender, packets) in pending {
        for (entity, outbox) in &conns {
            if *entity == sender {
                continue;
            }
            for pkt in &packets {
                let _ = outbox.try_send(Outbound::Packet(pkt.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! Parity tests for `broadcast_movement` against vanilla
    //! `ServerEntity.sendChanges` (`net/minecraft/server/level/ServerEntity.java`,
    //! MC 26.2). Each case drives a *mover* whose `Tracking` base is the
    //! last-sent state and whose `Pos` is the new state, runs one tick, and
    //! inspects the packets delivered to a second, stationary *observer*.
    //!
    //! Vanilla anchors (confirmed in the decompile):
    //!   TOLERANCE_LEVEL_POSITION = 7.6293945E-6   (position delta threshold)
    //!   FORCED_TELEPORT_PERIOD   = 400            (teleportDelay > 400 -> resync)
    //!   delta short range        = [-32768, 32767]
    //!   forced resend            = tickCount % 60 == 0
    //!   updateInterval (player)  = 2

    use super::*;
    use crate::protocol::buffer::PacketReader;
    use crate::sim::components::{Conn, Pos, Profile, Tracking};
    use crate::sim::packets::pack_angle;
    use tokio::sync::mpsc;

    // Clientbound packet ids we assert on. Derived from the builders themselves
    // (rather than hard-coded) so they track any renumbering in `sim::packets`.
    fn frame_id(b: bytes::Bytes) -> i32 {
        let mut r = PacketReader::new(b);
        r.read_varint().unwrap(); // length
        r.read_varint().unwrap() // id
    }
    fn id_move_pos() -> i32 {
        frame_id(packets::move_entity_pos(0, 0, 0, 0, false))
    }
    fn id_move_rot() -> i32 {
        frame_id(packets::move_entity_rot(0, 0, 0, false))
    }
    fn id_move_pos_rot() -> i32 {
        frame_id(packets::move_entity_pos_rot(0, 0, 0, 0, 0, 0, false))
    }
    fn id_position_sync() -> i32 {
        frame_id(packets::entity_position_sync(0, 0.0, 0.0, 0.0, 0.0, 0.0, false))
    }
    fn id_rotate_head() -> i32 {
        frame_id(packets::rotate_head(0, 0))
    }

    fn pos(x: f64, y: f64, z: f64, yaw: f32, pitch: f32, on_ground: bool) -> Pos {
        Pos { x, y, z, yaw, pitch, on_ground }
    }

    /// A `Tracking` base representing state already synced to `(bx,by,bz)` /
    /// `(byaw,bpitch)`, with an explicit `teleport_delay` and `tick_count`.
    #[allow(clippy::too_many_arguments)]
    fn base(
        bx: f64,
        by: f64,
        bz: f64,
        byaw: f32,
        bpitch: f32,
        on_ground: bool,
        teleport_delay: u32,
        tick_count: u32,
    ) -> Tracking {
        Tracking {
            base_x: bx,
            base_y: by,
            base_z: bz,
            yaw: pack_angle(byaw),
            pitch: pack_angle(bpitch),
            head: pack_angle(byaw),
            on_ground,
            teleport_delay,
            tick_count,
        }
    }

    fn spawn_player(
        world: &mut World,
        entity_id: i32,
        p: Pos,
        t: Tracking,
    ) -> (Entity, mpsc::Receiver<Outbound>) {
        let (tx, rx) = mpsc::channel(64);
        let e = world
            .spawn((
                Profile { name: format!("p{entity_id}"), entity_id },
                p,
                t,
                Conn { outbox: tx },
            ))
            .id();
        (e, rx)
    }

    /// A stationary observer whose own tick is gated off (odd `tick_count`), so
    /// it emits nothing and only receives the mover's packets.
    fn spawn_observer(world: &mut World) -> mpsc::Receiver<Outbound> {
        let p = pos(1000.0, 64.0, 1000.0, 0.0, 0.0, true);
        let t = base(1000.0, 64.0, 1000.0, 0.0, 0.0, true, 0, 1);
        spawn_player(world, 999, p, t).1
    }

    fn drain(rx: &mut mpsc::Receiver<Outbound>) -> Vec<i32> {
        let mut ids = Vec::new();
        while let Ok(o) = rx.try_recv() {
            if let Outbound::Packet(b) = o {
                ids.push(frame_id(b));
            }
        }
        ids
    }

    #[test]
    fn position_only_delta_uses_move_pos() {
        // Moved +1.0 in x, no rotation: cheapest packet is MoveEntity.Pos.
        let mut world = World::new();
        let mut obs = spawn_observer(&mut world);
        spawn_player(
            &mut world,
            1,
            pos(1.0, 64.0, 0.0, 0.0, 0.0, true),
            base(0.0, 64.0, 0.0, 0.0, 0.0, true, 0, 2), // tick 2: acts, not %60
        );

        broadcast_movement(&mut world);

        assert_eq!(drain(&mut obs), vec![id_move_pos()]);
    }

    #[test]
    fn rotation_only_uses_move_rot_plus_head() {
        // Same position, yaw turned 90°: MoveEntity.Rot, and head yaw follows.
        let mut world = World::new();
        let mut obs = spawn_observer(&mut world);
        spawn_player(
            &mut world,
            1,
            pos(5.0, 64.0, 5.0, 90.0, 0.0, true),
            base(5.0, 64.0, 5.0, 0.0, 0.0, true, 0, 2),
        );

        broadcast_movement(&mut world);

        assert_eq!(drain(&mut obs), vec![id_move_rot(), id_rotate_head()]);
    }

    #[test]
    fn position_and_rotation_uses_pos_rot_plus_head() {
        let mut world = World::new();
        let mut obs = spawn_observer(&mut world);
        spawn_player(
            &mut world,
            1,
            pos(1.0, 64.0, 0.0, 90.0, 0.0, true),
            base(0.0, 64.0, 0.0, 0.0, 0.0, true, 0, 2),
        );

        broadcast_movement(&mut world);

        assert_eq!(drain(&mut obs), vec![id_move_pos_rot(), id_rotate_head()]);
    }

    #[test]
    fn subthreshold_move_emits_nothing() {
        // A move below TOLERANCE_LEVEL_POSITION (7.6293945e-6) with no rotation,
        // on a tick that is not a %60 forced resend, produces no packet.
        let mut world = World::new();
        let mut obs = spawn_observer(&mut world);
        spawn_player(
            &mut world,
            1,
            pos(0.001, 64.0, 0.0, 0.0, 0.0, true), // 1e-6 sqr < threshold
            base(0.0, 64.0, 0.0, 0.0, 0.0, true, 0, 2),
        );

        broadcast_movement(&mut world);

        assert!(drain(&mut obs).is_empty());
    }

    #[test]
    fn forced_resend_every_60_ticks_on_still_entity() {
        // tickCount % 60 == 0 forces a (zero-delta) position resend even when the
        // entity has not moved. Rotation unchanged -> MoveEntity.Pos.
        let mut world = World::new();
        let mut obs = spawn_observer(&mut world);
        spawn_player(
            &mut world,
            1,
            pos(0.0, 64.0, 0.0, 0.0, 0.0, true),
            base(0.0, 64.0, 0.0, 0.0, 0.0, true, 0, 60),
        );

        broadcast_movement(&mut world);

        assert_eq!(drain(&mut obs), vec![id_move_pos()]);
    }

    #[test]
    fn delta_too_big_falls_back_to_position_sync() {
        // A jump larger than the short delta range (|round(d*4096)| > 32767, i.e.
        // > 8 blocks) can't be a relative delta -> EntityPositionSync.
        let mut world = World::new();
        let mut obs = spawn_observer(&mut world);
        spawn_player(
            &mut world,
            1,
            pos(10.0, 64.0, 0.0, 0.0, 0.0, true), // 10 * 4096 = 40960 > 32767
            base(0.0, 64.0, 0.0, 0.0, 0.0, true, 0, 2),
        );

        broadcast_movement(&mut world);

        assert_eq!(drain(&mut obs), vec![id_position_sync()]);
    }

    #[test]
    fn teleport_delay_over_400_forces_position_sync() {
        // teleportDelay increments to 401 (> FORCED_TELEPORT_PERIOD) this tick,
        // forcing a resync even without movement.
        let mut world = World::new();
        let mut obs = spawn_observer(&mut world);
        spawn_player(
            &mut world,
            1,
            pos(0.0, 64.0, 0.0, 0.0, 0.0, true),
            base(0.0, 64.0, 0.0, 0.0, 0.0, true, 400, 2),
        );

        broadcast_movement(&mut world);

        assert_eq!(drain(&mut obs), vec![id_position_sync()]);
    }

    #[test]
    fn on_ground_change_forces_position_sync() {
        // A change in the on-ground flag can't ride a delta packet -> resync.
        let mut world = World::new();
        let mut obs = spawn_observer(&mut world);
        spawn_player(
            &mut world,
            1,
            pos(0.0, 64.0, 0.0, 0.0, 0.0, false), // was on ground, now airborne
            base(0.0, 64.0, 0.0, 0.0, 0.0, true, 0, 2),
        );

        broadcast_movement(&mut world);

        assert_eq!(drain(&mut obs), vec![id_position_sync()]);
    }

    #[test]
    fn odd_tick_is_gated_but_still_advances_counter() {
        // updateInterval = 2: nothing is broadcast on an odd tick even for a big
        // move, but tickCount still advances.
        let mut world = World::new();
        let mut obs = spawn_observer(&mut world);
        let (mover, _rx) = spawn_player(
            &mut world,
            1,
            pos(5.0, 64.0, 5.0, 90.0, 0.0, true),
            base(0.0, 64.0, 0.0, 0.0, 0.0, true, 0, 1), // odd tick
        );

        broadcast_movement(&mut world);

        assert!(drain(&mut obs).is_empty());
        assert_eq!(world.get::<Tracking>(mover).unwrap().tick_count, 2);
    }

    #[test]
    fn rotation_only_send_does_not_advance_position_base() {
        // Base advancement is per-field: a rotation-only send must leave the
        // position base untouched (and vice versa), matching vanilla's separate
        // positionCodec.setBase / rot state updates.
        let mut world = World::new();
        let _obs = spawn_observer(&mut world);
        let (mover, _rx) = spawn_player(
            &mut world,
            1,
            pos(5.0, 64.0, 5.0, 90.0, 0.0, true),
            base(5.0, 64.0, 5.0, 0.0, 0.0, true, 0, 2),
        );

        broadcast_movement(&mut world);

        let t = world.get::<Tracking>(mover).unwrap();
        assert_eq!(t.base_x, 5.0, "position base must not move on a rot-only send");
        assert_eq!(t.yaw, pack_angle(90.0), "yaw base must advance to the sent angle");
    }

    #[test]
    fn position_only_send_does_not_advance_rotation_base() {
        let mut world = World::new();
        let _obs = spawn_observer(&mut world);
        let (mover, _rx) = spawn_player(
            &mut world,
            1,
            pos(1.0, 64.0, 0.0, 0.0, 0.0, true),
            base(0.0, 64.0, 0.0, 0.0, 0.0, true, 0, 2),
        );

        broadcast_movement(&mut world);

        let t = world.get::<Tracking>(mover).unwrap();
        assert_eq!(t.base_x, 1.0, "position base must advance to the sent position");
        assert_eq!(t.yaw, pack_angle(0.0), "yaw base must not change on a pos-only send");
    }

    #[test]
    fn mover_does_not_receive_its_own_packets() {
        // Phase 2 skips the sender: a lone moving player broadcasts to nobody.
        let mut world = World::new();
        let (_mover, mut own_rx) = spawn_player(
            &mut world,
            1,
            pos(1.0, 64.0, 0.0, 0.0, 0.0, true),
            base(0.0, 64.0, 0.0, 0.0, 0.0, true, 0, 2),
        );

        broadcast_movement(&mut world);

        assert!(drain(&mut own_rx).is_empty());
    }
}
