# Vela — Architecture

This document describes the **target runtime architecture** for Vela once it
grows past Milestone 1 (a single client standing in a void world) into
multi-player simulation. The current code is a single per-connection state
machine with no shared world; this is the shape it evolves toward.

The organizing principle:

> **`tokio` owns sockets. `bevy_ecs` owns the game. One channel each way.**

Neither side calls into the other directly — they only pass messages. The
network layer never touches game state; the simulation never touches a socket.

---

## Why this split

A Minecraft server's Play simulation is a textbook ECS workload — entities are
heterogeneous bags of components (position, velocity, health, inventory, AI),
systems map cleanly onto the 20 TPS tick loop, and change detection
(`Changed<Position>` → broadcast movement) replaces fiddly hand-rolled dirty
tracking. `bevy_ecs` standalone (no rendering, no Bevy app) is the natural fit,
and is exactly what Valence uses for the same reason.

The trap is letting the ECS own networking too. `bevy_ecs` systems are
synchronous and run inside the tick; sockets are async `tokio` tasks. Forcing
async socket I/O into ECS systems fights both libraries. So the connection tasks
you already have stay as they are, and the ECS owns *game* state, not
*connection* state. The boundary between them is two channels.

This also keeps the project philosophy intact: we hand-write the **protocol**
(the part worth owning); world/entity *storage* is plumbing in the same sense
`tokio`, `bytes`, and `serde` are. Adopting `bevy_ecs` does not violate
"build it ourselves" — adopting a Minecraft *framework* (Valence/Pumpkin) would.

---

## Data flow

```
tokio tasks (per connection)            bevy_ecs World (single owner, 20 TPS)
  read_frame → decode                     ── tick ──► movement, AI, tracking
       │  ToSim::Packet                                     │ clientbound bytes
       └──────────► mpsc ───► [ drain_ingress ] ──► per-conn mpsc ──► write task
                                                                          │
                                                                       socket
```

- Network read tasks decode bytes into typed `Serverbound` values and push them
  into one shared ingress channel.
- A single system at the **start** of each tick drains that channel and mutates
  components (or spawns/despawns entities).
- Systems run the simulation.
- Systems push already-framed clientbound bytes into each player's **outbox**
  channel; a per-connection write task pumps them to the socket.

---

## Directory layout

```
src/
  main.rs                 // spawn sim thread + net accept loop, wire the channels
  net/
    mod.rs                // pub fn handle(stream, peer, to_sim)
    connection.rs         // handshake/status/login/config  (current code, pre-Play)
    play_io.rs            // per-Play connection: read task + write task
  protocol/               // codec — unchanged (varint, buffer, nbt, uuid)
  sim/
    mod.rs                // run(): builds World + Schedule, drives the 20 TPS loop
    bridge.rs             // ToSim / OutboxTx / Serverbound — the ONLY shared types
    components.rs         // Position, Velocity, Player, ClientConn, KeepAlive…
    resources.rs          // Ingress(rx), PlayerIndex, Tick
    systems/
      ingress.rs          // drain ToSim → spawn/despawn/mutate
      join.rs             // on Joined → build + push the play join sequence
      movement.rs         // apply MovePlayer* to Position/Velocity
      tracking.rs         // Changed<Position> → broadcast MoveEntity to nearby
      keepalive.rs        // per-player ping + timeout
```

`registries.rs` / `registry_tags.rs` stay where they are — they are consumed by
`net/connection.rs` during the Configuration state and never touch the ECS.

---

## The bridge — `sim/bridge.rs`

The entire contract between the two worlds. Kept deliberately small.

```rust
use bytes::Bytes;
use uuid::Uuid;

/// net → sim. One shared channel, cloned per connection.
pub enum ToSim {
    Joined { id: Uuid, name: String, outbox: OutboxTx },
    Left   { id: Uuid },
    Packet { id: Uuid, packet: Serverbound },   // already decoded by the read task
}

/// sim → net, one per connection. Bytes are ALREADY framed (len|id|body),
/// so the write task is a dumb pump and systems own all encoding.
pub type OutboxTx = tokio::sync::mpsc::Sender<Bytes>;

/// Decoded serverbound Play packets the sim cares about. The net layer owns
/// the codec; the sim stays protocol-shape-agnostic.
pub enum Serverbound {
    Move { x: Option<f64>, y: Option<f64>, z: Option<f64>,
           yaw: Option<f32>, pitch: Option<f32>, on_ground: bool },
    KeepAlive(i64),
    AcceptTeleport(i32),
}
```

