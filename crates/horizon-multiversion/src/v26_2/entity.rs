//! Entity packets (ViaVersion's EntityPacketRewriter26_3): entity type and
//! data ids, the movement packets 26.3 sends in interpolation steps, the
//! arm swing that became its own packet, and the game modes in the spawn info.
use super::{Dir, entity, item, state, tables, world};
use crate::{Out, wire::Rw};
use horizon_protocol::{
    Result,
    buf::{Codec, LpVec3, Reader, Vec3d, Writer},
};

/// 26.3 moves entities with a step count; every entity that steps used 3
/// ticks a step in 26.2, which one step of 3 ticks matches.
const INTERPOLATION_STEP_TICKS: i32 = 3;
/// The movement properties flag for one step (bit 0 is on ground).
const SINGLE_STEP: i32 = 1 << 1;
/// `SwingAnimationType.WHACK`, the swing every 26.2 arm swing was.
const WHACK: i32 = 1;
const SWING_DURATION: i32 = 6;

pub fn add_entity(rw: &mut Rw) -> Result<()> {
    rw.varint()?;
    rw.uuid()?;
    let kind = rw.r.varint()?;
    rw.w.varint(entity(Dir::Up, kind));
    rw.pass(|r| Vec3d::read(r).map(drop))?;
    rw.pass(|r| LpVec3::read(r).map(drop))?;
    rw.i8()?;
    rw.i8()?;
    rw.i8()?;
    // A falling block's data is its block state.
    if kind == tables::entity::old::FALLING_BLOCK {
        rw.map_varint(|s| state(Dir::Up, s))?;
    }
    Ok(())
}

/// ClientboundAnimatePacket: 26.3 sends arm swings as
/// ClientboundSwingAnimationPacket and renumbered the rest.
pub fn animate(data: &[u8], out: &mut Out) -> Result<()> {
    use super::tables::packets::play_clientbound::new as n;
    let mut r = Reader::new(data);
    let id = r.varint()?;
    let action = r.u8()?;
    let mut w = Writer::new();
    w.varint(id);
    let packet = match action {
        0 | 3 => {
            w.varint(i32::from(action == 3));
            w.varint(WHACK);
            w.varint(SWING_DURATION);
            n::SWING_ANIMATION
        }
        2 | 4 | 5 => {
            w.u8(match action {
                2 => 0,
                4 => 1,
                _ => 2,
            });
            n::ANIMATE
        }
        _ => return Ok(()),
    };
    out.push(packet, w.data);
    Ok(())
}

/// MOVE_ENTITY_POS(_ROT): deltas, rotation, then on ground in 26.2; the
/// movement properties, step ticks, deltas and rotation in 26.3.
pub fn move_pos(rw: &mut Rw, rotation: bool) -> Result<()> {
    rw.varint()?;
    let delta = [rw.r.i16()?, rw.r.i16()?, rw.r.i16()?];
    let rot = if rotation {
        Some((rw.r.i8()?, rw.r.i8()?))
    } else {
        None
    };
    let on_ground = rw.r.bool()?;
    rw.w.varint(i32::from(on_ground) | SINGLE_STEP);
    rw.w.varint(INTERPOLATION_STEP_TICKS);
    delta.iter().for_each(|v| rw.w.i16(*v));
    if let Some((y, x)) = rot {
        rw.w.i8(y);
        rw.w.i8(x);
    }
    Ok(())
}

/// MOVE_ENTITY_ROT: on ground moved before the rotation.
pub fn move_rot(rw: &mut Rw) -> Result<()> {
    rw.varint()?;
    let (y, x) = (rw.r.i8()?, rw.r.i8()?);
    rw.bool()?;
    rw.w.i8(y);
    rw.w.i8(x);
    Ok(())
}

/// ENTITY_POSITION_SYNC: 26.3 sends a position path (one step) without the
/// velocity.
pub fn position_sync(rw: &mut Rw) -> Result<()> {
    rw.varint()?;
    rw.w.varint(1);
    rw.w.varint(1);
    for _ in 0..3 {
        rw.f64()?;
    }
    rw.w.varint(INTERPOLATION_STEP_TICKS);
    for _ in 0..3 {
        rw.r.f64()?;
    }
    Ok(())
}

