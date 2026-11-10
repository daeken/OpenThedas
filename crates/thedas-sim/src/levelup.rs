//! Levelling party members up the way the game's auto-level setting does, for players that
//! do not pick themselves (the MCP's `level_up` tool and `thedas autoplay`).

use thedas_script::vm::ObjectId;

use crate::sim::Sim;

/// The level-up screen (levelup.gfx) opened for the creature (its id).
pub const MSG_START: &str = "GUILevelUp.CharGenStart";
/// The auto-level option is on: the screen asks the game to spend the points and closes.
pub const MSG_AUTO: &str = "GUILevelUp.AutoLevelUp";
/// Attribute points: the creature, the attribute and the amount (negative to take back).
pub const MSG_ATTRIBUTE: &str = "GUICharGen.CharGenAssignAttribute";
/// A talent, spell or skill: the creature and the ability (negative to unlearn).
pub const MSG_ABILITY: &str = "GUICharGen.CharGenAssignAbility";
/// A specialization: the creature and the ability (negative to take back).
pub const MSG_SPECIALIZATION: &str = "GUILevelUp.CharGenAssignAbility";
/// The screen closed: the creature and how it was left.
pub const MSG_END: &str = "GUILevelUp.CharGenEnd";
/// Character creation's name: the creature and the name typed (the interface adds the name
/// from `GUICharGen.TempCharName`, where the screen leaves it). The module's chargen script
/// hears `EVENT_TYPE_CHARGEN_SELECT_NAME` with the name and calls `SetName`.
pub const MSG_SELECT_NAME: &str = "GUICharGen.CharGenSelectName";
/// The screen asks for the creature's ability lists (creature, ability type, display mode),
/// which the interface binding provides; nothing changes in the game.
pub const MSG_CHAINS: &str = "GameModeController.FindAbilityChains";

impl Sim {
    /// With [`World::pool_xp`](crate::world::World::pool_xp) on, the experience the hero
    /// just gained goes to the followers waiting in the pool (not in the active party) as
    /// well, up to the level cap's experience; one that reaches its next level may level up
    /// (`SetCanLevelUp`), as `RewardXP` marks the active party. Provisional and off by
    /// default: in the original, companions left in camp are said to keep pace with the
    /// hero, but no script gives them experience (`RewardXPParty` pays `GetPartyList`, the
    /// active party), so if it happens the executable does it, by a rule not recovered.
    pub fn share_pool_xp(&mut self, gained: f32) {
        if !self.world.pool_xp || gained <= 0.0 {
            return;
        }
        let xp = crate::sim::constant(&self.table, "PROPERTY_SIMPLE_EXPERIENCE");
        let level = crate::sim::constant(&self.table, "PROPERTY_SIMPLE_LEVEL");
        let tables = self.world.tables();
        let needed = |l: i64| {
            tables
                .get(0, "exptable")
                .and_then(|t| match t.cell("XP", l)? {
                    thedas_formats::gda::Cell::Int(v) => Some(*v as f32),
                    thedas_formats::gda::Cell::Float(v) => Some(*v as f32),
                    _ => None,
                })
        };
        let cap = tables
            .get(0, "exptable")
            .and_then(|t| t.order.iter().max().copied())
            .and_then(needed)
            .unwrap_or(f32::MAX);
        let active = crate::engine::party_of(self);
        for id in self.party_pool() {
            if active.contains(&id) {
                continue;
            }
            let Some(o) = self.world.get_mut(id) else {
                continue;
            };
            let e = o.properties.entry(xp).or_insert((0.0, 0.0));
            e.0 = (e.0 + gained).min(cap);
            let (have, lvl) = (e.0, o.properties.get(&level).map_or(1.0, |l| l.0.max(l.1)));
            if needed(lvl as i64 + 1).is_some_and(|n| have >= n) {
                o.fields.insert("CanLevelUp".into(), 1);
            }
        }
    }

