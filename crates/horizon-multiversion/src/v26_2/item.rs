//! Item stacks between 26.2 and 26.3: item ids, data component ids, the
//! ids inside component values (items, blocks, entity types, sounds,
//! component types), and the components whose value or existence changed
//! (ViaVersion's BlockItemPacketRewriter26_3 `upgradeData`/`downgradeData`).
//!
//! Every function copies one value from `r` to `w` in direction `d`.
use super::{Dir, block, component, entity, item, sound, tables};
use crate::wire::error;
use horizon_protocol::{
    Result,
    buf::{Reader, Writer},
};

type Copy = fn(&mut Reader, &mut Writer, Dir) -> Result<()>;

/// ItemStack.OPTIONAL_STREAM_CODEC: a count, then (when positive) the item
/// and its patch.
pub fn slot(r: &mut Reader, w: &mut Writer, d: Dir) -> Result<()> {
    let count = r.varint()?;
    w.varint(count);
    if count <= 0 {
        return Ok(());
    }
    w.varint(item(d, r.varint()?));
    patch(r, w, d)
}

/// ItemStackTemplate.STREAM_CODEC: the item, the count, the patch.
pub fn template(r: &mut Reader, w: &mut Writer, d: Dir) -> Result<()> {
    w.varint(item(d, r.varint()?));
    w.varint(r.varint()?);
    patch(r, w, d)
}

/// An optional stack for ServerboundSetCreativeModeSlotPacket: read as the
/// client wrote it, written as ItemStack.OPTIONAL_UNTRUSTED_STREAM_CODEC
/// (each component value behind its byte length).
pub fn slot_delimited(r: &mut Reader, w: &mut Writer, d: Dir) -> Result<()> {
    let count = r.varint()?;
    w.varint(count);
    if count <= 0 {
        return Ok(());
    }
    w.varint(item(d, r.varint()?));
    patch_with(r, w, d, true)
}

/// DataComponentPatch: the added components' and the removed ones' counts,
/// then the added (type id and value) and the removed type ids.
pub fn patch(r: &mut Reader, w: &mut Writer, d: Dir) -> Result<()> {
    patch_with(r, w, d, false)
}

/// A patch whose written values are each behind their length when `delimited`.
fn patch_with(r: &mut Reader, w: &mut Writer, d: Dir, delimited: bool) -> Result<()> {
    let added = r.count(65536)?;
    let removed = r.count(65536)?;
    let mut body = Writer::new();
    let mut n = 0;
    let mut seen = Vec::new();
    for _ in 0..added {
        let kind = r.varint()?;
        if delimited {
            let mut typed_body = Writer::new();
            let count = typed(kind, r, &mut typed_body, d, &mut seen)?;
            delimit(&typed_body.data, count, d, &mut body)?;
            n += count;
        } else {
            n += typed(kind, r, &mut body, d, &mut seen)?;
        }
    }
    let mut gone = Vec::new();
    for _ in 0..removed {
        for kind in removed_ids(d, r.varint()?) {
            if !gone.contains(&kind) {
                gone.push(kind);
            }
        }
    }
    w.varint(n as i32);
    w.varint(gone.len() as i32);
    w.bytes(&body.data);
    gone.iter().for_each(|k| w.varint(*k));
    Ok(())
}

/// Re-write `count` typed components (in the version `d` writes) with each
/// value behind its length, measuring each value by reading it back.
fn delimit(typed: &[u8], count: usize, d: Dir, w: &mut Writer) -> Result<()> {
    // The written components are in the target version: read them back the
    // way that version's reader (the opposite direction's source) would.
    let back = match d {
        Dir::Up => Dir::Down,
        Dir::Down => Dir::Up,
    };
    let mut r = Reader::new(typed);
    for _ in 0..count {
        let kind = r.varint()?;
        let start = r.position();
        value(name(back, kind)?, back, &mut r, &mut Writer::new())?;
        let bytes = r.since(start);
        w.varint(kind);
        w.varint(bytes.len() as i32);
        w.bytes(bytes);
    }
    Ok(())
}

