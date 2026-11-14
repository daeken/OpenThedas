//! Orders a player gives a party member: move to a point, talk to someone, use something.
//! Each replaces the member's queue with ordinary commands the simulation runs over time
//! (a walk along the pathing grid, then the action), as clicking in the game does.
//!
//! Provisional: how close the original walks before talking or using is not traced.

use thedas_script::vm::ObjectId;

use crate::sim::{Sim, constant};
use crate::world::{Command, Location, facing_toward};

/// How far short of a creature or placeable the walker stops to talk to or use it (metres,
/// on top of the two bodies' radii). Provisional.
pub const INTERACT_GAP: f32 = 0.6;

fn command(sim: &Sim, ty: &str, name: &str) -> Command {
    Command {
        ty: constant(&sim.table, ty),
        name: name.into(),
        target: None,
        location: None,
        seconds: 0.0,
        ints: Vec::new(),
        strings: Vec::new(),
        due: None,
        from: None,
        approach: None,
        ring_wait: false,
        after_impact_ms: None,
        impacted: false,
        arrived: false,
        path: Vec::new(),
    }
}

/// How far apart in height two points of walkable ground may be and still count as one
/// level for a player's move (see `Sim::order_move`). Provisional.
const SAME_LEVEL: f32 = 1.0;

impl Sim {
    /// Makes `who` the party member the player controls (the hero when `None`, or when `who`
    /// is not in the party): its AI stops and the rest follow it.
    pub fn set_controlled(&mut self, who: Option<ObjectId>) {
        self.world.controlled = who.filter(|&w| crate::engine::party_of(self).contains(&w));
    }

    /// The player takes control of party member `who` (a portrait clicked, F1-F4, the pad's
    /// LB/RB, the screens' previous/next character): it becomes the controlled member, and
    /// when it is a follower a training step waiting for one hears of it
    /// (`EVENT_TYPE_TRAINING_FOLLOWER_SELECTED`, the party tutorial's "Select Alistair").
    /// Every way the player changes member goes through here. Provisional: the hero
    /// selected is taken not to count.
    pub fn player_selects(&mut self, who: ObjectId) {
        self.set_controlled(Some(who));
        if self.world.controlled == Some(who) && Some(who) != self.world.hero {
            self.training_event("FOLLOWER_SELECTED", 0);
        }
    }

    /// The player has zoomed the camera fully in (wheel, keys or pad; a script's zoom never
    /// counts): the exe's zoom setter sends the training event for it then (0x7ee680 sends
    /// 1065, `MAXIMUM_ZOOM_IN`, at 0x7ee72e). Returns whether a training step heard it.
    pub fn player_zoomed_in(&mut self) -> bool {
        self.training_event("MAXIMUM_ZOOM_IN", 0)
    }

    /// Walks `who` to `to` (in its area), dropping what it was doing.
    pub fn order_move(&mut self, who: ObjectId, to: [f32; 3]) {
        self.note_player_order(who);
        let Some((area, from, facing)) = self
            .world
            .get(who)
            .and_then(|o| Some((o.area?, o.position, o.facing)))
        else {
            return;
        };
        // A walk to where it already stands (the pad's stop) keeps the way it faces: the
        // direction of a zero step is undefined (atan2(0, 0) would turn it to face +X).
        let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
        let facing = if dx * dx + dy * dy > 1e-6 {
            facing_toward(dx, dy)
        } else {
            facing
        };
        // A place that is not walkable ground (beyond the walkable area, or in a wall: a
        // held stick aims there) is not walked onto. The move goes to the walkable ground
        // nearest it on the walker's own level, which carries a walker pushing into an edge
        // along it; failing that, along the line towards it as far as the ground stays
        // walkable. (A route otherwise ends at the place asked for, and a move without a
        // route takes a straight line: at Lothering the hero ran 290 m off the highway.)
        // Provisional: DAOrigins.exe moves the player's character with its physics
        // controller over the walkable mesh.
        let to = match self.nav_grid(area) {
            Some(g) if !crate::nav::on_walkable(&g, to) => {
                crate::nav::nearest_walkable_point(&g, to)
                    .filter(|p| (p[2] - from[2]).abs() <= SAME_LEVEL)
                    .or_else(|| crate::nav::walkable_reach(&g, from, to))
                    .unwrap_or(from)
            }
            _ => to,
        };
        let mut c = command(
            self,
            "COMMAND_TYPE_MOVE_TO_LOCATION",
            "CommandMoveToLocation",
        );
        c.location = Some(Location {
            area,
            position: to,
            facing,
        });
        if let Some(o) = self.world.get_mut(who) {
            o.commands.clear();
            o.commands.push(c);
        }
    }

