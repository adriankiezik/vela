# Vela — Architecture

Vela is split into two halves joined by channels:

> **`tokio` owns sockets. `bevy_ecs` owns the game. One channel each way.**

The network layer never touches game state; the simulation never touches a
socket. Everything crossing between them is a message, so either side can be
reworked without disturbing the other.

```
  connections (async, one task each)        game world (one owner, fixed tick)
      decode inbound  ──►  ingress channel  ──►  apply + simulate
      write outbound  ◄──  per-connection outbox  ◄──  produce
```

## Layers

- **protocol** — the hand-written wire codec: framing, VarInt, NBT, and the
  connection state machine. Pure and synchronous; no I/O.
- **net** — owns sockets. One async task per connection. Pre-Play states are
  plain request/response and run inline; in Play the connection bridges to the
  simulation through channels and otherwise holds no game state.
- **sim** — owns all game state. A single ECS world advanced on a fixed tick
  (20 per second) on its own thread. Players and other game objects are
  entities; per-tick behaviour is expressed as systems. Its state is organized
  into domain modules behind that one owner — **world** (chunks, terrain,
  palettes, heightmaps), **inventory** (containers, item stacks), and
  **registry** (the clean-room static data tables and tags). These are
  sub-domains of the simulation, not separate layers.

## Connection lifecycle

A connection is one async task. It carries the client through handshake,
status/login, and configuration as direct request/response. Configuration is
where static data is synced: the server shares the vanilla core data pack with
the client and sends each registry (dimension types, biomes, damage types, tags)
as entry IDs only, letting the client fill the definitions from its own built-in
copy — so no Mojang registry assets are shipped. On entering Play the connection
splits in two: one side decodes inbound packets and forwards them to the
simulation, the other drains an outbox of outbound packets to the socket. The
connection registers with the simulation once on join and signals once on
disconnect; in between it is a pure conduit.

## Simulation

The simulation runs on its own thread — not the async runtime — because it is
CPU-bound and synchronous. Each tick it drains everything the network delivered
since the previous tick, applies it, then advances the world. It performs no
I/O: inbound work arrives as messages and outbound packets leave through
per-connection outboxes. Because all game state sits behind a single owner,
there is no shared mutable state between connections and nothing to lock on the
hot path.

## The bridge

A small set of message types is the entire contract between the halves:

- **net → sim**: a player joined (with the handle to reach them), a player
  left, or a decoded packet from a player.
- **sim → net**: a packet to send, or a request to close the connection.

The simulation produces fully encoded packets; the network side only moves
bytes. All protocol knowledge stays on one side of the boundary and the channel
between them carries opaque bytes — which is what lets the codec and the game
logic evolve independently.

## Conventions

- The protocol layer is built by hand (the part worth owning); general-purpose
  crates handle plumbing. High-level Minecraft frameworks are out of scope.
- `bevy_ecs` is used as core ECS only — no rendering, reflection, or math
  features a headless server doesn't need.
- Offline mode only: no encryption. Packet compression is supported and
  negotiated mid-Login (zlib above a size threshold).
