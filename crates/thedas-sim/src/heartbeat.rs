//! Origins' per-creature scripted heartbeat (0x6be889), after the event pump.

use crate::Sim;
use crate::world::{Heartbeat, ObjectKind};

impl Sim {
    /// Older snapshots have no float clock, only the interval in whole milliseconds (the
    /// field Origins' setters no longer write). Their pending events are kept: those cannot be
    /// distinguished from script-created HEARTBEAT2 events. Done once when a save loads, so
    /// that ticks need not look the field up on every creature.
    pub(crate) fn restore_script_heartbeats(&mut self) {
        if self.game != thedas_vfs::Game::Origins {
            return;
        }
        for o in self.world.objects.values_mut() {
            if o.kind != ObjectKind::Creature {
                continue;
            }
            if let Some(ms) = o.fields.remove("HeartbeatMs") {
                o.heartbeat.get_or_insert(Heartbeat {
                    interval_bits: (ms as f32 * 0.001).to_bits(),
                    elapsed: 0.0,
                });
            }
        }
    }

    pub(crate) fn advance_script_heartbeats(&mut self, ms: u64) {
        if self.game != thedas_vfs::Game::Origins {
            return;
        }
        let seconds = ms as f32 * 0.001;
        let mut beats = Vec::new();
        for o in self.world.objects.values_mut() {
            if o.kind != ObjectKind::Creature {
                continue;
            }
            if !o.active || o.parked.is_some() {
                continue;
            }
            let Some(clock) = o.heartbeat.as_mut().filter(|c| c.interval() > 0.0) else {
                continue;
            };
            clock.elapsed += seconds;
            if clock.elapsed > clock.interval() {
                beats.push((o.id, clock.elapsed));
                clock.elapsed = 0.0;
            }
        }
        // 0x64ace0 is queued, not immediate. The next pump delivers these, even if
        // EndHeartbeat runs meanwhile. No catch-up loop or leftover delta.
        for (target, elapsed) in beats {
            let mut ev = self.new_event("HEARTBEAT2", Some(target));
            ev.floats.push(elapsed);
            self.world.queue_event(0, target, ev, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_script::vm::{ObjectId, Value};
    use thedas_vfs::{Game, Vfs};

    use super::*;
    use crate::sim::ScriptHost;
    use crate::world::Event;

    fn sim() -> (Sim, ObjectId) {
        let table = EngineTable::parse(
            b"void InitHeartbeat(object o, float f) = 342;\n\
              void EndHeartbeat(object o) = 343;\n\
              int EVENT_TYPE_HEARTBEAT2 = 81;\n",
        )
        .unwrap();
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(table), 1);
        let id = s.world.spawn(ObjectKind::Creature, "clock");
        (s, id)
    }

    fn call(s: &mut Sim, id: ObjectId, interval: Option<f32>) {
        let name = if interval.is_some() {
            "InitHeartbeat"
        } else {
            "EndHeartbeat"
        };
        let f = s.table.function_by_name(name).cloned().unwrap();
        let mut host = ScriptHost {
            sim: s,
            event: None,
            script: "test".into(),
        };
        let mut args = vec![Value::Object(id)];
        if let Some(interval) = interval {
            args.push(Value::Float(interval));
        }
        crate::engine::call(&mut host, id, &f, &args)
            .unwrap()
            .unwrap();
    }

    fn beats(s: &Sim) -> Vec<(u64, Event)> {
        s.world
            .queue
            .iter()
            .filter(|(_, q)| q.event.ty == 81)
            .map(|(k, q)| (k.0, q.event.clone()))
            .collect()
    }

    #[test]
    fn threshold_payload_and_next_pump_follow_the_creature_update() {
        let (mut s, id) = sim();
        call(&mut s, id, Some(0.0625));
        assert!(beats(&s).is_empty());
        s.tick(0);
        s.tick(62); // Below the exact 1/16-second threshold.
        assert!(beats(&s).is_empty());
        s.tick(1);
        let beat = beats(&s);
        assert_eq!(beat.len(), 1);
        assert_eq!(beat[0].0, 63);
        assert_eq!(beat[0].1.creator, Some(id));
        assert_eq!(beat[0].1.floats, vec![0.062 + 0.001]);
        assert!(beat[0].1.ints.is_empty() && beat[0].1.objects.is_empty());
        assert_eq!(s.world.get(id).unwrap().heartbeat.unwrap().elapsed, 0.0);
        s.tick(0); // Only the next pump consumes the emitted event.
        assert!(beats(&s).is_empty());

        call(&mut s, id, Some(1.0));
        s.tick(1000);
        assert!(beats(&s).is_empty()); // Equality is not enough.
        s.tick(1);
        assert_eq!(beats(&s)[0].1.floats, vec![1.0 + 0.001]);
        s.tick(10_000);
        assert_eq!(beats(&s).len(), 1); // One emission, no catch-up or remainder.
        assert_eq!(beats(&s)[0].1.floats, vec![10.0]);
    }

    #[test]
    fn restart_and_stop_do_not_create_chains_or_cancel_emitted_events() {
        let (mut s, id) = sim();
        call(&mut s, id, Some(1.0));
        s.tick(750);
        call(&mut s, id, Some(2.0));
        s.tick(1500);
        assert!(beats(&s).is_empty());
        call(&mut s, id, None);
        s.tick(10_000);
        assert!(beats(&s).is_empty());

        call(&mut s, id, Some(0.01));
        s.tick(20);
        let emitted = beats(&s);
        call(&mut s, id, None);
        assert_eq!(beats(&s), emitted);
        // Script-created beats must not restart the clock either.
        s.world.queue_event(
            100,
            id,
            Event {
                ty: 81,
                ..Default::default()
            },
            None,
        );
        s.tick(5000);
        assert!(beats(&s).is_empty());
        assert!(s.world.get(id).unwrap().heartbeat.is_none());
    }

    #[test]
    fn inactive_and_parked_clocks_pause_but_death_does_not_stop_them() {
        let (mut s, id) = sim();
        call(&mut s, id, Some(1.0));
        s.tick(500);
        s.world.get_mut(id).unwrap().active = false;
        s.tick(5000);
        s.world.get_mut(id).unwrap().active = true;
        s.world.get_mut(id).unwrap().parked = Some(true);
        s.tick(5000);
        assert_eq!(s.world.get(id).unwrap().heartbeat.unwrap().elapsed, 0.5);
        assert!(beats(&s).is_empty());
        s.world.get_mut(id).unwrap().parked = None;
        s.world.get_mut(id).unwrap().dead = true;
        s.tick(501);
        assert_eq!(beats(&s).len(), 1);
    }

    #[test]
    fn intervals_below_the_old_minimum_and_disabled_intervals_are_preserved() {
        let (mut s, id) = sim();
        for interval in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            call(&mut s, id, Some(interval));
            s.tick(10_000);
            assert!(beats(&s).is_empty());
            assert_eq!(
                s.world.get(id).unwrap().heartbeat.unwrap().interval_bits,
                interval.to_bits()
            );
            let bytes = s.save_json().unwrap();
            s.load_json(&bytes).unwrap();
            assert_eq!(
                s.world.get(id).unwrap().heartbeat.unwrap().interval_bits,
                interval.to_bits()
            );
        }
        call(&mut s, id, Some(0.0005));
        s.tick(1);
        assert_eq!(beats(&s).len(), 1);
        assert_eq!(beats(&s)[0].1.floats, vec![0.001]);
    }

    #[test]
    fn snapshots_keep_fractional_progress_and_old_pending_events_run_once() {
        let (mut s, id) = sim();
        call(&mut s, id, Some(0.12345));
        s.tick(47);
        let bytes = s.save_json().unwrap();
        let (mut loaded, _) = sim();
        loaded.load_json(&bytes).unwrap();
        s.tick(80);
        loaded.tick(80);
        assert_eq!(s.save_json().unwrap(), loaded.save_json().unwrap());
        assert_eq!(beats(&s).len(), 1);

        let (mut old, id) = sim();
        old.world
            .get_mut(id)
            .unwrap()
            .fields
            .insert("HeartbeatMs".into(), 1250);
        old.world.queue_event(
            500,
            id,
            Event {
                ty: 81,
                ..Default::default()
            },
            None,
        );
        let bytes = old.save_json().unwrap();
        let (mut loaded, _) = sim();
        loaded.load_json(&bytes).unwrap();
        loaded.tick(600);
        assert!(beats(&loaded).is_empty());
        let o = loaded.world.get(id).unwrap();
        assert!(!o.fields.contains_key("HeartbeatMs"));
        assert_eq!(
            o.heartbeat.unwrap(),
            Heartbeat {
                interval_bits: 1.25f32.to_bits(),
                elapsed: 0.6
            }
        );
        loaded.tick(700);
        assert_eq!(beats(&loaded).len(), 1);
        loaded.tick(0);
        assert!(beats(&loaded).is_empty());

        let (mut da2, id) = sim();
        da2.game = Game::DragonAge2;
        call(&mut da2, id, Some(0.01));
        da2.tick(100);
        assert!(da2.world.get(id).unwrap().heartbeat.is_none());
        assert_eq!(da2.world.get(id).unwrap().fields["HeartbeatMs"], 100);
        assert_eq!(beats(&da2)[0].0, 200); // Existing DA2 future-event model.
    }
}
