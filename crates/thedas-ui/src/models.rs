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
//!   shape and colours. A morph kept on the creature (the player's face from character
//!   creation) comes first. Part-based creatures with neither get the race's base head
//!   (`hm_uhm_basa_0`, eyes and lashes; provisional).
//! - Body tints: a whole-body creature's `Appearance Data` / `BodyTint` is a row of
//!   `body_tints.gda`, whose `TintFile` (`T3_DOG_O00.tnt`) colours the body through its
//!   material's tint mask. The mabari's diffuse texture is grey: its coat comes from this.

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
    /// Equipment tints: (model, `.tnt` without extension).
    pub tints: Vec<(String, String)>,
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

/// Where weapons hang when no table says: the main hand's and the off hand's attachment
/// nodes, under the wrists of the humanoid skeletons (`weaponparts.gda` rows 14 and 15).
pub const MAIN_HAND_NODE: &str = "Prop01";
pub const OFF_HAND_NODE: &str = "Prop02";

/// `BITM_base.gda`'s `weaponparts.gda` rows a base item hangs from (column names not known):
/// sheathed from the main hand's slot and from the off hand's, drawn in the main hand's and
/// in the off hand's. The executable picks among the four at 0x6a64b0 (drawn, and main
/// hand, select the column; the cutscenes' `ActionDrawWeapon`, 0x7d1210, reads them too).
/// A longsword is sheathed on the upper right back (`Prop10`) and drawn into the right hand
/// (`Prop01`); a shield goes on the back (`Prop11`) and onto the left forearm (`Prop03`); a
/// bow on the back (`Prop17`) and into the left hand (`Prop02`).
pub const COL_SHEATHED_MAIN: u32 = 0x7083_f7d4;
pub const COL_SHEATHED_OFF: u32 = 0xa4c2_c80b;
pub const COL_DRAWN_MAIN: u32 = 0x4885_9b70;
pub const COL_DRAWN_OFF: u32 = 0x9cc4_a4af;

/// `weaponparts.gda`'s attachment node name (`Prop10`; column name not known, read by the
/// executable at 0x7d3da2 and 0xa66bfb).
pub const COL_PART_NODE: u32 = 0x9002_8895;

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

/// A creature's head morph, if the resource exists: the one it keeps (the player's face
/// from character creation), else the one its template names (`Appearance Data` /
/// `HeadMorph`). Character creation's creature, before the face screen gives it a face,
/// wears the face screen's first head for its race and gender (`hm_cps_p01`: its hair,
/// beard and colours too); its templates (`default_player.utc`, the backgrounds'
/// `default_hm_noble.utc` and the rest) name none. `chargen.rim` carries exactly those
/// presets (`hm_cps_p01.mop` and the rest) and their hair and beard models for the first
/// pages, and the original's first page shows the hero with hair. Provisional: the
/// executable's choice of head there is not traced.
pub fn head_morph(sim: &Sim, o: &Object) -> Option<String> {
    let found = kept_or_template_morph(sim, o);
    if found.is_some() || sim.world.chargen != Some(o.id) {
        return found;
    }
    sim.face_screen_default(o.id)
}

fn kept_or_template_morph(sim: &Sim, o: &Object) -> Option<String> {
    // A kept face is a finished morph, or a face-screen preset (`.mop`) the renderer puts
    // on its race and gender's base face.
    if let Some(m) = o.head_morph.as_ref().filter(|m| {
        sim.vfs.lookup(&format!("{m}.mor")).is_some()
            || sim.vfs.lookup(&format!("{m}.mop")).is_some()
    }) {
        // With the choices character creation made over it (hair, beard).
        return Some(sim.face_name(o.id).unwrap_or_else(|| m.clone()));
    }
    use thedas_formats::gff3::Value;
    let name = match appearance_data(sim, o, "HeadMorph")? {
        Value::ExoString(b) | Value::ResRef(b) => String::from_utf8_lossy(&b)
            .trim_end_matches('\0')
            .trim()
            .to_ascii_lowercase(),
        _ => return None,
    };
    let stem = name.strip_suffix(".mor").unwrap_or(&name).to_string();
    (!stem.is_empty() && sim.vfs.lookup(&format!("{stem}.mor")).is_some()).then_some(stem)
}

