//! 26.2 (protocol 776) servers for 26.3 clients: ViaVersion's
//! Protocol26_2To26_3, rewritten for Horizon. Ids come from `tables`
//! (generated from both versions' data generator reports); `item`, `world`,
//! `entity` and `config` hold the packets whose layout changed.
use crate::{Out, wire::*};
use horizon_protocol::{Result, State, translate::Packet};
use std::sync::OnceLock;

mod config;
mod entity;
mod item;
mod tables;
mod world;

use tables::packets::{
    configuration_clientbound as cfg_cb, configuration_serverbound as cfg_sb,
    play_clientbound as cb, play_serverbound as sb,
};

/// Which way a value goes: Up from the 26.2 server to the 26.3 client,
/// Down from the client to the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
}

/// The new -> old tables.
struct Inverse {
    items: Vec<i32>,
    blocks: Vec<i32>,
    states: Vec<i32>,
    entities: Vec<i32>,
    sounds: Vec<i32>,
    particles: Vec<i32>,
    stats: Vec<i32>,
    arguments: Vec<i32>,
    /// Data component types by name, both ways (`None`: the other version
    /// lacks it).
    components_up: Vec<Option<i32>>,
    components_down: Vec<Option<i32>>,
}
fn inverse() -> &'static Inverse {
    static INVERSE: OnceLock<Inverse> = OnceLock::new();
    INVERSE.get_or_init(|| {
        use tables::registry as reg;
        let by_name = |from: &[&str], to: &[&str]| {
            from.iter()
                .map(|n| to.iter().position(|m| m == n).map(|i| i as i32))
                .collect()
        };
        let (old, new) = (&tables::component::old::NAMES, &tables::component::new::NAMES);
        Inverse {
            items: invert(&reg::ITEMS, reg::ITEMS_NEW_LEN, tables::item::old::STONE),
            blocks: invert(&reg::BLOCKS, reg::BLOCKS_NEW_LEN, 0),
            states: invert(&tables::BLOCK_STATES, tables::BLOCK_STATES_NEW_LEN, 0),
            entities: invert(&reg::ENTITY_TYPES, reg::ENTITY_TYPES_NEW_LEN, tables::entity::old::PIG),
            sounds: invert(&reg::SOUNDS, reg::SOUNDS_NEW_LEN, 0),
            particles: invert(&reg::PARTICLES, reg::PARTICLES_NEW_LEN, 0),
            stats: invert(&reg::CUSTOM_STATS, reg::CUSTOM_STATS_NEW_LEN, 0),
            arguments: invert(&reg::ARGUMENT_TYPES, reg::ARGUMENT_TYPES_NEW_LEN, 5),
            components_up: by_name(old, new),
            components_down: by_name(new, old),
        }
    })
}

pub fn item(d: Dir, id: i32) -> i32 {
    match d {
        Dir::Up => up(&tables::registry::ITEMS, id),
        Dir::Down => down(&inverse().items, id, tables::item::old::STONE),
    }
}
pub fn block(d: Dir, id: i32) -> i32 {
    match d {
        Dir::Up => up(&tables::registry::BLOCKS, id),
        Dir::Down => down(&inverse().blocks, id, 0),
    }
}
pub fn state(d: Dir, id: i32) -> i32 {
    match d {
        Dir::Up => up(&tables::BLOCK_STATES, id),
        Dir::Down => down(&inverse().states, id, 0),
    }
}
pub fn entity(d: Dir, id: i32) -> i32 {
    match d {
        Dir::Up => up(&tables::registry::ENTITY_TYPES, id),
        Dir::Down => down(&inverse().entities, id, tables::entity::old::PIG),
    }
}
pub fn sound(d: Dir, id: i32) -> i32 {
    match d {
        Dir::Up => up(&tables::registry::SOUNDS, id),
        Dir::Down => down(&inverse().sounds, id, 0),
    }
}
pub fn particle(d: Dir, id: i32) -> i32 {
    match d {
        Dir::Up => up(&tables::registry::PARTICLES, id),
        Dir::Down => down(&inverse().particles, id, 0),
    }
}
pub fn custom_stat(d: Dir, id: i32) -> i32 {
    match d {
        Dir::Up => up(&tables::registry::CUSTOM_STATS, id),
        Dir::Down => down(&inverse().stats, id, 0),
    }
}
pub fn argument_type(d: Dir, id: i32) -> i32 {
    match d {
        Dir::Up => up(&tables::registry::ARGUMENT_TYPES, id),
        Dir::Down => down(&inverse().arguments, id, 5),
    }
}
/// A data component type in the other version, if it has it.
pub fn component(d: Dir, id: i32) -> Option<i32> {
    let table = match d {
        Dir::Up => &inverse().components_up,
        Dir::Down => &inverse().components_down,
    };
    usize::try_from(id).ok().and_then(|i| table.get(i)).copied().flatten()
}