/// The ids a removed component becomes: none when the other version lacks it.
fn removed_ids(d: Dir, kind: i32) -> Vec<i32> {
    use tables::component::{new, old};
    match (d, kind) {
        (Dir::Up, old::SWING_ANIMATION) => vec![new::ATTACK_ANIMATION, new::INTERACT_ANIMATION],
        (Dir::Down, new::ATTACK_ANIMATION | new::INTERACT_ANIMATION) => vec![old::SWING_ANIMATION],
        _ => component(d, kind).into_iter().collect(),
    }
}

/// One typed component (its type id, then its value): how many it became.
/// `seen` holds the component types written so far (two 26.3 animations
/// become one 26.2 swing animation).
pub fn typed(
    kind: i32,
    r: &mut Reader,
    w: &mut Writer,
    d: Dir,
    seen: &mut Vec<i32>,
) -> Result<usize> {
    use tables::component::{new, old};
    let name = name(d, kind)?;
    match (d, name) {
        // 26.3 dropped map_color.
        (Dir::Up, "map_color") => {
            r.i32()?;
            Ok(0)
        }
        // swing_animation split into attack_animation and interact_animation.
        (Dir::Up, "swing_animation") => {
            let (kind_, duration) = (r.varint()?, r.varint()?);
            for id in [new::ATTACK_ANIMATION, new::INTERACT_ANIMATION] {
                w.varint(id);
                w.varint(kind_);
                w.varint(duration);
            }
            Ok(2)
        }
        (Dir::Down, "attack_animation" | "interact_animation") => {
            let (kind_, duration) = (r.varint()?, r.varint()?);
            if seen.contains(&old::SWING_ANIMATION) {
                return Ok(0);
            }
            seen.push(old::SWING_ANIMATION);
            w.varint(old::SWING_ANIMATION);
            w.varint(kind_);
            w.varint(duration);
            Ok(1)
        }
        // Components 26.2 doesn't have: read past them.
        (Dir::Down, _) if component(d, kind).is_none() => {
            value(name, d, r, &mut Writer::new())?;
            Ok(0)
        }
        _ => {
            let to = component(d, kind).ok_or_else(|| error(format!("component {name}")))?;
            seen.push(to);
            w.varint(to);
            value(name, d, r, w)?;
            Ok(1)
        }
    }
}

/// A component type's name in the version `d` reads.
fn name(d: Dir, kind: i32) -> Result<&'static str> {
    let names: &[&str] = match d {
        Dir::Up => &tables::component::old::NAMES,
        Dir::Down => &tables::component::new::NAMES,
    };
    usize::try_from(kind)
        .ok()
        .and_then(|i| names.get(i))
        .copied()
        .ok_or_else(|| error(format!("unknown data component type {kind}")))
}

