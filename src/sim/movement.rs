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