/// The destroy-progress bookkeeping 26.3 clients leave to the server
/// (ViaVersion's ProtocolStorables26_3).
#[derive(Default)]
struct Destroy {
    position: Option<i64>,
    direction: i32,
    ticks: i32,
    punched: bool,
}
impl Destroy {
    /// The destroy tick that just ended, if the client kept destroying in it.
    fn end_tick(&mut self) -> Option<i32> {
        self.position?;
        if !self.punched {
            self.position = None;
            return None;
        }
        self.punched = false;
        self.ticks += 1;
        Some(self.ticks - 1)
    }
}

#[derive(Default)]
pub struct Step {
    destroy: Destroy,
}

/// LevelEvent ids the step sends itself.
const SPLASH_POTION_SOUND: i32 = 1053;
const INSTANT_SPLASH_POTION_SOUND: i32 = 1054;
const SPLASH_POTION_PARTICLES: i32 = 2002;
const INSTANT_SPLASH_POTION_PARTICLES: i32 = 2007;
const DESTROY_PROGRESS_PARTICLES: i32 = 2019;
const DESTROY_PROGRESS_PARTICLES_AND_SOUND: i32 = 2020;
const BLOCK_BREAK_PARTICLES: i32 = 2001;
const BREWING_STAND_MENU: i32 = 11;

fn mapped(table: &[i16], id: i32) -> Option<i32> {
    usize::try_from(id)
        .ok()
        .and_then(|i| table.get(i))
        .map(|&v| i32::from(v))
        .filter(|&v| v >= 0)
}

impl crate::Step for Step {
    fn clientbound(&mut self, state: State, packet: Packet, out: &mut Out) -> Result<()> {
        match state {
            State::Configuration => config::clientbound(packet, out),
            State::Play => self.play_clientbound(packet, out),
            _ => {
                out.packets.push(packet);
                Ok(())
            }
        }
    }

    fn serverbound(&mut self, state: State, packet: Packet, out: &mut Out) -> Result<()> {
        match state {
            State::Configuration => {
                if let Some(id) = mapped(&cfg_sb::MAP, packet.id) {
                    out.push(id, packet.data);
                }
                Ok(())
            }
            State::Play => self.play_serverbound(packet, out),
            _ => {
                out.packets.push(packet);
                Ok(())
            }
        }
    }
}