/// A component's value (DataComponents' network codecs), by its name.
fn value(name: &str, d: Dir, r: &mut Reader, w: &mut Writer) -> Result<()> {
    match name {
        "custom_data" | "custom_name" | "item_name" | "intangible_projectile"
        | "map_decorations" | "debug_stick_state" | "bucket_entity_data" | "recipes"
        | "lock" | "container_loot" => nbt(r, w, d),
        "max_stack_size" | "max_damage" | "damage" | "rarity" | "repair_cost" | "map_id"
        | "map_post_processing" | "damage_type" | "enchantable" | "additional_trade_cost"
        | "block_transformer" | "villager_food" | "dye" | "ominous_bottle_amplifier"
        | "base_color" | "provides_pottery_pattern" | "cushion/color" => varint(r, w, d),
        n if n.contains('/') && n != "painting/variant" => varint(r, w, d),
        "unbreakable" | "creative_slot_lock" | "glider" | "waxed" => Ok(()),
        "enchantment_glint_override" => boolean(r, w, d),
        "minimum_attack_charge" | "potion_duration_scale" => f32(r, w, d),
        "dyed_color" | "map_color" => i32(r, w, d),
        "item_model" | "tooltip_style" | "note_block_sound" => string(r, w, d),
        "use_effects" => {
            boolean(r, w, d)?;
            boolean(r, w, d)?;
            f32(r, w, d)
        }
        "lore" => list(r, w, d, nbt),
        "enchantments" | "stored_enchantments" | "suspicious_stew_effects" => {
            list(r, w, d, |r, w, d| {
                varint(r, w, d)?;
                varint(r, w, d)
            })
        }
        "can_place_on" | "can_break" => list(r, w, d, block_predicate),
        "attribute_modifiers" => list(r, w, d, attribute_modifier),
        "custom_model_data" => {
            list(r, w, d, f32)?;
            list(r, w, d, boolean)?;
            list(r, w, d, string)?;
            list(r, w, d, i32)
        }
        "tooltip_display" => {
            boolean(r, w, d)?;
            // The hidden components, by type id.
            let n = r.count(r.remaining())?;
            let ids: Vec<i32> = (0..n)
                .map(|_| r.varint())
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .flat_map(|k| removed_ids(d, k))
                .collect();
            w.varint(ids.len() as i32);
            ids.iter().for_each(|k| w.varint(*k));
            Ok(())
        }
        "food" => {
            varint(r, w, d)?;
            f32(r, w, d)?;
            boolean(r, w, d)
        }
        "consumable" => {
            f32(r, w, d)?;
            varint(r, w, d)?;
            sound_holder(r, w, d)?;
            boolean(r, w, d)?;
            list(r, w, d, consume_effect)
        }
        "use_remainder" | "sulfur_cube_content" => template(r, w, d),
        "use_cooldown" => {
            f32(r, w, d)?;
            optional(r, w, d, string)
        }
        // Holder sets of data-driven registries: damage types, banner patterns.
        "damage_resistant" | "provides_banner_patterns" => holder_set(r, w, d, keep),
        "repairable" => holder_set(r, w, d, item),
        "tool" => {
            list(r, w, d, |r, w, d| {
                holder_set(r, w, d, block)?;
                optional(r, w, d, f32)?;
                optional(r, w, d, boolean)
            })?;
            f32(r, w, d)?;
            varint(r, w, d)?;
            boolean(r, w, d)
        }
        "weapon" => {
            varint(r, w, d)?;
            f32(r, w, d)
        }
        "attack_range" => (0..6).try_for_each(|_| f32(r, w, d)),
        "equippable" => {
            varint(r, w, d)?;
            sound_holder(r, w, d)?;
            optional(r, w, d, string)?;
            optional(r, w, d, string)?;
            optional(r, w, d, |r, w, d| holder_set(r, w, d, entity))?;
            (0..5).try_for_each(|_| boolean(r, w, d))?;
            sound_holder(r, w, d)
        }
        "death_protection" => list(r, w, d, consume_effect),
        "blocks_attacks" => {
            f32(r, w, d)?;
            f32(r, w, d)?;
            list(r, w, d, |r, w, d| {
                f32(r, w, d)?;
                optional(r, w, d, |r, w, d| holder_set(r, w, d, keep))?;
                f32(r, w, d)?;
                f32(r, w, d)
            })?;
            (0..3).try_for_each(|_| f32(r, w, d))?;
            optional(r, w, d, |r, w, d| holder_set(r, w, d, keep))?;
            optional(r, w, d, sound_holder)?;
            optional(r, w, d, sound_holder)
        }
        "piercing_weapon" => {
            boolean(r, w, d)?;
            boolean(r, w, d)?;
            optional(r, w, d, sound_holder)?;
            optional(r, w, d, sound_holder)
        }
        "kinetic_weapon" => {
            varint(r, w, d)?;
            varint(r, w, d)?;
            (0..3).try_for_each(|_| {
                optional(r, w, d, |r, w, d| {
                    varint(r, w, d)?;
                    f32(r, w, d)?;
                    f32(r, w, d)
                })
            })?;
            f32(r, w, d)?;
            f32(r, w, d)?;
            optional(r, w, d, sound_holder)?;
            optional(r, w, d, sound_holder)
        }
        "swing_animation" | "attack_animation" | "interact_animation" => {
            varint(r, w, d)?;
            varint(r, w, d)
        }
        "charged_projectiles" | "bundle_contents" => list(r, w, d, template),
        "potion_contents" => {
            optional(r, w, d, varint)?;
            optional(r, w, d, i32)?;
            list(r, w, d, mob_effect)?;
            optional(r, w, d, string)
        }
        "writable_book_content" => list(r, w, d, |r, w, d| filterable(r, w, d, string)),
        "written_book_content" => {
            filterable(r, w, d, string)?;
            string(r, w, d)?;
            varint(r, w, d)?;
            list(r, w, d, |r, w, d| filterable(r, w, d, nbt))?;
            boolean(r, w, d)
        }
        "trim" => {
            trim_material(r, w, d)?;
            holder(r, w, d, keep, |r, w, d| {
                string(r, w, d)?;
                nbt(r, w, d)?;
                boolean(r, w, d)
            })
        }
        "provides_trim_material" => trim_material(r, w, d),
        // TypedEntityData: the entity type, then its data.
        "entity_data" => {
            w.varint(entity(d, r.varint()?));
            nbt(r, w, d)
        }
        // The block entity type (unchanged), then its data.
        "block_entity_data" => {
            varint(r, w, d)?;
            nbt(r, w, d)
        }
        "instrument" => holder(r, w, d, keep, |r, w, d| {
            sound_holder(r, w, d)?;
            f32(r, w, d)?;
            f32(r, w, d)?;
            // 26.3 added the durability damage.
            match d {
                Dir::Up => w.varint(0),
                Dir::Down => drop(r.varint()?),
            }
            nbt(r, w, d)
        }),
        "jukebox_playable" => holder(r, w, d, keep, |r, w, d| {
            sound_holder(r, w, d)?;
            nbt(r, w, d)?;
            f32(r, w, d)?;
            varint(r, w, d)
        }),
        "lodestone_tracker" => {
            optional(r, w, d, |r, w, d| {
                string(r, w, d)?;
                i64(r, w, d)
            })?;
            boolean(r, w, d)
        }
        "firework_explosion" => firework_explosion(r, w, d),
        "fireworks" => {
            varint(r, w, d)?;
            list(r, w, d, firework_explosion)
        }
        "profile" => profile(r, w, d),
        "banner_patterns" => list(r, w, d, |r, w, d| {
            holder(r, w, d, keep, |r, w, d| {
                string(r, w, d)?;
                string(r, w, d)
            })?;
            varint(r, w, d)
        }),
        "pot_decorations" => pot_decorations(r, w, d),
        "container" => list(r, w, d, |r, w, d| optional(r, w, d, template)),
        "block_state" => list(r, w, d, |r, w, d| {
            string(r, w, d)?;
            string(r, w, d)
        }),
        // Bee occupants: TypedEntityData, ticksInHive, minTicksInHive.
        "bees" => list(r, w, d, |r, w, d| {
            w.varint(entity(d, r.varint()?));
            nbt(r, w, d)?;
            varint(r, w, d)?;
            varint(r, w, d)
        }),
        "break_sound" => sound_holder(r, w, d),
        "compostable" => resolvable(r, w, d, i32),
        "cooking_fuel" | "brewing_fuel" => {
            resolvable(r, w, d, i32)?;
            resolvable(r, w, d, f32)
        }
        "mob_visibility" => {
            holder_set(r, w, d, entity)?;
            f32(r, w, d)
        }
        "painting/variant" => holder(r, w, d, keep, |r, w, d| {
            varint(r, w, d)?;
            varint(r, w, d)?;
            string(r, w, d)?;
            optional(r, w, d, nbt)?;
            optional(r, w, d, nbt)
        }),
        "sign_text_front" | "sign_text_back" => {
            (0..4).try_for_each(|_| nbt(r, w, d))?;
            optional(r, w, d, |r, w, d| (0..4).try_for_each(|_| nbt(r, w, d)))?;
            varint(r, w, d)?;
            boolean(r, w, d)
        }
        other => Err(error(format!("unsupported item component {other}"))),
    }
}

