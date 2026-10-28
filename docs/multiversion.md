# Multiversion: joining servers of other Minecraft versions

This fork adds ViaFabricPlus-style multi-version support to Horizon (Minecraft 26.3, protocol 777):
the client can join servers running other versions by translating every packet between the server's
protocol and 26.3. It ports ViaVersion's approach, the protocol chain that ViaFabricPlus is built on,
to Horizon's engine. It can't be a Horizon mod, because the mod sandbox has no network access.

> **Licence.** The translation code follows ViaVersion's (GPLv3) protocol steps, so this fork is a
> derivative of GPLv3 code. Keep it private, or distribute it under GPLv3. Upstream Horizon (MIT, with
> an "only original code" rule) won't accept it.

## Status

| Server version | Protocol | State |
|---|---|---|
| 26.3 | 777 | native |
| 26.2 | 776 | translated; verified against a real 26.2 vanilla server (see Testing) |
| 26.1 and older | | not yet: add one step per version (see below) |

What ViaFabricPlus has that this doesn't, yet:

- Steps older than 26.2. ViaVersion has about 60 of them back to 1.8, plus ViaLegacy for older.
- Client behaviour emulation: old movement, collision shapes, hitboxes and combat rules.
- Classic/Beta authentication.

## Using it

- **Edit Server** has a **Protocol** button, which cycles Auto → 26.3 → 26.2 → …. The choice is saved per
  address in `horizon.txt` (`protocolVersion.<address>`), so `servers.dat` stays vanilla's.
- **Auto** (the default) pings the server first. When its version is one Horizon can join (other than
  its own), the connection is translated. Direct Connection always uses Auto.
- The connect screen shows "Connecting as 26.2..." when a translation is used.

## How it works

```
server bytes -> frame/decompress -> (id, payload) -> Pipeline (steps, server side first) -> 26.3 (id, payload) -> Horizon's decoders
                                                 <- Pipeline (steps, client side first) <- Horizon's encoders
```

- `crates/horizon-protocol/src/translate.rs`: the `Translator` hook. `ConnReader::recv` and
  `ConnWriter::send` run every packet body through it, after decompression and before encryption.
  `Connection::set_translator` turns it on.
- `crates/horizon-multiversion`:
  - `lib.rs`: the version list (`VERSIONS`), `translator(protocol)`, and the `Pipeline`. The pipeline
    tracks the connection state of each direction, rewrites the handshake's protocol number, and
    runs the chain of steps. A step can drop or split packets, and can answer for the other side:
    `Out::to_client` / `Out::to_server`.
  - `wire.rs`: `Rw`, a field-by-field rewriter (ViaVersion's PacketWrapper), plus id-table helpers.
  - `v26_2/`: the 26.2 → 26.3 step (ViaVersion's `Protocol26_2To26_3`):
    - `tables.rs` (generated): packet ids per state, registry ids (items, blocks, block states,
      entities, sounds, particles, data components, stats, argument types), and the tags only 26.3 has.
    - `item.rs`: item stacks and every data component, both directions. This covers the components
      that changed (swing animation split, map colour removed, trims, instruments, pot decorations,
      consume effects) and drops the 26.3-only ones on the way to the server.
    - `world.rs`: chunks (palette remap; global-palette sections repacked from 15 to 16 bits), light
      and other BitSets (long arrays → byte arrays), particles, explosions, commands, recipe displays,
      recipes, advancements.
    - `entity.rs`: entity types, movement packets (26.3's interpolation steps), arm swings,
      spawn-info game modes, entity data.
    - `config.rs`: registry data (trim materials, enchantment terms), tags, and the
      `decorated_pot_pattern` / `block_transformer` registries 26.3 expects from the server.
    - `mod.rs`: packet dispatch, plus the serverbound side (teleport acceptance, punch → swing,
      mining progress, signs, container clicks, creative slots).
- `crates/horizon-client/src/multiversion.rs`: the per-server choice and auto-detect.
  `net.rs` attaches the translator when it connects.

## Adding an older version

Each step bridges two adjacent versions. To add, say, 26.1 (protocol 775):

1. Run the data generator for both versions:
   `java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --reports`.
2. Generate the tables:
   `python3 tools/multiversion/gen_step_tables.py <26.1 dir> <26.2 dir> crates/horizon-multiversion/src/v26_1/tables.rs`
3. Write `v26_1/` from ViaVersion's `Protocol26_1To26_2`. Clientbound means 26.1 → 26.2 and
   serverbound means 26.2 → 26.1. Use `v26_2` as the template.
4. In `lib.rs`, add the version to `VERSIONS` and to `steps_to`. Older steps go in front: 775 runs
   `[v26_1, v26_2]`.
5. Test it with the probe (below).

## Testing

- Unit tests: `cargo test -p horizon-multiversion`.
- Live: start a vanilla server of the target version (offline mode), op `HorizonProbe`, then run:

  ```
  cargo run -p horizon-multiversion --example join_probe -- 127.0.0.1:25565 776 20
  ```

  The probe logs in through the translator and decodes every packet with Horizon's own decoders. It
  sends the rewritten serverbound packets (teleport, mining, punches, a creative slot) and runs some
  commands, then prints packet counts and decode errors. Against 26.2.0 it reports 0 decode errors,
  and the server accepts everything it sends.
