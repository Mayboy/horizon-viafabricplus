//! Join a server of another version through the translation chain in
//! offline mode, decode everything it sends with Horizon's 26.3 decoders for
//! a while, and report packet counts and any decode failures. It also sends
//! the serverbound packets the step rewrites (teleport acceptance, mining,
//! punching, a creative slot) and runs a few commands (op the probe first).
//!
//!     cargo run -p horizon-multiversion --example join_probe -- 127.0.0.1:25565 776 [seconds]
use horizon_protocol::{
    PROTOCOL_VERSION, State,
    buf::Writer,
    connection::Connection,
    ids,
    item::{Component, Stack},
    packets::{Empty, configuration as cfg, handshake, login, play},
    registry::{Dynamic, ceil_log2},
    servers::parse_address,
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (host, port) =
        parse_address(args.get(1).map_or("127.0.0.1", String::as_str)).ok_or("bad address")?;
    let protocol: i32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(776);
    let seconds: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(20);
    let mut c = Connection::connect(&host, port, Duration::from_secs(5))?;
    let translator = horizon_multiversion::translator(protocol).ok_or("unsupported protocol")?;
    c.set_translator(translator);
    c.send(
        &handshake::Serverbound::Intention(handshake::Intention {
            protocol: PROTOCOL_VERSION,
            host: host.clone(),
            port,
            intent: handshake::LOGIN,
        })
        .encode(),
    )?;
    let name = "HorizonProbe";
    c.send(
        &login::Serverbound::Hello(login::ServerHello {
            name: name.into(),
            uuid: horizon_protocol::crypto::offline_uuid(name),
        })
        .encode(),
    )?;
    loop {
        let (id, payload) = c.recv()?;
        match login::Clientbound::decode(id, &payload)? {
            login::Clientbound::Compression(p) => c.set_compression(p.threshold),
            login::Clientbound::LoginFinished(p) => {
                println!("logged in as {}", p.profile.name);
                c.send(&login::Serverbound::LoginAcknowledged(Empty).encode())?;
                break;
            }
            login::Clientbound::Disconnect(p) => return Err(p.reason_json.into()),
            other => println!("login: {other:?}"),
        }
    }
    c.send(&cfg::Serverbound::ClientInformation(cfg::ClientInformation::new(6)).encode())?;
    let mut registries = Dynamic::default();
    let mut config_errors = 0;
    loop {
        let (id, payload) = c.recv()?;
        match cfg::Clientbound::decode(id, &payload) {
            Ok(cfg::Clientbound::SelectKnownPacks(_)) => c.send(
                &cfg::Serverbound::SelectKnownPacks(cfg::SelectKnownPacks { packs: vec![] })
                    .encode(),
            )?,
            Ok(cfg::Clientbound::RegistryData(p)) => registries.add(p),
            Ok(cfg::Clientbound::KeepAlive(p)) => c.send(&cfg::Serverbound::KeepAlive(p).encode())?,
            Ok(cfg::Clientbound::Ping(p)) => c.send(&cfg::Serverbound::Pong(p).encode())?,
            Ok(cfg::Clientbound::CodeOfConduct(_)) => {
                c.send(&cfg::Serverbound::AcceptCodeOfConduct(Empty).encode())?
            }
            Ok(cfg::Clientbound::FinishConfiguration(_)) => {
                c.send(&cfg::Serverbound::FinishConfiguration(Empty).encode())?;
                break;
            }
            Ok(cfg::Clientbound::Disconnect(p)) => return Err(format!("{:?}", p.reason).into()),
            Ok(cfg::Clientbound::UpdateTags(p)) => {
                println!("tags for {} registries", p.registries.len())
            }
            Ok(_) => {}
            Err(e) => {
                config_errors += 1;
                println!("configuration decode error in {id}: {e}");
            }
        }
    }
    for registry in ["minecraft:block_transformer", "minecraft:decorated_pot_pattern", "minecraft:trim_material", "minecraft:enchantment"] {
        println!("registry {registry}: {} entries", registries.len(registry));
    }
    let biome_bits = ceil_log2(registries.len("minecraft:worldgen/biome"));
    let mut counts: BTreeMap<&str, (u32, u32)> = BTreeMap::new();
    let mut dimension = registries.dimension(0);
    let start = Instant::now();
    c.reader.set_read_timeout(Some(Duration::from_millis(50)))?;
    let mut position: Option<[f64; 3]> = None;
    let mut step = 0u32;
    let mut last_tick = Instant::now();
    while start.elapsed() < Duration::from_secs(seconds) {
        // One client tick every 50 ms: move a little, end the tick, and run
        // the scripted actions.
        if last_tick.elapsed() >= Duration::from_millis(50)
            && let Some([x, y, z]) = position
        {
            last_tick = Instant::now();
            step += 1;
            let send = |c: &mut Connection, p: play::Serverbound| c.send(&p.encode());
            send(
                &mut c,
                play::Serverbound::MovePlayerPos(play::MovePlayerPos { x, y, z, flags: 1 }),
            )?;
            let below = horizon_protocol::buf::pack_block_pos([x.floor() as i32, y.floor() as i32 - 1, z.floor() as i32]);
            match step {
                20 => command(&mut c, "gamemode creative")?,
                25 => command(&mut c, "give @s minecraft:diamond_sword[minecraft:enchantments={sharpness:5}]")?,
                26 => command(&mut c, "give @s minecraft:goat_horn")?,
                27 => command(&mut c, "summon minecraft:falling_block ~ ~5 ~ {BlockState:{Name:\"minecraft:oak_stairs\"}}")?,
                28 => command(&mut c, "summon minecraft:item_frame ~1 ~ ~ {Item:{id:\"minecraft:shield\",count:1}}")?,
                29 => command(&mut c, "particle minecraft:block{block_state:\"minecraft:dirt\"} ~ ~2 ~ 1 1 1 0.1 20")?,
                30 => command(&mut c, "setblock ~ ~-1 ~ minecraft:decorated_pot")?,
                31 => command(&mut c, "gamemode survival")?,
                35 => {
                    // Mine the block below for a while (start, punches, stop).
                    let mut w = Writer::new();
                    w.varint(0);
                    w.i64(below);
                    w.u8(1);
                    w.varint(1);
                    raw(&mut c, ids::play::serverbound::PLAYER_ACTION, w.data)?;
                }
                36..=45 => raw(&mut c, ids::play::serverbound::PUNCH, Vec::new())?,
                46 => {
                    let mut w = Writer::new();
                    w.varint(1);
                    w.i64(below);
                    w.u8(2);
                    w.varint(2);
                    raw(&mut c, ids::play::serverbound::PLAYER_ACTION, w.data)?;
                }
                47 => {
                    let mut w = Writer::new();
                    w.varint(2);
                    w.i64(below);
                    w.u8(1);
                    w.varint(3);
                    raw(&mut c, ids::play::serverbound::PLAYER_ACTION, w.data)?;
                }
                50 => command(&mut c, "gamemode creative")?,
                55 => {
                    let mut stack = Stack::new(NETHERITE_SWORD, 1);
                    stack.added.push((horizon_protocol::item::component::DAMAGE, Component::Int(3)));
                    send(
                        &mut c,
                        play::Serverbound::SetCreativeModeSlot(play::SetCreativeModeSlot {
                            slot: 36,
                            item: Some(stack),
                        }),
                    )?;
                }
                60 => command(&mut c, "data get entity @s SelectedItem")?,
                _ => {}
            }
            c.send(&play::Serverbound::ClientTickEnd(Empty).encode())?;
        }
        let (id, payload) = match c.recv() {
            Ok(p) => p,
            Err(e) if e.0.contains("timed out") || e.0.contains("temporarily unavailable") || e.0.contains("os error 10060") => {
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        let name =
            ids::name(State::Play, horizon_protocol::Direction::Clientbound, id).unwrap_or("?");
        let entry = counts.entry(name).or_default();
        entry.0 += 1;
        match play::Clientbound::decode(id, &payload) {
            Ok(packet) => match packet {
                play::Clientbound::Login(p) => {
                    dimension = registries.dimension(p.spawn.dimension_type);
                    println!("login: entity {} in {} game mode {}", p.player_id, p.spawn.dimension, p.spawn.game_type);
                    c.send(&play::Serverbound::PlayerLoaded(Empty).encode())?;
                }
                play::Clientbound::KeepAlive(p) => c.send(&play::Serverbound::KeepAlive(p).encode())?,
                play::Clientbound::Ping(p) => c.send(&play::Serverbound::Pong(p).encode())?,
                play::Clientbound::PlayerPosition(p) => {
                    println!("position {:?} teleport {}", p.position, p.teleport);
                    position = Some(p.position);
                    let [x, y, z] = p.position;
                    c.send(
                        &play::Serverbound::AcceptTeleportation(play::AcceptTeleportation {
                            id: p.teleport,
                            x,
                            y,
                            z,
                            yaw: p.yaw,
                            pitch: p.pitch,
                        })
                        .encode(),
                    )?;
                    c.send(&play::Serverbound::ClientTickEnd(Empty).encode())?;
                }
                play::Clientbound::ChunkBatchFinished(_) => c.send(
                    &play::Serverbound::ChunkBatchReceived(play::ChunkBatchReceived {
                        chunks_per_tick: 50.0,
                    })
                    .encode(),
                )?,
                play::Clientbound::LevelChunkWithLight(p) => {
                    if let Err(e) = p.chunk.sections(dimension.sections(), 16, biome_bits) {
                        println!("chunk {} {} sections: {e}", p.x, p.z);
                        entry.1 += 1;
                    }
                }
                play::Clientbound::SystemChat(p) => println!(
                    "chat: {}",
                    horizon_protocol::text::plain(&p.content, &Default::default())
                ),
                play::Clientbound::LevelEvent(p) if counts.get("level_event").is_some_and(|c| c.0 < 30) => {
                    println!("level event {} at {:?} data {}", p.kind, p.pos, p.data)
                }
                play::Clientbound::ContainerSetSlot(p) => println!("slot {} = {:?}", p.slot, p.item.map(|s| (s.item, s.count, s.added.len()))),
                play::Clientbound::SetPlayerInventory(p) => println!("inventory {} = {:?}", p.slot, p.item.map(|s| (s.item, s.count, s.added.len()))),
                play::Clientbound::Disconnect(p) => {
                    return Err(format!("disconnected: {:?}", p.reason).into());
                }
                _ => {}
            },
            Err(e) => {
                entry.1 += 1;
                if entry.1 <= 2 {
                    println!("decode error in {name} ({} bytes): {e}", payload.len());
                }
            }
        }
    }
    let mut errors = config_errors;
    for (name, (n, e)) in &counts {
        errors += e;
        println!("{name:32} {n:6} {}", if *e > 0 { format!("{e} ERRORS") } else { String::new() });
    }
    println!("{} decode errors", errors);
    if errors > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn raw(c: &mut Connection, id: i32, data: Vec<u8>) -> Result<(), Box<dyn std::error::Error>> {
    let mut w = Writer::new();
    w.varint(id);
    w.bytes(&data);
    Ok(c.send(&w.data)?)
}

fn command(c: &mut Connection, command: &str) -> Result<(), Box<dyn std::error::Error>> {
    Ok(c.send(&play::Serverbound::ChatCommand(play::ChatCommand { command: command.into() }).encode())?)
}

/// minecraft:netherite_sword in 26.3's item registry: the probe's creative slot.
const NETHERITE_SWORD: i32 = 1055;
