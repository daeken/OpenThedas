//! Which models draw the simulation's creatures and placeables. Reads the simulation and its
//! tables; never writes either.
//!
//! - A placeable's appearance row in `placeable_types.gda` names its model (`ModelName`,
//!   e.g. `PLC_SmFerDrB_01_0`).
//! - A creature's appearance row in `apr_base.gda` gives a model type (`ModelType`) and,
//!   for whole-body types, the model (`ModelName`, e.g. `C_DOGa`, `CN_BDY_HLKa`), which the
//!   resources name with a level-of-detail suffix: `c_doga_0.mmh`. Part-based types (`P`:
//!   humans, elves, dwarves, qunari) are assembled from body parts named
//!   `<race><gender>_<type>_<subtype><variation>_0` (`hm_arm_hvya_0`).
//! - Equipment: an item's `BaseItem` row in `BITM_base.gda` names its variation table (by
//!   its `m2da_base.gda` id), whose `ModelVariation` row gives the model's type, subtype and
//!   variation letters. Worn items replace or add body parts; weapons (`w_lsw_ls01a_0`) hang
//!   from the body's attachment nodes.
//! - Heads: a creature template's `Appearance Data` names a head morph (`HeadMorph`, a
//!   `.mor`), which names the head, eyes, hair, beard and lashes and carries the face's
//!   shape and colours. Part-based creatures without one (the hero, made in character
//!   creation) get the race's base head (`hm_uhm_basa_0`, eyes and lashes; provisional).

use thedas_formats::gda::Cell;
use thedas_script::vm::ObjectId;
use thedas_sim::Sim;
use thedas_sim::world::{Object, ObjectKind};

/// A model hung from a node of the body (a weapon in a hand).
#[derive(Clone, Debug, PartialEq)]
pub struct Attached {
    pub model: String,
    pub node: String,
}

/// The model resources (without `.mmh`) that draw one object, and where it stands.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectModel {
    pub id: ObjectId,
    pub tag: String,
    pub kind: ObjectKind,
    /// The body (or the placeable): models sharing the object's placement.
    pub models: Vec<String>,
    pub attached: Vec<Attached>,
    /// The head morph resource (without `.mor`), if any.
    pub morph: Option<String>,
    /// A helmet hides the morph's hair and beard (provisional).
    pub hide_hair: bool,
    /// What the creature's body is doing, for its skeleton.
    pub motion: Option<Motion>,
    pub position: [f32; 3],
    /// The game's facing (degrees about Z; see `thedas_sim::world::heading_of`).
    pub facing: f32,
}

/// The level-of-detail suffix of the most detailed model.
pub const LOD0: &str = "_0";

/// The body parts of a part-based creature, each with the unclothed variation it wears when
/// that slot is empty. Provisional: the parts the toolset's naming suggests.
pub const BODY_PARTS: [(&str, &str); 3] = [
    ("arm", "arm_nuda"),
    ("boo", "boo_nuda"),
    ("glv", "glv_nuda"),
];

/// `BITM_base.gda`: the `m2da_base.gda` id of a base item's variation table (column name not
/// known; found by matching every base item against the variation tables' ids).
pub const COL_VARIATION_TABLE: u32 = 0x43b5_4707;

/// Where weapons hang: the main hand's and the off hand's attachment nodes, under the
/// wrists of the humanoid skeletons. Provisional: weapons are shown drawn, whatever the
/// creature is doing.
pub const MAIN_HAND_NODE: &str = "Prop01";
pub const OFF_HAND_NODE: &str = "Prop02";

fn text(cell: Option<&Cell>) -> Option<String> {
    match cell? {
        Cell::String(s) => {
            let s = s.trim_end_matches('\0').trim();
            (!s.is_empty()).then(|| s.to_string())
        }
        _ => None,
    }
}

/// `apr_base.gda`'s race letter (`ModelRace`) and the creature's gender as model prefix letters
/// (`hm`, `ef`, ...). Gender 2 is female, anything else male.
fn part_prefix(race: &str, gender: i32) -> Option<String> {
    let r = race.chars().next()?.to_ascii_lowercase();
    if !matches!(r, 'h' | 'e' | 'd' | 'q') {
        return None;
    }
    let g = if gender == 2 { 'f' } else { 'm' };
    Some(format!("{r}{g}"))
}