impl Step {
    fn play_clientbound(&mut self, packet: Packet, out: &mut Out) -> Result<()> {
        use cb::{new as n, old as o};
        let Some(id) = mapped(&cb::MAP, packet.id) else {
            return Ok(());
        };
        let d = Dir::Up;
        let mut rw = Rw::new(&packet.data);
        match packet.id {
            o::ADD_ENTITY => entity::add_entity(&mut rw)?,
            o::ANIMATE => return entity::animate(&packet.data, out),
            o::AWARD_STATS => {
                rw.list(|rw| {
                    let kind = rw.varint()?;
                    rw.map_varint(|v| stat_value(d, kind, v))?;
                    rw.varint().map(drop)
                })?;
            }
            o::BLOCK_ENTITY_DATA => {
                rw.pos()?;
                rw.varint()?;
                let tag = rw.r.nbt()?;
                rw.w.nbt(&world::block_entity(tag));
            }
            o::BLOCK_EVENT => {
                rw.pos()?;
                rw.u8()?;
                rw.u8()?;
                rw.map_varint(|b| block(d, b))?;
            }
            o::BLOCK_UPDATE => {
                rw.pos()?;
                rw.map_varint(|s| state(d, s))?;
            }
            o::COMMANDS => world::commands(&mut rw)?,
            o::CONTAINER_SET_CONTENT => {
                rw.varint()?;
                rw.varint()?;
                rw.list(|rw| item::slot(&mut rw.r, &mut rw.w, d))?;
                item::slot(&mut rw.r, &mut rw.w, d)?;
            }
            o::CONTAINER_SET_SLOT => {
                rw.varint()?;
                rw.varint()?;
                rw.i16()?;
                item::slot(&mut rw.r, &mut rw.w, d)?;
            }
            o::ENTITY_POSITION_SYNC => entity::position_sync(&mut rw)?,
            o::EXPLODE => {
                world::explode(&mut rw)?;
                rw.w.bool(true);
            }
            o::LEVEL_CHUNK_WITH_LIGHT => {
                rw.i32()?;
                rw.i32()?;
                world::chunk(&mut rw)?;
                world::light(&mut rw)?;
            }
            o::LIGHT_UPDATE => {
                rw.varint()?;
                rw.varint()?;
                world::light(&mut rw)?;
            }
            o::PLAYER_CHAT => {
                rw.varint()?;
                rw.uuid()?;
                rw.varint()?;
                rw.optional(|rw| rw.pass(|r| r.bytes(256).map(drop)))?;
                rw.pass(|r| r.string(256).map(drop))?;
                rw.i64()?;
                rw.i64()?;
                rw.list(|rw| {
                    if rw.varint()? == 0 {
                        rw.pass(|r| r.bytes(256).map(drop))?;
                    }
                    Ok(())
                })?;
                rw.optional(|rw| rw.nbt().map(drop))?;
                // The filter mask's BitSet (partially filtered).
                if rw.varint()? == 2 {
                    world::bit_set(&mut rw)?;
                }
            }
            o::LEVEL_EVENT => {
                let event = rw.i32()?;
                let pos = rw.pos()?;
                if event == BLOCK_BREAK_PARTICLES {
                    let s = rw.r.i32()?;
                    rw.w.i32(state(d, s));
                }
                let body = rw.finish();
                out.push(id, body);
                // 26.3 plays the splash sound as its own event.
                let sound = match event {
                    SPLASH_POTION_PARTICLES => SPLASH_POTION_SOUND,
                    INSTANT_SPLASH_POTION_PARTICLES => INSTANT_SPLASH_POTION_SOUND,
                    _ => return Ok(()),
                };
                out.push(id, level_event(sound, pos, 0));
                return Ok(());
            }
            o::LEVEL_PARTICLES => world::level_particles(&mut rw)?,
            o::LOGIN => entity::login(&mut rw)?,
            o::RESPAWN => entity::spawn_info(&mut rw)?,
            o::MERCHANT_OFFERS => {
                rw.varint()?;
                rw.list(|rw| {
                    let (r, w) = (&mut rw.r, &mut rw.w);
                    item_cost(r, w, d)?;
                    item::slot(r, w, d)?;
                    item::optional(r, w, d, item_cost)?;
                    // Out of stock, uses, max uses, xp, special price,
                    // price multiplier, demand.
                    rw.bool()?;
                    (0..4).try_for_each(|_| rw.i32().map(drop))?;
                    rw.f32()?;
                    rw.i32().map(drop)
                })?;
            }
            o::MOVE_ENTITY_POS => entity::move_pos(&mut rw, false)?,
            o::MOVE_ENTITY_POS_ROT => entity::move_pos(&mut rw, true)?,
            o::MOVE_ENTITY_ROT => entity::move_rot(&mut rw)?,
            o::OPEN_SCREEN => {
                let container = rw.varint()?;
                let menu = rw.varint()?;
                out.push(id, rw.finish());
                if menu == BREWING_STAND_MENU {
                    // 26.3 takes the total brew time and fuel from the server.
                    for (slot, value) in [(2i16, 400i16), (3, 20)] {
                        let mut w = horizon_protocol::buf::Writer::new();
                        w.varint(container);
                        w.i16(slot);
                        w.i16(value);
                        out.push(n::CONTAINER_SET_DATA, w.data);
                    }
                }
                return Ok(());
            }
            o::OPEN_SIGN_EDITOR => {
                rw.pos()?;
                let front = rw.r.bool()?;
                rw.w.varint(i32::from(front));
            }
            o::PLACE_GHOST_RECIPE => {
                rw.varint()?;
                world::recipe_display(&mut rw.r, &mut rw.w)?;
            }
            o::RECIPE_BOOK_ADD => world::recipe_book_add(&mut rw)?,
            o::SECTION_BLOCKS_UPDATE => {
                rw.i64()?;
                rw.list(|rw| {
                    let v = rw.r.varlong()?;
                    let s = state(d, (v >> 12) as i32);
                    rw.w.varlong(i64::from(s) << 12 | (v & 0xFFF));
                    Ok(())
                })?;
            }
            o::SET_CURSOR_ITEM => item::slot(&mut rw.r, &mut rw.w, d)?,
            o::SET_ENTITY_DATA => {
                rw.varint()?;
                entity::entity_data(&mut rw)?;
            }
            o::SET_EQUIPMENT => {
                rw.varint()?;
                loop {
                    let slot = rw.u8()?;
                    item::slot(&mut rw.r, &mut rw.w, d)?;
                    if slot & 0x80 == 0 {
                        break;
                    }
                }
            }
            o::SET_PLAYER_INVENTORY => {
                rw.varint()?;
                item::slot(&mut rw.r, &mut rw.w, d)?;
            }
            o::SOUND | o::SOUND_ENTITY => item::sound_holder(&mut rw.r, &mut rw.w, d)?,
            o::UPDATE_ADVANCEMENTS => world::advancements(&mut rw)?,
            o::UPDATE_RECIPES => world::update_recipes(&mut rw)?,
            o::UPDATE_TAGS => return config::update_tags(id, &packet.data, out),
            _ => {}
        }
        out.push(id, rw.finish());
        Ok(())
    }

