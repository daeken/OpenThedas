//! DA2's non-ballistic projectile update. See docs/notes/da2-projectile-flight.md
//! for the native trace and the remaining attachment/collision limitations.

use serde::{Deserialize, Serialize};
use thedas_formats::gda::Cell;
use thedas_script::vm::ObjectId;

use crate::sim::Sim;
use crate::world::{ObjectKind, QueuedEvent};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Flight {
    updated_ms: u64,
    target: Option<ObjectId>,
    point: [f32; 3],
    speed: f32,
    acceleration: f32,
    minimum: f32,
    maximum: f32,
    travelled: f32,
    pub(crate) impact: Option<QueuedEvent>,
    pub(crate) custom: bool,
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.into_iter()
        .zip(b)
        .map(|(a, b)| (b - a).powi(2))
        .sum::<f32>()
        .sqrt()
}

impl Flight {
    fn advance(&mut self, from: [f32; 3], point: [f32; 3], now: u64) -> ([f32; 3], bool) {
        let dt = now.saturating_sub(self.updated_ms) as f32 / 1000.;
        self.updated_ms = now;
        if dt == 0. {
            return (from, false);
        }
        self.point = point;
        // 0x5ee598: acceleration first, then bounds, then velocity * update dt.
        self.speed = (self.speed + self.acceleration * dt).clamp(self.minimum, self.maximum);
        let remaining = distance(from, point);
        let step = self.speed * dt;
        let arrived = remaining <= step || remaining < 0.01;
        let to = if arrived {
            point
        } else {
            std::array::from_fn(|i| from[i] + (point[i] - from[i]) * step / remaining)
        };
        self.travelled += distance(from, to);
        (to, arrived)
    }
}

impl Sim {
    /// Returns None for missing/invalid rows or ballistic shots, retaining the older
    /// path for those cases. Native table 4 is a merged table, including overrides.
    pub(crate) fn launch_da2_projectile(
        &mut self,
        kind: i32,
        caster: ObjectId,
        from: [f32; 3],
        target: Option<ObjectId>,
        point: [f32; 3],
    ) -> Option<ObjectId> {
        if self.game != thedas_vfs::Game::DragonAge2
            || !from.into_iter().chain(point).all(f32::is_finite)
        {
            return None;
        }
        let tables = self.world.tables();
        let table = tables.get(4, "")?;
        let number = |hash| {
            match table.cell_by_hash(hash, i64::from(kind))? {
                Cell::Int(n) => Some(*n as f32),
                Cell::Float(n) => Some(*n as f32),
                _ => None,
            }
            .filter(|v| v.is_finite())
        };
        // 0x5aa180 loads acceleration/bounds; 0x5c0030 loads initial speed.
        if number(0xa216_6177)? != 0. {
            return None;
        }
        let (speed, acceleration, minimum, maximum) = (
            number(0x8836_50aa)?,
            number(0xfd48_0c46)?,
            number(0xdb26_d384)?,
            number(0xfd91_9690)?,
        );
        if speed < 0. || minimum < 0. || maximum <= 0. || maximum < minimum {
            return None;
        }
        let area = self.world.get(caster).and_then(|o| o.area);
        let id = self.world.spawn(ObjectKind::Projectile, "");
        let o = self.world.get_mut(id).unwrap();
        o.area = area;
        o.position = from;
        o.fields.insert("ProjectileType".into(), kind);
        self.world.da2_projectiles.insert(
            id,
            Flight {
                updated_ms: self.world.time_ms,
                target,
                point,
                speed,
                acceleration,
                minimum,
                maximum,
                travelled: 0.,
                impact: None,
                custom: false,
            },
        );
        Some(id)
    }