    /// Spends a creature's remaining level-up points from its auto-level table, through the
    /// chargen events: the table is the follower's own (`ALAlistair`, ...), its package's
    /// `LevelupTable`, or its class's default (`alwarrior_default`, ...), as player_core picks
    /// it. Provisional reading of the table: column 5 of rows 1-6 weighs the attributes
    /// (strength .. constitution) and columns 1-4 list abilities, earlier rows first; each is
    /// tried while points of its kind remain (skills spend skill points, the rest talent points).
    pub fn auto_level(&mut self, who: ObjectId) {
        let sim = self;
        let prop = |sim: &Sim, p: i32| {
            sim.world
                .get(who)
                .and_then(|o| o.properties.get(&p))
                .map_or(0.0, |v| v.0.max(v.1))
        };
        let tag = sim
            .world
            .get(who)
            .map(|o| o.tag.to_ascii_lowercase())
            .unwrap_or_default();
        let own = [
            ("gen00fl_alistair", 256),
            ("gen00fl_dog", 257),
            ("gen00fl_leliana", 258),
            ("gen00fl_loghain", 259),
            ("gen00fl_morrigan", 260),
            ("gen00fl_oghren", 261),
            ("gen00fl_sten", 262),
            ("gen00fl_wynne", 263),
        ];
        let package = sim
            .world
            .get(who)
            .and_then(|o| o.fields.get("Package").copied())
            .unwrap_or(0);
        let tables = sim.world.tables();
        let int_cell = |c: Option<&thedas_formats::gda::Cell>| match c {
            Some(thedas_formats::gda::Cell::Int(v)) => *v as i32,
            Some(thedas_formats::gda::Cell::Float(v)) => *v as i32,
            _ => 0,
        };
        let from_package = tables
            .get(163, "")
            .map(|t| int_cell(t.cell("LevelupTable", package as i64)))
            .unwrap_or(0);
        let class = prop(sim, 27) as i32;
        let table = own
            .iter()
            .find(|(t, _)| *t == tag)
            .map(|(_, id)| *id)
            .or((from_package > 0).then_some(from_package))
            .unwrap_or(match class {
                2 => 1003,
                3 => 1004,
                _ => 1002,
            });
        let Some(t) = tables.get(table, "") else {
            return;
        };
        let col = |i: usize| t.columns.iter().find(|(_, c)| **c == i).map(|(h, _)| *h);
        // Attributes, by weight.
        let points = prop(sim, 34) as i32;
        if points > 0 {
            let weights: Vec<f32> = (1..=6)
                .map(|r| match col(5).and_then(|h| t.cell_by_hash(h, r)) {
                    Some(thedas_formats::gda::Cell::Float(v)) => *v as f32,
                    Some(thedas_formats::gda::Cell::Int(v)) => *v as f32,
                    _ => 0.0,
                })
                .collect();
            let total: f32 = weights.iter().sum();
            if total > 0.0 {
                let mut give: Vec<i32> = weights
                    .iter()
                    .map(|w| (points as f32 * w / total).floor() as i32)
                    .collect();
                let mut left = points - give.iter().sum::<i32>();
                let mut order: Vec<usize> = (0..6).collect();
                order.sort_by(|a, b| weights[*b].total_cmp(&weights[*a]));
                for i in order.iter().cycle().take(64) {
                    if left <= 0 {
                        break;
                    }
                    if weights[*i] > 0.0 {
                        give[*i] += 1;
                        left -= 1;
                    }
                }
                for (i, n) in give.iter().enumerate() {
                    if *n > 0 {
                        sim.chargen_event(
                            "CHARGEN_ASSIGN_ATTRIBUTES",
                            vec![i as i32 + 1, *n],
                            Vec::new(),
                        );
                        sim.tick(0);
                    }
                }
            }
        }
        // Abilities, in table order.
        let skill_type = 3;
        for &row in &t.order {
            if row == 0 {
                continue;
            }
            for c in 1..=4 {
                let Some(id) = col(c).map(|h| int_cell(t.cell_by_hash(h, row))) else {
                    continue;
                };
                if id <= 0
                    || sim
                        .world
                        .get(who)
                        .is_some_and(|o| o.abilities.contains(&id))
                {
                    continue;
                }
                let kind = tables
                    .get(1, "")
                    .map(|a| int_cell(a.cell("AbilityType", id as i64)))
                    .unwrap_or(0);
                let pool = if kind == skill_type { 35 } else { 36 };
                if prop(sim, 35) < 1.0 && prop(sim, 36) < 1.0 {
                    return;
                }
                if prop(sim, pool) < 1.0 {
                    continue;
                }
                sim.chargen_event("CHARGEN_ASSIGN_ABILITIES", vec![id, 0], Vec::new());
                sim.tick(0);
            }
        }
    }