    fn play_serverbound(&mut self, packet: Packet, out: &mut Out) -> Result<()> {
        use sb::{new as n, old as o};
        let d = Dir::Down;
        let mut rw = Rw::new(&packet.data);
        let id = match packet.id {
            n::ACCEPT_TELEPORTATION => {
                // 26.3 sends the position it moved to; 26.2 clients sent a
                // movement packet right after instead.
                rw.varint()?;
                let (x, y, z) = (rw.r.f64()?, rw.r.f64()?, rw.r.f64()?);
                let (yaw, pitch) = (rw.r.f32()?, rw.r.f32()?);
                out.push(o::ACCEPT_TELEPORTATION, rw.written());
                let mut w = horizon_protocol::buf::Writer::new();
                w.f64(x);
                w.f64(y);
                w.f64(z);
                w.f32(yaw);
                w.f32(pitch);
                w.u8(0);
                out.push(o::MOVE_PLAYER_POS_ROT, w.data);
                return Ok(());
            }
            // Punching a block every tick it is mined: 26.2 knew it as a swing.
            n::PUNCH => {
                self.destroy.punched = true;
                rw.w.varint(0);
                o::SWING
            }
            n::CLIENT_TICK_END => {
                // 26.3 shows the destroy progress only as the server reports it.
                if let (Some(tick), Some(pos)) = (self.destroy.end_tick(), self.destroy.position) {
                    let event = if tick % 4 == 0 {
                        DESTROY_PROGRESS_PARTICLES_AND_SOUND
                    } else {
                        DESTROY_PROGRESS_PARTICLES
                    };
                    let body = level_event(event, pos, self.destroy.direction);
                    out.to_client
                        .push(Packet::new(cb::new::LEVEL_EVENT, body));
                }
                o::CLIENT_TICK_END
            }
            n::PLAYER_ACTION => {
                const START: i32 = 0;
                const CHANGE_DIRECTION: i32 = 1;
                const ABORT: i32 = 2;
                const STOP: i32 = 3;
                let action = rw.r.varint()?;
                if action == CHANGE_DIRECTION {
                    rw.r.i64()?;
                    self.destroy.direction = i32::from(rw.r.u8()?);
                    return Ok(());
                }
                rw.w.varint(if action > CHANGE_DIRECTION { action - 1 } else { action });
                if action == START {
                    let pos = rw.pos()?;
                    let direction = rw.u8()?;
                    self.destroy = Destroy {
                        position: Some(pos),
                        direction: i32::from(direction),
                        ticks: 0,
                        punched: false,
                    };
                } else if action == ABORT || action == STOP {
                    self.destroy.position = None;
                }
                o::PLAYER_ACTION
            }
            n::SIGN_UPDATE => {
                rw.pos()?;
                let lines: Vec<String> = (0..4).map(|_| rw.r.string(384)).collect::<Result<_>>()?;
                let front = rw.r.varint()? == 1;
                rw.w.bool(front);
                lines.iter().for_each(|l| rw.w.string(l));
                o::SIGN_UPDATE
            }
            n::CONTAINER_CLICK => {
                rw.varint()?;
                rw.varint()?;
                rw.i16()?;
                rw.i8()?;
                rw.varint()?;
                rw.list(|rw| {
                    rw.i16()?;
                    item::hashed(&mut rw.r, &mut rw.w, d)
                })?;
                item::hashed(&mut rw.r, &mut rw.w, d)?;
                o::CONTAINER_CLICK
            }
            n::SET_CREATIVE_MODE_SLOT => {
                rw.i16()?;
                // The server reads ItemStack.OPTIONAL_UNTRUSTED_STREAM_CODEC:
                // each component value behind its length.
                item::slot_delimited(&mut rw.r, &mut rw.w, d)?;
                o::SET_CREATIVE_MODE_SLOT
            }
            other => match mapped(&sb::MAP, other) {
                Some(id) => id,
                None => return Ok(()),
            },
        };
        out.push(id, rw.finish());
        Ok(())
    }
}