/// An item's model letters from its variation table: type (`arm`, `lsw`), subtype and
/// variation together (`hvya`, `ls01a`). `None` for items without a model (creature
/// weapons, rings).
pub fn item_variation(sim: &Sim, item: &Object) -> Option<(String, String)> {
    let tables = sim.world.tables();
    let base = *item.fields.get("BaseItem")? as i64;
    let row = *item.fields.get("ModelVariation")? as i64;
    let bitm = tables.get(0, "bitm_base")?;
    let id = match bitm.cell_by_hash(COL_VARIATION_TABLE, base)? {
        Cell::Int(v) if *v > 0 => *v as i32,
        _ => return None,
    };
    let t = tables.get(id, "")?;
    let ty = text(t.cell("ModelType", row))?.to_ascii_lowercase();
    let sub = text(t.cell("ModelSubType", row)).unwrap_or_default();
    let var = text(t.cell("ModelVariation", row)).unwrap_or_default();
    Some((ty, format!("{sub}{var}").to_ascii_lowercase()))
}

fn has_model(sim: &Sim, model: &str) -> bool {
    sim.vfs.lookup(&format!("{model}.mmh")).is_some()
}

/// The head morph a creature's template names (`Appearance Data` / `HeadMorph`), if the
/// resource exists.
pub fn head_morph(sim: &Sim, o: &Object) -> Option<String> {
    use thedas_formats::gff3::{Gff3, Struct, Value};
    fn field<'a>(s: &'a Struct, label: &str) -> Option<&'a Value> {
        s.fields.iter().find(|f| f.label == label).map(|f| &f.value)
    }
    if o.template.is_empty() {
        return None;
    }
    let (data, _) = sim.vfs.read(&format!("{}.utc", o.template)).ok()?;
    let g = Gff3::parse(&data).ok()?;
    let app = match field(g.structs.first()?, "Appearance Data")? {
        Value::Struct(i) => g.structs.get(*i as usize)?,
        _ => return None,
    };
    let name = match field(app, "HeadMorph")? {
        Value::ExoString(b) | Value::ResRef(b) => String::from_utf8_lossy(b)
            .trim_end_matches('\0')
            .trim()
            .to_ascii_lowercase(),
        _ => return None,
    };
    let stem = name.strip_suffix(".mor").unwrap_or(&name).to_string();
    (!stem.is_empty() && sim.vfs.lookup(&format!("{stem}.mor")).is_some()).then_some(stem)
}

/// The race's base head for a part-based creature without a morph: head, eyes, lashes.
fn base_head(sim: &Sim, prefix: &str) -> Vec<String> {
    ["uhm_basa", "uem_basa", "ulm_basa"]
        .iter()
        .map(|p| format!("{prefix}_{p}{LOD0}"))
        .filter(|m| has_model(sim, m))
        .collect()
}

/// What draws one creature or placeable.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drawn {
    pub models: Vec<String>,
    pub attached: Vec<Attached>,
    pub morph: Option<String>,
    pub hide_hair: bool,
}

/// What draws one creature or placeable; no models when it has nothing to draw.
pub fn object_models(sim: &Sim, o: &Object) -> Drawn {
    let tables = sim.world.tables();
    match o.kind {
        ObjectKind::Placeable => {
            let Some(t) = tables.get(0, "placeable_types") else {
                return Drawn::default();
            };
            Drawn {
                models: text(t.cell("ModelName", o.appearance as i64))
                    .map(|m| vec![m.to_ascii_lowercase()])
                    .unwrap_or_default(),
                ..Default::default()
            }
        }
        ObjectKind::Creature => {
            let Some(t) = tables.get(0, "apr_base") else {
                return Drawn::default();
            };
            let app = o.appearance as i64;
            let kind = text(t.cell("ModelType", app)).unwrap_or_default();
            let morph = head_morph(sim, o);
            let mut hide_hair = false;
            let models = if kind.eq_ignore_ascii_case("P") {
                let race = text(t.cell("ModelRace", app)).unwrap_or_default();
                let gender = o.fields.get("Gender").copied().unwrap_or(1);
                match part_prefix(&race, gender) {
                    Some(p) => {
                        let (mut parts, helmet) = worn_parts(sim, o, &p);
                        hide_hair = helmet;
                        if morph.is_none() {
                            parts.extend(base_head(sim, &p));
                        }
                        parts
                    }
                    None => vec![],
                }
            } else {
                text(t.cell("ModelName", app))
                    .map(|m| vec![format!("{}{LOD0}", m.to_ascii_lowercase())])
                    .unwrap_or_default()
            };
            if models.is_empty() {
                return Drawn::default();
            }
            Drawn {
                models,
                attached: weapons(sim, o),
                morph,
                hide_hair,
            }
        }
        _ => Drawn::default(),
    }
}

