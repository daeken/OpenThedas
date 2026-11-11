//! World maps and travel between areas.
//!
//! A world map is a `.map` resource (GFF4 `MAPO`): field 24000 names it, 24001 is its row in
//! `worldmaps.gda`, and 24002 lists its pins (`MAPP`). A pin has a tag (24007, e.g.
//! `wml_wow_lothering`), a destination area (24008), an optional arrival waypoint (24013), a
//! status (24003, `WM_LOCATION_*`), a name (24006), a position on the map (24004, 24005) and
//! two small integers (24009, 24010; the first is taken to be the terrain). Each map and pin
//! becomes an object so scripts can find them by tag (`SetWorldMapLocationStatus`).
//!
//! Travel (provisional, from `sp_module_travel`): clicking a pin sends `EVENT_TYPE_BEGIN_TRAVEL`
//! to the module, whose script may pick a random encounter and calls
//! `WorldMapStartTravelling`. Arriving sends `EVENT_TYPE_WORLDMAP_PRETRANSITION`, on which the
//! script moves the party to the area it stored, and `EVENT_TYPE_WORLDMAP_POSTTRANSITION` once
//! the party is there. The arrival waypoint comes from the map's `target_wps_*` table: the
//! column is the destination area and the row the area travelled from (or `default`).

use serde::{Deserialize, Serialize};
use thedas_formats::gda::Cell;
use thedas_formats::{Gff4, List, StructId, Value};
use thedas_script::vm::ObjectId;

use crate::sim::Sim;
use crate::world::ObjectKind;

/// The world map's state: which maps are shown, where the party is on them, and a journey
/// in progress.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorldMapState {
    pub primary: Option<ObjectId>,
    pub secondary: Option<ObjectId>,
    pub open: bool,
    pub gui_status: i32,
    pub player_pin: Option<ObjectId>,
    /// The pin travelled to, between `EVENT_TYPE_BEGIN_TRAVEL` and arrival.
    pub travelling_to: Option<ObjectId>,
    /// Set when the travel transition has been asked for; arrival then sends
    /// `EVENT_TYPE_WORLDMAP_POSTTRANSITION`.
    pub arriving: bool,
    /// The player swapped the maps on the world map screen (`WorldMap.WorldMapSwapMaps`):
    /// the secondary map is shown as the primary until the screen closes.
    #[serde(default)]
    pub swapped: bool,
    /// The journey stopped at a random encounter (`WorldMapStartTravelling` with an area);
    /// `WorldMapCompleteRandomEncounter` resumes it.
    #[serde(default)]
    pub encounter: bool,
}

/// The world map screen's pin click: the area the party is in (`PrimaryMap.PlayerLocation
/// .AreaTag`) and the pin's index in the shown map's `Locations`.
pub const MSG_TRAVEL: &str = "GameModeController.AreaTransition";
/// The world map screen's map toggle button.
pub const MSG_SWAP: &str = "WorldMap.WorldMapSwapMaps";
/// The world map screen closing: 1 when it closed for travel (a pin was clicked), 0 when the
/// player closed it.
pub const MSG_CLOSED: &str = "WorldMap.WorldMapClosed";

fn text(g: &Gff4, id: StructId, l: u32) -> Option<String> {
    g.field(id, l)
        .and_then(|v| v.as_str())
        .map(|x| x.trim_end_matches('\0').to_string())
        .filter(|x| !x.is_empty())
}

fn int(g: &Gff4, id: StructId, l: u32) -> i32 {
    g.field(id, l).and_then(|v| v.as_i64()).unwrap_or(0) as i32
}

fn float(g: &Gff4, id: StructId, l: u32) -> f32 {
    match g.field(id, l) {
        Some(Value::F32(v)) => *v,
        Some(v) => v.as_i64().unwrap_or(0) as f32,
        None => 0.0,
    }
}

