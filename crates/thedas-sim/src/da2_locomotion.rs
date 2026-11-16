//! DA2's persistent locomotion mode (IsMoving), apart from command ownership.
//!
//! The headless controller has no native controller +0x54/+0xd0 completion
//! blockers. This implements the traced unblocked stop/turn path; it does not
//! infer locomotion from task-37 animation displacement.

use serde::{Deserialize, Serialize};
use thedas_script::vm::ObjectId;

use crate::{Sim, world::ObjectKind};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Locomotion {
    /// Native +0xf0: 0 idle, 1 walk, 2 run, 3 stopping, 4 turning.
    mode: u8,
    desired_facing: Option<f32>,
    updated_ms: u64,
}

impl Locomotion {
    pub fn is_moving(&self) -> bool {
        self.mode != 0
    }

    fn stop(&mut self, now: u64) {
        self.mode = match self.mode {
            1 | 2 => 3,
            4 => 0,
            other => other,
        };
        self.updated_ms = now;
    }

    fn needs_turn(&self, facing: f32) -> bool {
        self.desired_facing
            .is_some_and(|want| ((want - facing + 180.).rem_euclid(360.) - 180.).abs() >= 1. - 1e-6)
    }

    fn advance(&mut self, facing: &mut f32, now: u64) {
        if now <= self.updated_ms {
            return;
        }
        let dt = (now - self.updated_ms) as f32 / 1000.;
        self.updated_ms = now;
        if !matches!(self.mode, 3 | 4) {
            return;
        }
        if let Some(want) = self.desired_facing {
            *facing = crate::sim::turn_toward(*facing, want, dt);
        }
        // 0x8c5c0b: with no controller blocker, stop resets the mode and
        // re-applies the retained direction. That may enter a stationary turn.
        if self.mode == 3 {
            self.mode = if self.needs_turn(*facing) { 4 } else { 0 };
        } else if !self.needs_turn(*facing) {
            self.mode = 0;
        }
    }
}

impl Sim {
    pub(crate) fn da2_stop_locomotion(&mut self, id: ObjectId) {
        if self.game != thedas_vfs::Game::DragonAge2 {
            return;
        }
        let now = self.world.time_ms;
        if let Some(state) = self
            .world
            .get_mut(id)
            .and_then(|o| o.da2_locomotion.as_mut())
        {
            state.stop(now);
        }
    }

    pub(crate) fn da2_reset_locomotion(&mut self, id: ObjectId) {
        if self.game == thedas_vfs::Game::DragonAge2
            && let Some(o) = self.world.get_mut(id)
        {
            o.da2_locomotion = None;
        }
    }

    pub(crate) fn da2_begin_locomotion(&mut self, id: ObjectId, run: bool) {
        if self.game != thedas_vfs::Game::DragonAge2 {
            return;
        }
        let now = self.world.time_ms;
        if let Some(o) = self
            .world
            .get_mut(id)
            .filter(|o| o.kind == ObjectKind::Creature)
        {
            let state = o.da2_locomotion.get_or_insert_with(Locomotion::default);
            state.mode = if run { 2 } else { 1 };
            state.updated_ms = now;
        }
    }

    pub(crate) fn da2_locomotion_direction(&mut self, id: ObjectId, facing: f32) {
        let now = self.world.time_ms;
        if self.game == thedas_vfs::Game::DragonAge2
            && let Some(state) = self
                .world
                .get_mut(id)
                .and_then(|o| o.da2_locomotion.as_mut())
        {
            state.desired_facing = Some(facing);
            state.updated_ms = now;
        }
    }

    pub(crate) fn da2_turn_to(&mut self, id: ObjectId, facing: f32) {
        let now = self.world.time_ms;
        if let Some(o) = self
            .world
            .get_mut(id)
            .filter(|o| o.kind == ObjectKind::Creature)
        {
            let state = o.da2_locomotion.get_or_insert_with(Locomotion::default);
            state.desired_facing = Some(facing);
            state.updated_ms = now;
            if state.mode == 0 && state.needs_turn(o.facing) {
                state.mode = 4;
            }
        }
    }

    pub(crate) fn advance_da2_locomotion(&mut self) {
        if self.game != thedas_vfs::Game::DragonAge2 {
            return;
        }
        let now = self.world.time_ms;
        for o in self.world.objects.values_mut() {
            if !o.active || o.parked.is_some() {
                continue;
            }
            if let Some(state) = &mut o.da2_locomotion {
                state.advance(&mut o.facing, now);
            }
        }
    }
}