`Move` carries `Option`s — the current `has_pos`/`has_rot` logic moves to the
decode site, so the sim simply applies whatever fields are present.

---

## Components & resources

```rust
#[derive(Component)] struct Position { x: f64, y: f64, z: f64, yaw: f32, pitch: f32 }
#[derive(Component)] struct OnGround(bool);
#[derive(Component)] struct Player    { id: Uuid, name: String, entity_id: i32 }
#[derive(Component)] struct ClientConn { outbox: OutboxTx }       // how the sim talks back
#[derive(Component)] struct KeepAlive  { pending: Option<i64>, last_tick: u64 }

#[derive(Resource)] struct Ingress(tokio::sync::mpsc::Receiver<ToSim>);
#[derive(Resource, Default)] struct PlayerIndex(HashMap<Uuid, Entity>);  // Uuid → entity
#[derive(Resource, Default)] struct Tick(u64);
```

`ClientConn` is the key idea: a player's socket-write side lives **in the ECS as
a component**. Any system with `Query<&ClientConn>` can send packets; none of
them know what a socket is.

---

## The tick driver — `sim/mod.rs`

Runs on its own OS thread (**not** a tokio worker — it is CPU-bound and
synchronous):

```rust
pub fn run(rx: tokio::sync::mpsc::Receiver<ToSim>) {
    let mut world = World::new();
    world.insert_resource(Ingress(rx));
    world.init_resource::<PlayerIndex>();
    world.init_resource::<Tick>();

    let mut schedule = Schedule::default();
    schedule.add_systems((
        drain_ingress,        // 1. apply everything the network delivered
        send_join_sequence,   // 2. newly-spawned players get login+chunks+teleport
        apply_movement,       // 3. simulation
        track_entities,       // 4. Changed<Position> → MoveEntity broadcasts
        keepalive,            // 5. pings + timeouts
    ).chain());

    let period = Duration::from_millis(50);   // 20 TPS
    loop {
        let start = Instant::now();
        schedule.run(&mut world);
        world.resource_mut::<Tick>().0 += 1;
        if let Some(rem) = period.checked_sub(start.elapsed()) {
            std::thread::sleep(rem);          // crude but fine until a real scheduler is needed
        }
    }
}
```

`drain_ingress` is the only system that touches the channel. It runs as an
exclusive system because it spawns and despawns entities:

```rust
fn drain_ingress(world: &mut World) {
    let mut buf = Vec::new();
    {
        let mut ing = world.resource_mut::<Ingress>();
        while let Ok(msg) = ing.0.try_recv() { buf.push(msg); }   // non-blocking drain
    }
    for msg in buf {
        match msg {
            ToSim::Joined { id, name, outbox } => { /* spawn entity, index.insert */ }
            ToSim::Left   { id }               => { /* index.remove + despawn */ }
            ToSim::Packet { id, packet }       => { /* look up entity, mutate components */ }
        }
    }
}
```

`try_recv()` is the load-bearing detail: the sim never `.await`s. It drains
whatever arrived since the previous tick and moves on.

---

## The connection lifecycle — `net/play_io.rs`

When `connection.rs` reaches Play it stops looping on the state enum and splits
into **two tasks plus one message**:

```rust
// at the Configuration → Play transition:
let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Bytes>(64);
let (rd, mut wr) = stream.into_split();

// 1. announce the player to the sim (hands it the outbox)
to_sim.send(ToSim::Joined { id: uuid, name, outbox: out_tx }).await.ok();

// 2. write task: dumb pump, sim → socket
tokio::spawn(async move {
    while let Some(bytes) = out_rx.recv().await {
        if wr.write_all(&bytes).await.is_err() { break; }
    }
});

// 3. read task: socket → sim (decode here, send Serverbound)
let mut rd = BufReader::new(rd);
while let Ok(Some((id, reader))) = read_frame(&mut rd).await {
    if let Some(pkt) = decode_play(id, reader) {
        if to_sim.send(ToSim::Packet { id: uuid, packet: pkt }).await.is_err() { break; }
    }
}
to_sim.send(ToSim::Left { id: uuid }).await.ok();   // EOF → tell the sim to despawn
```