fn slot(sim: &Sim, name: &str) -> i32 {
    thedas_sim::sim::constant(&sim.table, name)
}

fn equipped<'a>(sim: &'a Sim, o: &Object, slot_name: &str) -> Option<&'a Object> {
    sim.world.get(*o.equipment.get(&slot(sim, slot_name))?)
}

/// A part-based creature's body: each body part from the item in its slot (chest, boots,
/// gloves), or the unclothed part, plus a helmet when one is worn (and whether one is). A
/// worn item whose model this race lacks leaves the unclothed part.
fn worn_parts(sim: &Sim, o: &Object, prefix: &str) -> (Vec<String>, bool) {
    let slots = [
        "INVENTORY_SLOT_CHEST",
        "INVENTORY_SLOT_BOOTS",
        "INVENTORY_SLOT_GLOVES",
    ];
    let worn = |slot_name: &str| {
        let (ty, v) = item_variation(sim, equipped(sim, o, slot_name)?)?;
        let model = format!("{prefix}_{ty}_{v}{LOD0}");
        has_model(sim, &model).then_some(model)
    };
    let mut out: Vec<String> = BODY_PARTS
        .iter()
        .zip(slots)
        .map(|((_, nude), slot_name)| {
            worn(slot_name).unwrap_or_else(|| format!("{prefix}_{nude}{LOD0}"))
        })
        .collect();
    let helmet = worn("INVENTORY_SLOT_HEAD");
    let has_helmet = helmet.is_some();
    out.extend(helmet);
    (out, has_helmet)
}

/// The creature's weapons in its hands.
fn weapons(sim: &Sim, o: &Object) -> Vec<Attached> {
    [
        ("INVENTORY_SLOT_MAIN", MAIN_HAND_NODE),
        ("INVENTORY_SLOT_OFFHAND", OFF_HAND_NODE),
    ]
    .into_iter()
    .filter_map(|(slot_name, node)| {
        let (ty, v) = item_variation(sim, equipped(sim, o, slot_name)?)?;
        let model = format!("w_{ty}_{v}{LOD0}");
        has_model(sim, &model).then(|| Attached {
            model,
            node: node.to_string(),
        })
    })
    .collect()
}

/// An animation to play: the action in the first of `trees` (`.anb`) that has it, at `time`
/// seconds (wrapped when `looping`, else held at the end).
#[derive(Clone, Debug, PartialEq)]
pub struct Motion {
    pub trees: Vec<String>,
    pub action: String,
    pub time: f32,
    pub looping: bool,
}

/// `apr_base.gda`'s animation trees: ambient and combat (column names not known), movement
/// out of and in combat.
pub const COL_AMBIENT_TREE: u32 = 0x5009_76ec;
pub const COL_COMBAT_TREE: u32 = 0xde80_b418;

/// Actions in the trees (from `humanoid.anb`, `combat_humanoid.anb`, `movement.anb`).
pub mod action {
    pub const PAUSE: &str = "pause";
    pub const RUN: &str = "runf";
    pub const ATTACK: &str = "attk_v1";
    pub const DIE: &str = "die";
}

/// How long an attack plays when the script set no duration (milliseconds). Provisional.
pub const DEFAULT_ATTACK_MS: u64 = 1500;