/// A world map's pins as read from its `MAPO` resource.
#[derive(Clone, Debug, PartialEq)]
pub struct MapPin {
    pub tag: String,
    pub area: Option<String>,
    pub waypoint: Option<String>,
    pub status: i32,
    pub name_strref: Option<u32>,
    pub position: [f32; 2],
    pub terrain: i32,
    pub icon: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorldMap {
    pub name: String,
    pub id: i32,
    pub pins: Vec<MapPin>,
}

pub fn parse_map(data: &[u8]) -> Option<WorldMap> {
    map_from_gff4(&Gff4::parse(data).ok()?)
}

pub fn map_from_gff4(g: &Gff4) -> Option<WorldMap> {
    let root = g.root;
    let pins = match g.field(root, 24002) {
        Some(Value::List(Some(List::Struct(v)))) => v.iter().flatten().copied().collect(),
        _ => Vec::new(),
    };
    let pins = pins
        .into_iter()
        .filter_map(|p| {
            Some(MapPin {
                tag: text(g, p, 24007)?,
                area: text(g, p, 24008),
                waypoint: text(g, p, 24013),
                status: int(g, p, 24003),
                name_strref: match g.field(p, 24006) {
                    Some(Value::Tlk(t)) => Some(t.id),
                    _ => None,
                },
                position: [float(g, p, 24004), float(g, p, 24005)],
                terrain: int(g, p, 24009),
                icon: int(g, p, 24010),
            })
        })
        .collect();
    Some(WorldMap {
        name: text(g, root, 24000)?,
        id: int(g, root, 24001),
        pins,
    })
}

impl Sim {
    /// The maps as the world map screen shows them, (primary, secondary): swapped while the
    /// player has swapped them.
    pub fn shown_maps(&self) -> (Option<ObjectId>, Option<ObjectId>) {
        let wm = &self.world.world_map;
        let valid = |m: Option<ObjectId>| m.filter(|&m| self.is_world_map(m));
        let (primary, secondary) = (valid(wm.primary), valid(wm.secondary));
        if wm.swapped && secondary.is_some() {
            (secondary, primary)
        } else {
            (primary, secondary)
        }
    }

    /// Whether `id` is a world map (scripts pass OBJECT_INVALID to clear one).
    pub fn is_world_map(&self, id: ObjectId) -> bool {
        self.world
            .get(id)
            .is_some_and(|o| o.kind == ObjectKind::Map)
    }

    /// A map's pins in the order the world map screen lists them (`Locations.N`), hidden ones
    /// included: by object id.
    pub fn map_pins(&self, map: ObjectId) -> Vec<ObjectId> {
        self.world
            .objects
            .values()
            .filter(|o| {
                o.kind == ObjectKind::MapPin && o.locals.objects.get("WM_MAP") == Some(&map)
            })
            .map(|o| o.id)
            .collect()
    }

    /// The world map screen's messages, as player input (see `MSG_*`). Clicking a pin travels
    /// exactly as the travel scripts expect ([`Sim::begin_travel`]); the screen only sends
    /// the click for an active pin, and an inactive one is refused here as well.
    ///
    /// Provisional, from worldmap.gfx: `PinClicked` sends [`MSG_TRAVEL`] with the area tag
    /// and the pin's index into `WorldMap.PrimaryMap.Locations` (the screen always lists the
    /// map it reports as primary). The area tag is not needed: the area left is the current
    /// one.
    pub(crate) fn world_map_message(&mut self, name: &str, args: &[String]) -> bool {
        match name {
            MSG_TRAVEL => {
                let index = args.get(1).and_then(|a| a.trim().parse::<f64>().ok());
                let pin = match (index, self.shown_maps().0) {
                    (Some(i), Some(map)) if i >= 0.0 && i.fract() == 0.0 => {
                        self.map_pins(map).get(i as usize).copied()
                    }
                    _ => None,
                };
                pin.is_some_and(|p| self.begin_travel(p))
            }
            MSG_SWAP => {
                if self.shown_maps().1.is_none() {
                    return false;
                }
                let wm = &mut self.world.world_map;
                wm.swapped = !wm.swapped;
                true
            }
            MSG_CLOSED => {
                let wm = &mut self.world.world_map;
                wm.open = false;
                wm.swapped = false;
                true
            }
            _ => {
                self.client_option_message(name, args)
                    || self.chargen_message(name, args)
                    || self.level_up_message(name, args)
            }
        }
    }