/// Creature `o`'s gender: its `Gender` field, or its template's `Appearance Data` one (for
/// objects saved before the simulation read that struct), male when neither says.
fn creature_gender(sim: &Sim, o: &Object) -> i32 {
    use thedas_formats::gff3::Value;
    o.fields
        .get("Gender")
        .copied()
        .unwrap_or_else(|| match appearance_data(sim, o, "Gender") {
            Some(Value::Byte(v)) => v as i32,
            Some(Value::Char(v)) => v as i32,
            Some(Value::Word(v)) => v as i32,
            Some(Value::Short(v)) => v as i32,
            Some(Value::Dword(v)) => v as i32,
            Some(Value::Int(v)) => v,
            _ => 1,
        })
}

/// A field of creature `o`'s template's `Appearance Data` struct.
fn appearance_data(sim: &Sim, o: &Object, label: &str) -> Option<thedas_formats::gff3::Value> {
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
    field(app, label).cloned()
}

/// The `.tnt` (without extension) `body_tints.gda` gives tint `id` on appearance
/// `appearance`; id 0 is none.
///
/// The table numbers each appearance's tints on its own: rows share ids (63 is both "Dog -
/// Party Member - Tint 0", `T3_DOG_O00.tnt`, and "Deepstalker - jumper", `T3_DPS_B03.tnt`)
/// and an `Apr` column names the appearance each row is for. So the row is the one with this
/// id and appearance, else one with this id for no appearance (`Apr` 0). Provisional, from
/// the table's layout.
pub fn body_tint_file(gda: &thedas_formats::Gda, id: i64, appearance: i64) -> Option<String> {
    use thedas_formats::gda::column_hash;
    if id <= 0 {
        return None;
    }
    let col = |name: &str| gda.columns.iter().position(|c| c.hash == column_hash(name));
    let (file, apr) = (col("TintFile")?, col("Apr")?);
    let int = |row: &[Cell], i: usize| match row.get(i) {
        Some(Cell::Int(v)) => Some(*v),
        _ => None,
    };
    let rows = || gda.rows.iter().filter(|r| int(r, 0) == Some(id));
    let row = rows()
        .find(|r| int(r, apr) == Some(appearance))
        .or_else(|| rows().find(|r| int(r, apr).unwrap_or(0) == 0))?;
    let file = text(row.get(file))?.to_ascii_lowercase();
    let stem = file.strip_suffix(".tnt").unwrap_or(&file).to_string();
    (!stem.is_empty()).then_some(stem)
}

/// Creature `o`'s body tint (its template's `Appearance Data` / `BodyTint`), as a `.tnt`
/// without extension. Provisional: read for whole-body creatures only; scripts that change
/// it are not followed.
fn body_tint(sim: &Sim, o: &Object) -> Option<String> {
    use thedas_formats::gff3::Value;
    let row = match appearance_data(sim, o, "BodyTint")? {
        Value::Byte(v) => v as i64,
        Value::Char(v) => v as i64,
        Value::Word(v) => v as i64,
        Value::Short(v) => v as i64,
        Value::Dword(v) => v as i64,
        Value::Int(v) => v as i64,
        _ => return None,
    };
    let (data, _) = sim.vfs.read("body_tints.gda").ok()?;
    let gda = thedas_formats::Gda::parse(&data).ok()?;
    body_tint_file(&gda, row, o.appearance as i64)
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
    /// Equipment tints: (model, `.tnt` without extension).
    pub tints: Vec<(String, String)>,
    pub morph: Option<String>,
    pub hide_hair: bool,
}

/// What draws one creature or placeable; no models when it has nothing to draw.
pub fn object_models(sim: &Sim, o: &Object) -> Drawn {
    object_models_in(sim, o, Gear::World)
}

/// Where a creature's equipped items are found: in the world (its equipment names world
/// objects), or among items standing in for them (by inventory slot), for a look built from
/// a template rather than from a creature of the world (see [`template_look`]).
#[derive(Clone, Copy)]
enum Gear<'a> {
    World,
    Scratch(&'a [(i32, Object)]),
}