/// A ClientboundLevelEventPacket body (not global).
fn level_event(event: i32, pos: i64, data: i32) -> Vec<u8> {
    let mut w = horizon_protocol::buf::Writer::new();
    w.i32(event);
    w.i64(pos);
    w.i32(data);
    w.bool(false);
    w.data
}

/// A stat's value id by its stat type (`minecraft:stat_type` order: mined,
/// crafted, used, broken, picked_up, dropped, killed, killed_by, custom).
fn stat_value(d: Dir, kind: i32, value: i32) -> i32 {
    match kind {
        0 => block(d, value),
        1..=5 => item(d, value),
        6 | 7 => entity(d, value),
        8 => custom_stat(d, value),
        _ => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_protocol::buf::{Reader, Writer};
    use tables::component::{new as cn, old as co};

    fn up_slot(bytes: &[u8]) -> Vec<u8> {
        let mut w = Writer::new();
        item::slot(&mut Reader::new(bytes), &mut w, Dir::Up).unwrap();
        w.data
    }

    /// A stack's item and component ids move to 26.3's; map_color goes and
    /// swing_animation becomes the attack and interact animations.
    #[test]
    fn stacks_upgrade() {
        let mut w = Writer::new();
        w.varint(2);
        w.varint(tables::item::old::DIAMOND_SWORD);
        w.varint(3);
        w.varint(1);
        w.varint(co::DAMAGE);
        w.varint(5);
        w.varint(co::MAP_COLOR);
        w.i32(0xff00ff);
        w.varint(co::SWING_ANIMATION);
        w.varint(2);
        w.varint(8);
        w.varint(co::MAP_ID);
        let mut r_ = Writer::new();
        r_.varint(2);
        r_.varint(tables::item::new::DIAMOND_SWORD);
        r_.varint(3);
        r_.varint(1);
        r_.varint(cn::DAMAGE);
        r_.varint(5);
        for id in [cn::ATTACK_ANIMATION, cn::INTERACT_ANIMATION] {
            r_.varint(id);
            r_.varint(2);
            r_.varint(8);
        }
        r_.varint(cn::MAP_ID);
        assert_eq!(up_slot(&w.data), r_.data);
    }

    /// Down again, the two animations are one swing animation and 26.3-only
    /// components are dropped.
    #[test]
    fn stacks_downgrade() {
        let mut w = Writer::new();
        w.varint(1);
        w.varint(tables::item::new::STONE);
        w.varint(3);
        w.varint(0);
        for id in [cn::ATTACK_ANIMATION, cn::INTERACT_ANIMATION] {
            w.varint(id);
            w.varint(1);
            w.varint(6);
        }
        w.varint(cn::WAXED);
        let mut out = Writer::new();
        item::slot(&mut Reader::new(&w.data), &mut out, Dir::Down).unwrap();
        let mut expected = Writer::new();
        expected.varint(1);
        expected.varint(tables::item::old::STONE);
        expected.varint(1);
        expected.varint(0);
        expected.varint(co::SWING_ANIMATION);
        expected.varint(1);
        expected.varint(6);
        assert_eq!(out.data, expected.data);
    }

    /// The creative slot's components go behind their lengths.
    #[test]
    fn creative_slots_are_delimited() {
        let mut w = Writer::new();
        w.varint(1);
        w.varint(tables::item::new::STONE);
        w.varint(1);
        w.varint(0);
        w.varint(cn::DAMAGE);
        w.varint(300);
        let mut out = Writer::new();
        item::slot_delimited(&mut Reader::new(&w.data), &mut out, Dir::Down).unwrap();
        let mut expected = Writer::new();
        expected.varint(1);
        expected.varint(tables::item::old::STONE);
        expected.varint(1);
        expected.varint(0);
        expected.varint(co::DAMAGE);
        expected.varint(2);
        expected.varint(300);
        assert_eq!(out.data, expected.data);
    }

    /// Long-array BitSets become trimmed little-endian bytes.
    #[test]
    fn bit_sets_become_bytes() {
        let mut w = Writer::new();
        w.long_array(&[0x0102, 0]);
        let mut rw = Rw::new(&w.data);
        world::bit_set(&mut rw).unwrap();
        assert_eq!(rw.written(), vec![2, 0x02, 0x01]);
    }

    /// Every 26.2 block state has a 26.3 state, and state ids round-trip.
    #[test]
    fn block_states_round_trip() {
        for old in 0..tables::BLOCK_STATES.len() as i32 {
            assert_eq!(state(Dir::Down, state(Dir::Up, old)), old);
        }
        assert_eq!(state(Dir::Up, 0), 0);
    }

    /// The handshake carries the server's protocol, and the states follow
    /// the packets that switch them.
    #[test]
    fn pipeline_rewrites_the_handshake() {
        let t = crate::translator(776).unwrap();
        let mut t = t.lock().unwrap();
        let mut w = Writer::new();
        w.varint(horizon_protocol::PROTOCOL_VERSION);
        w.string("localhost");
        w.u16(25565);
        w.varint(2);
        let mut out = Vec::new();
        t.serverbound(Packet::new(0, w.data), &mut out).unwrap();
        assert_eq!(Reader::new(&out[0].data).varint().unwrap(), 776);
    }
}

/// ItemCost: the item, the count, the exact components.
fn item_cost(
    r: &mut horizon_protocol::buf::Reader,
    w: &mut horizon_protocol::buf::Writer,
    d: Dir,
) -> Result<()> {
    w.varint(item(d, r.varint()?));
    w.varint(r.varint()?);
    item::exact_components(r, w, d)
}