    /// Counts an order the player gave `who` (see [`Sim::fail_blocked`]).
    pub(crate) fn note_player_order(&mut self, who: ObjectId) {
        if let Some(o) = self.world.get_mut(who) {
            o.combat.player_orders += 1;
        }
    }

    /// Walks `who` up to `target` and starts its conversation.
    pub fn order_talk(&mut self, who: ObjectId, target: ObjectId) {
        self.note_player_order(who);
        let mut then = command(
            self,
            "COMMAND_TYPE_START_CONVERSATION",
            "CommandStartConversation",
        );
        then.target = Some(target);
        self.approach_then(who, target, then);
    }

    /// Has `who` use `ability` on `target` (itself for an ability used on its user), from
    /// the item tagged `item` when it is an item's (a potion): an ordinary
    /// `CommandUseAbility`, which brings it in range and uses it.
    pub fn order_ability(&mut self, who: ObjectId, ability: i32, target: ObjectId, item: &str) {
        self.note_player_order(who);
        let mut c = command(self, "COMMAND_TYPE_USE_ABILITY", "CommandUseAbility");
        c.target = Some(target);
        c.ints = vec![ability];
        c.strings = vec![item.to_string()];
        if let Some(o) = self.world.get_mut(who) {
            o.commands.clear();
            o.commands.push(c);
        }
    }

    /// Has `who` use `ability` at place `at` in its area (a fireball's centre), from the item
    /// tagged `item` when it is an item's: a `CommandUseAbility` with a location and no
    /// target, which walks `who` into the ability's range first (see `Sim::ability_range`).
    pub fn order_ability_at(&mut self, who: ObjectId, ability: i32, at: [f32; 3], item: &str) {
        self.note_player_order(who);
        let Some((area, from)) = self
            .world
            .get(who)
            .and_then(|o| Some((o.area?, o.position)))
        else {
            return;
        };
        let mut c = command(self, "COMMAND_TYPE_USE_ABILITY", "CommandUseAbility");
        c.location = Some(Location {
            area,
            position: at,
            facing: facing_toward(at[0] - from[0], at[1] - from[1]),
        });
        c.ints = vec![ability];
        c.strings = vec![item.to_string()];
        if let Some(o) = self.world.get_mut(who) {
            o.commands.clear();
            o.commands.push(c);
        }
    }

    /// The cursor a placeable shows under the mouse: its default action in its current state
    /// is a row of `placeable_interactions.gda` (M2DA 100), whose column hashed 0x4f6f860f
    /// names the cursor (`open`, `close`, `unlock`, `chest`, `area_transition`, ...), as
    /// DAOrigins.exe's 0x70cda0 reads it. `None` without an action or a name. Provisional:
    /// the exe shows the row's other cursor (0xd930276d, `disabled_unlock`) for a lock the
    /// controlled creature cannot open; that is not told apart here.
    pub fn placeable_cursor(&self, placeable: ObjectId) -> Option<String> {
        let action = crate::engine::placeable_default_action(self, placeable)?;
        match self
            .world
            .tables()
            .get(100, "")?
            .cell_by_hash(0x4f6f_860f, action as i64)?
        {
            thedas_formats::gda::Cell::String(s) => {
                let s = s.trim_end_matches('\0').trim().to_ascii_lowercase();
                (!s.is_empty()).then_some(s)
            }
            _ => None,
        }
    }

    /// Walks `who` up to `target` (a door, a container, a lever) and uses it.
    pub fn order_use(&mut self, who: ObjectId, target: ObjectId) {
        self.note_player_order(who);
        let mut then = command(self, "COMMAND_TYPE_USE_OBJECT", "CommandUseObject");
        then.target = Some(target);
        self.approach_then(who, target, then);
    }

    /// Replaces `who`'s queue with a walk to within reach of `target`, when it is not already
    /// there, then `then`.
    fn approach_then(&mut self, who: ObjectId, target: ObjectId, then: Command) {
        let (Some(me), Some(t)) = (self.world.get(who), self.world.get(target)) else {
            return;
        };
        if me.area.is_none() || me.area != t.area {
            return;
        }
        let (from, to) = (me.position, t.position);
        let (dx, dy) = (from[0] - to[0], from[1] - to[1]);
        let d = (dx * dx + dy * dy).sqrt();
        let reach = self.body_radius(who) + self.body_radius(target) + INTERACT_GAP;
        let mut queue = Vec::new();
        if d > reach {
            let spot = [to[0] + dx / d * reach, to[1] + dy / d * reach, to[2]];
            let mut c = command(
                self,
                "COMMAND_TYPE_MOVE_TO_LOCATION",
                "CommandMoveToLocation",
            );
            c.location = Some(Location {
                area: me.area.unwrap_or_default(),
                position: spot,
                facing: facing_toward(-dx, -dy),
            });
            queue.push(c);
        }
        queue.push(then);
        if let Some(o) = self.world.get_mut(who) {
            o.commands = queue;
        }
    }