    /// Creates an object for every world map in the resources and each of its pins.
    pub fn load_world_maps(&mut self) {
        let mut names: Vec<String> = self
            .vfs
            .names()
            .into_iter()
            .filter(|n| n.ends_with(".map"))
            .collect();
        names.sort();
        for n in names {
            let Some(m) = self.vfs.read(&n).ok().and_then(|(d, _)| parse_map(&d)) else {
                continue;
            };
            let map = self.world.spawn(ObjectKind::Map, &m.name);
            if let Some(o) = self.world.get_mut(map) {
                o.template = n.trim_end_matches(".map").to_string();
                o.fields.insert("MapId".into(), m.id);
            }
            for p in m.pins {
                let id = self.world.spawn(ObjectKind::MapPin, &p.tag);
                if let Some(o) = self.world.get_mut(id) {
                    o.name_strref = p.name_strref;
                    o.position = [p.position[0], p.position[1], 0.0];
                    o.fields.insert("WMStatus".into(), p.status);
                    o.fields.insert("WMMap".into(), m.id);
                    o.fields.insert("WMTerrain".into(), p.terrain);
                    o.fields.insert("WMIcon".into(), p.icon);
                    o.locals.objects.insert("WM_MAP".into(), map);
                    if let Some(a) = p.area {
                        o.locals.strings.insert("WM_AREA".into(), a);
                    }
                    if let Some(w) = p.waypoint {
                        o.locals.strings.insert("WM_WAYPOINT".into(), w);
                    }
                }
            }
        }
    }

    /// The waypoint the party arrives at when travelling from area `from` to `pin`: the
    /// map's arrival table (column: destination area; row: the area left, or `default`),
    /// else the pin's own waypoint.
    pub fn arrival_waypoint(&self, from: &str, pin: ObjectId) -> String {
        let Some(p) = self.world.get(pin) else {
            return String::new();
        };
        let own = p
            .locals
            .strings
            .get("WM_WAYPOINT")
            .cloned()
            .unwrap_or_default();
        let Some(to) = p.locals.strings.get("WM_AREA").cloned() else {
            return own;
        };
        let map_id = p.fields.get("WMMap").copied().unwrap_or(0) as i64;
        let tables = self.world.tables();
        // worldmaps.gda: the sixth column is the arrival table's M2DA id.
        let table_id = tables.get(0, "worldmaps").and_then(|t| {
            let col = t.columns.iter().find(|(_, i)| **i == 5).map(|(h, _)| *h)?;
            match t.cell_by_hash(col, map_id) {
                Some(Cell::Int(v)) => Some(*v as i32),
                _ => None,
            }
        });
        let Some(t) = table_id.and_then(|id| tables.get(id, "")) else {
            return own;
        };
        let Some(label_col) = t.columns.iter().find(|(_, i)| **i == 1).map(|(h, _)| *h) else {
            return own;
        };
        let row_of = |name: &str| {
            t.order.iter().copied().find(|&r| {
                matches!(t.cell_by_hash(label_col, r),
                    Some(Cell::String(s)) if s.trim_end_matches('\0').eq_ignore_ascii_case(name))
            })
        };
        let cell = row_of(from)
            .or_else(|| row_of("default"))
            .and_then(|r| t.cell(&to, r).cloned());
        match cell {
            Some(Cell::String(s)) if !s.trim_end_matches('\0').trim().is_empty() => {
                s.trim_end_matches('\0').trim().to_string()
            }
            _ => own,
        }
    }

