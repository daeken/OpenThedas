//! Origins' script-requested target selection (native 860).

use serde::{Deserialize, Serialize};
use thedas_script::vm::{OBJECT_INVALID, ObjectId};

use crate::sim::Sim;
use crate::world::{Event, ObjectKind};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TargetRequest {
    pub mask: i32,
    pub radius: f32,
    pub event_type: i32,
    pub caller: ObjectId,
    pub recipient: ObjectId,
    pub script: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use thedas_script::{EngineTable, vm::Value};
    use thedas_vfs::Vfs;

    fn sim() -> (Sim, ObjectId, ObjectId, ObjectId) {
        let mut s = Sim::new(
            Arc::new(Vfs::new()),
            Arc::new(EngineTable::default().with_inferred()),
            1,
        );
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        s.world
            .get_mut(hero)
            .unwrap()
            .properties
            .insert(7, (10.0, 10.0));
        let caller = s.world.spawn(ObjectKind::Module, "caller");
        let recipient = s.world.spawn(ObjectKind::Placeable, "recipient");
        s.world.get_mut(recipient).unwrap().script = Some("default_callback".into());
        (s, hero, caller, recipient)
    }

    fn request(
        s: &mut Sim,
        caller: ObjectId,
        recipient: ObjectId,
        mask: i32,
        radius: f32,
        unused: f32,
        script: &str,
    ) {
        let f = s.table.function(860).unwrap().clone();
        assert_eq!(f.name, "RequestTarget");
        let mut h = crate::sim::ScriptHost {
            sim: s,
            event: None,
            script: "calling_script".into(),
        };
        assert!(
            crate::engine::call(
                &mut h,
                caller,
                &f,
                &[
                    Value::Int(mask),
                    Value::Float(radius),
                    Value::Float(unused),
                    Value::Int(1234),
                    Value::Object(recipient),
                    Value::String(script.into()),
                ]
            )
            .unwrap()
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn declaration_and_dispatch_retain_native_fields_and_ignore_the_second_float() {
        let (mut s, _, caller, recipient) = sim();
        request(&mut s, caller, recipient, 0x20, 4.5, 12.0, "callback");
        let first = s.world.target_request.clone().unwrap();
        assert_eq!(
            first,
            TargetRequest {
                mask: 0x20,
                radius: 4.5,
                event_type: 1234,
                caller,
                recipient,
                script: Some("callback".into())
            }
        );
        request(&mut s, caller, recipient, 0x20, 4.5, 99.0, "callback");
        assert_eq!(s.world.target_request, Some(first));
        request(&mut s, caller, recipient, 8, 0.0, 0.0, "");
        assert_eq!(s.world.target_request.as_ref().unwrap().script, None);
        s.cancel_target_request();
        assert!(s.world.target_request.is_none());
    }

    #[test]
    fn selection_callback_uses_current_controlled_creator_object_and_native_position() {
        let (mut s, hero, caller, recipient) = sim();
        let follower = s.world.spawn(ObjectKind::Creature, "follower");
        s.world.get_mut(hero).unwrap().followers.push(follower);
        let selected = s.world.spawn(ObjectKind::Placeable, "selected");
        s.world.get_mut(selected).unwrap().position = [7.0, 8.0, 9.0];
        request(&mut s, caller, recipient, 8, 0.0, 0.0, "callback");
        s.set_controlled(Some(follower));
        let (request, event) = s.requested_target_event(selected, [1.0; 3]).unwrap();
        assert_eq!(request.caller, caller);
        assert_eq!(event.ty, 1234);
        assert_eq!(event.creator, Some(follower));
        assert_eq!(event.target, Some(recipient));
        assert_eq!(event.objects, vec![selected]);
        assert_eq!(event.vectors, vec![[7.0, 8.0, 9.0]]);
        assert!(event.ints.is_empty() && event.locations.is_empty());
        assert!(s.respond_to_target_request(selected, [1.0; 3]));
        assert!(s.world.target_request.is_none());
        assert!(s.world.queue.is_empty(), "callback is synchronous");
    }

    #[test]
    fn clicks_select_valid_ground_or_objects_and_do_not_queue_orders_while_waiting() {
        let (mut s, hero, caller, recipient) = sim();
        request(&mut s, caller, recipient, 8, 0.0, 0.0, "");
        s.order_move(hero, [1.0, 2.0, 3.0]);
        assert!(
            s.world.target_request.is_some(),
            "invalid ground click leaves targeting active"
        );
        assert!(s.world.get(hero).unwrap().commands.is_empty());
        s.order_use(hero, recipient);
        assert!(s.world.target_request.is_none());
        assert!(s.world.get(hero).unwrap().commands.is_empty());
        request(&mut s, caller, recipient, 0x20, 3.0, 0.0, "");
        let (_, event) = s
            .requested_target_event(OBJECT_INVALID, [2.0, 4.0, 6.0])
            .unwrap();
        assert_eq!(event.objects, vec![OBJECT_INVALID]);
        assert_eq!(event.vectors, vec![[2.0, 4.0, 6.0]]);
        s.order_move(hero, [2.0, 4.0, 6.0]);
        assert!(s.world.target_request.is_none());
        assert!(s.world.get(hero).unwrap().commands.is_empty());
    }

    #[test]
    fn target_masks_separate_self_dead_friendly_hostile_party_placeables_and_items() {
        let (mut s, hero, _, recipient) = sim();
        let item = s.world.spawn(ObjectKind::Item, "item");
        let other = s.world.spawn(ObjectKind::Creature, "other");
        s.world
            .get_mut(other)
            .unwrap()
            .properties
            .insert(7, (10.0, 10.0));
        s.world
            .get_mut(other)
            .unwrap()
            .fields
            .insert("IsHostile".into(), 0);
        for (id, mask) in [
            (hero, 1),
            (other, 2),
            (recipient, 8),
            (item, 0x10),
            (OBJECT_INVALID, 0x20),
        ] {
            assert!(s.requested_target_valid(id, mask));
            assert!(!s.requested_target_valid(id, 0));
        }
        s.world
            .get_mut(other)
            .unwrap()
            .fields
            .insert("IsHostile".into(), 1);
        assert!(!s.requested_target_valid(other, 2));
        assert!(s.requested_target_valid(other, 4));
        s.world.get_mut(hero).unwrap().followers.push(other);
        assert!(!s.requested_target_valid(other, 4));
        assert!(s.requested_target_valid(other, 0x80));
        s.world
            .get_mut(other)
            .unwrap()
            .properties
            .insert(7, (10.0, 0.9));
        assert!(!s.requested_target_valid(other, 0x80));
        assert!(s.requested_target_valid(other, 0x40));
        assert!(!s.requested_target_valid(123456789, 0xff));
    }

    #[test]
    fn pending_requests_round_trip_and_da2_dispatch_remains_unimplemented() {
        let (mut s, hero, caller, recipient) = sim();
        request(&mut s, caller, recipient, 0x20, 1.5, 0.0, "callback");
        let restored: crate::World =
            serde_json::from_slice(&serde_json::to_vec(&s.world).unwrap()).unwrap();
        assert_eq!(s.world.target_request, restored.target_request);
        let mut value = serde_json::to_value(&s.world).unwrap();
        value.as_object_mut().unwrap().remove("target_request");
        assert!(
            serde_json::from_value::<crate::World>(value)
                .unwrap()
                .target_request
                .is_none()
        );
        s.cancel_target_request();
        s.game = thedas_vfs::Game::DragonAge2;
        let f = s.table.function(860).unwrap().clone();
        let mut h = crate::sim::ScriptHost {
            sim: &mut s,
            event: None,
            script: "test".into(),
        };
        assert!(crate::engine::call(&mut h, hero, &f, &[]).is_none());
        assert!(s.world.target_request.is_none());
    }
}

impl Sim {
    /// Replaces the previous request without issuing its callback. The native's
    /// second float is unused; radius selects circular targeting when nonzero.
    pub(crate) fn request_target(&mut self, request: TargetRequest) {
        self.world.target_request = Some(request);
    }

    pub fn cancel_target_request(&mut self) {
        self.world.target_request = None;
    }

    fn requested_target_valid(&self, selected: ObjectId, mask: i32) -> bool {
        if selected == OBJECT_INVALID {
            return mask & 0x20 != 0;
        }
        let Some(o) = self.world.get(selected) else {
            return false;
        };
        match o.kind {
            ObjectKind::Creature => {
                if self.world.controlled() == Some(selected) {
                    return mask & 1 != 0;
                }
                let no_health = !o
                    .properties
                    .get(&7)
                    .is_some_and(|&(_, h)| (1.0..2147483648.0).contains(&h));
                if no_health {
                    return mask & 0x40 != 0;
                }
                // The executable reads its cached IsHostile attribute. Reuse
                // our relationship calculation when no explicit attribute exists.
                let hostile = o.fields.get("IsHostile").map_or_else(
                    || {
                        self.world
                            .controlled()
                            .is_some_and(|who| self.hostile(who, selected))
                    },
                    |&h| h != 0,
                );
                if !hostile {
                    mask & 2 != 0
                } else if crate::engine::party_of(self).contains(&selected) {
                    mask & 0x80 != 0
                } else {
                    mask & 4 != 0
                }
            }
            ObjectKind::Placeable => mask & 8 != 0,
            ObjectKind::Item => mask & 0x10 != 0,
            ObjectKind::Area
            | ObjectKind::Module
            | ObjectKind::Party
            | ObjectKind::Stage
            | ObjectKind::Map
            | ObjectKind::MapPin => false,
            _ => true,
        }
    }

    fn requested_target_event(
        &self,
        selected: ObjectId,
        position: [f32; 3],
    ) -> Option<(TargetRequest, Event)> {
        let request = self.world.target_request.as_ref()?;
        if !self.requested_target_valid(selected, request.mask) {
            return None;
        }
        let position = self.world.get(selected).map_or(position, |o| o.position);
        Some((
            request.clone(),
            Event {
                ty: request.event_type,
                creator: self.world.controlled(),
                target: Some(request.recipient),
                objects: vec![selected],
                vectors: vec![position],
                ..Default::default()
            },
        ))
    }

    /// Completes script targeting with an object, or OBJECT_INVALID and a ground
    /// point. Invalid choices leave the request active. Delivery is synchronous;
    /// the native clears targeting after the callback, including a replacement
    /// request made inside that callback.
    pub fn respond_to_target_request(&mut self, selected: ObjectId, position: [f32; 3]) -> bool {
        let Some((request, event)) = self.requested_target_event(selected, position) else {
            return false;
        };
        self.deliver(request.recipient, event, request.script);
        self.world.target_request = None;
        true
    }

    /// A click while targeting selects a target instead of issuing an order.
    pub(crate) fn target_request_click(
        &mut self,
        who: ObjectId,
        selected: ObjectId,
        position: [f32; 3],
    ) -> bool {
        if self.game != thedas_vfs::Game::Origins
            || self.world.target_request.is_none()
            || self.world.controlled() != Some(who)
        {
            return false;
        }
        self.respond_to_target_request(selected, position);
        true
    }
}