/// ClientboundLoginPacket up to its spawn info.
pub fn login(rw: &mut Rw) -> Result<()> {
    rw.i32()?;
    rw.bool()?;
    rw.list(|rw| rw.string().map(drop))?;
    rw.varint()?;
    rw.varint()?;
    rw.varint()?;
    rw.bool()?;
    rw.bool()?;
    rw.bool()?;
    spawn_info(rw)
}

/// CommonPlayerSpawnInfo: the game modes became a VarInt and an optional
/// VarInt (the value plus one, 0 for none; 26.2 sent -1 for none).
pub fn spawn_info(rw: &mut Rw) -> Result<()> {
    rw.varint()?;
    rw.string()?;
    rw.i64()?;
    let game_mode = rw.r.i8()?;
    rw.w.varint(i32::from(game_mode));
    let previous = rw.r.i8()?;
    rw.w.varint(if previous < 0 { 0 } else { i32::from(previous) + 1 });
    Ok(())
}

/// SynchedEntityData entries: (index, serializer, value) until 0xFF. The
/// serializers kept their ids; values holding ids are remapped.
pub fn entity_data(rw: &mut Rw) -> Result<()> {
    let d = Dir::Up;
    loop {
        let index = rw.u8()?;
        if index == 0xFF {
            return Ok(());
        }
        let kind = rw.varint()?;
        let (r, w) = (&mut rw.r, &mut rw.w);
        match kind {
            0 => w.i8(r.i8()?),
            8 => item::boolean(r, w, d)?,
            // int, direction, optional unsigned int, pose, the variant
            // holders and the enum states, arm.
            1 | 12 | 19..=32 | 34..=38 | 42 | 43 => item::varint(r, w, d)?,
            2 => w.varlong(r.varlong()?),
            3 => item::f32(r, w, d)?,
            4 => item::string(r, w, d)?,
            5 => item::nbt(r, w, d)?,
            6 => item::optional(r, w, d, item::nbt)?,
            7 => item::slot(r, w, d)?,
            // rotations, vector3
            9 | 39 => (0..3).try_for_each(|_| item::f32(r, w, d))?,
            10 => item::i64(r, w, d)?,
            11 => item::optional(r, w, d, item::i64)?,
            13 => item::optional(r, w, d, |r, w, _| {
                w.uuid(r.uuid()?);
                Ok(())
            })?,
            // block state, optional block state (0 is none, which maps to 0)
            14 | 15 => w.varint(state(d, r.varint()?)),
            16 => world::particle(r, w, d)?,
            17 => item::list(r, w, d, world::particle)?,
            // villager data: type, profession, level
            18 => (0..3).try_for_each(|_| item::varint(r, w, d))?,
            // optional global pos: dimension, pos
            33 => item::optional(r, w, d, |r, w, d| {
                item::string(r, w, d)?;
                item::i64(r, w, d)
            })?,
            40 => (0..4).try_for_each(|_| item::f32(r, w, d))?,
            41 => profile(r, w)?,
            other => {
                return Err(crate::wire::error(format!(
                    "unknown entity data serializer {other}"
                )));
            }
        }
    }
}

/// ResolvableProfile as entity data: either a full profile or a partial
/// one, then the skin patch (three optional textures and the model).
fn profile(r: &mut Reader, w: &mut Writer) -> Result<()> {
    let d = Dir::Up;
    let full = r.bool()?;
    w.bool(full);
    if full {
        w.uuid(r.uuid()?);
        item::string(r, w, d)?;
    } else {
        item::optional(r, w, d, item::string)?;
        item::optional(r, w, d, |r, w, _| {
            w.uuid(r.uuid()?);
            Ok(())
        })?;
    }
    item::list(r, w, d, |r, w, d| {
        item::string(r, w, d)?;
        item::string(r, w, d)?;
        item::optional(r, w, d, item::string)
    })?;
    (0..3).try_for_each(|_| item::optional(r, w, d, item::string))?;
    item::optional(r, w, d, item::boolean)
}