/// Creature `o` drawn as creature template `template` (`.utc`, without extension) makes it:
/// the template's appearance (`Appearance Data` / `Appearance_Type`), race and gender where
/// `o` has none of its own, its head morph and body tint, and the items its
/// `Equip_ItemList` wears (each entry's struct id is the `INVENTORY_SLOT_*`, as the
/// simulation reads it). The creature's own gender, appearance and kept face win; it stands
/// where `o` stands. `None` when the template cannot be read or draws nothing.
pub fn template_look(sim: &Sim, o: &Object, template: &str) -> Option<ObjectModel> {
    use thedas_formats::gff3::{Gff3, Struct, Value};
    fn field<'a>(s: &'a Struct, label: &str) -> Option<&'a Value> {
        s.fields.iter().find(|f| f.label == label).map(|f| &f.value)
    }
    fn int(v: Option<&Value>) -> Option<i32> {
        Some(match v? {
            Value::Byte(v) => *v as i32,
            Value::Char(v) => *v as i32,
            Value::Word(v) => *v as i32,
            Value::Short(v) => *v as i32,
            Value::Dword(v) => *v as i32,
            Value::Int(v) => *v,
            _ => return None,
        })
    }
    let read = |name: &str| -> Option<Gff3> {
        let (data, _) = sim.vfs.read(name).ok()?;
        Gff3::parse(&data).ok()
    };
    let g = read(&format!("{template}.utc"))?;
    let root = g.structs.first()?;
    let app = match field(root, "Appearance Data") {
        Some(Value::Struct(i)) => g.structs.get(*i as usize),
        _ => None,
    };
    let mut look = o.clone();
    look.template = template.to_string();
    if look.appearance == 0 {
        look.appearance = app.and_then(|a| int(field(a, "Appearance_Type")))?;
    }
    if !look.fields.contains_key("Race")
        && let Some(r) = int(field(root, "Race"))
    {
        look.fields.insert("Race".into(), r);
    }
    look.equipment.clear();
    let mut items = Vec::new();
    if let Some(Value::List(entries)) = field(root, "Equip_ItemList") {
        for e in entries.iter().filter_map(|&i| g.structs.get(i as usize)) {
            let resref = match field(e, "TemplateResRef") {
                Some(Value::ResRef(b) | Value::ExoString(b)) => String::from_utf8_lossy(b)
                    .trim_end_matches('\0')
                    .trim()
                    .to_ascii_lowercase(),
                _ => continue,
            };
            let Some(t) = read(&format!("{resref}.uti")) else {
                continue;
            };
            let Some(r) = t.structs.first() else { continue };
            let mut item = Object::new(0, ObjectKind::Item, &resref);
            item.template = resref;
            for f in &r.fields {
                if let Some(v) = int(Some(&f.value)) {
                    item.fields.insert(f.label.clone(), v);
                }
            }
            items.push((e.id as i32, item));
        }
    }
    let d = object_models_in(sim, &look, Gear::Scratch(&items));
    (!d.models.is_empty()).then(|| ObjectModel {
        id: o.id,
        tag: o.tag.clone(),
        kind: o.kind,
        models: d.models,
        attached: d.attached,
        tints: d.tints,
        morph: d.morph,
        hide_hair: d.hide_hair,
        motion: motion(sim, &look),
        position: o.position,
        facing: o.facing,
    })
}