    /// Called at the simulation's existing time advances, after moving actors. No
    /// guessed native fixed timestep is introduced. The update partition is an input
    /// to this semi-implicit integrator, just as the native update dt is.
    pub(crate) fn advance_da2_projectiles(&mut self) {
        let flights = std::mem::take(&mut self.world.da2_projectiles);
        for (id, mut flight) in flights {
            let Some(o) = self.world.get(id) else {
                continue;
            };
            if o.parked.is_some() {
                flight.updated_ms = self.world.time_ms;
                self.world.da2_projectiles.insert(id, flight);
                continue;
            }
            let point = flight
                .target
                .and_then(|t| self.world.get(t))
                .filter(|t| t.area == o.area && t.parked.is_none())
                .map(|t| t.position)
                .unwrap_or(flight.point);
            // 0x5ef840 expires a shot after 1000 metres, independently of impact.
            if flight.travelled > 1000. {
                self.world.queue_destroy(0, id);
                continue;
            }
            let (to, arrived) = flight.advance(o.position, point, self.world.time_ms);
            self.world.get_mut(id).unwrap().position = to;
            if arrived {
                self.world
                    .get_mut(id)
                    .unwrap()
                    .fields
                    .insert("FlightComplete".into(), 1);
                if let Some(mut hit) = flight.impact.take() {
                    // Custom events are owned by the projectile (0x5e9da0).
                    if flight.custom {
                        hit.event.creator = Some(id);
                    }
                    self.world.queue_event(0, hit.target, hit.event, hit.script);
                }
                self.world.queue_destroy(0, id);
            } else {
                self.world.da2_projectiles.insert(id, flight);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::Event;
    use std::sync::Arc;
    use thedas_script::EngineTable;
    use thedas_vfs::{Game, Vfs};

    fn sim() -> (Sim, ObjectId, ObjectId) {
        let mut s = Sim::new(
            Arc::new(Vfs::new()),
            Arc::new(EngineTable::parse(b"").unwrap()),
            1,
        );
        s.game = Game::DragonAge2;
        let tables = crate::m2da::Tables::empty();
        tables.insert(
            "#4",
            &[
                "#a2166177",
                "#883650aa",
                "#fd480c46",
                "#db26d384",
                "#fd919690",
            ],
            vec![(
                17,
                vec![
                    Cell::Int(0),
                    Cell::Float(10.),
                    Cell::Float(10.),
                    Cell::Int(15),
                    Cell::Int(20),
                ],
            )],
        );
        s.world.tables = Arc::new(tables);
        let caster = s.world.spawn(ObjectKind::Creature, "caster");
        let target = s.world.spawn(ObjectKind::Creature, "target");
        s.world.get_mut(target).unwrap().position = [30., 0., 0.];
        (s, caster, target)
    }

    #[test]
    fn acceleration_bounds_homing_and_reload_keep_one_replaceable_payload() {
        let (mut s, caster, target) = sim();
        s.world.time_ms = i32::MAX as u64 + 100;
        let launch = s.world.time_ms;
        let id = s
            .launch_da2_projectile(17, caster, [0.; 3], Some(target), [30., 0., 0.])
            .unwrap();
        s.world.set_projectile_impact_event(
            id,
            caster,
            Event {
                ty: 100,
                ..Default::default()
            },
        );
        s.world.time_ms += 500;
        s.advance_da2_projectiles();
        assert_eq!(s.world.get(id).unwrap().position, [7.5, 0., 0.]);
        assert!(s.world.pop_due(u64::MAX).is_none());
        let bytes = serde_json::to_vec(&s.world).unwrap();
        let mut restored = sim().0;
        restored.world = serde_json::from_slice(&bytes).unwrap();
        for s in [&mut s, &mut restored] {
            s.world.set_projectile_impact_event(
                id,
                caster,
                Event {
                    ty: 200,
                    ..Default::default()
                },
            );
            // Homing changes direction after launch; the next step is clamped at 20 m/s.
            s.world.get_mut(target).unwrap().position = [7.5, 20., 0.];
            s.world.time_ms = launch + 1000;
            s.advance_da2_projectiles();
            assert_eq!(s.world.get(id).unwrap().position, [7.5, 10., 0.]);
            assert!(s.world.pop_due(u64::MAX).is_none());
            s.world.time_ms = launch + 1500;
            s.advance_da2_projectiles();
            let (at, hit) = s.world.pop_due(u64::MAX).unwrap();
            assert_eq!(at, launch + 1500);
            assert_eq!(
                (hit.target, hit.event.ty, hit.event.creator),
                (caster, 200, Some(id))
            );
            // The object survives to end-of-update cleanup, but replacing a landed
            // payload cannot cause another arrival.
            s.world.set_projectile_impact_event(
                id,
                caster,
                Event {
                    ty: 300,
                    ..Default::default()
                },
            );
            assert!(s.world.pop_due(u64::MAX).is_none());
            assert!(s.world.da2_projectiles.is_empty());
            s.world.destroy_due();
            assert!(s.world.get(id).is_none());
        }
        assert_eq!(
            serde_json::to_vec(&s.world).unwrap(),
            serde_json::to_vec(&restored.world).unwrap()
        );
    }

    #[test]
    fn tick_delivers_arrival_in_same_update_and_explicit_destruction_cancels_it() {
        let (mut s, caster, _) = sim();
        s.world.trace.keep = true;
        s.world.get_mut(caster).unwrap().script = Some("hit.ncs".into());
        let mut bytes = b"NCS V1.0B".to_vec();
        bytes.extend(15_u32.to_be_bytes());
        bytes.extend([0x20, 0]);
        s.add_script("hit.ncs", thedas_script::Ncs::parse(&bytes).unwrap());
        let id = s
            .launch_da2_projectile(17, caster, [0.; 3], None, [7.5, 0., 0.])
            .unwrap();
        s.world.set_projectile_impact_event(
            id,
            caster,
            Event {
                ty: 123,
                ..Default::default()
            },
        );
        s.tick(0);
        assert!(s.world.get(id).is_some());
        s.tick(500);
        assert!(s.world.trace.entries.iter().any(|e| matches!(
            e,
            crate::trace::TraceEntry::EventDelivered {
                t: 500,
                ty: 123,
                ..
            }
        )));
        assert!(s.world.get(id).is_none());
        let id = s
            .launch_da2_projectile(17, caster, [0.; 3], None, [30., 0., 0.])
            .unwrap();
        s.world.set_projectile_impact_event(
            id,
            caster,
            Event {
                ty: 456,
                ..Default::default()
            },
        );
        s.world.queue_destroy(0, id);
        s.world.destroy_due();
        s.tick(2000);
        assert!(
            !s.world
                .trace
                .entries
                .iter()
                .any(|e| matches!(e, crate::trace::TraceEntry::EventDelivered { ty: 456, .. }))
        );
    }

    #[test]
    fn origins_and_missing_rows_retain_existing_path() {
        let (mut s, caster, _) = sim();
        assert!(
            s.launch_da2_projectile(99, caster, [0.; 3], None, [30., 0., 0.])
                .is_none()
        );
        s.game = Game::Origins;
        assert!(
            s.launch_da2_projectile(17, caster, [0.; 3], None, [30., 0., 0.])
                .is_none()
        );
        assert!(
            !serde_json::to_string(&s.world)
                .unwrap()
                .contains("da2_projectiles")
        );
    }
}
