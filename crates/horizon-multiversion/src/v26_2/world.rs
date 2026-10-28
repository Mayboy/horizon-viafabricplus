//! Blocks, chunks, particles, commands, recipes and advancements
//! (ViaVersion's BlockPacketRewriter26_3, the particle, command and recipe
//! rewriters and BlockItemPacketRewriter26_3's packets).
use super::{Dir, argument_type, component, item, particle as particle_id, state, tables};
use crate::wire::{Rw, error};
use horizon_protocol::{
    Result,
    buf::{Codec, Reader, Vec3d, Writer},
    chunk::{Paletted, Strategy},
    packets::play::ArgumentProperties,
};
use horizon_world::nbt::Tag;

/// Block state ids need 16 bits in 26.3's global palette (35723 states).
const GLOBAL_BLOCK_BITS: u8 = 16;

/// A decorated pot's sherds: a list of item ids in 26.2, a compound of
/// sides with item templates in 26.3.
pub fn block_entity(mut tag: Tag) -> Tag {
    const SIDES: [&str; 4] = ["back", "left", "right", "front"];
    let Some(Tag::List(sherds)) = tag.get("sherds").cloned() else {
        return tag;
    };
    let sides = SIDES.iter().enumerate().map(|(i, side)| {
        let id = match sherds.get(i) {
            Some(Tag::String(s)) => s.clone(),
            _ => "minecraft:brick".into(),
        };
        (*side, Tag::compound([("id", Tag::String(id))]))
    });
    if let Some(slot) = tag.get_mut("sherds") {
        *slot = Tag::compound(sides);
    }
    tag
}

/// ClientboundCommandsPacket: the argument parsers' ids.
pub fn commands(rw: &mut Rw) -> Result<()> {
    rw.list(|rw| {
        let flags = rw.u8()?;
        rw.list(|rw| rw.varint().map(drop))?;
        if flags & 8 != 0 {
            rw.varint()?;
        }
        let kind = flags & 3;
        if kind >= 1 {
            rw.string()?;
        }
        if kind == 2 {
            let parser = rw.map_varint(|p| argument_type(Dir::Up, p))?;
            rw.pass(|r| ArgumentProperties::read(r, parser).map(drop))?;
            if flags & 16 != 0 {
                rw.string()?;
            }
        }
        Ok(())
    })?;
    rw.varint().map(drop)
}

/// ClientboundExplodePacket up to (not including) 26.3's play-sound flag.
pub fn explode(rw: &mut Rw) -> Result<()> {
    let d = Dir::Up;
    rw.pass(|r| Vec3d::read(r).map(drop))?;
    rw.f32()?;
    rw.i32()?;
    rw.optional(|rw| rw.pass(|r| Vec3d::read(r).map(drop)))?;
    particle(&mut rw.r, &mut rw.w, d)?;
    item::sound_holder(&mut rw.r, &mut rw.w, d)?;
    rw.list(|rw| {
        particle(&mut rw.r, &mut rw.w, d)?;
        rw.f32()?;
        rw.f32()?;
        rw.varint().map(drop)
    })?;
    Ok(())
}

/// ClientboundLevelChunkPacketData: heightmaps, the sections (block states
/// remapped and, in the global palette, repacked to 26.3's width), block
/// entities. The light data after it is unchanged.
pub fn chunk(rw: &mut Rw) -> Result<()> {
    rw.list(|rw| {
        rw.varint()?;
        rw.pass(|r| r.long_array().map(drop))
    })?;
    let n = rw.r.count(rw.r.remaining())?;
    let buffer = rw.r.bytes(n)?;
    let sections = sections(buffer)?;
    rw.w.varint(sections.len() as i32);
    rw.w.bytes(&sections);
    rw.list(|rw| {
        rw.i8()?;
        rw.i16()?;
        rw.varint()?;
        let tag = rw.r.nbt()?;
        rw.w.nbt(&block_entity(tag));
        Ok(())
    })?;
    Ok(())
}

fn sections(buffer: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(buffer);
    let mut w = Writer::new();
    while r.remaining() > 0 {
        w.i16(r.i16()?);
        w.i16(r.i16()?);
        // Reading with no global width keeps the width on the wire.
        let mut states = Paletted::read(&mut r, Strategy::Blocks, 0)?;
        if states.palette.is_empty() {
            let values: Vec<i32> = (0..horizon_protocol::chunk::BLOCKS)
                .map(|i| state(Dir::Up, states.get(i)))
                .collect();
            states = Paletted::from_values(&values, Strategy::Blocks, GLOBAL_BLOCK_BITS);
        } else {
            for p in &mut states.palette {
                *p = state(Dir::Up, *p);
            }
        }
        states.write(&mut w);
        let start = r.position();
        Paletted::read(&mut r, Strategy::Biomes, 0)?;
        w.bytes(r.since(start));
    }
    Ok(w.data)
}

