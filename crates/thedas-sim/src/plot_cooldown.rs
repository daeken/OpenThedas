//! Origins native 861: plot-action cooldowns and their completion callbacks.

use serde::{Deserialize, Serialize};
use thedas_formats::gda::Cell;
use thedas_script::vm::ObjectId;

use crate::sim::Sim;
use crate::world::{Event, ObjectKind};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlotCooldown {
    /// Raw float bits retain native infinity and the resulting NaN percentage.
    pub total_bits: u32,
    pub remaining_bits: u32,
    pub percent_bits: u32,
    pub last_ms: u64,
    pub event_type: i32,
    pub caller: ObjectId,
    pub recipient: ObjectId,
    pub script: Option<String>,
}

impl PlotCooldown {
    fn event(&self, action: i32, creator: Option<ObjectId>) -> Event {
        Event {
            ty: self.event_type,
            creator,
            target: Some(self.recipient),
            ints: vec![action],
            ..Default::default()
        }
    }
}

impl Sim {
    fn plot_action_group(&self, action: i32) -> Option<i32> {
        let table = self.world.tables().get(142, "")?;
        if !table.rows.contains_key(&i64::from(action)) {
            return None;
        }
        Some(match table.cell_by_hash(0x16a3_cb96, i64::from(action)) {
            Some(Cell::Int(n)) => *n as i32,
            _ => 0,
        })
    }

    pub(crate) fn set_plot_action_cooldown(
        &mut self,
        action: i32,
        seconds: f32,
        mut state: PlotCooldown,
    ) {
        if self.game != thedas_vfs::Game::Origins
            || seconds < 0.0
            || seconds.is_nan()
            || self.plot_action_group(action).is_none()
        {
            return;
        }
        self.advance_plot_action_cooldowns();
        state.total_bits = seconds.to_bits();
        state.remaining_bits = seconds.to_bits();
        state.percent_bits = 1.0_f32.to_bits();
        state.last_ms = self.world.time_ms;
        self.world.plot_actions.cooldowns.insert(action, state);
    }

    /// CooldownTime and CooldownPercent reflected by the native action record.
    pub fn plot_action_cooldown(&self, action: i32) -> Option<(f32, f32)> {
        self.world.plot_actions.cooldowns.get(&action).map(|c| {
            (
                f32::from_bits(c.remaining_bits),
                f32::from_bits(c.percent_bits),
            )
        })
    }

