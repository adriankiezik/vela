# Vela — Porting Roadmap

What needs to be reimplemented (clean-room, from the wire protocol + observed
behavior) to go from "reaches login" to a functional 26.2 server. Reference
decompile lives at `C:\Users\kiezi\mc-decompile\src-server`.

**Scope reference (decompiled 26.2):** 194 protocol/game files (58 serverbound,
127 clientbound), 27 common + 14 configuration + 19 login packets, ~60 synced
registries, ~30 inventory menus.

**Size key:** `S` ≈ hours · `M` ≈ a day or two · `L` ≈ several days · `XL` ≈ a
week+ / ongoing. Unordered — pick by dependency, not list order.

**Status key:** `[done]` · `[partial]` (reached far enough to function; noted
what remains). Unmarked items are not started.

### Milestone 1 — achieved (a client reaches Play and stands in a void world)

The initial port carries a connection from handshake through status, login,
configuration, and into play: it completes login, syncs registries (via
known-packs passthrough) + tags + features, hands off to play, and sends the
join + void-chunk + teleport sequence with a keep-alive loop. Items below are
marked accordingly.

---

## Network / transport layer

- `S` — `[done]` **VarLong codec** (`src/protocol/varint.rs`, with tests)
- `S` — `[done]` **Packet framing polish**: length-prefix done; max-size guard (`MAX_FRAME_LEN`, 2 MiB) and read-side buffering (`BufReader` feeding the play loop) added
- `M` — `[done]` **Compression** (`SetCompression` / zlib): threshold negotiation (`src/net/connection.rs`), compress/decompress framing (`src/protocol/framing.rs`, used in `src/net/frame.rs`)
- `L` — **Encryption** (online mode): RSA keypair, `Hello`→`Key` exchange, AES-CFB8 stream cipher, Mojang session-server `hasJoined` auth, encrypted profile
- `M` — **Cookie store** (`cookie/` — 6 packets): request/response/store for transfers
- `M` — **Transfer / cross-server** support (`Intent::Transfer` path, `ClientboundTransfer`)
- `S` — **Plugin/custom-query channels** (login `CustomQuery`/`CustomQueryAnswer`, play `CustomPayload`)
- `M` — **Generic registry-codec layer**: a reusable encode/decode framework (the decompile's `StreamCodec`) so packets are described declaratively instead of hand-rolled per field

## NBT & data formats

- `L` — `[done]` **NBT codec** (read + write, all 12 tag types, named/nameless root framing, modified-UTF-8, depth guards — `src/protocol/nbt.rs`, tested)
- `M` — **SNBT / text-component parsing** (chat components as NBT in 26.2, not JSON)
- `M` — `[done]` **Text component model**: typed Style/Color, hover/click events, serialization to network NBT (`src/protocol/text.rs`, tested)
- `S` — **Resource location / identifier** type (`namespace:path`) with validation
- `M` — `[partial]` **Bit-packed `PalettedContainer`** (block states + biomes storage, the core chunk encoding primitive): wire serialization done incl. palette growth (single-value → 4–8 bit indirect → direct/global palette ≥256 states, `src/world/encoding.rs`). Mutable edits supported via a sparse per-cell edit map (`set_block`/`block_state_at`, `src/world/chunk_data.rs`). Remaining gap: the network *read* path for incoming/foreign palettes

## Login → Configuration → Play handshake

- `S` — `[done]` **`LoginFinished`** packet — real login completion (offline GameProfile, no signed properties)
- `S` — `[done]` **`LoginAcknowledged`** transition into configuration state
- `S` — `[done]` **State enum + listener split**: `Handshake/Status/Login/Configuration/Play` (play owns the split stream)
- `L` — `[partial]` **Registry data sync** (`ClientboundRegistryDataPacket`): reaches Play via **known-packs passthrough** — entry IDs sent with data absent, client fills definitions from its core pack. Full network-NBT serialization of the ~60 registries still pending (needs the NBT codec)
- `S` — `[done]` **`UpdateEnabledFeatures`** + **feature flags** (`minecraft:vanilla`)
- `S` — `[partial]` **Tags sync** (`UpdateTags`): packet implemented; **block & item tags carry real generated member ids** (`src/registry/tags.rs`). Only non-block/item registry tags (entity_type, damage_type, …) remain bound empty — see the deferred follow-up note below
- `S` — `[done]` **Known packs negotiation** (`SelectKnownPacks`, vanilla `minecraft:core/26.2`)
- `S` — **Code-of-conduct** packets (new in 26.x: `CodeOfConduct` / `AcceptCodeOfConduct`)
- `S` — `[done]` **`FinishConfiguration`** ↔ `ServerboundFinishConfiguration` handoff to Play
- `S` — `[partial]` **Client information / brand** (`ClientInformation`, `CustomPayload` brand): received and tolerated (logged, not yet acted upon)
- `S` — **Resource-pack push/pop** packets (common)

## Play: join & keep-alive

- `M` — `[done]` **`Login` (play) packet**: single overworld dimension, spectator game mode, view/sim distance, spawn info — the big "join game" packet
- `S` — `[done]` **Keep-alive loop** (clientbound ping + serverbound echo, with timeout disconnect on a missed response)
- `S` — **`Disconnect`** (play) + **`Ping`/`Pong`** (common)
- `S` — `[done]` **Set default spawn / `PlayerPosition`** (initial teleport id 1 + `AcceptTeleportation` confirm)
- `S` — `[done]` **Game-event packet** (`ClientboundGameEvent`: `LEVEL_CHUNKS_LOAD_START`)
- `S` — **Server links / `ServerData`** (MOTD/icon in-game)

## World representation & chunks

- `L` — **Block-state model**: `Block` registry, `BlockState` with properties, global palette IDs
- `L` — **Chunk data structures**: `LevelChunk`, 16×16×16 sections, heightmaps, biome storage
- `XL` — `[partial]` **Chunk serialization** (`ClientboundLevelChunkWithLight`): packet wire format implemented with **per-chunk-varying terrain** (noise heightmap) and the full indirect (4–8 bit) + direct/global palette path (`src/world/encoding.rs`), real `WORLD_SURFACE`/`MOTION_BLOCKING` heightmaps, empty light. Dynamic edits supported. Block entities still pending
- `M` — `[partial]` **Light engine**: empty-light payload sent (four empty BitSets + two empty arrays); no real sky/block light propagation yet
- `M` — `[done]` **Chunk streaming**: `SetChunkCacheCenter` + dynamic load/unload by movement with a rounded-corner tracking-view diff (`src/sim/chunking.rs`)
- `M` — `[done]` **Heightmaps** computation & maintenance: `WORLD_SURFACE`/`MOTION_BLOCKING` computed and sent; live recomputation of edited columns on block change (`src/world/heightmap.rs`)
- `S` — `[partial]` **Block-change packets**: `BlockUpdate` implemented and broadcast on edits (`src/sim/packets.rs`); `SectionBlocksUpdate` built + tested but not yet wired into a batched-edit path
- `M` — **Block entities** (`BlockEntityData`, chests/signs/etc. NBT) — model + per-type data
- `M` — **Region / `.mca` persistence** (Anvil format) — or start with in-memory only and defer
- `L` — `[partial]` **World generation**: a real per-chunk noise heightmap generator is in place (fbm value noise, continuous across chunk boundaries — `src/world/terrain.rs`). Full `levelgen` (biomes, carvers, features, structures) is `XL` and still out of early scope
- `S` — **World border** (`world/level/border`, `SetBorder*` packets)

## Blocks, items, registries (content)

- `XL` — **Block registry population** (~1000 blocks with states) — generate from data, not hand-write
- `XL` — **Item registry population** (~1500 items) + **data components** (`core/component`, `item/component` — the 26.x item model)
- `M` — **Fluids** (water/lava state, levels)
- `L` — **Block behavior**: collision shapes (`phys/shapes` voxel shapes), hardness, placement rules, `BlockState` neighbor updates
- `M` — **Recipes** (`item/crafting`) + recipe book sync (`UpdateRecipes`, `RecipeBookAdd/Remove/Settings`)
- `M` — **Creative inventory / creative-mode item set** (`SetCreativeModeSlot`)

## Entities

- `XL` — **Entity base + type registry** (~120 entity types): id, bounding box, tracking
- `L` — **Entity metadata / data-syncher** (`network/syncher` — `SynchedEntityData`, `SetEntityData`)
- `L` — `[partial]` **Entity spawn/remove/track**: `AddEntity` (players, type `minecraft:player`) + `RemoveEntities` on join/leave. Every player tracks every other — correct here, since the whole world sits well inside the 32-chunk player tracking range. Per-player view-distance culling / dynamic add-remove on movement still pending
- `M` — `[partial]` **Entity movement packets**: `MoveEntityPos/Rot/PosRot` + `RotateHead` + `EntityPositionSync` (absolute resync fallback), broadcast per tick via a 1:1 port of `ServerEntity.sendChanges` (update interval 2, `VecDeltaCodec` deltas, on-ground-flip / >8-block / 400-tick resync). `TeleportEntity`/`SetEntityMotion`/velocity pending (player velocity not modelled yet, so it stays zero)
- `M` — `[partial]` **Entity events / status / animations**: arm-swing `Animate` wired (`src/sim/packets.rs`, broadcast from `packet_handlers.rs`); `EntityEvent`, `HurtAnimation`, `TakeItemEntity` pending
- `M` — **Equipment & passengers**: `SetEquipment`, `SetPassengers`, `SetEntityLink` (leash)
- `M` — **Attributes** (`world/attribute` — `UpdateAttributes`)
- `M` — **Mob effects / potions** (`world/effect`, `UpdateMobEffect`/`RemoveMobEffect`)
- `XL` — **Entity AI / brain / pathfinding** (`entity/ai`, `pathfinder`) — large, defer; needed for living mobs to behave
- `L` — **Projectiles, vehicles, item entities, decorations** (per-family behavior)
- `S` — **Experience orbs / XP** (`AddExperienceOrb`, `SetExperience`)

## Player

- `M` — **Player entity + `GameProfile`** (skin/properties from auth or offline)
- `M` — `[partial]` **Player list** (`PlayerInfoUpdate`/`Remove`): tab entries added on join (ADD_PLAYER + game mode + listed + latency) and removed on leave. Offline profiles (no skin properties), no header/footer or display-name/list-order yet
- `M` — `[partial]` **Movement handling (serverbound)**: `MovePlayerPos/Rot/PosRot/StatusOnly` decoded and applied; `AcceptTeleportation` handled; the result is rebroadcast to other players. Server-side validation (move speed / clipping) still pending
- `M` — `[partial]` **Player actions**: `PlayerAction` (dig, breaks on STOP), `UseItemOn` (simplified block placement), `SwingArm` handled (`src/sim/packet_handlers.rs`). `UseItem`, `PlayerCommand` (sneak/sprint), `Interact`, and dig-timing/hardness pending
- `M` — **Abilities & game mode**: `PlayerAbilities`, `GameMode` change, flying/creative
- `M` — **Health / food / damage**: `SetHealth`, `DamageEvent`, death + `Respawn`, combat
- `S` — **Held-item / hotbar**: `SetCarriedItem` ↔ `SetHeldSlot`
- `M` — **Inventory state**: player inventory container, `ContainerSetContent/Slot`, pick-item

## Inventory / containers / crafting

- `L` — **Menu framework** (`AbstractContainerMenu`): slots, click handling (`ContainerClick` with all click modes), `ContainerSetData`, cursor item
- `L` — **~30 menu types**: chest, furnace family, crafting, anvil, enchanting, brewing, beacon, loom, smithing, stonecutter, merchant, etc.
- `M` — **Crafting resolution** (shaped/shapeless/special recipes) + `PlaceRecipe`
- `M` — **Enchantments** (`item/enchantment`) + **anvil/grindstone** logic
- `M` — **Trading / villager merchant** (`item/trading`, `MerchantOffers`)

## World simulation / tick loop

- `L` — `[partial]` **Server tick loop** (20 TPS scheduler in `src/sim/mod.rs`, spawned from `main.rs`); per-world ticking / tick-budget refinements pending
- `M` — **Block ticks / scheduled ticks** (`world/ticks` — random & scheduled updates)
- `L` — **Redstone** (`world/level/redstone`) — defer, large
- `M` — **Fluid flow simulation**
- `M` — **Random tick** (crop growth, leaf decay, fire spread)
- `M` — `[done]` **Day/night time + weather** (`SetTime`, rain `GameEvent`, clock — `src/sim/world_tick.rs`, tested)
- `M` — **Mob spawning** (natural spawn rules, `poi`)
- `S` — **Game rules** (`world/level/gamerules`)
- `M` — **Block events / piston & note block** (`BlockEvent`)
- `M` — **Explosions** (`Explode`) + TNT
- `M` — **Sound & particle dispatch** (`SoundEvent`/`EntityPlaySound`/`LevelParticles`, `sounds`, `core/particles`)

## Chat / commands / UI

- `L` — `[partial]` **Command system** (`commands/` — Brigadier-style tree, `arguments/`, `Commands` packet sync, `ChatCommand`/`SignedChatCommand`, suggestions): the full vanilla dedicated-server command surface (93 roots + 5 aliases) is scaffolded in one declarative table in `sim/commands.rs`, each with its permission level and a `Ready`/`Pending` status; the table drives both the advertised graph and dispatch. The literal-only graph is serialized into the join sequence (`ClientboundCommandsPacket`), `ChatCommand`/`ChatCommandSigned` are decoded (signing fields ignored), `/seed` + `/list` (+ `list uuids`) run with faithful translatable replies via `SystemChat`, and every other command replies with a "not yet implemented (needs …)" notice naming its blocking subsystem. Argument-node serialization (the `ArgumentTypeInfos` registry + parser properties), suggestions, signed-message args, redirect/alias nodes, and op/permission enforcement still pending
- `M` — **Chat** (`PlayerChat`, `SystemChat`, `DisguisedChat`, `chat` types, message signing/`ChatSession`)
- `M` — **Scoreboard / teams** (`world/scores`, `SetObjective`/`SetScore`/`SetPlayerTeam`)
- `M` — **Boss bars** (`server/bossevents`, `BossEvent`)
- `M` — **Title / actionbar / tab** (`SetTitle*`, `SetActionBarText`, `TabList`)
- `M` — **Advancements** (`advancements/`, `UpdateAdvancements`, `SelectAdvancementsTab`)
- `M` — **Statistics** (`stats`, `Award/ClientboundStats`)
- `M` — **Dialogs** (`server/dialog`, new 26.x `ShowDialog`/`ClearDialog`)
- `M` — **Maps** (`MapItemData`, map decorations)
- `S` — **Waypoints** (`server/waypoints`, new locator-bar packets)
- `M` — **Bossbar/jukebox/instrument/painting** content registries (sync only — covered above)

## Server infrastructure

- `M` — `[partial]` **Player connection manager**: accept loop + per-connection async task + (in play) a buffered reader task feeding the select loop over a channel. Per-player session state, dispatch queues, and graceful disconnect still to add
- `M` — `[done]` **`server.properties` / config loading** with backfill/rewrite (`src/config/properties.rs`, tested)
- `M` — `[partial]` **Server status**: favicon implemented (`src/net/connection.rs`); player count hardcoded 0, no player sample / secure-chat enforcement yet
- `S` — `[partial]` **Ban/whitelist/op lists**: loads/creates `ops.json`/`whitelist.json`/`banned-players.json`/`banned-ips.json`/`usercache.json` (`src/config/players.rs`); login-time enforcement still pending
- `M` — **Permissions** (`server/permissions`, op levels)
- `M` — **RCON** (`server/rcon`) — optional remote console
- `S` — **Query protocol** (legacy UDP server query) — optional
- `M` — **JSON-RPC management API** (`server/jsonrpc`) — new 26.x, optional
- `L` — **World save/load** (`world/level/storage` — `level.dat`, player data, region files)
- `S` — `[partial]` **Console / log handling, command-line args, ticking watchdog**: `tracing` logging (`RUST_LOG`) and a bind-address CLI arg in place; ticking watchdog pending
- `M` — **Datapack / tag loading** (`server/packs`, `tags`) — feed registry & tag sync
- `S` — `[partial]` **Brand & version reporting**: brand send + version reporting wired (`src/net/connection.rs`); ping debug charts (`util/debugchart`) pending

## Cross-cutting / foundational

- `M` — **Math & geometry**: `BlockPos`, `ChunkPos`, `Vec3`, AABB, direction, rotation helpers
- `S` — `[done]` **Position/angle wire encoding** (`pack_block_pos`/`unpack_block_pos` in `src/protocol/buffer.rs`, `pack_angle` in `src/sim/packets.rs`, tested)
- `M` — **Registry framework** (`core/Registry`, `Holder`, `ResourceKey`, tags) — underpins almost everything
- `M` — **DataComponent framework** (`core/component`) — the 26.x replacement for item NBT
- `S` — **Damage sources** (`world/damagesource`)
- `S` — `[partial]` **UUID / GameProfile utilities**: offline UUID done (MD5 `OfflinePlayer:<name>`, tested); property/signature handling still to add
- `M` — **Region/chunk coordinate + seed-based RNG utilities** (`util/random`, `valueproviders`)
- `XL` — **Data generation pipeline**: extract blocks/items/registries from the reference data so content isn't hand-written (clean-room: derive from observable IDs, not copied code)

---

## Deferred code-review follow-ups

Items raised in review that are larger features/refactors than the surrounding
fix and were intentionally deferred (nothing silently dropped):

- `S` — **Real tag ids for non-block/item registries** (review **M4**): the
  `entity_type`, `damage_type`, `enchantment`, `game_event`, `fluid`,
  `point_of_interest_type`, … tags in `registry/tags.rs` are bound with the right
  *names* but **empty** id lists, because Vela does not yet enumerate those
  registries' member ids. Populate them (same generator approach as block/item)
  once each registry's registration order is enumerated. The block/item tags now
  carry real ids.
- `M` — **Compression encode path efficiency** (review **F2**): reduce the ~3
  allocations per outbound packet (frame → strip → re-encode → copy) and, the
  bigger win, give each connection a reused `Deflater`/scratch buffer as vanilla
  `CompressionEncoder` does. Deferred to avoid destabilizing the hot path in this
  pass; the outbound 8 MiB guard (F1) and the malformed-frame `expect` (F4) are
  done.
- `M` — **Framer/Codec unification** (review **F3/F5**): collapse the `Option<i32>`
  threshold threading through ~10 signatures and the "sim emits framed bytes, net
  re-parses" design into a small `Framer`/`Codec` type so the sim emits `id+body`
  and net owns all framing. Broader refactor; deferred to keep this pass green.
- `M` — **Chunk batch flow control** (review): `ChunkBatchStart` /
  `ChunkBatchFinished` framing + `ServerboundChunkBatchReceived` so the client
  paces chunk delivery, around both the join stream and `stream_chunks`. New
  sub-feature, not a bug.
- `S` — **Full clock resync maps all clocks**: `world_tick`'s full sync sends only
  the overworld clock; vanilla `createFullSyncPacket` maps every dimension's clock
  (overworld + the_end). Fine for a single-dimension server; revisit when the End
  is added.
- `M` — **World store as an ECS `Resource`**: `world::store()` is a process-global
  `OnceLock<Mutex<HashMap>>` rather than a sim-`World`-owned `Resource`. Moving it
  behind a `Resource` would improve test isolation (note the far-apart test
  coords) and enable reset/unload, but cleanly threading it through the
  `chunk_columns(cx,cz)` / `level_chunk(cx,cz)` free-function API the streaming
  feature depends on (including call sites with no `World` access, e.g.
  `send_join_sequence`) is invasive. Deferred rather than risk regressing the
  streaming path.
- `S` — **Palette encode micro-optimization**: `write_block_palette` still does a
  linear `Vec::contains` / `position` per cell. A `HashMap<state,index>` was tried
  but the per-section map allocation + hashing measurably *regressed* the hot
  encode path (sections are overwhelmingly uniform/tiny-paletted), so it was
  reverted. Revisit with a reusable/scratch structure that doesn't allocate per
  section if this ever shows up in a profile.

## Suggested near-term path (matches existing M2–M5)

1. NBT codec + text components + identifiers (foundational)
2. State split (Configuration/Play) + LoginFinished/Acknowledged
3. Registry data sync + tags + feature flags → reach **Play**
4. Play `Login` packet + keep-alive + initial teleport
5. Flat/void chunk + lighting → **player stands on ground**
6. Movement in/out + entity tracking → **see yourself move / others join**

Everything else layers on after a player can join and move.