/// ClientboundLightUpdatePacketData: 26.3 sends the four section masks as
/// BitSet bytes instead of long arrays; the light layers are unchanged.
pub fn light(rw: &mut Rw) -> Result<()> {
    (0..4).try_for_each(|_| bit_set(rw))
}

/// A BitSet as 26.2's long array, rewritten as 26.3's byte array (the
/// longs' little-endian bytes, without trailing zeros).
pub fn bit_set(rw: &mut Rw) -> Result<()> {
    let longs = rw.r.long_array()?;
    let mut bytes: Vec<u8> = longs.iter().flat_map(|l| l.to_le_bytes()).collect();
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    rw.w.byte_array(&bytes);
    Ok(())
}

/// ClientboundLevelParticlesPacket: 26.2 sends the particle last, one
/// speed and an int count; 26.3 the particle first, a speed per axis, a
/// VarInt count and a randomization type.
pub fn level_particles(rw: &mut Rw) -> Result<()> {
    let r = &mut rw.r;
    let override_limiter = r.bool()?;
    let always_show = r.bool()?;
    let pos = [r.f64()?, r.f64()?, r.f64()?];
    let dist = [r.f32()?, r.f32()?, r.f32()?];
    let speed = r.f32()?;
    let count = r.i32()?;
    particle(r, &mut rw.w, Dir::Up)?;
    let w = &mut rw.w;
    w.bool(override_limiter);
    w.bool(always_show);
    pos.iter().for_each(|v| w.f64(*v));
    dist.iter().for_each(|v| w.f32(*v));
    (0..3).for_each(|_| w.f32(speed));
    w.varint(count);
    w.varint(0);
    Ok(())
}

/// ParticleTypes.STREAM_CODEC: the type, then the data it carries (the
/// same per particle in both versions).
pub fn particle(r: &mut Reader, w: &mut Writer, d: Dir) -> Result<()> {
    let id = r.varint()?;
    let names: &[&str] = match d {
        Dir::Up => &tables::particle::old::NAMES,
        Dir::Down => &tables::particle::new::NAMES,
    };
    let name = usize::try_from(id)
        .ok()
        .and_then(|i| names.get(i))
        .copied()
        .ok_or_else(|| error(format!("unknown particle {id}")))?;
    w.varint(particle_id(d, id));
    match name {
        "block" | "block_marker" | "falling_dust" | "dust_pillar" | "block_crumble" => {
            w.varint(state(d, r.varint()?))
        }
        "entity_effect" | "tinted_leaves" | "flash" | "geyser" | "geyser_plume" => {
            item::i32(r, w, d)?
        }
        "dust" | "effect" | "instant_effect" | "geyser_base" | "geyser_poof" => {
            item::i32(r, w, d)?;
            item::f32(r, w, d)?;
        }
        "dust_color_transition" => {
            item::i32(r, w, d)?;
            item::i32(r, w, d)?;
            item::f32(r, w, d)?;
        }
        "dragon_breath" | "sculk_charge" => item::f32(r, w, d)?,
        "shriek" => item::varint(r, w, d)?,
        "trail" => {
            for _ in 0..3 {
                w.f64(r.f64()?);
            }
            item::i32(r, w, d)?;
            item::varint(r, w, d)?;
        }
        "item" => item::template(r, w, d)?,
        "vibration" => {
            let source = r.varint()?;
            w.varint(source);
            match source {
                0 => item::i64(r, w, d)?,
                1 => {
                    item::varint(r, w, d)?;
                    item::f32(r, w, d)?;
                }
                other => return Err(error(format!("position source type {other}"))),
            }
            item::varint(r, w, d)?;
        }
        _ => {}
    }
    Ok(())
}

/// SlotDisplay. A tag display named its tag in 26.2 and is a holder set of
/// items in 26.3.
pub fn slot_display(r: &mut Reader, w: &mut Writer) -> Result<()> {
    let d = Dir::Up;
    let kind = r.varint()?;
    w.varint(kind);
    match kind {
        0 | 1 => Ok(()),
        2 => slot_display(r, w),
        3 => {
            slot_display(r, w)?;
            let c = r.varint()?;
            w.varint(component(d, c).ok_or_else(|| error("slot display component"))?);
            Ok(())
        }
        4 => {
            w.varint(super::item(d, r.varint()?));
            Ok(())
        }
        5 => item::template(r, w, d),
        6 => {
            w.varint(0);
            item::string(r, w, d)
        }
        7 | 9 => {
            slot_display(r, w)?;
            slot_display(r, w)
        }
        8 => {
            slot_display(r, w)?;
            slot_display(r, w)?;
            item::holder(r, w, d, |_, id| id, |r, w, d| {
                item::string(r, w, d)?;
                item::nbt(r, w, d)?;
                item::boolean(r, w, d)
            })
        }
        10 => item::list(r, w, d, |r, w, _| slot_display(r, w)),
        other => Err(error(format!("unknown slot display {other}"))),
    }
}

