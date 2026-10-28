//! The configuration phase: registry data (trim materials, enchantment
//! terms), tags, and the registries 26.3 synchronizes that a 26.2 server
//! has built in (ViaVersion's RegistryDataRewriter26_3 and Protocol26_2To26_3).
use super::{Dir, block, entity, item, mapped, tables};
use crate::{Out, wire::Rw};
use horizon_protocol::{
    Result,
    buf::{Reader, Writer},
    translate::Packet,
};
use horizon_world::nbt::{JavaString, Tag};
use std::sync::OnceLock;

use super::cfg_cb::{new as n, old as o};

/// 26.3 moved these decorated pot patterns from a built-in registry to a
/// synchronized one.
const POT_PATTERNS: &[&str] = &[
    "angler", "archer", "arms_up", "blade", "brewer", "burn", "danger", "explorer", "flow",
    "friend", "guster", "heart", "heartbreak", "howl", "miner", "mourner", "plenty", "prize",
    "scrape", "sheaf", "shelter", "skull", "snort",
];

pub fn clientbound(packet: Packet, out: &mut Out) -> Result<()> {
    let Some(id) = mapped(&super::cfg_cb::MAP, packet.id) else {
        return Ok(());
    };
    match packet.id {
        o::REGISTRY_DATA => out.push(id, registry_data(&packet.data)?),
        o::UPDATE_TAGS => update_tags(id, &packet.data, out)?,
        o::FINISH_CONFIGURATION => {
            out.push(n::REGISTRY_DATA, pot_patterns());
            out.push(n::REGISTRY_DATA, block_transformers()?);
            out.push(id, packet.data);
        }
        _ => out.push(id, packet.data),
    }
    Ok(())
}

/// ClientboundUpdateTagsPacket (configuration and play): the ids of the
/// remapped registries, and the tags 26.3 has that 26.2 lacks.
pub fn update_tags(id: i32, data: &[u8], out: &mut Out) -> Result<()> {
    let d = Dir::Up;
    let mut rw = Rw::new(data);
    let mut seen = Vec::new();
    let registries = rw.r.count(rw.r.remaining())?;
    let mut body = Writer::new();
    for _ in 0..registries {
        let registry = rw.r.string(32767)?;
        let map: fn(Dir, i32) -> i32 = match registry.as_str() {
            "minecraft:block" => block,
            "minecraft:item" => item,
            "minecraft:entity_type" => entity,
            _ => |_, id| id,
        };
        let n = rw.r.count(rw.r.remaining())?;
        let mut tags = Vec::with_capacity(n);
        for _ in 0..n {
            let name = rw.r.string(32767)?;
            let k = rw.r.count(rw.r.remaining())?;
            let ids = (0..k)
                .map(|_| Ok(map(d, rw.r.varint()?)))
                .collect::<Result<Vec<_>>>()?;
            tags.push((name, ids));
        }
        add_new_tags(&registry, &mut tags);
        write_tags(&mut body, &registry, &tags);
        seen.push(registry);
    }
    let mut extra = 0;
    let mut tail = Writer::new();
    for (registry, _, _) in tables::NEW_TAGS {
        if seen.iter().any(|s| s == registry) {
            continue;
        }
        seen.push((*registry).to_owned());
        let mut tags = Vec::new();
        add_new_tags(registry, &mut tags);
        write_tags(&mut tail, registry, &tags);
        extra += 1;
    }
    let mut w = Writer::new();
    w.varint((registries + extra) as i32);
    w.bytes(&body.data);
    w.bytes(&tail.data);
    out.push(id, w.data);
    Ok(())
}

fn add_new_tags(registry: &str, tags: &mut Vec<(String, Vec<i32>)>) {
    for (reg, tag, ids) in tables::NEW_TAGS {
        if *reg == registry && !tags.iter().any(|(t, _)| t == tag) {
            tags.push(((*tag).to_owned(), ids.iter().map(|&i| i32::from(i)).collect()));
        }
    }
}

fn write_tags(w: &mut Writer, registry: &str, tags: &[(String, Vec<i32>)]) {
    w.string(registry);
    w.varint(tags.len() as i32);
    for (name, ids) in tags {
        w.string(name);
        w.varint(ids.len() as i32);
        ids.iter().for_each(|i| w.varint(*i));
    }
}

