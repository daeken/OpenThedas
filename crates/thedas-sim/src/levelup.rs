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
    /// Unspent attribute, skill and talent points in the current game's property layout.
    /// DA2 has no skill pool; specialization points (40) are separate from its level-up mark.
    pub fn level_up_points(&self, who: ObjectId) -> [f32; 3] {
        let properties = if self.game == thedas_vfs::Game::DragonAge2 {
            [38, 0, 39]
        } else {
            [34, 35, 36]
        };
        properties.map(|p| {
            if p == 0 {
                0.0
            } else {
                self.property_total(who, p)
            }
        })
    }

    /// DA2's SetCanLevelUp ignores its flag and recomputes this from properties 38 + 39
    /// (0x4eaf30 -> 0x544580); Origins uses 34 + 35 + 36.
    pub fn has_level_up_points(&self, who: ObjectId) -> bool {
        self.level_up_points(who).iter().sum::<f32>() > 0.0
    }

    fn refresh_level_up_mark(&mut self, who: ObjectId) {
        let can = self.has_level_up_points(who);
        if let Some(o) = self.world.get_mut(who) {
            o.fields.insert("CanLevelUp".into(), i32::from(can));
        }
    }

    /// A manual attribute choice. DA2's creature BuyAttributeMessage handler (0x578c90)
    /// changes the attribute base by the signed amount and subtracts it from pool 38.
    /// Its sys_chargen deliberately has no CHARGEN_ASSIGN_ATTRIBUTES case.
    pub fn assign_level_up_attribute(&mut self, who: ObjectId, attribute: i32, n: i32) -> bool {
        if self.world.chargen != Some(who) {
            return false;
        }
        if self.game != thedas_vfs::Game::DragonAge2 {
            return self.chargen_event("CHARGEN_ASSIGN_ATTRIBUTES", vec![attribute, n], Vec::new());
        }
        if !(1..=6).contains(&attribute) || n == 0 {
            return false;
        }
        let Some(o) = self.world.get(who) else {
            return false;
        };
        let base = o.properties.get(&attribute).map_or(0.0, |p| p.0);
        let points = self.property_total(who, 38);
        // The movie normally validates its input before sending the creature message.
        if points < n as f32 || base + (n as f32) < 0.0 {
            return false;
        }
        let o = self.world.get_mut(who).unwrap();
        o.properties.entry(attribute).or_default().0 = base + n as f32;
        o.properties.entry(38).or_default().0 = points - n as f32;
        self.refresh_level_up_mark(who);
        true
    }

    /// A manual ability choice. DA2's BuyAbilityMessage (0x598ce0) uses the ability
    /// registry's point pool and cost, adds/removes the learned ability, and updates the
    /// quickbar. The registry loader (0x5cb490) maps types 1, 2, 5 to pool 39 and type 3
    /// to pool 40; its cost is column 0x2058ed0b. The sign requests purchase or refund.
    pub fn assign_level_up_ability(
        &mut self,
        who: ObjectId,
        ability: i32,
        specialization: bool,
    ) -> bool {
        if self.world.chargen != Some(who) {
            return false;
        }
        if self.game != thedas_vfs::Game::DragonAge2 {
            return self.chargen_event(
                "CHARGEN_ASSIGN_ABILITIES",
                vec![ability, i32::from(specialization)],
                Vec::new(),
            );
        }
        let Some(id) = ability.checked_abs().filter(|&id| id > 0) else {
            return false;
        };
        let Some(table) = self.world.tables().get(1, "") else {
            return false;
        };
        let int = |hash| match table.cell_by_hash(hash, i64::from(id)) {
            Some(thedas_formats::gda::Cell::Int(n)) => i32::try_from(*n).ok(),
            _ => None,
        };
        let pool = match int(0x100b_492a) {
            Some(1 | 2 | 5) => 39,
            Some(3) => 40,
            _ => return false,
        };
        let Some(cost) = int(0x2058_ed0b).filter(|&cost| cost >= 0) else {
            return false;
        };
        let Some(o) = self.world.get(who) else {
            return false;
        };
        let buying = ability > 0;
        if o.abilities.contains(&id) == buying {
            return false;
        }
        let points = self.property_total(who, pool);
        if buying && points < cost as f32 {
            return false;
        }
        let o = self.world.get_mut(who).unwrap();
        o.properties.entry(pool).or_default().0 =
            points + if buying { -(cost as f32) } else { cost as f32 };
        if buying {
            o.abilities.insert(id);
            // Native 0x5971d0 places usable abilities on the quickbar. Keep the existing
            // quickbar placement policy; its exact slot selection is still provisional.
            if matches!(self.ability_use_type(id), 2 | 4) {
                self.set_quickslot(who, -1, id, "");
            }
        } else {
            o.abilities.remove(&id);
            o.quickslots
                .retain(|_, q| q.ability != id || !q.item.is_empty());
        }
        self.refresh_level_up_mark(who);
        true
    }

    /// With [`World::pool_xp`](crate::world::World::pool_xp) on, the experience the hero
    /// just gained goes to the followers waiting in the pool (not in the active party) as
    /// well, up to the level cap's experience; one that reaches its next level may level up
    /// (`SetCanLevelUp`), as `RewardXP` marks the active party. A harness option, off by
    /// default, and NOT the original's rule: there, companions left in camp get nothing as
    /// the party fights (`RewardXPParty` pays `GetPartyList`, the active party); each time the
    /// party picker opens (`EVENT_TYPE_PARTYPICKER_INIT`, sent by `GameModePartyPicker` at
    /// 0x76c807), module_core tops every pool member up to the experience of one level below
    /// the hero's, which the simulation runs as the scripts do.
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
            let (have, lvl) = (e.0, o.properties.get(&level).map_or(1.0, |l| l.0));
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
        if self.game == thedas_vfs::Game::DragonAge2 {
            // AutoLevelUpMessage (0x45ef20) sends event 56 to the module, with the
            // creature in object 0. sys_chargen owns the authored allocation and talents.
            if self.world.chargen == Some(who)
                && let Some(module) = self.world.module
            {
                self.world.queue_event(
                    0,
                    module,
                    crate::world::Event {
                        ty: 56,
                        creator: Some(module),
                        objects: vec![who],
                        ..Default::default()
                    },
                    None,
                );
                self.tick(0);
            }
            return;
        }
        let sim = self;
        let prop = |sim: &Sim, p: i32| {
            sim.world
                .get(who)
                .and_then(|o| o.properties.get(&p))
                .map_or(0.0, |v| v.0)
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

    /// The level-up screen's messages, for the creature in level-up only. Origins sends
    /// character generation events; DA2's manual choices use the native creature handlers.
    /// Origins' layouts, from levelup.gfx and sys_chargen:
    /// - attributes: `CHARGEN_ASSIGN_ATTRIBUTES` (attribute, amount);
    /// - abilities: `CHARGEN_ASSIGN_ABILITIES` (ability, 0), and (ability, 1) for a
    ///   specialization; sys_chargen takes a negative ability back;
    /// - closing: `CHARGEN_END` (1, 0), level-up mode, as the screen gives no mode;
    /// - the auto-level option: the points are spent as [`Sim::auto_level`] does, then the
    ///   screen closes.
    pub(crate) fn level_up_message(&mut self, name: &str, args: &[String]) -> bool {
        // DA2 exposes these messages on its creature smart-data object. The bridge
        // includes that object's creature token before the message's own arguments.
        let name = if self.game == thedas_vfs::Game::DragonAge2 && name.ends_with(".BuyAttribute") {
            MSG_ATTRIBUTE
        } else if self.game == thedas_vfs::Game::DragonAge2 && name.ends_with(".BuyAbility") {
            MSG_ABILITY
        } else {
            name
        };
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
                (Some(attr), Some(n)) if n != 0 => self.assign_level_up_attribute(who, attr, n),
                _ => false,
            },
            MSG_ABILITY | MSG_SPECIALIZATION => match int(1).filter(|&a| a != 0) {
                Some(ability) => {
                    self.assign_level_up_ability(who, ability, name == MSG_SPECIALIZATION)
                }
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

    fn da2() -> (Sim, ObjectId) {
        use thedas_formats::gda::Cell;
        let mut s = Sim::new(
            Arc::new(Vfs::new()),
            Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap()),
            1,
        );
        s.game = thedas_vfs::Game::DragonAge2;
        let who = s.world.spawn(ObjectKind::Creature, "hero");
        let module = s.world.spawn(ObjectKind::Module, "module");
        s.world.module = Some(module);
        s.world.hero = Some(who);
        s.world.chargen = Some(who);
        s.world.trace.keep = true;
        let o = s.world.get_mut(who).unwrap();
        o.properties.extend([
            (1, (10.0, 0.0)),
            (7, (50.0, 17.0)),
            (35, (1600.0, 0.0)),
            (36, (2.0, 0.0)),
            (38, (3.0, 0.0)),
            (39, (1.0, 0.0)),
            (40, (1.0, 0.0)),
        ]);
        s.world.tables.insert(
            "#1",
            &["#100b492a", "#2058ed0b"],
            vec![
                (102010, vec![Cell::Int(1), Cell::Int(1)]),
                (102020, vec![Cell::Int(1), Cell::Int(2)]),
                (100100, vec![Cell::Int(3), Cell::Int(1)]),
                (3110, vec![Cell::Int(10), Cell::Int(0)]),
            ],
        );
        (s, who)
    }

    #[test]
    fn da2_manual_levelup_spends_its_own_pools_and_survives_reload() {
        let (mut s, who) = da2();
        let args = |a: &[i32]| a.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert!(s.gui_message("CharGenCreature.BuyAttribute", &args(&[who as i32, 1, 3])));
        assert!(s.gui_message("CharGenCreature.BuyAbility", &args(&[who as i32, 102010])));
        assert_eq!(s.level_up_points(who), [0.0; 3]);
        assert!(
            !s.has_level_up_points(who),
            "specializations do not set the DA2 mark"
        );
        assert_eq!(s.world.get(who).unwrap().properties[&1].0, 13.0);
        assert!(s.world.get(who).unwrap().abilities.contains(&102010));
        assert!(
            s.world.queue.is_empty(),
            "DA2 scripts ignore Origins assignment events"
        );
        s.world = serde_json::from_slice(&serde_json::to_vec(&s.world).unwrap()).unwrap();
        let o = s.world.get(who).unwrap();
        assert_eq!(o.properties[&35].0, 1600.0, "XP is not a skill pool");
        assert_eq!(o.properties[&36].0, 2.0, "level is not a talent pool");
        assert_eq!(o.properties[&7].1, 17.0, "levelling does not heal");
        assert_eq!(o.fields["CanLevelUp"], 0);
        assert!(o.abilities.contains(&102010));
    }

    #[test]
    fn da2_purchases_reject_invalid_or_unaffordable_choices_and_refund_once() {
        let (mut s, who) = da2();
        assert!(!s.assign_level_up_attribute(who, 35, 1));
        assert!(!s.assign_level_up_attribute(who, 1, 4));
        assert!(!s.assign_level_up_ability(who, 102020, false));
        assert!(!s.assign_level_up_ability(who, 3110, false));
        assert!(!s.assign_level_up_ability(who, i32::MIN, false));
        assert!(!s.assign_level_up_ability(who, -102010, false));
        assert!(s.assign_level_up_ability(who, 102010, false));
        assert!(!s.assign_level_up_ability(who, 102010, false));
        assert!(s.assign_level_up_ability(who, -102010, false));
        assert!(!s.assign_level_up_ability(who, -102010, false));
        assert_eq!(s.level_up_points(who), [3.0, 0.0, 1.0]);
        assert!(s.assign_level_up_ability(who, 100100, true));
        assert_eq!(s.world.get(who).unwrap().properties[&40].0, 0.0);
        assert_eq!(s.level_up_points(who), [3.0, 0.0, 1.0]);
        s.world.chargen = None;
        assert!(!s.assign_level_up_attribute(who, 1, 1));
        assert!(!s.assign_level_up_ability(who, 102010, false));
    }

    #[test]
    fn da2_auto_level_uses_the_module_script_event() {
        use crate::trace::TraceEntry;
        let (mut s, who) = da2();
        s.auto_level(who);
        assert_eq!(
            s.level_up_points(who),
            [3.0, 0.0, 1.0],
            "no invented fallback allocation without a script"
        );
        assert!(s.world.trace.entries.iter().any(|e| matches!(e, TraceEntry::EventQueued { ty: 56, target, .. } if Some(*target) == s.world.module)));
    }

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