/// What a creature's body is doing: dead (the death animation's last pose: when it died
/// is not kept), attacking (the attack animation over the attack's duration), moving (a
/// run), or standing (pause, the combat stance when fighting). Looping animations start at
/// a different point per creature so a crowd does not move in step. Provisional: the
/// original picks among variants (`attk_v1`..`v6`, weapon styles) that are not modelled.
pub fn motion(sim: &Sim, o: &Object) -> Option<Motion> {
    if o.kind != ObjectKind::Creature {
        return None;
    }
    let t = sim.world.tables().get(0, "apr_base")?;
    let app = o.appearance as i64;
    let tree = |hash: u32| match t.cell_by_hash(hash, app) {
        Some(Cell::String(s)) => {
            let s = s.trim_end_matches('\0').trim().to_ascii_lowercase();
            (!s.is_empty()).then_some(s)
        }
        _ => None,
    };
    let ambient = tree(COL_AMBIENT_TREE);
    let combat = tree(COL_COMBAT_TREE);
    let movement = text(t.cell("MovementTree", app)).map(|s| s.to_ascii_lowercase());
    let combat_move = text(t.cell("CombatMoveTree", app)).map(|s| s.to_ascii_lowercase());
    let now = sim.world.time_ms;
    let secs = |ms: u64| ms as f32 / 1000.0;
    let phase = (o.id % 97) as f32 * 0.37;
    let trees = |list: &[&Option<String>]| -> Vec<String> {
        list.iter().filter_map(|t| (*t).clone()).collect()
    };
    let fighting = o.combat.in_combat;
    if o.dead {
        return Some(Motion {
            trees: trees(&[&ambient, &combat]),
            action: action::DIE.into(),
            time: f32::MAX,
            looping: false,
        });
    }
    if let Some(c) = o.commands.first() {
        if c.from.is_some() {
            let list = if fighting {
                trees(&[&combat_move, &movement])
            } else {
                trees(&[&movement, &combat_move])
            };
            return Some(Motion {
                trees: list,
                action: action::RUN.into(),
                time: secs(now) + phase,
                looping: true,
            });
        }
        if c.name == "CommandAttack" {
            if let Some(due) = c.due {
                let length = o.combat.attack_ms.unwrap_or(DEFAULT_ATTACK_MS);
                let start = due.saturating_sub(length);
                return Some(Motion {
                    trees: trees(&[&combat, &ambient]),
                    action: action::ATTACK.into(),
                    time: secs(now.saturating_sub(start)),
                    looping: false,
                });
            }
        }
    }
    Some(Motion {
        trees: if fighting {
            trees(&[&combat, &ambient])
        } else {
            trees(&[&ambient, &combat])
        },
        action: action::PAUSE.into(),
        time: secs(now) + phase,
        looping: true,
    })
}

/// Every active creature and placeable in `area` that has a model, in id order.
pub fn area_models(sim: &Sim, area: ObjectId) -> Vec<ObjectModel> {
    sim.world
        .objects
        .values()
        .filter(|o| o.area == Some(area) && o.active)
        .filter(|o| matches!(o.kind, ObjectKind::Creature | ObjectKind::Placeable))
        .filter_map(|o| {
            let d = object_models(sim, o);
            (!d.models.is_empty()).then(|| ObjectModel {
                id: o.id,
                tag: o.tag.clone(),
                kind: o.kind,
                models: d.models,
                attached: d.attached,
                morph: d.morph,
                hide_hair: d.hide_hair,
                motion: motion(sim, o),
                position: o.position,
                facing: o.facing,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_prefixes_follow_race_and_gender() {
        assert_eq!(part_prefix("H", 1).as_deref(), Some("hm"));
        assert_eq!(part_prefix("E", 2).as_deref(), Some("ef"));
        assert_eq!(part_prefix("d", 0).as_deref(), Some("dm"));
        assert_eq!(part_prefix("C", 1), None);
        assert_eq!(part_prefix("", 1), None);
    }

    #[test]
    fn items_name_their_models_through_the_variation_tables() {
        use std::sync::Arc;
        use thedas_script::EngineTable;
        use thedas_vfs::Vfs;
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let t = s.world.tables();
        let st = |v: &str| Cell::String(v.into());
        t.insert(
            "bitm_base",
            &["Label", "#43b54707"],
            vec![
                (2, vec![st("Longsword"), Cell::Int(30)]),
                (23, vec![st("Creature claw"), Cell::Int(0)]),
            ],
        );
        t.insert(
            "#30",
            &["Label", "ModelType", "ModelSubType", "ModelVariation"],
            vec![(1, vec![st("Longsword 1"), st("LSW\0"), st("ls01"), st("a")])],
        );
        let mut item = |base: i32, variation: i32| {
            let id = s.world.spawn(ObjectKind::Item, "item");
            let o = s.world.get_mut(id).unwrap();
            o.fields.insert("BaseItem".into(), base);
            o.fields.insert("ModelVariation".into(), variation);
            id
        };
        let (sword, claw, unknown) = (item(2, 1), item(23, 1), item(2, 9));
        let var = |id| item_variation(&s, s.world.get(id).unwrap());
        assert_eq!(var(sword), Some(("lsw".into(), "ls01a".into())));
        assert_eq!(var(claw), None, "no variation table");
        assert_eq!(var(unknown), None, "no such variation");
    }

    #[test]
    fn text_cells_lose_their_terminator() {
        assert_eq!(
            text(Some(&Cell::String("C_DOGa\0".into()))).as_deref(),
            Some("C_DOGa")
        );
        assert_eq!(text(Some(&Cell::String("\0".into()))), None);
        assert_eq!(text(Some(&Cell::Int(3))), None);
    }
}