/// ClientboundRegistryDataPacket: the registry, then (id, optional data).
fn registry_data(data: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(data);
    let registry = r.string(32767)?;
    let n = r.count(r.remaining())?;
    let mut w = Writer::new();
    w.string(&registry);
    w.varint(n as i32);
    for _ in 0..n {
        w.string(&r.string(32767)?);
        let present = r.bool()?;
        w.bool(present);
        if present {
            let mut tag = r.nbt()?;
            match registry.as_str() {
                "minecraft:trim_material" => trim_material(&mut tag),
                "minecraft:enchantment" => enchantment_terms(&mut tag),
                _ => {}
            }
            w.nbt(&tag);
        }
    }
    Ok(w.data)
}

fn key(name: &str) -> JavaString {
    JavaString::from(name)
}
fn string_of(tag: &Tag) -> Option<String> {
    match tag {
        Tag::String(s) => Some(s.to_string_lossy()),
        _ => None,
    }
}
fn strip(id: &str) -> &str {
    id.strip_prefix("minecraft:").unwrap_or(id)
}

/// A trim material names its palette (`trim/<asset_name>`) in 26.3.
fn trim_material(tag: &mut Tag) {
    let Tag::Compound(map) = tag else { return };
    if let Some(asset) = map.remove(&key("asset_name")).as_ref().and_then(string_of) {
        map.insert(
            key("palette_id"),
            Tag::String(item::palette_id(&asset).as_str().into()),
        );
    }
}

/// Every enchantment's conditions: `condition` became `type`, with the
/// renamed conditions and the number providers split into int and float
/// ones.
fn enchantment_terms(tag: &mut Tag) {
    match tag {
        Tag::Compound(map) => {
            for value in map.values_mut() {
                enchantment_terms(value);
            }
            if let Some(Tag::String(condition)) = map.get(&key("condition")).cloned() {
                update_term(map, &condition.to_string_lossy());
            }
        }
        Tag::List(list) => list.iter_mut().for_each(enchantment_terms),
        _ => {}
    }
}

type Compound = horizon_world::nbt::Compound;

fn update_term(term: &mut Compound, condition: &str) {
    let mut condition = condition.to_owned();
    term.remove(&key("condition"));
    let kind = strip(&condition).to_owned();
    match kind.as_str() {
        "value_check" => {
            condition = "minecraft:int_value_check".into();
            convert(term, "value", "value", to_int_provider);
            convert(term, "range", "test", int_range);
        }
        "time_check" => convert(term, "value", "value", int_range),
        "random_chance" => convert(term, "chance", "chance", to_float_provider),
        "damage_source_properties" => {
            if let Some(Tag::List(tags)) = term
                .get_mut(&key("predicate"))
                .and_then(|p| p.get_mut("tags"))
            {
                for t in tags {
                    if let Some(Tag::String(id)) = t.get_mut("id") {
                        *id = format!("#{}", id.to_string_lossy()).as_str().into();
                    }
                }
            }
        }
        "block_state_property" => {
            condition = "minecraft:match_block".into();
            if let Some(b) = term.remove(&key("block")) {
                term.insert(key("blocks"), b);
            }
            if let Some(p) = term.remove(&key("properties")) {
                term.insert(key("state"), p);
            }
        }
        _ => {}
    }
    term.insert(key("type"), Tag::String(condition.as_str().into()));
}