/// [`object_models`] with the creature's items found in `gear`.
fn object_models_in(sim: &Sim, o: &Object, gear: Gear) -> Drawn {
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
            let mut prefix: Option<String> = None;
            let models = if kind.eq_ignore_ascii_case("P") {
                let race = text(t.cell("ModelRace", app)).unwrap_or_default();
                let gender = creature_gender(sim, o);
                match part_prefix(&race, gender) {
                    Some(p) => {
                        let (mut parts, helmet) = worn_parts(sim, o, &p, gear);
                        hide_hair = helmet;
                        prefix = Some(p.clone());
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
            let mut tints = equipment_tints(sim, o, prefix.as_deref(), gear);
            if prefix.is_none()
                && let Some(t) = body_tint(sim, o)
            {
                tints.push((models[0].clone(), t));
            }
            Drawn {
                models,
                attached: weapons(sim, o, gear),
                tints,
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
    equipped_in(sim, o, slot_name, Gear::World)
}

/// The item creature `o` wears in a slot, found in `gear`.
fn equipped_in<'a>(
    sim: &'a Sim,
    o: &Object,
    slot_name: &str,
    gear: Gear<'a>,
) -> Option<&'a Object> {
    let slot = slot(sim, slot_name);
    match gear {
        Gear::World => sim.world.get(*o.equipment.get(&slot)?),
        Gear::Scratch(items) => items.iter().find(|(s, _)| *s == slot).map(|(_, i)| i),
    }
}

/// A part-based creature's body: each body part from the item in its slot (chest, boots,
/// gloves), or the unclothed part, plus a helmet when one is worn (and whether one is). A
/// worn item whose model this race lacks leaves the unclothed part.
fn worn_parts(sim: &Sim, o: &Object, prefix: &str, gear: Gear) -> (Vec<String>, bool) {
    let slots = [
        "INVENTORY_SLOT_CHEST",
        "INVENTORY_SLOT_BOOTS",
        "INVENTORY_SLOT_GLOVES",
    ];
    let worn = |slot_name: &str| {
        let (ty, v) = item_variation(sim, equipped_in(sim, o, slot_name, gear)?)?;
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

/// The tint an item's material gives it (`Material` -> `materialtypes.gda` `TintFile`),
/// without the `.tnt` extension.
pub fn item_tint(sim: &Sim, item: &Object) -> Option<String> {
    let material = *item.fields.get("Material")? as i64;
    let t = sim.world.tables().get(0, "materialtypes")?;
    let file = text(t.cell("TintFile", material))?.to_ascii_lowercase();
    let stem = file.strip_suffix(".tnt").unwrap_or(&file).to_string();
    (!stem.is_empty()).then_some(stem)
}

/// The tints of a creature's equipment by the model each item draws: worn parts under
/// `prefix` (part-based creatures) and weapons.
fn equipment_tints(
    sim: &Sim,
    o: &Object,
    prefix: Option<&str>,
    gear: Gear,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut add = |slot_name: &str, model: &dyn Fn(&str, &str) -> String| {
        let Some(item) = equipped_in(sim, o, slot_name, gear) else {
            return;
        };
        let (Some((ty, v)), Some(tint)) = (item_variation(sim, item), item_tint(sim, item)) else {
            return;
        };
        out.push((model(&ty, &v), tint));
    };
    if let Some(p) = prefix {
        for slot_name in [
            "INVENTORY_SLOT_CHEST",
            "INVENTORY_SLOT_BOOTS",
            "INVENTORY_SLOT_GLOVES",
            "INVENTORY_SLOT_HEAD",
        ] {
            add(slot_name, &|ty, v| format!("{p}_{ty}_{v}{LOD0}"));
        }
    }
    for slot_name in ["INVENTORY_SLOT_MAIN", "INVENTORY_SLOT_OFFHAND"] {
        add(slot_name, &|ty, v| format!("w_{ty}_{v}{LOD0}"));
    }
    out
}

/// The creature's weapons: drawn into its hands while it is in combat, else sheathed (see
/// [`weapon_node`]). In the hands they sit in the grip only once the combat stance closes
/// the hands around them (the combat animation trees animate the fingers; nothing else
/// poses them). Provisional: the original draws and sheathes them with animations
/// (`COMMAND_UNSHEATHEWEAPONS`, `COMMAND_SHEATHEWEAPONS`) whose timing is not traced; here
/// they are drawn for as long as the creature's combat state (`GetCombatState`) is set.
fn weapons(sim: &Sim, o: &Object, gear: Gear) -> Vec<Attached> {
    let drawn = o.combat.in_combat;
    [
        ("INVENTORY_SLOT_MAIN", true),
        ("INVENTORY_SLOT_OFFHAND", false),
    ]
    .into_iter()
    .filter_map(|(slot_name, main)| {
        let item = equipped_in(sim, o, slot_name, gear)?;
        let (ty, v) = item_variation(sim, item)?;
        let model = format!("w_{ty}_{v}{LOD0}");
        let node = weapon_node(sim, item, main, drawn)?;
        has_model(sim, &model).then_some(Attached { model, node })
    })
    .collect()
}

/// The node an item hangs from: `BITM_base.gda`'s part for its slot (main or off hand) and
/// state (drawn or sheathed), named by `weaponparts.gda`. `None` when the item has no part
/// there (part 0: a shield is not sheathed from the main hand's slot). Without the tables,
/// the hands' nodes.
pub fn weapon_node(sim: &Sim, item: &Object, main: bool, drawn: bool) -> Option<String> {
    let hand = || Some(if main { MAIN_HAND_NODE } else { OFF_HAND_NODE }.to_string());
    let tables = sim.world.tables();
    let (Some(bitm), Some(parts), Some(&base)) = (
        tables.get(0, "bitm_base"),
        tables.get(0, "weaponparts"),
        item.fields.get("BaseItem"),
    ) else {
        return hand();
    };
    let col = match (drawn, main) {
        (false, true) => COL_SHEATHED_MAIN,
        (false, false) => COL_SHEATHED_OFF,
        (true, true) => COL_DRAWN_MAIN,
        (true, false) => COL_DRAWN_OFF,
    };
    let part = match bitm.cell_by_hash(col, base as i64) {
        Some(Cell::Int(p)) => *p,
        _ => return hand(),
    };
    if part <= 0 {
        return None;
    }
    text(parts.cell_by_hash(COL_PART_NODE, part))
}

/// An animation to play: the first of `actions` in the first of `trees` (`.anb`) that has
/// one, at `time` seconds (wrapped when `looping`, else held at the end).
#[derive(Clone, Debug, PartialEq)]
pub struct Motion {
    pub trees: Vec<String>,
    pub actions: Vec<String>,
    pub time: f32,
    pub looping: bool,
}

/// `apr_base.gda`'s animation trees: ambient and combat (column names not known), movement
/// out of and in combat.
pub const COL_AMBIENT_TREE: u32 = 0x5009_76ec;
pub const COL_COMBAT_TREE: u32 = 0xde80_b418;

/// Actions in the trees (from `humanoid.anb`, `combat_humanoid.anb`, `movement.anb` and the
/// weapon-style trees).
pub mod action {
    pub const PAUSE: &str = "pause";
    pub const RUN: &str = "runf";
    pub const DIE: &str = "die";
    /// Ranged styles' shot.
    pub const FIRE: &str = "fire";
    /// Melee swings `attk_v1` to `attk_v6`; misses `miss_v1` to `miss_v4`.
    pub const ATTACK_VARIANTS: u64 = 6;
    pub const MISS_VARIANTS: u64 = 4;
}

/// The weapon-style animation tree a creature fights with, from the variation types of
/// what it holds (`.anb` stems): `tw` two-handed (greatsword, battleaxe, maul), `ws` a
/// one-handed weapon and a shield, `dw` two one-handed weapons, `ow` one alone, `bow`
/// (short and long bows), `cbow` and `staff`. `None` when it holds nothing with a model.
/// Provisional: the original's choice of style is not traced; this follows the trees'
/// names.
pub fn weapon_style(sim: &Sim, o: &Object) -> Option<&'static str> {
    let kind = |slot_name: &str| {
        equipped(sim, o, slot_name)
            .and_then(|i| item_variation(sim, i))
            .map(|(ty, _)| ty)
    };
    let main = kind("INVENTORY_SLOT_MAIN");
    let off = kind("INVENTORY_SLOT_OFFHAND");
    let one_handed = |t: &str| matches!(t, "lsw" | "axe" | "mce" | "dgr" | "wnd");
    Some(match (main.as_deref(), off.as_deref()) {
        (Some("gsw" | "gax" | "mal"), _) => "tw",
        (Some("sbw" | "lbw"), _) => "bow",
        (Some("cbw"), _) => "cbow",
        (Some("stf"), _) => "staff",
        (Some(m), Some("shd")) if one_handed(m) => "ws",
        (Some(m), Some(x)) if one_handed(m) && one_handed(x) => "dw",
        (Some(m), _) if one_handed(m) => "ow",
        (None, Some("shd")) => "ws",
        _ => return None,
    })
}

/// How long an attack plays when the script set no duration (milliseconds). Provisional.
pub const DEFAULT_ATTACK_MS: u64 = 1500;

/// What a creature's body is doing: dead (the death animation from when it died, then its
/// last pose), attacking (the attack animation over the attack's duration), moving (a
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
    let style = weapon_style(sim, o).map(|s| format!("{s}.anb"));
    if o.dead {
        // From when it died, held on the last frame; a death of unknown time (a save from
        // before deaths were timed) shows the last frame.
        let time = o.died_at.map_or(f32::MAX, |t| secs(now.saturating_sub(t)));
        return Some(Motion {
            trees: trees(&[&style, &ambient, &combat]),
            actions: vec![action::DIE.into()],
            time,
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
                actions: vec![action::RUN.into()],
                time: secs(now) + phase,
                looping: true,
            });
        }
        if c.name == "CommandAttack" {
            if let Some(due) = c.due {
                let length = o.combat.attack_ms.unwrap_or(DEFAULT_ATTACK_MS);
                let start = due.saturating_sub(length);
                return Some(Motion {
                    trees: trees(&[&style, &combat, &ambient]),
                    actions: attack_actions(sim, o, due),
                    time: secs(now.saturating_sub(start)),
                    looping: false,
                });
            }
        }
    }
    Some(Motion {
        trees: if fighting {
            trees(&[&style, &combat, &ambient])
        } else {
            trees(&[&ambient, &combat])
        },
        actions: vec![action::PAUSE.into()],
        time: secs(now) + phase,
        looping: true,
    })
}

/// The swing for an attack due at `due`: a ranged style's shot, else one of the variants,
/// picked per creature and attack so swings vary (a miss tries the miss swings first).
/// Provisional: the original's choice of variant is not traced.
fn attack_actions(sim: &Sim, o: &Object, due: u64) -> Vec<String> {
    let pick = (o.id as u64).wrapping_mul(31).wrapping_add(due / 100);
    let attack = format!("attk_v{}", 1 + pick % action::ATTACK_VARIANTS);
    let missed = o.combat.attack.as_ref().is_some_and(|r| {
        r.results[0] == thedas_sim::sim::constant(&sim.table, "COMBAT_RESULT_MISS")
    });
    let mut out = vec![action::FIRE.to_string()];
    if missed {
        out.push(format!("miss_v{}", 1 + pick % action::MISS_VARIANTS));
    }
    out.push(attack);
    out.push("attk_v1".into());
    out
}

/// Every active creature and placeable in `area` that has a model, in id order.
pub fn area_models(sim: &Sim, area: ObjectId) -> Vec<ObjectModel> {
    area_models_cached(sim, area, &mut ModelCache::default())
}

/// What each object draws, kept between calls: working it out reads templates and tables,
/// so a caller asking every frame (a running game) keeps one of these. An object's entry is
/// worked out again when anything it depends on changes.
#[derive(Default)]
pub struct ModelCache {
    drawn: std::collections::HashMap<ObjectId, (DrawnKey, Drawn)>,
}

/// What [`object_models`] reads from an object.
#[derive(Clone, PartialEq)]
struct DrawnKey {
    template: String,
    appearance: i32,
    gender: Option<i32>,
    head_morph: Option<String>,
    /// Character creation's creature wears a face of its own (see [`head_morph`]).
    in_chargen: bool,
    /// Weapons are drawn in combat and sheathed out of it (their attachment nodes).
    in_combat: bool,
    equipment: Vec<(i32, ObjectId, Option<i32>, Option<i32>)>,
}

impl DrawnKey {
    fn of(sim: &Sim, o: &Object) -> Self {
        let field =
            |id: ObjectId, f: &str| sim.world.get(id).and_then(|i| i.fields.get(f).copied());
        DrawnKey {
            template: o.template.clone(),
            appearance: o.appearance,
            gender: o.fields.get("Gender").copied(),
            // With the choices made over it (character creation's hair, beard, colours).
            head_morph: sim.face_name(o.id).or_else(|| o.head_morph.clone()),
            in_chargen: sim.world.chargen == Some(o.id),
            in_combat: o.combat.in_combat,
            equipment: o
                .equipment
                .iter()
                .map(|(&slot, &item)| {
                    (
                        slot,
                        item,
                        field(item, "BaseItem"),
                        field(item, "ModelVariation"),
                    )
                })
                .collect(),
        }
    }
}

/// [`area_models`] keeping what objects draw in `cache`.
pub fn area_models_cached(sim: &Sim, area: ObjectId, cache: &mut ModelCache) -> Vec<ObjectModel> {
    let objects: Vec<&Object> = sim
        .world
        .objects
        .values()
        .filter(|o| o.area == Some(area) && o.active)
        .filter(|o| matches!(o.kind, ObjectKind::Creature | ObjectKind::Placeable))
        .collect();
    cache
        .drawn
        .retain(|id, _| objects.iter().any(|o| o.id == *id));
    objects
        .into_iter()
        .filter_map(|o| {
            let key = DrawnKey::of(sim, o);
            let d = match cache.drawn.get(&o.id) {
                Some((k, d)) if *k == key => d.clone(),
                _ => {
                    let d = object_models(sim, o);
                    cache.drawn.insert(o.id, (key, d.clone()));
                    d
                }
            };
            (!d.models.is_empty()).then(|| ObjectModel {
                id: o.id,
                tag: o.tag.clone(),
                kind: o.kind,
                models: d.models,
                attached: d.attached,
                tints: d.tints,
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
    fn the_dead_play_their_death_from_when_they_died() {
        use std::sync::Arc;
        use thedas_script::EngineTable;
        use thedas_vfs::Vfs;
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let st = |v: &str| Cell::String(format!("{v}\0"));
        s.world.tables().insert(
            "apr_base",
            &["#500976ec", "#de80b418", "MovementTree", "CombatMoveTree"],
            vec![(
                15,
                vec![
                    st("humanoid.anb"),
                    st("combat_humanoid.anb"),
                    st("movement.anb"),
                    st("combat_movement.anb"),
                ],
            )],
        );
        let id = s.world.spawn(ObjectKind::Creature, "bandit");
        {
            let o = s.world.get_mut(id).unwrap();
            o.appearance = 15;
            o.dead = true;
            o.died_at = Some(1_000);
        }
        s.world.time_ms = 2_500;
        let m = motion(&s, s.world.get(id).unwrap()).unwrap();
        assert_eq!(m.actions, vec![action::DIE]);
        assert_eq!(m.trees, vec!["humanoid.anb", "combat_humanoid.anb"]);
        assert!(!m.looping);
        assert!((m.time - 1.5).abs() < 1e-6);
        // A death of unknown time shows the end of the animation.
        s.world.get_mut(id).unwrap().died_at = None;
        assert_eq!(motion(&s, s.world.get(id).unwrap()).unwrap().time, f32::MAX);
    }

    #[test]
    fn the_weapons_held_pick_the_fighting_style() {
        use std::sync::Arc;
        use thedas_script::EngineTable;
        use thedas_vfs::Vfs;
        let ldf = "int INVENTORY_SLOT_MAIN = 0;\nint INVENTORY_SLOT_OFFHAND = 1;\n";
        let table = Arc::new(EngineTable::parse(ldf.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let t = s.world.tables();
        let st = |v: &str| Cell::String(v.into());
        // Base item -> variation table -> model type.
        let kinds = [
            (2, 30, "lsw"),
            (7, 37, "SHD"),
            (5, 12, "gsw"),
            (19, 42, "sbw"),
        ];
        t.insert(
            "bitm_base",
            &["Label", "#43b54707"],
            kinds
                .iter()
                .map(|&(base, table, _)| (base, vec![st("item"), Cell::Int(table)]))
                .collect(),
        );
        for (_, table, ty) in kinds {
            t.insert(
                &format!("#{table}"),
                &["ModelType", "ModelSubType", "ModelVariation"],
                vec![(1, vec![st(ty), st("x01"), st("a")])],
            );
        }
        let mut item = |base: i32| {
            let id = s.world.spawn(ObjectKind::Item, "item");
            let o = s.world.get_mut(id).unwrap();
            o.fields.insert("BaseItem".into(), base);
            o.fields.insert("ModelVariation".into(), 1);
            id
        };
        let (sword, sword2, shield, great, bow) = (item(2), item(2), item(7), item(5), item(19));
        let mut style = |main: Option<u32>, off: Option<u32>| {
            let c = s.world.spawn(ObjectKind::Creature, "c");
            let o = s.world.get_mut(c).unwrap();
            if let Some(m) = main {
                o.equipment.insert(0, m);
            }
            if let Some(f) = off {
                o.equipment.insert(1, f);
            }
            weapon_style(&s, s.world.get(c).unwrap())
        };
        assert_eq!(style(Some(sword), Some(shield)), Some("ws"));
        assert_eq!(style(Some(sword), Some(sword2)), Some("dw"));
        assert_eq!(style(Some(sword), None), Some("ow"));
        assert_eq!(style(Some(great), None), Some("tw"));
        assert_eq!(style(Some(bow), None), Some("bow"));
        assert_eq!(style(None, None), None);
    }

    #[test]
    fn a_creatures_look_is_redrawn_as_combat_and_character_creation_change_it() {
        use std::sync::Arc;
        use thedas_script::EngineTable;
        use thedas_vfs::Vfs;
        let table = Arc::new(EngineTable::parse(b"int X = 0;\n".as_slice()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let c = s.world.spawn(ObjectKind::Creature, "c");
        let key = |s: &Sim| DrawnKey::of(s, s.world.get(c).unwrap());
        let calm = key(&s);
        s.world.get_mut(c).unwrap().combat.in_combat = true;
        let fighting = key(&s);
        assert!(calm != fighting);
        s.world.chargen = Some(c);
        assert!(key(&s) != fighting);
    }

    #[test]
    fn weapons_hang_from_the_parts_their_base_item_names() {
        use std::sync::Arc;
        use thedas_script::EngineTable;
        use thedas_vfs::Vfs;
        let table = Arc::new(EngineTable::parse(b"int X = 0;\n".as_slice()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let item = |s: &mut Sim, base: i32| {
            let id = s.world.spawn(ObjectKind::Item, "item");
            let o = s.world.get_mut(id).unwrap();
            o.fields.insert("BaseItem".into(), base);
            id
        };
        // No tables: the hands.
        let sword = item(&mut s, 2);
        let node = |s: &Sim, id, main, drawn| weapon_node(s, s.world.get(id).unwrap(), main, drawn);
        assert_eq!(
            node(&s, sword, true, false).as_deref(),
            Some(MAIN_HAND_NODE)
        );
        assert_eq!(node(&s, sword, false, true).as_deref(), Some(OFF_HAND_NODE));
        let t = s.world.tables();
        let cols = [
            format!("#{COL_SHEATHED_MAIN:08x}"),
            format!("#{COL_SHEATHED_OFF:08x}"),
            format!("#{COL_DRAWN_MAIN:08x}"),
            format!("#{COL_DRAWN_OFF:08x}"),
        ];
        let cols: Vec<&str> = cols.iter().map(String::as_str).collect();
        let parts = |v: [i64; 4]| v.map(Cell::Int).to_vec();
        // As the game's: a longsword, and a shield (not sheathed from the main hand).
        t.insert(
            "bitm_base",
            &cols,
            vec![(2, parts([5, 10, 14, 15])), (6, parts([0, 8, 0, 16]))],
        );
        let st = |v: &str| Cell::String(v.into());
        t.insert(
            "weaponparts",
            &[&format!("#{COL_PART_NODE:08x}")],
            [
                (5, "Prop10"),
                (8, "Prop11"),
                (10, "Prop07"),
                (14, "Prop01"),
                (15, "Prop02"),
                (16, "Prop03"),
            ]
            .into_iter()
            .map(|(r, n)| (r, vec![st(n)]))
            .collect(),
        );
        let shield = item(&mut s, 6);
        assert_eq!(node(&s, sword, true, false).as_deref(), Some("Prop10"));
        assert_eq!(node(&s, sword, false, false).as_deref(), Some("Prop07"));
        assert_eq!(node(&s, sword, true, true).as_deref(), Some("Prop01"));
        assert_eq!(node(&s, sword, false, true).as_deref(), Some("Prop02"));
        assert_eq!(node(&s, shield, false, false).as_deref(), Some("Prop11"));
        assert_eq!(node(&s, shield, false, true).as_deref(), Some("Prop03"));
        assert_eq!(node(&s, shield, true, false), None);
    }

    #[test]
    fn items_take_their_materials_tint() {
        use std::sync::Arc;
        use thedas_script::EngineTable;
        use thedas_vfs::Vfs;
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        s.world.tables().insert(
            "materialtypes",
            &["Label", "TintFile"],
            vec![
                (
                    40,
                    vec![
                        Cell::String("Weapon, Iron".into()),
                        Cell::String("T3_WPN_IRN.tnt\0".into()),
                    ],
                ),
                (
                    0,
                    vec![Cell::String("INVALID".into()), Cell::String("".into())],
                ),
            ],
        );
        let mut item = |material: Option<i32>| {
            let id = s.world.spawn(ObjectKind::Item, "item");
            if let Some(m) = material {
                s.world
                    .get_mut(id)
                    .unwrap()
                    .fields
                    .insert("Material".into(), m);
            }
            id
        };
        let (iron, none, unset) = (item(Some(40)), item(Some(0)), item(None));
        let tint = |id| item_tint(&s, s.world.get(id).unwrap());
        assert_eq!(tint(iron).as_deref(), Some("t3_wpn_irn"));
        assert_eq!(tint(none), None);
        assert_eq!(tint(unset), None);
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

    #[test]
    fn body_tints_are_numbered_per_appearance() {
        use thedas_formats::gda::{Column, column_hash};
        let col = |n: &str| Column {
            hash: column_hash(n),
            kind: None,
        };
        let st = |v: &str| Cell::String(v.into());
        let gda = thedas_formats::Gda {
            version: thedas_formats::FourCC(*b"V0.2"),
            columns: vec![col("ID"), col("Label"), col("TintFile"), col("Apr")],
            rows: vec![
                vec![
                    Cell::Int(63),
                    st("Dog 0"),
                    st("T3_DOG_O00.tnt\0"),
                    Cell::Int(35),
                ],
                vec![
                    Cell::Int(63),
                    st("Jumper"),
                    st("T3_DPS_B03.tnt"),
                    Cell::Int(33),
                ],
                vec![Cell::Int(5), st("Any"), st("T3_ANY.tnt"), Cell::Int(0)],
            ],
        };
        assert_eq!(body_tint_file(&gda, 63, 35).as_deref(), Some("t3_dog_o00"));
        assert_eq!(body_tint_file(&gda, 63, 33).as_deref(), Some("t3_dps_b03"));
        assert_eq!(body_tint_file(&gda, 63, 34), None);
        assert_eq!(body_tint_file(&gda, 5, 34).as_deref(), Some("t3_any"));
        assert_eq!(body_tint_file(&gda, 0, 35), None);
    }
}