    /// Clicking a pin on the world map: `EVENT_TYPE_BEGIN_TRAVEL` to the module (strings:
    /// the area left, the destination area and its arrival waypoint; integers: 0, the
    /// destination pin's terrain and the map's `worldmaps.gda` row; object: the pin the party
    /// is at). Returns false if the pin cannot be travelled to.
    ///
    /// The integer order is read from sp_module_travel, which logs "World Map: " with integer
    /// 2 and "Target Terrain: " with integer 1, and looks the map up in `worldmaps.gda` for
    /// the trip counter table. Nothing found reads integer 0 (provisional).
    pub fn begin_travel(&mut self, pin: ObjectId) -> bool {
        let active = crate::sim::constant(&self.table, "WM_LOCATION_ACTIVE");
        let Some(p) = self.world.get(pin).filter(|o| o.kind == ObjectKind::MapPin) else {
            return false;
        };
        if p.fields.get("WMStatus").copied() != Some(active) {
            return false;
        }
        let Some(to) = p.locals.strings.get("WM_AREA").cloned() else {
            return false;
        };
        let ints = vec![
            0,
            p.fields.get("WMTerrain").copied().unwrap_or(0),
            p.fields.get("WMMap").copied().unwrap_or(0),
        ];
        let from = self
            .world
            .area
            .and_then(|a| self.world.get(a))
            .map(|a| a.tag.clone())
            .unwrap_or_default();
        let wp = self.arrival_waypoint(&from, pin);
        let Some(module) = self.world.module else {
            return false;
        };
        let mut ev = self.new_event("BEGIN_TRAVEL", self.world.hero);
        ev.strings = vec![from, to, wp];
        ev.ints = ints;
        ev.objects = vec![
            self.world
                .world_map
                .player_pin
                .unwrap_or(thedas_script::vm::OBJECT_INVALID),
        ];
        self.world.world_map.travelling_to = Some(pin);
        self.world.queue_event(0, module, ev, None);
        true
    }

    /// The journey ends (no encounter, or after one): `EVENT_TYPE_WORLDMAP_PRETRANSITION`
    /// to the module, whose script moves the party; arrival then sends the post-transition
    /// event.
    pub fn finish_travel(&mut self) {
        let Some(module) = self.world.module else {
            return;
        };
        self.world.world_map.open = false;
        self.world.world_map.swapped = false;
        self.world.world_map.encounter = false;
        if let Some(pin) = self.world.world_map.travelling_to.take() {
            self.world.world_map.player_pin = Some(pin);
        }
        self.world.world_map.arriving = true;
        let ev = self.new_event("WORLDMAP_PRETRANSITION", self.world.hero);
        self.world.queue_event(0, module, ev, None);
    }

    /// After entering an area: the party's place on the world map becomes the pin that
    /// leads to that area (`WM_AREA`), the primary map's first. No script calls
    /// `SetWorldMapPlayerLocation`, so the engine keeps it. Provisional: inferred from that
    /// and from the travel event carrying the pin the party is at.
    pub(crate) fn place_party_on_map(&mut self, area_tag: &str) {
        let primary = self.world.world_map.primary;
        let mut pins: Vec<(bool, ObjectId)> = self
            .world
            .objects
            .values()
            .filter(|o| o.kind == ObjectKind::MapPin)
            .filter(|o| {
                o.locals
                    .strings
                    .get("WM_AREA")
                    .is_some_and(|a| a.eq_ignore_ascii_case(area_tag))
            })
            .map(|o| (o.locals.objects.get("WM_MAP").copied() != primary, o.id))
            .collect();
        pins.sort();
        if let Some(&(_, pin)) = pins.first() {
            self.world.world_map.player_pin = Some(pin);
        }
    }