Three things worth calling out:

- **The join sequence moves into the sim** (`send_join_sequence`), because the
  play-login packet needs the entity id and spawn position the ECS just
  assigned. So *all* clientbound Play traffic flows through the `ClientConn`
  outbox uniformly — no special-casing the first packets.
- **Backpressure = disconnect.** If a system's `outbox.try_send()` returns
  `Full`, the client cannot keep up; drop it. Use `try_send` from systems
  (sync, non-blocking), never `blocking_send`.
- **Forced disconnect needs one extra wire.** Dropping `ClientConn` ends the
  *write* task (its channel closes) but the *read* task is parked on the socket.
  Store an `AbortHandle` (or a `oneshot` shutdown) alongside the outbox so the
  keepalive-timeout system can actually kill the read task. This is the one bit
  of bookkeeping the naive picture misses.

---

## `main.rs` after the change

```rust
#[tokio::main]
async fn main() -> std::io::Result<()> {
    let (to_sim_tx, to_sim_rx) = tokio::sync::mpsc::channel(1024);
    std::thread::spawn(move || sim::run(to_sim_rx));     // sim owns the World, on its own thread

    let listener = TcpListener::bind(&addr).await?;
    loop {
        let (stream, peer) = listener.accept().await?;
        stream.set_nodelay(true).ok();
        let to_sim = to_sim_tx.clone();
        tokio::spawn(net::handle(stream, peer, to_sim));
    }
}
```

The accept loop is essentially today's code — it just hands each connection a
clone of the ingress sender.

---

## Migration order

Do this in three steps so the dependency lands only after the boundary is proven:

1. **Refactor first, no ECS.** Split `net/` from the rest and tighten the codec
   import boundary. Server stays single-player.
2. **Stand up `sim::run` with a placeholder world** (`HashMap<Uuid, …>`) and the
   channel bridge — single player, no `bevy_ecs` yet. Prove the two-thread +
   two-channel shape works end to end.
3. **Swap the hand-rolled world for `bevy_ecs`.** At this point it is a localized
   change inside `sim/`, and `net/` never notices.

Doing it in this order means `bevy_ecs` lands *after* the architecture is
validated — so if the dependency turns out not to be worth it, nothing is lost:
the channel-based boundary is good with or without an ECS behind it.

---

## Implementation status

All three steps are implemented. The shipped code follows this design with a few
deliberate refinements:

- **`bevy_ecs = "0.18"` with `default-features = false, features = ["std"]`.**
  The default features drag in `bevy_reflect`/`glam`/`wgpu-types`/`web-sys`,
  none of which a headless server needs; core ECS pulls only a handful of small
  `bevy_*` crates. (0.19 was skipped — it requires rustc 1.95.)
- **Module layout:** the sketch's `resources.rs` + `systems/` directory landed
  as `sim/components.rs` (components *and* resources) and a single
  `sim/systems.rs`. Same separation, fewer files at this size.
- **Ingress lives in a `Mutex<Receiver<ToSim>>` resource.** The tokio receiver
  is `!Sync`, so the `Mutex` lets it sit in a `Send + Sync` ECS resource; the
  drain system is exclusive and single-threaded, so the lock never contends.
- **Shutdown** is signalled by a `Control { stop }` resource set when the
  ingress channel closes, checked by the run loop after each `schedule.run`.
- **Systems:** `advance_tick` → `drain_ingress` (exclusive: spawn/despawn +
  chat fan-out) → `keepalive` (ordinary system: `Query` + `Commands` despawn).
  Players are entities carrying `PlayerId` + `Profile` + `Pos` + `Conn` +
  `KeepAlive`; `PlayerIndex` maps `Uuid → Entity`.

`net/` was not touched by the swap, as intended.