    /// The level-up screen's messages, as player input: each choice becomes the character
    /// generation event sys_chargen handles (the same events [`Sim::level_up_auto`] and the
    /// MCP's level-up tool send), for the creature in level-up only. Provisional, from
    /// levelup.gfx and sys_chargen:
    /// - attributes: `CHARGEN_ASSIGN_ATTRIBUTES` (attribute, amount);
    /// - abilities: `CHARGEN_ASSIGN_ABILITIES` (ability, 0), and (ability, 1) for a
    ///   specialization; sys_chargen takes a negative ability back;
    /// - closing: `CHARGEN_END` (1, 0), level-up mode, as the screen gives no mode;
    /// - the auto-level option: the points are spent as [`Sim::auto_level`] does, then the
    ///   screen closes.
    pub(crate) fn level_up_message(&mut self, name: &str, args: &[String]) -> bool {
        let int = |i: usize| {
            args.get(i)
                .and_then(|a| a.trim().parse::<f64>().ok())
                .filter(|v| v.fract() == 0.0 && v.abs() < i32::MAX as f64)
                .map(|v| v as i32)
        };
        let Some(who) = int(0).filter(|&w| w >= 0).map(|w| w as ObjectId) else {
            return false;
        };
        if !matches!(
            name,
            MSG_START
                | MSG_AUTO
                | MSG_ATTRIBUTE
                | MSG_ABILITY
                | MSG_SPECIALIZATION
                | MSG_END
                | MSG_CHAINS
                | MSG_SELECT_NAME
        ) || self.world.chargen != Some(who)
        {
            return false;
        }
        match name {
            MSG_START | MSG_CHAINS => true,
            MSG_AUTO => {
                self.auto_level(who);
                self.chargen_event("CHARGEN_END", vec![1, 0], Vec::new())
            }
            MSG_SELECT_NAME => match args.get(1) {
                Some(name) if !name.trim().is_empty() => self.chargen_event(
                    "CHARGEN_SELECT_NAME",
                    Vec::new(),
                    vec![name.trim().to_string()],
                ),
                _ => false,
            },
            MSG_ATTRIBUTE => match (int(1), int(2)) {
                (Some(attr), Some(n)) if n != 0 => {
                    self.chargen_event("CHARGEN_ASSIGN_ATTRIBUTES", vec![attr, n], Vec::new())
                }
                _ => false,
            },
            MSG_ABILITY | MSG_SPECIALIZATION => match int(1).filter(|&a| a != 0) {
                Some(ability) => self.chargen_event(
                    "CHARGEN_ASSIGN_ABILITIES",
                    vec![ability, (name == MSG_SPECIALIZATION) as i32],
                    Vec::new(),
                ),
                None => false,
            },
            _ => self.chargen_event("CHARGEN_END", vec![1, 0], Vec::new()),
        }
    }