    pub(crate) fn advance_plot_action_cooldowns(&mut self) {
        if self.game != thedas_vfs::Game::Origins {
            return;
        }
        let now = self.world.time_ms;
        let actions: Vec<_> = self.world.plot_actions.cooldowns.keys().copied().collect();
        for action in actions {
            let active = self
                .world
                .controlled()
                .and_then(|id| self.world.get(id))
                .is_some_and(|o| o.kind == ObjectKind::Creature)
                && self.world.plot_actions.set != 0
                && self.plot_action_group(action) == Some(self.world.plot_actions.set)
                && self
                    .world
                    .plot_actions
                    .states
                    .get(&action)
                    .copied()
                    .unwrap_or(0)
                    != 0;
            let Some(c) = self.world.plot_actions.cooldowns.get_mut(&action) else {
                continue;
            };
            let elapsed = now.saturating_sub(c.last_ms) as f32 / 1000.0;
            c.last_ms = now;
            let remaining = f32::from_bits(c.remaining_bits);
            if !active || remaining <= 0.0 || remaining.is_nan() {
                continue;
            }
            let remaining = (remaining - elapsed).max(0.0);
            c.remaining_bits = remaining.to_bits();
            c.percent_bits = (remaining / f32::from_bits(c.total_bits)).to_bits();
            if remaining == 0.0 {
                // Mark expiry before synchronous delivery, so a reentrant setter
                // can safely replace this record without delivering it twice.
                let c = c.clone();
                let event = c.event(action, self.world.controlled());
                self.deliver(c.recipient, event, c.script);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{trace::TraceEntry, world::ObjectKind};
    use std::sync::Arc;
    use thedas_script::{
        EngineTable,
        vm::{OBJECT_INVALID, Value},
    };
    use thedas_vfs::Vfs;

    fn sim() -> (Sim, ObjectId, ObjectId) {
        let table = EngineTable::parse(
            b"void SetPlotActionSet(int n) = 810;\nvoid SetPlotActionState(int n, int s) = 796;\n",
        )
        .unwrap()
        .with_inferred();
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(table), 1);
        s.world.trace.keep = true;
        s.world.tables().insert(
            "#142",
            &["#16a3cb96"],
            vec![(1, vec![Cell::Int(7)]), (2, vec![Cell::Int(8)])],
        );
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        s.world
            .get_mut(hero)
            .unwrap()
            .properties
            .insert(7, (10.0, 10.0));
        let recipient = s.world.spawn(ObjectKind::Module, "recipient");
        s.world.get_mut(recipient).unwrap().script = Some("default_callback".into());
        s.world.plot_actions.set = 7;
        s.world.plot_actions.states.insert(1, 1);
        (s, hero, recipient)
    }

    fn call(s: &mut Sim, name: &str, args: &[Value]) {
        let f = s.table.function_by_name(name).unwrap().clone();
        let caller = s.world.hero.unwrap_or(OBJECT_INVALID);
        let mut h = crate::sim::ScriptHost {
            sim: s,
            event: None,
            script: "calling_script".into(),
        };
        assert!(
            crate::engine::call(&mut h, caller, &f, args)
                .unwrap()
                .unwrap()
                .is_none()
        );
    }

    fn start(s: &mut Sim, recipient: ObjectId, action: i32, seconds: f32, script: &str) {
        assert_eq!(s.table.function(861).unwrap().name, "SetPlotActionCooldown");
        call(
            s,
            "SetPlotActionCooldown",
            &[
                Value::Int(action),
                Value::Float(seconds),
                Value::Int(9001),
                Value::Object(recipient),
                Value::String(script.into()),
            ],
        );
    }

    fn callbacks(s: &Sim) -> usize {
        s.world
            .trace
            .entries
            .iter()
            .filter(|e| matches!(e, TraceEntry::EventDelivered { ty: 9001, .. }))
            .count()
    }

    #[test]
    fn cooldown_native_maps_its_callback_and_expires_once_with_the_action_id() {
        let (mut s, hero, recipient) = sim();
        start(&mut s, recipient, 1, 2.0, "callback");
        assert_eq!(s.plot_action_cooldown(1), Some((2.0, 1.0)));
        let c = s.world.plot_actions.cooldowns.get(&1).unwrap();
        assert_eq!(c.caller, hero);
        assert_eq!(c.recipient, recipient);
        assert_eq!(c.script.as_deref(), Some("callback"));
        let event = c.event(1, Some(hero));
        assert_eq!(event.ty, 9001);
        assert_eq!(event.creator, Some(hero));
        assert_eq!(event.target, Some(recipient));
        assert_eq!(event.ints, vec![1]);
        assert!(event.objects.is_empty());
        s.tick(1000);
        assert_eq!(s.plot_action_cooldown(1), Some((1.0, 0.5)));
        assert_eq!(callbacks(&s), 0);
        s.tick(1000);
        assert_eq!(s.plot_action_cooldown(1), Some((0.0, 0.0)));
        assert_eq!(callbacks(&s), 1);
        s.tick(1000);
        assert_eq!(callbacks(&s), 1);
    }

    #[test]
    fn cooldowns_pause_outside_the_active_set_or_state_and_ignore_button_disablement() {
        let (mut s, _, recipient) = sim();
        start(&mut s, recipient, 1, 3.0, "");
        call(&mut s, "SetPlotActionSet", &[Value::Int(8)]);
        s.tick(5000);
        assert_eq!(s.plot_action_cooldown(1), Some((3.0, 1.0)));
        call(&mut s, "SetPlotActionSet", &[Value::Int(7)]);
        call(
            &mut s,
            "SetPlotActionState",
            &[Value::Int(1), Value::Int(0)],
        );
        s.tick(5000);
        assert_eq!(s.plot_action_cooldown(1), Some((3.0, 1.0)));
        call(
            &mut s,
            "SetPlotActionState",
            &[Value::Int(1), Value::Int(1)],
        );
        s.world.plot_actions.disabled = true;
        s.tick(1000);
        assert_eq!(s.plot_action_cooldown(1), Some((2.0, 2.0 / 3.0)));
        s.tick(2000);
        assert_eq!(callbacks(&s), 1);
    }

    #[test]
    fn cooldown_replacement_can_shorten_it_and_invalid_or_negative_requests_are_inert() {
        let (mut s, _, recipient) = sim();
        start(&mut s, recipient, 1, 10.0, "");
        start(&mut s, recipient, 1, 1.0, "");
        for seconds in [-1.0, f32::NEG_INFINITY, f32::NAN] {
            start(&mut s, recipient, 1, seconds, "ignored");
        }
        start(&mut s, recipient, 999, 1.0, "ignored");
        assert!(!s.world.plot_actions.cooldowns.contains_key(&999));
        assert_eq!(s.plot_action_cooldown(1), Some((1.0, 1.0)));
        assert!(s.world.plot_actions.cooldowns[&1].script.is_none());
        s.tick(1000);
        assert_eq!(callbacks(&s), 1);
        start(&mut s, recipient, 1, 0.0, "");
        s.tick(1000);
        assert_eq!(s.plot_action_cooldown(1), Some((0.0, 1.0)));
        assert_eq!(
            callbacks(&s),
            1,
            "zero does not cross from a positive cooldown"
        );
    }

    #[test]
    fn cooldown_waits_without_a_controlled_creature_and_preserves_nonfinite_float_bits_in_json() {
        let (mut s, hero, recipient) = sim();
        start(&mut s, recipient, 1, 2.0, "");
        s.world.hero = None;
        s.tick(5000);
        assert_eq!(s.plot_action_cooldown(1), Some((2.0, 1.0)));
        s.world.hero = Some(hero);
        s.tick(1000);
        assert_eq!(s.plot_action_cooldown(1), Some((1.0, 0.5)));
        start(&mut s, recipient, 1, f32::INFINITY, "");
        s.tick(1000);
        let (remaining, percent) = s.plot_action_cooldown(1).unwrap();
        assert!(remaining.is_infinite() && percent.is_nan());
        let restored: crate::World =
            serde_json::from_slice(&serde_json::to_vec(&s.world).unwrap()).unwrap();
        assert_eq!(
            restored.plot_actions.cooldowns,
            s.world.plot_actions.cooldowns
        );
        let mut value = serde_json::to_value(&s.world).unwrap();
        value["plot_actions"]
            .as_object_mut()
            .unwrap()
            .remove("cooldowns");
        assert!(
            serde_json::from_value::<crate::World>(value)
                .unwrap()
                .plot_actions
                .cooldowns
                .is_empty()
        );
    }

    #[test]
    fn cooldown_da2_dispatch_and_clock_are_unchanged() {
        let (mut s, hero, recipient) = sim();
        start(&mut s, recipient, 1, 2.0, "");
        s.game = thedas_vfs::Game::DragonAge2;
        let f = s.table.function(861).unwrap().clone();
        let mut h = crate::sim::ScriptHost {
            sim: &mut s,
            event: None,
            script: "test".into(),
        };
        assert!(crate::engine::call(&mut h, hero, &f, &[]).is_none());
        s.tick(1000);
        assert_eq!(s.plot_action_cooldown(1), Some((2.0, 1.0)));
    }
}