fn keep(_: Dir, id: i32) -> i32 {
    id
}
pub fn boolean(r: &mut Reader, w: &mut Writer, _: Dir) -> Result<()> {
    w.bool(r.bool()?);
    Ok(())
}
pub fn varint(r: &mut Reader, w: &mut Writer, _: Dir) -> Result<()> {
    w.varint(r.varint()?);
    Ok(())
}
pub fn f32(r: &mut Reader, w: &mut Writer, _: Dir) -> Result<()> {
    w.f32(r.f32()?);
    Ok(())
}
pub fn i32(r: &mut Reader, w: &mut Writer, _: Dir) -> Result<()> {
    w.i32(r.i32()?);
    Ok(())
}
pub fn i64(r: &mut Reader, w: &mut Writer, _: Dir) -> Result<()> {
    w.i64(r.i64()?);
    Ok(())
}
pub fn string(r: &mut Reader, w: &mut Writer, _: Dir) -> Result<()> {
    w.string(&r.string(32767)?);
    Ok(())
}
pub fn nbt(r: &mut Reader, w: &mut Writer, _: Dir) -> Result<()> {
    let start = r.position();
    r.nbt()?;
    w.bytes(r.since(start));
    Ok(())
}
pub fn optional(r: &mut Reader, w: &mut Writer, d: Dir, value: impl FnOnce(&mut Reader, &mut Writer, Dir) -> Result<()>) -> Result<()> {
    let present = r.bool()?;
    w.bool(present);
    if present { value(r, w, d) } else { Ok(()) }
}
pub fn list(r: &mut Reader, w: &mut Writer, d: Dir, mut element: impl FnMut(&mut Reader, &mut Writer, Dir) -> Result<()>) -> Result<()> {
    let n = r.count(r.remaining())?;
    w.varint(n as i32);
    (0..n).try_for_each(|_| element(r, w, d))
}
/// ByteBufCodecs.holder: 0 and the value itself, or the registry id plus one.
pub fn holder(
    r: &mut Reader,
    w: &mut Writer,
    d: Dir,
    map: fn(Dir, i32) -> i32,
    direct: impl FnOnce(&mut Reader, &mut Writer, Dir) -> Result<()>,
) -> Result<()> {
    match r.varint()? {
        0 => {
            w.varint(0);
            direct(r, w, d)
        }
        id => {
            w.varint(map(d, id - 1) + 1);
            Ok(())
        }
    }
}
/// ByteBufCodecs.holderSet: 0 and a tag, or the size plus one and the ids.
pub fn holder_set(r: &mut Reader, w: &mut Writer, d: Dir, map: fn(Dir, i32) -> i32) -> Result<()> {
    match r.varint()? {
        0 => {
            w.varint(0);
            string(r, w, d)
        }
        n => {
            let n = usize::try_from(n - 1).map_err(|_| error("negative holder set size"))?;
            if n > r.remaining() {
                return Err(error("holder set past the end"));
            }
            w.varint(n as i32 + 1);
            (0..n).try_for_each(|_| {
                w.varint(map(d, r.varint()?));
                Ok(())
            })
        }
    }
}
/// SoundEvent.STREAM_CODEC: a sound id holder, or a location and a range.
pub fn sound_holder(r: &mut Reader, w: &mut Writer, d: Dir) -> Result<()> {
    holder(r, w, d, sound, |r, w, d| {
        string(r, w, d)?;
        optional(r, w, d, f32)
    })
}
/// Filterable: the raw value, then the filtered one if any.
fn filterable(r: &mut Reader, w: &mut Writer, d: Dir, value: Copy) -> Result<()> {
    value(r, w, d)?;
    optional(r, w, d, value)
}
/// ResolvableInt/ResolvableFloat: a constant (true) or a provider's key.
fn resolvable(r: &mut Reader, w: &mut Writer, d: Dir, constant: Copy) -> Result<()> {
    let is_constant = r.bool()?;
    w.bool(is_constant);
    if is_constant { constant(r, w, d) } else { string(r, w, d) }
}
/// MobEffectInstance: the effect, then Details (amplifier, duration, ambient,
/// particles, icon, hidden effect).
fn mob_effect(r: &mut Reader, w: &mut Writer, d: Dir) -> Result<()> {
    varint(r, w, d)?;
    effect_details(r, w, d)
}
fn effect_details(r: &mut Reader, w: &mut Writer, d: Dir) -> Result<()> {
    varint(r, w, d)?;
    varint(r, w, d)?;
    (0..3).try_for_each(|_| boolean(r, w, d))?;
    optional(r, w, d, effect_details)
}
/// ConsumeEffect: its type, then that type's fields. teleport_randomly
/// gained `directionalParticles` in 26.3.
fn consume_effect(r: &mut Reader, w: &mut Writer, d: Dir) -> Result<()> {
    let kind = r.varint()?;
    w.varint(kind);
    match kind {
        0 => {
            list(r, w, d, mob_effect)?;
            f32(r, w, d)
        }
        1 => holder_set(r, w, d, keep),
        2 => Ok(()),
        3 => {
            f32(r, w, d)?;
            match d {
                Dir::Up => w.bool(false),
                Dir::Down => drop(r.bool()?),
            }
            Ok(())
        }
        4 => sound_holder(r, w, d),
        other => Err(error(format!("unknown consume effect type {other}"))),
    }
}
/// BlockPredicate: blocks, state properties, NBT, exact components, partial
/// component predicates.
fn block_predicate(r: &mut Reader, w: &mut Writer, d: Dir) -> Result<()> {
    optional(r, w, d, |r, w, d| holder_set(r, w, d, block))?;
    optional(r, w, d, |r, w, d| {
        list(r, w, d, |r, w, d| {
            string(r, w, d)?;
            let exact = r.bool()?;
            w.bool(exact);
            if exact {
                string(r, w, d)
            } else {
                optional(r, w, d, string)?;
                optional(r, w, d, string)
            }
        })
    })?;
    optional(r, w, d, nbt)?;
    exact_components(r, w, d)?;
    // Either a predicate type (true) or a component type (false), then NBT.
    list(r, w, d, |r, w, d| {
        let predicate = r.bool()?;
        w.bool(predicate);
        let id = r.varint()?;
        if predicate {
            w.varint(id);
        } else {
            w.varint(component(d, id).ok_or_else(|| error("component predicate"))?);
        }
        nbt(r, w, d)
    })
}
/// DataComponentExactPredicate: typed components.
pub fn exact_components(r: &mut Reader, w: &mut Writer, d: Dir) -> Result<()> {
    let n = r.count(r.remaining())?;
    let mut body = Writer::new();
    let mut written = 0;
    let mut seen = Vec::new();
    for _ in 0..n {
        let kind = r.varint()?;
        written += typed(kind, r, &mut body, d, &mut seen)?;
    }
    w.varint(written as i32);
    w.bytes(&body.data);
    Ok(())
}
/// ItemAttributeModifiers.Entry.
fn attribute_modifier(r: &mut Reader, w: &mut Writer, d: Dir) -> Result<()> {
    varint(r, w, d)?;
    string(r, w, d)?;
    w.f64(r.f64()?);
    varint(r, w, d)?;
    varint(r, w, d)?;
    let display = r.varint()?;
    w.varint(display);
    if display == 2 { nbt(r, w, d) } else { Ok(()) }
}
fn firework_explosion(r: &mut Reader, w: &mut Writer, d: Dir) -> Result<()> {
    varint(r, w, d)?;
    list(r, w, d, i32)?;
    list(r, w, d, i32)?;
    boolean(r, w, d)?;
    boolean(r, w, d)
}
/// ResolvableProfile: a full or partial GameProfile, then the skin patch.
fn profile(r: &mut Reader, w: &mut Writer, d: Dir) -> Result<()> {
    let full = r.u8()?;
    w.u8(full);
    if full == 0 {
        optional(r, w, d, string)?;
        optional(r, w, d, |r, w, _| {
            w.uuid(r.uuid()?);
            Ok(())
        })?;
    } else {
        w.uuid(r.uuid()?);
        string(r, w, d)?;
    }
    list(r, w, d, |r, w, d| {
        string(r, w, d)?;
        string(r, w, d)?;
        optional(r, w, d, string)
    })?;
    (0..4).try_for_each(|_| {
        optional(r, w, d, |r, w, d| {
            string(r, w, d)?;
            string(r, w, d)
        })
    })
}
/// The trim material holder: 26.2 inlines (assetName, override map,
/// description), 26.3 (paletteId, description).
fn trim_material(r: &mut Reader, w: &mut Writer, d: Dir) -> Result<()> {
    holder(r, w, d, keep, |r, w, d| match d {
        Dir::Up => {
            let asset = r.string(32767)?;
            let n = r.count(r.remaining())?;
            for _ in 0..n {
                r.string(32767)?;
                r.string(32767)?;
            }
            w.string(&palette_id(&asset));
            nbt(r, w, d)
        }
        Dir::Down => {
            let palette = r.string(32767)?;
            w.string(&asset_name(&palette));
            w.varint(0);
            nbt(r, w, d)
        }
    })
}
/// `asset_name` -> `minecraft:trim/<asset_name>`.
pub fn palette_id(asset: &str) -> String {
    let path = asset.strip_prefix("minecraft:").unwrap_or(asset);
    format!("minecraft:trim/{path}")
}
pub fn asset_name(palette: &str) -> String {
    let path = palette.split_once(':').map_or(palette, |(_, p)| p);
    path.strip_prefix("trim/").unwrap_or(path).to_owned()
}
/// PotDecorations: 26.2 sends item ids, 26.3 four optional item templates
/// (back, left, right, front).
fn pot_decorations(r: &mut Reader, w: &mut Writer, d: Dir) -> Result<()> {
    match d {
        Dir::Up => {
            let n = r.count(r.remaining())?;
            let ids: Vec<i32> = (0..n).map(|_| r.varint()).collect::<Result<_>>()?;
            for i in 0..4 {
                match ids.get(i) {
                    Some(&id) if id >= 0 => {
                        w.bool(true);
                        w.varint(item(d, id));
                        w.varint(1);
                        w.varint(0);
                        w.varint(0);
                    }
                    _ => w.bool(false),
                }
            }
        }
        Dir::Down => {
            w.varint(4);
            for _ in 0..4 {
                if r.bool()? {
                    let id = r.varint()?;
                    r.varint()?;
                    // The template's patch: read past it.
                    patch(r, &mut Writer::new(), d)?;
                    w.varint(item(d, id));
                } else {
                    w.varint(tables::item::old::BRICK);
                }
            }
        }
    }
    Ok(())
}

/// HashedStack (a container click's view of a slot): the item, count, the
/// added components' type ids with hashes, the removed type ids. Hashes of
/// components that changed shape no longer match, so the server resends
/// those slots.
pub fn hashed(r: &mut Reader, w: &mut Writer, d: Dir) -> Result<()> {
    let present = r.bool()?;
    w.bool(present);
    if !present {
        return Ok(());
    }
    w.varint(item(d, r.varint()?));
    w.varint(r.varint()?);
    let n = r.count(256)?;
    let mut added = Vec::new();
    for _ in 0..n {
        let (kind, hash) = (r.varint()?, r.i32()?);
        for k in removed_ids(d, kind) {
            if !added.iter().any(|(a, _)| *a == k) {
                added.push((k, hash));
            }
        }
    }
    w.varint(added.len() as i32);
    for (k, hash) in added {
        w.varint(k);
        w.i32(hash);
    }
    let n = r.count(256)?;
    let mut removed = Vec::new();
    for _ in 0..n {
        for k in removed_ids(d, r.varint()?) {
            if !removed.contains(&k) {
                removed.push(k);
            }
        }
    }
    w.varint(removed.len() as i32);
    removed.iter().for_each(|k| w.varint(*k));
    Ok(())
}