    /// After an area transition: the world map screen is gone, and a journey has arrived.
    /// One that arrived through `EVENT_TYPE_WORLDMAP_PRETRANSITION` is told so with
    /// `EVENT_TYPE_WORLDMAP_POSTTRANSITION`; one the travel script ended with a transition of
    /// its own (leaving camp: `DoAreaTransition` back to where the party was) just ends. A
    /// random encounter's area does not end the journey. Provisional.
    pub(crate) fn travel_arrived(&mut self) {
        let wm = &mut self.world.world_map;
        wm.open = false;
        wm.swapped = false;
        if !wm.encounter {
            wm.travelling_to = None;
        }
        if !std::mem::take(&mut self.world.world_map.arriving) {
            return;
        }
        if let Some(module) = self.world.module {
            let ev = self.new_event("WORLDMAP_POSTTRANSITION", self.world.hero);
            self.world.queue_event(0, module, ev, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use thedas_formats::gff4::{FieldDef, FourCC, StructData, StructTemplate, TlkString, Version};

    #[test]
    fn entering_an_area_puts_the_party_on_its_pin_on_the_primary_map() {
        use std::sync::Arc;
        let mut s = Sim::new(
            Arc::new(thedas_vfs::Vfs::new()),
            Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let (main, other) = (
            s.world.spawn(ObjectKind::Map, "ferelden"),
            s.world.spawn(ObjectKind::Map, "denerim"),
        );
        s.world.world_map.primary = Some(main);
        let pin = |s: &mut Sim, map, area: &str| {
            let p = s.world.spawn(ObjectKind::MapPin, "pin");
            let o = s.world.get_mut(p).unwrap();
            o.locals.strings.insert("WM_AREA".into(), area.into());
            o.locals.objects.insert("WM_MAP".into(), map);
            p
        };
        let elsewhere = pin(&mut s, other, "lot100ar_lothering");
        let lothering = pin(&mut s, main, "lot100ar_lothering");
        let _camp = pin(&mut s, main, "cam100ar_camp_plains");
        s.place_party_on_map("LOT100AR_Lothering");
        assert_eq!(s.world.world_map.player_pin, Some(lothering));
        assert_ne!(Some(elsewhere), s.world.world_map.player_pin);
        // An area with no pin leaves the party where it was.
        s.place_party_on_map("pre410ar_tower_level_1");
        assert_eq!(s.world.world_map.player_pin, Some(lothering));
    }

    #[test]
    fn parses_world_maps() {
        let tmpl = |label: &[u8; 4], fields: &[u32]| StructTemplate {
            label: FourCC::new(label),
            fields: fields
                .iter()
                .map(|&l| FieldDef {
                    label: l,
                    type_code: 0,
                    flags: 0,
                    offset: 0,
                })
                .collect(),
            size: 0,
        };
        let st = |t: u32, values: Vec<Value>| StructData {
            template: t,
            values,
        };
        let g = Gff4 {
            version: Version::V40,
            platform: FourCC::new(b"PC  "),
            file_type: FourCC::new(b"MAPO"),
            file_version: FourCC::new(b"V1.0"),
            templates: vec![
                tmpl(b"MAPO", &[24000, 24001, 24002]),
                tmpl(
                    b"MAPP",
                    &[
                        24003, 24004, 24005, 24006, 24007, 24008, 24009, 24010, 24013,
                    ],
                ),
            ],
            structs: vec![
                st(
                    0,
                    vec![
                        Value::String(Some("big_map\0".into())),
                        Value::U32(1),
                        Value::List(Some(List::Struct(vec![Some(StructId(1))]))),
                    ],
                ),
                st(
                    1,
                    vec![
                        Value::U32(3),
                        Value::U32(410),
                        Value::U32(372),
                        Value::Tlk(TlkString { id: 9, text: None }),
                        Value::String(Some("wml_town\0".into())),
                        Value::String(Some("town_area\0".into())),
                        Value::U32(5),
                        Value::U32(6),
                        Value::String(None),
                    ],
                ),
            ],
            root: StructId(0),
            shared_strings: None,
        };
        let m = map_from_gff4(&g).unwrap();
        assert_eq!((m.name.as_str(), m.id), ("big_map", 1));
        let p = &m.pins[0];
        assert_eq!(p.tag, "wml_town");
        assert_eq!(p.area.as_deref(), Some("town_area"));
        assert_eq!(p.waypoint, None);
        assert_eq!((p.status, p.terrain, p.icon), (3, 5, 6));
        assert_eq!(p.position, [410.0, 372.0]);
        assert_eq!(p.name_strref, Some(9));
    }

    #[test]
    fn travel_sends_the_terrain_and_map_where_the_travel_script_reads_them() {
        use std::sync::Arc;
        let mut s = Sim::new(
            Arc::new(thedas_vfs::Vfs::new()),
            Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let module = s.world.spawn(ObjectKind::Module, "module");
        s.world.module = Some(module);
        let pin = s.world.spawn(ObjectKind::MapPin, "wml_town");
        {
            let o = s.world.get_mut(pin).unwrap();
            // WM_LOCATION_ACTIVE is 0 without script.ldf constants.
            o.fields.insert("WMStatus".into(), 0);
            o.fields.insert("WMMap".into(), 1);
            o.fields.insert("WMTerrain".into(), 5);
            o.fields.insert("WMIcon".into(), 6);
            o.locals
                .strings
                .insert("WM_AREA".into(), "town_area".into());
        }
        assert!(s.begin_travel(pin));
        let ev = &s.world.queue.values().next().unwrap().event;
        assert_eq!(ev.ints, vec![0, 5, 1]);
        assert_eq!(ev.strings[1], "town_area");
    }

    #[test]
    fn clicking_a_pin_travels_to_the_shown_maps_pin_at_that_index() {
        use std::sync::Arc;
        let mut s = Sim::new(
            Arc::new(thedas_vfs::Vfs::new()),
            Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let module = s.world.spawn(ObjectKind::Module, "module");
        s.world.module = Some(module);
        let (main, deep) = (
            s.world.spawn(ObjectKind::Map, "ferelden"),
            s.world.spawn(ObjectKind::Map, "deep_roads"),
        );
        s.world.world_map.primary = Some(main);
        // WM_LOCATION_ACTIVE is 0 without script.ldf constants; 1 stands for any other status.
        let pin = |s: &mut Sim, map, area: &str, status| {
            let p = s.world.spawn(ObjectKind::MapPin, area);
            let o = s.world.get_mut(p).unwrap();
            o.fields.insert("WMStatus".into(), status);
            o.locals.strings.insert("WM_AREA".into(), area.into());
            o.locals.objects.insert("WM_MAP".into(), map);
            p
        };
        let _hidden = pin(&mut s, main, "lot100ar_lothering", 1);
        let town = pin(&mut s, main, "arl100ar_redcliffe", 0);
        let thaig = pin(&mut s, deep, "orz500ar_thaig", 0);
        let args = |i: &str| vec!["lot100ar_lothering".to_string(), i.to_string()];

        assert_eq!(s.map_pins(main).len(), 2);
        assert!(!s.gui_message(MSG_TRAVEL, &args("0")), "not an active pin");
        assert!(!s.gui_message(MSG_TRAVEL, &args("7")), "no such pin");
        assert!(!s.gui_message(MSG_TRAVEL, &args("x")));
        assert!(!s.gui_message(MSG_SWAP, &[]), "nothing to swap to");
        s.world.world_map.secondary = Some(thedas_script::vm::OBJECT_INVALID);
        assert!(!s.gui_message(MSG_SWAP, &[]), "a cleared secondary map");
        assert!(s.world.queue.is_empty());
        assert!(s.gui_message(MSG_TRAVEL, &args("1")));
        assert_eq!(s.world.world_map.travelling_to, Some(town));
        let ev = &s.world.queue.values().next().unwrap().event;
        assert_eq!(ev.strings[1], "arl100ar_redcliffe");

        // With a second map swapped in, the indices are the second map's.
        s.world.queue.clear();
        s.world.world_map.travelling_to = None;
        s.world.world_map.secondary = Some(deep);
        assert!(s.gui_message(MSG_SWAP, &[]));
        assert_eq!(s.shown_maps(), (Some(deep), Some(main)));
        assert!(s.gui_message(MSG_TRAVEL, &args("0")));
        assert_eq!(s.world.world_map.travelling_to, Some(thaig));
        s.world.world_map.open = true;
        assert!(s.gui_message(MSG_CLOSED, &["0".into()]));
        assert!(!s.world.world_map.open);
        assert_eq!(s.shown_maps(), (Some(main), Some(deep)));
    }
}
