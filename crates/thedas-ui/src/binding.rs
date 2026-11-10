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

/// `GM_DIALOG` (4) while a conversation is on screen, the world's game mode otherwise.
/// Provisional: the simulation does not switch game modes for conversations itself.
pub fn game_mode(sim: &Sim) -> i32 {
    if sim.dialog_state().is_some_and(|d| !d.ended) {
        4
    } else {
        sim.world.game_mode
    }
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
        v.insert(prefix.to_string(), num(id));
        v.insert(format!("{prefix}.ID"), num(id));
        // No combat or status simulation yet: nothing is poisoned, paralysed or busy.
        for k in [
            "Poisoned",
            "Paralyzed",
            "AbilityUpkeepCost",
            "CurrentAction",
            "PendingAction",
        ] {
            v.insert(format!("{prefix}.{k}"), num(0));
        }
        v.insert(
            format!("{prefix}.UserSelected"),
            num((Some(id) == sim.world.hero) as u8),
        );
        let mana = o.properties.contains_key(&8);
        v.insert(format!("{prefix}.Stats.ClassHasMana"), num(mana as u8));
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
    // Empty slots read as no creature (0); `GetNumMembers` does not count them.
    for i in members.len()..4 {
        v.insert(format!("Party.Members.{i}"), num(0));
        v.insert(format!("Party.Summons.{i}"), num(0));
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
        num(game_mode(sim)),
    );
    // The conversation in progress, as conversation.gfx reads it: the NPC line, the
    // responses (`Responses.N.Text`, counted with GetNumMembers) and a ready flag.
    if let Some(d) = sim.dialog_state().filter(|d| !d.ended) {
        set(
            "Conversation.NPCLine".into(),
            UiValue::String(d.line.clone()),
        );
        set("Conversation.Responses".into(), num(d.replies.len() as f64));
        for (i, (_, text)) in d.replies.iter().enumerate() {
            set(
                format!("Conversation.Responses.{i}.Text"),
                UiValue::String(text.clone()),
            );
            set(format!("Conversation.Responses.{i}.Icon"), num(0));
        }
        set("Conversation.ResponsesReady".into(), num(1));
        set("Conversation.HasVO".into(), num(0));
    }
    // The open job board, as chanters.gfx reads it (see thedas_sim::chanters).
    if let Some(b) = &sim.world.chanter_board {
        let text =
            |s: Option<u32>| UiValue::String(s.and_then(|s| sim.string(s)).unwrap_or_default());
        set("Journal.ChanterBoard.Header".into(), text(b.header_strref));
        for (i, j) in b.jobs.iter().enumerate() {
            let k = format!("Journal.ChanterBoard.ChanterEntries.{i}");
            set(format!("{k}.PlotName"), text(j.name_strref));
            set(format!("{k}.JournalText"), text(j.text_strref));
            set(format!("{k}.PlotResRef"), UiValue::String(j.plot.clone()));
            set(format!("{k}.Accepted"), num(j.accepted as u8));
        }
    }
    // The world map, as worldmap.gfx reads it: the primary and secondary maps (their
    // worldmaps.gda row as `Type`, the row's image as `ResourceName`), where the party is,
    // and each map's pins. Provisional: the pin states are taken to be the scripts'
    // WM_LOCATION_* values.
    let wm = &sim.world.world_map;
    set("WorldMap.GUIStatus".into(), num(wm.gui_status));
    set(
        "WorldMap.GUITravelStatus".into(),
        num(wm.travelling_to.is_some() as u8),
    );
    let player_area = wm
        .player_pin
        .and_then(|p| sim.world.get(p))
        .and_then(|p| p.locals.strings.get("WM_AREA").cloned())
        .unwrap_or_default();
    let tables = sim.world.tables();
    let maps = tables.get(0, "worldmaps");
    for (key, map) in [("PrimaryMap", wm.primary), ("SecondaryMap", wm.secondary)] {
        let Some(m) = map
            .and_then(|m| sim.world.get(m))
            .filter(|m| m.kind == ObjectKind::Map)
        else {
            continue;
        };
        let k = format!("WorldMap.{key}");
        let id = m.fields.get("MapId").copied().unwrap_or(0);
        set(format!("{k}.Type"), num(id));
        let image = maps
            .as_ref()
            .and_then(|t| match t.cell_by_hash(0xa90d_54b7, id as i64) {
                Some(thedas_formats::gda::Cell::String(s)) => {
                    Some(s.trim_end_matches('\0').trim().to_string())
                }
                _ => None,
            })
            .unwrap_or_default();
        set(format!("{k}.ResourceName"), UiValue::String(image));
        set(
            format!("{k}.PlayerLocation.AreaTag"),
            UiValue::String(player_area.clone()),
        );
        let pins = sim.world.objects.values().filter(|o| {
            o.kind == ObjectKind::MapPin && o.locals.objects.get("WM_MAP") == Some(&m.id)
        });
        for (i, p) in pins.enumerate() {
            let pk = format!("{k}.Locations.{i}");
            let name = p
                .name_strref
                .and_then(|s| sim.string(s))
                .unwrap_or_default();
            set(format!("{pk}.Tag"), UiValue::String(p.tag.clone()));
            set(format!("{pk}.Name"), UiValue::String(name.clone()));
            set(format!("{pk}.Tooltip"), UiValue::String(name));
            set(
                format!("{pk}.AreaTag"),
                UiValue::String(p.locals.strings.get("WM_AREA").cloned().unwrap_or_default()),
            );
            set(
                format!("{pk}.State"),
                num(p.fields.get("WMStatus").copied().unwrap_or(0)),
            );
            set(
                format!("{pk}.Type"),
                num(p.fields.get("WMIcon").copied().unwrap_or(0)),
            );
            set(format!("{pk}.RecentlyActivated"), num(0));
            // The pin's place on the map image, as the map file gives it.
            set(format!("{pk}.X"), num(p.position[0]));
            set(format!("{pk}.Y"), num(p.position[1]));
        }
    }
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
    fn the_world_map_shows_its_pins_and_where_the_party_is() {
        use thedas_sim::world::ObjectKind;
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let map = sim.world.spawn(ObjectKind::Map, "wide_open_world");
        sim.world
            .get_mut(map)
            .unwrap()
            .fields
            .insert("MapId".into(), 1);
        let pin = sim.world.spawn(ObjectKind::MapPin, "wml_wow_lothering");
        {
            let p = sim.world.get_mut(pin).unwrap();
            p.locals.objects.insert("WM_MAP".into(), map);
            p.locals
                .strings
                .insert("WM_AREA".into(), "lot100ar_lothering".into());
            p.fields
                .extend([("WMStatus".to_string(), 2), ("WMIcon".to_string(), 7)]);
            p.position = [0.25, 0.75, 0.0];
        }
        sim.world.world_map.primary = Some(map);
        sim.world.world_map.player_pin = Some(pin);
        let v = sim_values(&sim);
        let k = "WorldMap.PrimaryMap";
        assert_eq!(v.get(&format!("{k}.Type")), Some(&UiValue::Number(1.0)));
        assert_eq!(
            v.get(&format!("{k}.PlayerLocation.AreaTag")),
            Some(&UiValue::String("lot100ar_lothering".into()))
        );
        let l = format!("{k}.Locations.0");
        assert_eq!(
            v.get(&format!("{l}.Tag")),
            Some(&UiValue::String("wml_wow_lothering".into()))
        );
        assert_eq!(v.get(&format!("{l}.State")), Some(&UiValue::Number(2.0)));
        assert_eq!(v.get(&format!("{l}.Type")), Some(&UiValue::Number(7.0)));
        assert_eq!(v.get(&format!("{l}.X")), Some(&UiValue::Number(0.25)));
        assert_eq!(v.get(&format!("{l}.Y")), Some(&UiValue::Number(0.75)));
        assert!(!v.contains_key("WorldMap.SecondaryMap.Type"));
    }

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

    #[test]
    fn the_open_board_lists_its_jobs() {
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        sim.world.chanter_board = Some(thedas_sim::chanters::ChanterBoard {
            board: 3,
            header_strref: None,
            jobs: vec![thedas_sim::chanters::BoardJob {
                plot: "lot100pt_bears".into(),
                name_strref: None,
                text_strref: None,
                accepted_flag: 2,
                accepted: true,
            }],
        });
        let v = sim_values(&sim);
        let k = "Journal.ChanterBoard.ChanterEntries.0";
        assert_eq!(
            v.get(&format!("{k}.PlotResRef")),
            Some(&UiValue::String("lot100pt_bears".into()))
        );
        assert_eq!(v.get(&format!("{k}.Accepted")), Some(&UiValue::Number(1.0)));
        assert!(!v.contains_key("Journal.ChanterBoard.ChanterEntries.1.PlotName"));
    }
}
