# Vela — Architecture

Vela is split into two halves joined by channels:

> **`tokio` owns sockets. `bevy_ecs` owns the game. One channel each way.**

The network layer never touches game state; the simulation never touches a
socket. Everything crossing between them is a message.

```
tokio tasks (per connection)            bevy_ecs world (single owner, 20 TPS)
  read_frame → decode                     ── tick ──► systems
       │  ToSim                                            │ framed bytes
       └──────────► mpsc ───► [ drain_ingress ] ──► per-conn mpsc ──► write task
                                                                          │
                                                                       socket
```

## Modules

```
src/
  main.rs              spawn the sim thread + the accept loop; wire the channels
  protocol/            the hand-written wire codec
    varint.rs            VarInt read/write
    framing.rs           frame(id, body) → length-prefixed bytes
    buffer.rs            PacketReader / PacketWriter field accessors
    nbt.rs               binary NBT (named + network framings)
    uuid.rs              offline-mode player UUIDs
    mod.rs               protocol version, State / Intent enums
  net/                 owns sockets (async)
    connection.rs        pre-Play state machine: handshake → status | login → config
    play.rs              Play phase: read task, write task, serverbound decode
    frame.rs             async read_frame / send_packet
    mod.rs               net::handle entry point
  sim/                 owns game state (synchronous, one thread)
    bridge.rs            ToSim / Outbound / Serverbound — the only shared types
    components.rs        ECS components + resources
    systems.rs           per-tick systems
    packets.rs           clientbound Play packet builders → framed bytes
    mod.rs               build the World + Schedule, drive the 20 TPS loop
  registries.rs        network-synced registry entry ids (known-pack passthrough)
  registry_tags.rs     registry tag names, bound empty during configuration
```

## Connection lifecycle

A connection is one `tokio` task running `net::handle`:

1. **Handshake / Status / Login / Configuration** run inline in `connection.rs`,
   reading and writing the socket directly. These states are strict
   request/response, so no shared state is involved.
2. On reaching **Play**, the socket splits into two tasks (`play.rs`):
   - a **read task** decodes frames into `Serverbound` values and forwards them
     to the sim as `ToSim::Packet`;
   - a **write task** drains this connection's outbox and pumps framed bytes to
     the socket, batching a burst into one flush.
3. `play` registers the player (`ToSim::Joined`, handing over the outbox), waits
   for either task to finish, tears down the other, and emits one
   `ToSim::Left`.

`read_frame` is not cancellation-safe, so it lives alone in the read task and is
only ever aborted wholesale on teardown — never raced against a timer.

## Simulation

The sim is a single `bevy_ecs` world ticked at 20 TPS on its own OS thread (it
is CPU-bound and synchronous, so not a tokio worker). Each tick runs a chained
schedule:

- **`advance_tick`** — bump the tick counter.
- **`drain_ingress`** — exclusive system: drain the ingress channel and apply
  each message. Joins spawn an entity and push the join sequence; leaves
  despawn; packets mutate components (movement) or fan out (chat). Spawn/despawn
  and cross-entity broadcast don't fit the parallel `Query` model, so this stage
  owns `&mut World` directly.
- **`keepalive`** — ordinary system: send a keep-alive every 200 ticks (10 s)
  and despawn anyone who missed the previous one.

A player is an entity carrying `PlayerId` + `Profile` + `Pos` + `Conn` (its
outbox) + `KeepAlive`. World-wide state lives in resources: `Tick`,
`NextEntityId`, `PlayerIndex` (`Uuid → Entity`), `Ingress` (the receiving end of
the channel), and `Control` (shutdown flag).

## The bridge

`sim/bridge.rs` is the entire contract between the halves:

- **`ToSim`** (net → sim, one shared channel): `Joined { id, name, outbox }`,
  `Left { id }`, `Packet { id, packet }`.
- **`Outbound`** (sim → net, one channel per connection): `Packet(Bytes)` to
  write, or `Close` to tear the connection down.
- **`Serverbound`**: the decoded Play packets the sim acts on (movement, chat,
  keep-alive, teleport-accept). The `net` layer owns the wire codec; the sim
  stays protocol-shape-agnostic.

Clientbound bytes are framed by the sim (synchronously, in `packets.rs`) and the
write task only writes them — so all encoding lives on one side and the boundary
carries plain `Bytes`.

## Conventions

- The protocol layer is hand-written (VarInt, framing, NBT, state machine);
  general-purpose crates handle plumbing (`tokio`, `bytes`, `serde`,
  `bevy_ecs`). High-level Minecraft frameworks are out of scope.
- `bevy_ecs` is used with `default-features = false` (core ECS only) — no
  rendering, reflection, or math crates a headless server doesn't need.
- Offline mode only: no compression, no encryption. Every frame is
  `VarInt(length) | VarInt(id) | body`.
