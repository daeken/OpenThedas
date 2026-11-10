//! Engine values the movies read, computed from the simulation. The interface only reads the
//! simulation; nothing here writes it.
//!
//! Provisional: key names come from the movies' bytecode, and their meanings are inferred from
//! how the movies use them (`Party.Members.N.ID` is a creature id, `Attributes.P.Current` and
//! `.Total` are a property's current and base (maximum) values, as in `properties.gda`).

use std::collections::BTreeMap;

use thedas_sim::Sim;
use thedas_sim::world::ObjectKind;

use crate::UiValue;

fn num(v: impl Into<f64>) -> UiValue {
    UiValue::Number(v.into())
}

/// The party, hero first.
pub fn party(sim: &Sim) -> Vec<u32> {
    let hero = sim.world.hero;
    let mut v: Vec<u32> = hero.into_iter().collect();
    if let Some(h) = hero.and_then(|h| sim.world.get(h)) {
        v.extend(h.followers.iter().copied());
    }
    v
}

fn name(sim: &Sim, id: u32) -> String {
    sim.world
        .get(id)
        .and_then(|o| {
            o.name_strref
                .and_then(|s| sim.string(s))
                .or(Some(o.tag.clone()))
        })
        .unwrap_or_default()
}

/// Every value this module can answer for the current simulation state.
pub fn sim_values(sim: &Sim) -> BTreeMap<String, UiValue> {
    let mut v = BTreeMap::new();
    let mut set = |k: String, x: UiValue| {
        v.insert(k, x);
    };
    let members = party(sim);
    let hero = members.first().copied();
    set("Party.Hero.ID".into(), num(hero.unwrap_or(0)));
    set(
        "Party.SelectableMemberCount".into(),
        num(members.len() as f64),
    );
    set("Party.SelectedCount".into(), num(hero.is_some() as u8));
    set("Party.QuickbarLocked".into(), num(0));
    for i in 0..4 {
        set(
            format!("Party.MemberSelected.{i}"),
            num((i == 0 && hero.is_some()) as u8),
        );
    }
    let mut creature = |prefix: &str, id: u32| {
        let Some(o) = sim.world.get(id) else { return };
        v.insert(format!("{prefix}.ID"), num(id));
        v.insert(format!("{prefix}.Name"), UiValue::String(name(sim, id)));
        v.insert(format!("{prefix}.Dead"), num(o.dead as u8));
        // The simulation keeps (base, current); for depletable properties (health, mana)
        // the base is the maximum the bars show as Total.
        for (p, (total, cur)) in &o.properties {
            v.insert(format!("{prefix}.Attributes.{p}.Current"), num(*cur));
            v.insert(format!("{prefix}.Attributes.{p}.Total"), num(*total));
        }
    };
    for (i, id) in members.iter().enumerate() {
        creature(&format!("Party.Members.{i}"), *id);
    }
    if let Some(h) = hero {
        creature("GUI.SelectedChar", h);
    }
    let mut set = |k: String, x: UiValue| {
        v.insert(k, x);
    };
    if let Some(a) = sim.world.area.and_then(|a| sim.world.get(a)) {
        set("Area.ResRef".into(), UiValue::String(a.tag.clone()));
        set("Area.Name".into(), UiValue::String(name(sim, a.id)));
    }
    set(
        "GameModeController.CurrentGameMode".into(),
        num(sim.world.game_mode),
    );
    let creatures = sim
        .world
        .objects
        .values()
        .filter(|o| o.kind == ObjectKind::Creature && o.active)
        .count();
    set("Area.CreatureCount".into(), num(creatures as f64));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_world_has_no_party() {
        let sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let v = sim_values(&sim);
        assert_eq!(v.get("Party.Hero.ID"), Some(&UiValue::Number(0.0)));
        assert_eq!(v.get("Party.MemberSelected.0"), Some(&UiValue::Number(0.0)));
        assert!(!v.contains_key("Party.Members.0.ID"));
        assert!(!v.contains_key("GUI.SelectedChar.ID"));
    }
}
