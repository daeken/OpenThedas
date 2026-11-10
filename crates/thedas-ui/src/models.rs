//! Which models draw the simulation's creatures and placeables. Reads the simulation and its
//! tables; never writes either.
//!
//! - A placeable's appearance row in `placeable_types.gda` names its model (`ModelName`,
//!   e.g. `PLC_SmFerDrB_01_0`).
//! - A creature's appearance row in `apr_base.gda` gives a model type (`ModelType`) and,
//!   for whole-body types, the model (`ModelName`, e.g. `C_DOGa`, `CN_BDY_HLKa`), which the
//!   resources name with a level-of-detail suffix: `c_doga_0.mmh`. Part-based types (`P`:
//!   humans, elves, dwarves, qunari) are assembled from body parts named
//!   `<race><gender>_<part>_<variation>_0`; until equipment is read they wear the
//!   unclothed parts (`hm_arm_nuda_0` and so on).

use thedas_formats::gda::Cell;
use thedas_script::vm::ObjectId;
use thedas_sim::Sim;
use thedas_sim::world::{Object, ObjectKind};

/// The model resources (without `.mmh`) that draw one object, and where it stands.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectModel {
    pub id: ObjectId,
    pub tag: String,
    pub kind: ObjectKind,
    pub models: Vec<String>,
    pub position: [f32; 3],
    /// Degrees anticlockwise from +X, as the simulation keeps it.
    pub facing: f32,
}

/// The level-of-detail suffix of the most detailed model.
pub const LOD0: &str = "_0";

/// Body parts of a part-based creature without equipment, in draw order. Provisional: the
/// parts the toolset's naming suggests (body, boots, gloves).
pub const NUDE_PARTS: [&str; 3] = ["arm_nuda", "boo_nuda", "glv_nuda"];

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

/// The models for one creature or placeable, or none when it has nothing to draw.
pub fn object_models(sim: &Sim, o: &Object) -> Vec<String> {
    let tables = sim.world.tables();
    match o.kind {
        ObjectKind::Placeable => {
            let Some(t) = tables.get(0, "placeable_types") else {
                return vec![];
            };
            text(t.cell("ModelName", o.appearance as i64))
                .map(|m| vec![m.to_ascii_lowercase()])
                .unwrap_or_default()
        }
        ObjectKind::Creature => {
            let Some(t) = tables.get(0, "apr_base") else {
                return vec![];
            };
            let app = o.appearance as i64;
            let kind = text(t.cell("ModelType", app)).unwrap_or_default();
            if kind.eq_ignore_ascii_case("P") {
                let race = text(t.cell("ModelRace", app)).unwrap_or_default();
                let gender = o.fields.get("Gender").copied().unwrap_or(1);
                return part_prefix(&race, gender)
                    .map(|p| {
                        NUDE_PARTS
                            .iter()
                            .map(|part| format!("{p}_{part}{LOD0}"))
                            .collect()
                    })
                    .unwrap_or_default();
            }
            text(t.cell("ModelName", app))
                .map(|m| vec![format!("{}{LOD0}", m.to_ascii_lowercase())])
                .unwrap_or_default()
        }
        _ => vec![],
    }
}

/// Every active creature and placeable in `area` that has a model, in id order.
pub fn area_models(sim: &Sim, area: ObjectId) -> Vec<ObjectModel> {
    sim.world
        .objects
        .values()
        .filter(|o| o.area == Some(area) && o.active)
        .filter(|o| matches!(o.kind, ObjectKind::Creature | ObjectKind::Placeable))
        .filter_map(|o| {
            let models = object_models(sim, o);
            (!models.is_empty()).then(|| ObjectModel {
                id: o.id,
                tag: o.tag.clone(),
                kind: o.kind,
                models,
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
    fn text_cells_lose_their_terminator() {
        assert_eq!(
            text(Some(&Cell::String("C_DOGa\0".into()))).as_deref(),
            Some("C_DOGa")
        );
        assert_eq!(text(Some(&Cell::String("\0".into()))), None);
        assert_eq!(text(Some(&Cell::Int(3))), None);
    }
}
