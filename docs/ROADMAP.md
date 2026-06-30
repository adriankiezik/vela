# Vela — Porting Roadmap

What needs to be reimplemented (clean-room, from the wire protocol + observed
behavior) to go from "reaches login" to a functional 26.2 server. Reference
decompile lives at `C:\Users\kiezi\mc-decompile\src-server`.

**Scope reference (decompiled 26.2):** 194 protocol/game files (58 serverbound,
127 clientbound), 27 common + 14 configuration + 19 login packets, ~60 synced
registries, ~30 inventory menus.

**Size key:** `S` ≈ hours · `M` ≈ a day or two · `L` ≈ several days · `XL` ≈ a
week+ / ongoing. Unordered — pick by dependency, not list order.

---

## Network / transport layer

- `S` — **VarLong codec** (have VarInt; VarLong still needed for some fields)
- `S` — **Packet framing polish**: length-prefix already done; add max-size guards, partial-read buffering on the read side
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
- `M` — **Bit-packed `PalettedContainer`** (block states + biomes storage, the core chunk encoding primitive)

## Login → Configuration → Play handshake

- `S` — **`LoginFinished`** packet (replace the current "greet & disconnect" with real login completion)
- `S` — **`LoginAcknowledged`** transition into configuration state
- `S` — **State enum + listener split**: add `Configuration` and `Play` states (currently only Handshake/Status/Login)
- `L` — **Registry data sync** (`ClientboundRegistryDataPacket`): serialize the ~60 synced registries (dimension types, biomes, chat types, wolf/cat/cow/… variants, damage types, painting/banner/trim, etc.) as network NBT — the client refuses to join without these
- `S` — **`UpdateEnabledFeatures`** + **feature flags** (`world/flag`)
- `S` — **Tags sync** (`UpdateTags` — block/item/fluid/entity tags)
- `S` — **Known packs negotiation** (`SelectKnownPacks`, vanilla core datapack)
- `S` — **Code-of-conduct** packets (new in 26.x: `CodeOfConduct` / `AcceptCodeOfConduct`)
- `S` — **`FinishConfiguration`** ↔ `ServerboundFinishConfiguration` handoff to Play
- `S` — **Client information / brand** (`ClientInformation`, `CustomPayload` brand)
- `S` — **Resource-pack push/pop** packets (common)

## Play: join & keep-alive

- `M` — **`Login` (play) packet**: dimension list, spawn dimension, game mode, view distance, etc. — the big "join game" packet
- `S` — **Keep-alive loop** (clientbound ping + serverbound echo, timeout disconnect)
- `S` — **`Disconnect`** (play) + **`Ping`/`Pong`** (common)
- `S` — **Set default spawn / `PlayerPosition`** (initial teleport + confirm handshake)
- `S` — **Game-event packet** (`ClientboundGameEvent`: e.g. "start waiting for chunks")
- `S` — **Server links / `ServerData`** (MOTD/icon in-game)

## World representation & chunks

- `L` — **Block-state model**: `Block` registry, `BlockState` with properties, global palette IDs
- `L` — **Chunk data structures**: `LevelChunk`, 16×16×16 sections, heightmaps, biome storage
- `XL` — **Chunk serialization** (`ClientboundLevelChunkWithLight`): paletted block/biome data, block entities, lighting payload — the single most important packet to render a world
- `M` — **Light engine** (sky + block light propagation, `LightSection`, `ClientboundLightUpdate`)
- `M` — **Chunk streaming**: load/unload around players by view distance, `ForgetLevelChunk`, `SetChunkCacheCenter`/`Radius`
- `M` — **Heightmaps** computation & maintenance
- `S` — **Block-change packets**: `BlockUpdate`, `SectionBlocksUpdate`
- `M` — **Block entities** (`BlockEntityData`, chests/signs/etc. NBT) — model + per-type data
- `M` — **Region / `.mca` persistence** (Anvil format) — or start with in-memory only and defer
- `L` — **World generation**: at minimum a **flat / void generator** (`S`), then real `levelgen` (noise, biomes, carvers, features, structures) is `XL` and probably out of early scope
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

- `L` — **Command system** (`commands/` — Brigadier-style tree, `arguments/`, `Commands` packet sync, `ChatCommand`/`SignedChatCommand`, suggestions)
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

- `M` — **Player connection manager**: accept loop already exists; add per-player session, packet dispatch queues, graceful disconnect
- `M` — **`server.properties` / config loading** (`server/dedicated`)
- `M` — **Server status with player sample, favicon, secure-chat enforcement**
- `S` — **Ban/whitelist/op lists** (`server/players`, JSON files)
- `M` — **Permissions** (`server/permissions`, op levels)
- `M` — **RCON** (`server/rcon`) — optional remote console
- `S` — **Query protocol** (legacy UDP server query) — optional
- `M` — **JSON-RPC management API** (`server/jsonrpc`) — new 26.x, optional
- `L` — **World save/load** (`world/level/storage` — `level.dat`, player data, region files)
- `S` — **Console / log handling, command-line args, ticking watchdog**
- `M` — **Datapack / tag loading** (`server/packs`, `tags`) — feed registry & tag sync
- `S` — **Brand & version reporting, ping debug charts** (`util/debugchart`)

## Cross-cutting / foundational

- `M` — **Math & geometry**: `BlockPos`, `ChunkPos`, `Vec3`, AABB, direction, rotation helpers
- `S` — **Position/angle wire encoding** (packed long positions, byte angles)
- `M` — **Registry framework** (`core/Registry`, `Holder`, `ResourceKey`, tags) — underpins almost everything
- `M` — **DataComponent framework** (`core/component`) — the 26.x replacement for item NBT
- `S` — **Damage sources** (`world/damagesource`)
- `S` — **UUID / GameProfile utilities** (have offline UUID; add property/signature handling)
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
