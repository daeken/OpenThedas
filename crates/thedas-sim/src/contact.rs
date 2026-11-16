//! Walking into an area transition (DAOrigins.exe's physics trigger report, 0x7e96b0).
//!
//! An area transition placeable (state controller `StateCnt_AreaTransition`, such as Dust
//! Town's invisible `bdc200ip_to_commons`) carries an interaction volume in its model's
//! `.phy`. When the controlled creature's body enters it, the trigger report (0x7e96b0,
//! enter status, the placeable's side in the Placeables group) calls 0x74daa0, which, for
//! the controlled creature and a clear latch on the placeable, sets the latch and sends the
//! placeable `EVENT_TYPE_PLACEABLE_COLLISION` (77) with the creature in object slot 0
//! (0x74db45). `placeable_core` answers it as a click would be answered short of the
//! transition itself: it asks the party-gathering question (strref 321167), and a yes
//! travels. See OpenThedas-testdata/notes/tapsters-playtest-traces.md (item 1).
//!
//! Provisional: which placeables' volumes the exe registers as trigger shapes is not traced
//! (taken here to be the area transitions'); the body is a ring of horizontal segments
//! through the creature's middle at its capsule radius rather than PhysX's capsule; the
//! creature field the exe also requires (+0xb20) is not modelled; and the latch, which the
//! exe clears in the placeable's state update (0x74d9d0), is cleared here once the body has
//! left the volume, so walking back in asks again.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use thedas_formats::gda::Cell;
use thedas_script::vm::ObjectId;

use crate::collision::Collision;
use crate::sim::Sim;
use crate::world::ObjectKind;

/// Placeable types' state controller column (`StateController`).
const COL_STATE_CONTROLLER: u32 = 0x9a32_0a51;
/// Placeable types' model column.
const COL_MODEL: u32 = 0xfd84_03d5;
/// The placeable field holding 0x74daa0's latch.
pub(crate) const CONTACT_LATCH: &str = "CollisionLatch";
/// Height of the body's middle above its feet, for the contact test.
const BODY_MIDDLE: f32 = 1.0;

impl Sim {
    /// The controlled creature `id` has moved: area transitions whose volume its body now
    /// touches, and whose latch is clear, are sent `PLACEABLE_COLLISION`.
    pub(crate) fn update_placeable_contacts(&mut self, id: ObjectId) {
        if self.game != thedas_vfs::Game::Origins || self.world.controlled() != Some(id) {
            return;
        }
        let Some((pos, area)) = self
            .world
            .get(id)
            .filter(|o| o.active && !o.dead)
            .and_then(|o| Some((o.position, o.area?)))
        else {
            return;
        };
        let r = self.capsule_radius(id);
        let actors = self.world.actor_ids();
        for &p in actors.iter() {
            let Some(o) = self.world.get(p) else { continue };
            if o.kind != ObjectKind::Placeable || o.area != Some(area) {
                continue;
            }
            let latched = o.fields.get(CONTACT_LATCH).is_some_and(|&v| v != 0);
            let usable = o.active && o.interactive;
            let touching = usable
                && self
                    .contact_shape(p)
                    .is_some_and(|c| body_touches(&c, pos, r));
            if touching && !latched {
                self.world
                    .get_mut(p)
                    .unwrap()
                    .fields
                    .insert(CONTACT_LATCH.into(), 1);
                let mut ev = self.new_event("PLACEABLE_COLLISION", Some(id));
                ev.objects = vec![id];
                self.world.queue_event(0, p, ev, None);
            } else if !touching && latched {
                self.world.get_mut(p).unwrap().fields.remove(CONTACT_LATCH);
            }
        }
    }

    /// An area transition's interaction volume where it stands (`None` for any other
    /// placeable, or one whose model has no volume), built once per model and placement.
    pub(crate) fn contact_shape(&mut self, p: ObjectId) -> Option<Arc<Collision>> {
        let o = self.world.get(p)?;
        let (appearance, position, facing) = (o.appearance, o.position, o.facing);
        let key = contact_key(o);
        if let Some((k, c)) = self.contact_shapes.get(&p)
            && *k == key
        {
            return c.clone();
        }
        let tables = self.world.tables();
        let types = tables.get(93, "")?;
        let row = i64::from(appearance as u16);
        let controller = match types.cell_by_hash(COL_STATE_CONTROLLER, row) {
            Some(Cell::String(c)) => crate::engine::clean(c).to_ascii_lowercase(),
            _ => return None,
        };
        if controller != "statecnt_areatransition" {
            return None;
        }
        let model = match types.cell_by_hash(COL_MODEL, row) {
            Some(Cell::String(m)) => crate::engine::clean(m).to_string(),
            _ => return None,
        };
        let c = Some(crate::collision::load_placeables(
            &self.vfs,
            &[(model, position, facing)],
        ))
        .filter(|c| !c.is_empty())
        .map(Arc::new);
        self.contact_shapes.insert(p, (key, c.clone()));
        c
    }
}