/// RecipeDisplay.
pub fn recipe_display(r: &mut Reader, w: &mut Writer) -> Result<()> {
    let d = Dir::Up;
    let kind = r.varint()?;
    w.varint(kind);
    let slots = |n: usize, r: &mut Reader, w: &mut Writer| (0..n).try_for_each(|_| slot_display(r, w));
    match kind {
        0 => {
            item::list(r, w, d, |r, w, _| slot_display(r, w))?;
            slots(2, r, w)
        }
        1 => {
            item::varint(r, w, d)?;
            item::varint(r, w, d)?;
            item::list(r, w, d, |r, w, _| slot_display(r, w))?;
            slots(2, r, w)
        }
        2 => {
            slots(4, r, w)?;
            item::varint(r, w, d)?;
            item::f32(r, w, d)
        }
        3 => slots(3, r, w),
        4 => slots(5, r, w),
        other => Err(error(format!("unknown recipe display {other}"))),
    }
}

/// ClientboundRecipeBookAddPacket.
pub fn recipe_book_add(rw: &mut Rw) -> Result<()> {
    let d = Dir::Up;
    rw.list(|rw| {
        rw.varint()?;
        recipe_display(&mut rw.r, &mut rw.w)?;
        rw.varint()?;
        rw.varint()?;
        rw.optional(|rw| {
            rw.list(|rw| item::holder_set(&mut rw.r, &mut rw.w, d, super::item))
                .map(drop)
        })?;
        rw.i8().map(drop)
    })?;
    rw.bool().map(drop)
}

/// ClientboundUpdateAdvancementsPacket: the tab position moved out of the
/// display info to the end of each advancement.
pub fn advancements(rw: &mut Rw) -> Result<()> {
    rw.bool()?;
    rw.list(|rw| {
        rw.string()?;
        rw.optional(|rw| rw.string().map(drop))?;
        let mut at = (0.0, 0.0);
        rw.optional(|rw| {
            rw.nbt()?;
            rw.nbt()?;
            item::template(&mut rw.r, &mut rw.w, Dir::Up)?;
            rw.varint()?;
            let flags = rw.i32()?;
            if flags & 1 != 0 {
                rw.string()?;
            }
            at = (rw.r.f32()?, rw.r.f32()?);
            Ok(())
        })?;
        rw.list(|rw| rw.list(|rw| rw.string().map(drop)).map(drop))?;
        rw.bool()?;
        rw.w.f32(at.0);
        rw.w.f32(at.1);
        Ok(())
    })?;
    Ok(())
}

/// ClientboundUpdateRecipesPacket: the item property sets (with the
/// brewing stand's, which 26.3 reads from the server) and the stonecutter
/// recipes.
pub fn update_recipes(rw: &mut Rw) -> Result<()> {
    use tables::item::new as n;
    let d = Dir::Up;
    let count = rw.r.count(rw.r.remaining())?;
    rw.w.varint(count as i32 + 2);
    for _ in 0..count {
        rw.string()?;
        rw.list(|rw| rw.map_varint(|i| super::item(d, i)).map(drop))?;
    }
    let sets: [(&str, &[i32]); 2] = [
        (
            "minecraft:brewing_input",
            &[n::POTION, n::SPLASH_POTION, n::LINGERING_POTION, n::GLASS_BOTTLE],
        ),
        (
            "minecraft:brewing_reagent",
            &[
                n::BLAZE_POWDER,
                n::BREEZE_ROD,
                n::COBWEB,
                n::DRAGON_BREATH,
                n::FERMENTED_SPIDER_EYE,
                n::GHAST_TEAR,
                n::GLISTERING_MELON_SLICE,
                n::GLOWSTONE_DUST,
                n::GOLDEN_CARROT,
                n::GUNPOWDER,
                n::MAGMA_CREAM,
                n::NETHER_WART,
                n::PHANTOM_MEMBRANE,
                n::PUFFERFISH,
                n::RABBIT_FOOT,
                n::REDSTONE,
                n::SLIME_BLOCK,
                n::SPIDER_EYE,
                n::STONE,
                n::SUGAR,
                n::TURTLE_HELMET,
            ],
        ),
    ];
    for (name, ids) in sets {
        rw.w.string(name);
        rw.w.varint(ids.len() as i32);
        ids.iter().for_each(|i| rw.w.varint(*i));
    }
    rw.list(|rw| {
        item::holder_set(&mut rw.r, &mut rw.w, d, super::item)?;
        slot_display(&mut rw.r, &mut rw.w)
    })?;
    Ok(())
}
