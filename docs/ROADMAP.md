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

- `S` — **VarLong codec** (have VarInt; VarLong still needed for some fields)
- `S` — `[done]` **Packet framing polish**: length-prefix done; max-size guard (`MAX_FRAME_LEN`, 2 MiB) and read-side buffering (`BufReader` feeding the play loop) added
- `M` — **Compression** (`SetCompression` / zlib): threshold negotiation, compress/decompress framing once bodies grow
- `L` — **Encryption** (online mode): RSA keypair, `Hello`→`Key` exchange, AES-CFB8 stream cipher, Mojang session-server `hasJoined` auth, encrypted profile
- `M` — **Cookie store** (`cookie/` — 6 packets): request/response/store for transfers
- `M` — **Transfer / cross-server** support (`Intent::Transfer` path, `ClientboundTransfer`)
- `S` — **Plugin/custom-query channels** (login `CustomQuery`/`CustomQueryAnswer`, play `CustomPayload`)
- `M` — **Generic registry-codec layer**: a reusable encode/decode framework (the decompile's `StreamCodec`) so packets are described declaratively instead of hand-rolled per field

## NBT & data formats

- `L` — **NBT codec** (read + write, all tag types, network "nameless" variant) — needed almost everywhere downstream
- `M` — **SNBT / text-component parsing** (chat components as NBT in 26.2, not JSON)
- `M` — **Text component model**: styles, colors, translations, hover/click events, serialization to network NBT
- `S` — **Resource location / identifier** type (`namespace:path`) with validation
- `M` — `[partial]` **Bit-packed `PalettedContainer`** (block states + biomes storage, the core chunk encoding primitive): wire serialization done (single-value + 4-bit linear palette, non-spanning `SimpleBitStorage` packing) for static columns in `src/world`. No mutable container / resize / hashmap+global palette read path yet

## Login → Configuration → Play handshake

- `S` — `[done]` **`LoginFinished`** packet — real login completion (offline GameProfile, no signed properties)
- `S` — `[done]` **`LoginAcknowledged`** transition into configuration state
- `S` — `[done]` **State enum + listener split**: `Handshake/Status/Login/Configuration/Play` (play owns the split stream)
- `L` — `[partial]` **Registry data sync** (`ClientboundRegistryDataPacket`): reaches Play via **known-packs passthrough** — entry IDs sent with data absent, client fills definitions from its core pack. Full network-NBT serialization of the ~60 registries still pending (needs the NBT codec)
- `S` — `[done]` **`UpdateEnabledFeatures`** + **feature flags** (`minecraft:vanilla`)
- `S` — `[partial]` **Tags sync** (`UpdateTags`): packet implemented; all required tags bound **empty** to satisfy the client's presence check. Real tag contents pending
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
- `XL` — `[partial]` **Chunk serialization** (`ClientboundLevelChunkWithLight`): packet wire format implemented for a **static flat column** (bedrock/dirt/grass via single-value + 4-bit linear block palettes, single-value biome, real `WORLD_SURFACE`/`MOTION_BLOCKING` heightmaps, empty light). Block-state ids derived from the server's own `--reports` dump. Per-chunk variation, block entities, and dynamic edits pending
- `M` — `[partial]` **Light engine**: empty-light payload sent (four empty BitSets + two empty arrays); no real sky/block light propagation yet
- `M` — `[partial]` **Chunk streaming**: `SetChunkCacheCenter` + a fixed view-radius batch of chunks sent on join. Dynamic load/unload by movement and `ForgetLevelChunk` pending
- `M` — `[partial]` **Heightmaps** computation & maintenance: flat-profile `WORLD_SURFACE`/`MOTION_BLOCKING` computed and sent; live recomputation on block change pending
- `S` — **Block-change packets**: `BlockUpdate`, `SectionBlocksUpdate`
- `M` — **Block entities** (`BlockEntityData`, chests/signs/etc. NBT) — model + per-type data
- `M` — **Region / `.mca` persistence** (Anvil format) — or start with in-memory only and defer
- `L` — `[partial]` **World generation**: the **void generator** (`S`) is effectively in place (server streams all-air columns). Real `levelgen` (noise, biomes, carvers, features, structures) is `XL` and still out of early scope
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
- `L` — **Entity spawn/remove/track**: `AddEntity`, `RemoveEntities`, per-player entity tracker with view distance
- `M` — **Entity movement packets**: `MoveEntityPos/Rot/PosRot`, `TeleportEntity`, `EntityVelocity`, `SetEntityMotion`, `HeadRotation`
- `M` — **Entity events / status / animations**: `EntityEvent`, `Animate`, `HurtAnimation`, `TakeItemEntity`
- `M` — **Equipment & passengers**: `SetEquipment`, `SetPassengers`, `SetEntityLink` (leash)
- `M` — **Attributes** (`world/attribute` — `UpdateAttributes`)
- `M` — **Mob effects / potions** (`world/effect`, `UpdateMobEffect`/`RemoveMobEffect`)
- `XL` — **Entity AI / brain / pathfinding** (`entity/ai`, `pathfinder`) — large, defer; needed for living mobs to behave
- `L` — **Projectiles, vehicles, item entities, decorations** (per-family behavior)
- `S` — **Experience orbs / XP** (`AddExperienceOrb`, `SetExperience`)

## Player

- `M` — **Player entity + `GameProfile`** (skin/properties from auth or offline)
- `M` — **Player list** (`PlayerInfoUpdate`/`Remove`, tab list header/footer)
- `M` — **Movement handling (serverbound)**: `MovePlayerPos/Rot/PosRot/StatusOnly`, validation, `AcceptTeleportation`
- `M` — **Player actions**: `PlayerAction` (dig), `UseItem`, `UseItemOn`, `SwingArm`, `PlayerCommand` (sneak/sprint), `Interact`
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

- `L` — **Server tick loop** (20 TPS scheduler, per-world ticking, tick budget)
- `M` — **Block ticks / scheduled ticks** (`world/ticks` — random & scheduled updates)
- `L` — **Redstone** (`world/level/redstone`) — defer, large
- `M` — **Fluid flow simulation**
- `M` — **Random tick** (crop growth, leaf decay, fire spread)
- `M` — **Day/night time + weather** (`SetTime`, `GameEvent` rain, `world/clock`)
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
- `M` — **`server.properties` / config loading** (`server/dedicated`)
- `M` — **Server status with player sample, favicon, secure-chat enforcement**
- `S` — **Ban/whitelist/op lists** (`server/players`, JSON files)
- `M` — **Permissions** (`server/permissions`, op levels)
- `M` — **RCON** (`server/rcon`) — optional remote console
- `S` — **Query protocol** (legacy UDP server query) — optional
- `M` — **JSON-RPC management API** (`server/jsonrpc`) — new 26.x, optional
- `L` — **World save/load** (`world/level/storage` — `level.dat`, player data, region files)
- `S` — `[partial]` **Console / log handling, command-line args, ticking watchdog**: `tracing` logging (`RUST_LOG`) and a bind-address CLI arg in place; ticking watchdog pending
- `M` — **Datapack / tag loading** (`server/packs`, `tags`) — feed registry & tag sync
- `S` — **Brand & version reporting, ping debug charts** (`util/debugchart`)

## Cross-cutting / foundational

- `M` — **Math & geometry**: `BlockPos`, `ChunkPos`, `Vec3`, AABB, direction, rotation helpers
- `S` — **Position/angle wire encoding** (packed long positions, byte angles)
- `M` — **Registry framework** (`core/Registry`, `Holder`, `ResourceKey`, tags) — underpins almost everything
- `M` — **DataComponent framework** (`core/component`) — the 26.x replacement for item NBT
- `S` — **Damage sources** (`world/damagesource`)
- `S` — `[partial]` **UUID / GameProfile utilities**: offline UUID done (MD5 `OfflinePlayer:<name>`, tested); property/signature handling still to add
- `M` — **Region/chunk coordinate + seed-based RNG utilities** (`util/random`, `valueproviders`)
- `XL` — **Data generation pipeline**: extract blocks/items/registries from the reference data so content isn't hand-written (clean-room: derive from observable IDs, not copied code)

---

## Suggested near-term path (matches existing M2–M5)

1. NBT codec + text components + identifiers (foundational)
2. State split (Configuration/Play) + LoginFinished/Acknowledged
3. Registry data sync + tags + feature flags → reach **Play**
4. Play `Login` packet + keep-alive + initial teleport
5. Flat/void chunk + lighting → **player stands on ground**
6. Movement in/out + entity tracking → **see yourself move / others join**

Everything else layers on after a player can join and move.