/// What a placeable's volume is built from: its appearance and placement.
fn contact_key(o: &crate::world::Object) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (
        o.appearance,
        o.position.map(f32::to_bits),
        o.facing.to_bits(),
    )
        .hash(&mut h);
    h.finish()
}

/// Whether a body of radius `r` standing at `at` touches `c`: a horizontal segment through
/// its middle, across it in each of four directions, meets the volume.
fn body_touches(c: &Collision, at: [f32; 3], r: f32) -> bool {
    let m = [at[0], at[1], at[2] + BODY_MIDDLE];
    let d = std::f32::consts::FRAC_1_SQRT_2 * r;
    [[r, 0.0], [0.0, r], [d, d], [d, -d]]
        .iter()
        .any(|&[dx, dy]| c.blocked([m[0] - dx, m[1] - dy, m[2]], [m[0] + dx, m[1] + dy, m[2]]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    fn transition() -> (Sim, ObjectId, ObjectId) {
        let table = EngineTable::parse(b"int EVENT_TYPE_PLACEABLE_COLLISION = 77;\n").unwrap();
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(table), 1);
        let area = s.world.spawn(ObjectKind::Area, "slums");
        let hero = s.world.spawn(ObjectKind::Creature, "player");
        let exit = s.world.spawn(ObjectKind::Placeable, "to_commons");
        s.world.area = Some(area);
        s.world.hero = Some(hero);
        for id in [hero, exit] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.active = true;
        }
        s.world.get_mut(exit).unwrap().interactive = true;
        // A 10 m wide, 0.3 m thin wall-like volume across x = 0, 3.4 m tall.
        let wall = Collision::from_boxes(&[([0.0, 0.0, 1.7], [0.15, 5.0, 1.7])]);
        let key = contact_key(s.world.get(exit).unwrap());
        s.contact_shapes.insert(exit, (key, Some(Arc::new(wall))));
        (s, hero, exit)
    }

    fn collisions(s: &Sim, exit: ObjectId) -> usize {
        s.world
            .queue
            .values()
            .filter(|q| q.target == exit && q.event.ty == 77)
            .count()
    }

    #[test]
    fn walking_into_an_area_transition_sends_it_placeable_collision_once() {
        let (mut s, hero, exit) = transition();
        s.move_object(hero, [-3.0, 0.0, 0.0], None);
        assert_eq!(collisions(&s, exit), 0, "short of it");
        s.move_object(hero, [-0.2, 0.0, 0.0], None);
        assert_eq!(collisions(&s, exit), 1, "the body reaches the volume");
        let q = s.world.queue.values().find(|q| q.event.ty == 77).unwrap();
        assert_eq!(q.event.objects, vec![hero]);
        s.move_object(hero, [0.0, 1.0, 0.0], None);
        assert_eq!(collisions(&s, exit), 1, "latched while still touching");
        s.world.queue.clear();
        s.move_object(hero, [-3.0, 0.0, 0.0], None);
        s.move_object(hero, [-0.2, 0.0, 0.0], None);
        assert_eq!(collisions(&s, exit), 1, "walking back in asks again");
    }

    #[test]
    fn only_the_controlled_creature_and_a_usable_transition_count() {
        let (mut s, hero, exit) = transition();
        let other = s.world.spawn(ObjectKind::Creature, "leske");
        let area = s.world.area;
        s.world.get_mut(other).unwrap().area = area;
        s.move_object(other, [-0.2, 0.0, 0.0], None);
        assert_eq!(collisions(&s, exit), 0, "not the controlled creature");
        s.world.get_mut(exit).unwrap().interactive = false;
        s.move_object(hero, [-0.2, 0.0, 0.0], None);
        assert_eq!(collisions(&s, exit), 0, "not usable");
        s.game = thedas_vfs::Game::DragonAge2;
        s.world.get_mut(exit).unwrap().interactive = true;
        s.move_object(hero, [-3.0, 0.0, 0.0], None);
        s.move_object(hero, [-0.2, 0.0, 0.0], None);
        assert_eq!(collisions(&s, exit), 0, "Origins only");
    }
}