    /// The first point of the pathing grid of `area` a ray from `origin` along `dir` meets
    /// (within `max` metres): where a click on the ground lands. `None` off the grid.
    pub fn ground_hit(
        &mut self,
        area: ObjectId,
        origin: [f32; 3],
        dir: [f32; 3],
        max: f32,
    ) -> Option<[f32; 3]> {
        let g = self.nav_grid(area)?;
        let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        if len.is_nan() || len <= 0.0 || !max.is_finite() {
            return None;
        }
        let d = [dir[0] / len, dir[1] / len, dir[2] / len];
        let step = (g.cell_size * 0.5).max(0.05);
        let mut t = 0.0;
        while t <= max {
            let p = [
                origin[0] + d[0] * t,
                origin[1] + d[1] * t,
                origin[2] + d[2] * t,
            ];
            if let Some(c) = g.cell(p) {
                let floor = crate::nav::height(&g, c);
                if crate::nav::walkable(&g, c) && p[2] <= floor + 0.05 {
                    return Some([p[0], p[1], floor]);
                }
            }
            t += step;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use super::*;
    use crate::world::ObjectKind;

    fn sim() -> (Sim, ObjectId, ObjectId) {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let area = s.world.spawn(ObjectKind::Area, "a");
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let npc = s.world.spawn(ObjectKind::Creature, "npc");
        for (id, x) in [(hero, 0.0), (npc, 10.0)] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = [x, 0.0, 0.0];
        }
        (s, hero, npc)
    }

    #[test]
    fn a_walk_to_its_own_feet_keeps_its_facing() {
        let (mut s, hero, _) = sim();
        s.world.get_mut(hero).unwrap().facing = 90.0;
        s.order_move(hero, [0.0, 0.0, 0.0]);
        let q = &s.world.get(hero).unwrap().commands;
        assert_eq!(q[0].location.unwrap().facing, 90.0);
        // A real step faces along it: +Y is facing 0.
        s.order_move(hero, [0.0, 3.0, 0.0]);
        let f = s.world.get(hero).unwrap().commands[0]
            .location
            .unwrap()
            .facing;
        assert!(f.abs() < 1e-3 || (f - 360.0).abs() < 1e-3, "{f}");
    }

    #[test]
    fn talking_walks_up_to_the_target_first() {
        let (mut s, hero, npc) = sim();
        s.order_talk(hero, npc);
        let q = &s.world.get(hero).unwrap().commands;
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].name, "CommandMoveToLocation");
        let spot = q[0].location.unwrap().position;
        let reach = 2.0 * crate::combat::DEFAULT_RADIUS + INTERACT_GAP;
        assert!((spot[0] - (10.0 - reach)).abs() < 1e-4, "{spot:?}");
        assert_eq!(q[1].name, "CommandStartConversation");
        assert_eq!(q[1].target, Some(npc));
        // Already close: no walk.
        s.world.get_mut(hero).unwrap().position = [9.5, 0.0, 0.0];
        s.order_use(hero, npc);
        let q = &s.world.get(hero).unwrap().commands;
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].name, "CommandUseObject");
    }

    #[test]
    fn the_player_controls_one_party_member_at_a_time() {
        let (mut s, hero, npc) = sim();
        s.world.hero = Some(hero);
        let ally = s.world.spawn(ObjectKind::Creature, "ally");
        s.world.get_mut(hero).unwrap().followers.push(ally);
        assert_eq!(s.world.controlled(), Some(hero));
        s.set_controlled(Some(ally));
        assert_eq!(s.world.controlled(), Some(ally));
        // Someone outside the party cannot be controlled.
        s.set_controlled(Some(npc));
        assert_eq!(s.world.controlled(), Some(hero));
        // A member who leaves the party hands control back to the hero.
        s.set_controlled(Some(ally));
        s.world.get_mut(hero).unwrap().followers.clear();
        assert_eq!(s.world.controlled(), Some(hero));
    }

    #[test]
    fn moving_replaces_the_queue_and_faces_the_way() {
        let (mut s, hero, npc) = sim();
        s.order_talk(hero, npc);
        s.order_move(hero, [0.0, 5.0, 0.0]);
        let q = &s.world.get(hero).unwrap().commands;
        assert_eq!(q.len(), 1);
        let l = q[0].location.unwrap();
        assert_eq!(l.position, [0.0, 5.0, 0.0]);
        assert!(l.facing.abs() < 1e-4, "facing +Y is 0: {}", l.facing);
    }
}