fn convert(tag: &mut Compound, from: &str, to: &str, f: fn(Tag) -> Tag) {
    if let Some(value) = tag.remove(&key(from)) {
        tag.insert(key(to), f(value));
    }
}
fn int_range(mut range: Tag) -> Tag {
    if let Tag::Compound(map) = &mut range {
        convert(map, "min", "min", to_int_provider);
        convert(map, "max", "max", to_int_provider);
    }
    range
}
fn number(tag: &Tag) -> Option<f32> {
    Some(match tag {
        Tag::Byte(v) => f32::from(*v),
        Tag::Short(v) => f32::from(*v),
        Tag::Int(v) => *v as f32,
        Tag::Long(v) => *v as f32,
        Tag::Float(bits) => f32::from_bits(*bits),
        Tag::Double(bits) => f64::from_bits(*bits) as f32,
        _ => return None,
    })
}
fn unary(kind: &str, input: Tag) -> Tag {
    Tag::compound([
        ("type", Tag::String(format!("minecraft:{kind}").as_str().into())),
        ("input", input),
    ])
}
/// Old number providers were floats that `getInt` rounded.
fn to_int_provider(tag: Tag) -> Tag {
    if let Some(v) = number(&tag) {
        return Tag::Int(v.round() as i32);
    }
    if let Tag::Compound(mut map) = tag {
        let kind = map.get(&key("type")).and_then(string_of).unwrap_or_default();
        match strip(&kind) {
            "binomial" => {
                convert(&mut map, "n", "n", to_int_provider);
                convert(&mut map, "p", "p", to_float_provider);
                return Tag::Compound(map);
            }
            "score" if map.get(&key("scale")).and_then(number).unwrap_or(1.0) == 1.0 => {
                map.remove(&key("scale"));
                return Tag::Compound(map);
            }
            _ => return unary("from_float", unary("round", to_float_provider(Tag::Compound(map)))),
        }
    }
    unary("from_float", unary("round", tag))
}
fn to_float_provider(tag: Tag) -> Tag {
    let Tag::Compound(mut map) = tag else {
        return tag;
    };
    // Untyped providers were uniform.
    let kind = map
        .get(&key("type"))
        .and_then(string_of)
        .unwrap_or_else(|| "uniform".into());
    match strip(&kind) {
        "uniform" => {
            map.insert(key("type"), Tag::String("minecraft:uniform".into()));
            convert(&mut map, "min", "min", to_float_provider);
            convert(&mut map, "max", "max", to_float_provider);
        }
        "sum" => {
            map.insert(key("type"), Tag::String("minecraft:add".into()));
            if let Some(Tag::List(summands)) = map.remove(&key("summands")) {
                let inputs = summands.into_iter().map(float_compound).collect();
                map.insert(key("inputs"), Tag::List(inputs));
            }
        }
        "binomial" => return unary("from_int", to_int_provider(Tag::Compound(map))),
        "score" => {
            let scale = map.remove(&key("scale"));
            let score = unary("from_int", Tag::Compound(map));
            return match scale {
                None => score,
                Some(scale) => Tag::compound([
                    ("type", Tag::String("minecraft:mul".into())),
                    ("inputs", Tag::List(vec![score, float_compound(scale)])),
                ]),
            };
        }
        _ => {}
    }
    Tag::Compound(map)
}
fn float_compound(tag: Tag) -> Tag {
    match to_float_provider(tag) {
        c @ Tag::Compound(_) => c,
        constant => Tag::compound([
            ("type", Tag::String("minecraft:constant".into())),
            ("value", constant),
        ]),
    }
}

/// REGISTRY_DATA for `minecraft:decorated_pot_pattern`.
fn pot_patterns() -> Vec<u8> {
    let mut w = Writer::new();
    w.string("minecraft:decorated_pot_pattern");
    w.varint(POT_PATTERNS.len() as i32);
    for pattern in POT_PATTERNS {
        w.string(&format!("minecraft:{pattern}"));
        w.bool(true);
        w.nbt(&Tag::compound([(
            "asset_id",
            Tag::String(format!("minecraft:{pattern}_pottery_pattern").as_str().into()),
        )]));
    }
    w.data
}

/// REGISTRY_DATA for `minecraft:block_transformer` (26.3's vanilla entries).
fn block_transformers() -> Result<Vec<u8>> {
    static ENTRIES: OnceLock<Vec<(String, Tag)>> = OnceLock::new();
    let entries = ENTRIES.get_or_init(|| {
        let bytes = include_bytes!("../../data/block-transformer-registry-26.3.nbt");
        match horizon_world::nbt::read_named(bytes, horizon_world::nbt::Limits::default()) {
            Ok((named, _)) => match named.tag {
                Tag::Compound(map) => map
                    .into_iter()
                    .map(|(k, v)| (k.to_string_lossy(), v))
                    .collect(),
                _ => Vec::new(),
            },
            Err(e) => {
                eprintln!("[multiversion] block transformer registry: {e}");
                Vec::new()
            }
        }
    });
    let mut w = Writer::new();
    w.string("minecraft:block_transformer");
    w.varint(entries.len() as i32);
    for (id, tag) in entries {
        w.string(id);
        w.bool(true);
        w.nbt(tag);
    }
    Ok(w.data)
}
