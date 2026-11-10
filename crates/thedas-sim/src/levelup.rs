//! Levelling party members up the way the game's auto-level setting does, for players that
//! do not pick themselves (the MCP's `level_up` tool and `thedas autoplay`).

use thedas_script::vm::ObjectId;

use crate::sim::Sim;

impl Sim {
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