    /// Opens a creature's level-up screen (as reaching a new level does), spends its points
    /// with [`Sim::auto_level`] and closes it. Returns false if the screen did not open (in
    /// combat, or not a party member).
    pub fn level_up_auto(&mut self, who: ObjectId) -> bool {
        self.request_level_up(who);
        self.tick(0);
        if self.world.chargen != Some(who) {
            return false;
        }
        self.auto_level(who);
        self.chargen_event("CHARGEN_END", vec![1, 0], Vec::new());
        self.tick(0);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use super::*;
    use crate::world::ObjectKind;

    const LDF: &str = "int EVENT_TYPE_CHARGEN_ASSIGN_ATTRIBUTES = 50;\n\
        int EVENT_TYPE_CHARGEN_ASSIGN_ABILITIES = 51;\n\
        int EVENT_TYPE_CHARGEN_END = 59;\n";

    #[test]
    fn the_level_up_screens_choices_become_chargen_events_for_its_creature() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let module = s.world.spawn(ObjectKind::Module, "module");
        let (dog, hero) = (
            s.world.spawn(ObjectKind::Creature, "dog"),
            s.world.spawn(ObjectKind::Creature, "hero"),
        );
        s.world.module = Some(module);
        let args = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(
            !s.gui_message(MSG_ATTRIBUTE, &args(&["2", "1", "1"])),
            "no level-up open"
        );
        s.world.chargen = Some(dog);
        let d = dog.to_string();
        let h = hero.to_string();
        assert!(
            !s.gui_message(MSG_ATTRIBUTE, &args(&[&h, "1", "1"])),
            "someone else"
        );
        assert!(s.gui_message(MSG_START, &args(&[&d, "1"])));
        assert!(s.gui_message(MSG_CHAINS, &args(&[&d, "3", "2"])));
        assert!(s.world.queue.is_empty(), "nothing for the game yet");
        assert!(s.gui_message(MSG_ATTRIBUTE, &args(&[&d, "1", "2"])));
        assert!(!s.gui_message(MSG_ATTRIBUTE, &args(&[&d, "1", "0"])));
        assert!(s.gui_message(MSG_ABILITY, &args(&[&d, "90048", "0"])));
        assert!(s.gui_message(MSG_ABILITY, &args(&[&d, "-90048", "0"])));
        assert!(s.gui_message(MSG_SPECIALIZATION, &args(&[&d, "4016", "1"])));
        assert!(!s.gui_message(MSG_ABILITY, &args(&[&d, "x", "0"])));
        assert!(s.gui_message(MSG_END, &args(&[&d, "1", "1"])));
        assert_eq!(s.world.chargen, None);
        let sent: Vec<_> = s
            .world
            .queue
            .values()
            .map(|q| (q.target, q.event.ty, q.event.ints.clone()))
            .collect();
        assert_eq!(
            sent,
            vec![
                (module, 50, vec![1, 2]),
                (module, 51, vec![90048, 0]),
                (module, 51, vec![-90048, 0]),
                (module, 51, vec![4016, 1]),
                (module, 59, vec![1, 0]),
            ]
        );
    }

    #[test]
    fn the_name_typed_reaches_the_chargen_script_and_names_the_creature() {
        let table = EngineTable::parse(
            format!("{LDF}int EVENT_TYPE_CHARGEN_SELECT_NAME = 49;\n").as_bytes(),
        )
        .unwrap();
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(table), 1);
        let module = s.world.spawn(ObjectKind::Module, "module");
        let hero = s.world.spawn(ObjectKind::Creature, "player");
        s.world.module = Some(module);
        s.world.chargen = Some(hero);
        let h = hero.to_string();
        assert!(!s.gui_message(MSG_SELECT_NAME, &[h.clone(), "  ".into()]));
        assert!(s.gui_message(MSG_SELECT_NAME, &[h, "Elissa".into()]));
        let sent: Vec<_> = s
            .world
            .queue
            .values()
            .map(|q| (q.event.ty, q.event.strings.clone()))
            .collect();
        assert_eq!(sent, vec![(49, vec!["Elissa".to_string()])]);
        // What the chargen script then does (SetName), and how the name reads back.
        assert_eq!(s.display_name(hero), "player");
        s.world.get_mut(hero).unwrap().name = Some("Elissa".into());
        assert_eq!(s.display_name(hero), "Elissa");
        let saved = s.save_json().unwrap();
        assert!(String::from_utf8_lossy(&saved).contains("\"name\":\"Elissa\""));
    }
}
