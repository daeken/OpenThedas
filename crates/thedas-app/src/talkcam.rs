//! The conversation camera: while a conversation line with staged cinematics is up, the view
//! is that line's stage camera (see [`thedas_formats::stage`]), placed where the area puts
//! the line's stage. Lines without one (replies, unstaged lines) keep the last shot; a
//! conversation without any leaves the game camera alone.
//!
//! Presentation only: reads the simulation and the dialog and stage resources.
//!
//! Provisional: the stage is the area stage with the line's tag nearest the conversation's
//! owner (a tag can be placed more than once); the camera's field of view is taken as
//! vertical; the original's camera moves between and within lines, the decision camera
//! while the player chooses and lines staged at the speaker's current location
//! (`StageAtCurrentLocation`) are not followed.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Quat, Vec3};
use thedas_formats::stage::{LineShot, Stage, StageCamera};
use thedas_render::Camera;
use thedas_sim::Sim;
use thedas_sim::world::ObjectKind;

/// Line shots and stages read so far, and the shot in use.
#[derive(Default)]
pub struct TalkCamera {
    shots: HashMap<String, Option<Arc<Vec<Option<LineShot>>>>>,
    stages: HashMap<String, Option<Arc<Stage>>>,
    /// The conversation (dialog name, owner) and the camera its last staged line took.
    held: Option<((String, thedas_script::vm::ObjectId), Camera)>,
}

impl TalkCamera {
    /// The camera for the conversation `sim` has open, if one of its lines has given a shot;
    /// `base` supplies the clip planes.
    pub fn camera(&mut self, sim: &Sim, base: &Camera) -> Option<Camera> {
        let Some((dlg, line, owner)) = sim.conversation_line() else {
            self.held = None;
            return None;
        };
        let key = (dlg.to_ascii_lowercase(), owner);
        if self.held.as_ref().is_some_and(|(k, _)| *k != key) {
            self.held = None;
        }
        let shot = line.and_then(|l| {
            let shots = self.shots(sim, &key.0)?;
            shots.get(l).cloned().flatten()
        });
        if let Some(shot) = shot
            && let Some(cam) = self.shot_camera(sim, &shot, owner, base)
        {
            self.held = Some((key, cam));
        }
        self.held.as_ref().map(|(_, c)| *c)
    }

    fn shots(&mut self, sim: &Sim, dlg: &str) -> Option<Arc<Vec<Option<LineShot>>>> {
        self.shots
            .entry(dlg.to_string())
            .or_insert_with(|| {
                let (data, _) = sim.vfs.read(&format!("{dlg}.dlg")).ok()?;
                let g = thedas_formats::Gff4::parse(&data).ok()?;
                thedas_formats::stage::line_shots(&g).ok().map(Arc::new)
            })
            .clone()
    }

    fn stage(&mut self, sim: &Sim, file: &str) -> Option<Arc<Stage>> {
        self.stages
            .entry(file.to_string())
            .or_insert_with(|| {
                let (data, _) = sim.vfs.read(&format!("{file}.stg")).ok()?;
                Stage::parse(&data).ok().map(Arc::new)
            })
            .clone()
    }

    fn shot_camera(
        &mut self,
        sim: &Sim,
        shot: &LineShot,
        owner: thedas_script::vm::ObjectId,
        base: &Camera,
    ) -> Option<Camera> {
        let stage = self.stage(sim, &shot.stage_file)?;
        let cam = stage.camera_for(&shot.place)?;
        let near = sim.world.get(owner).map(|o| Vec3::from_array(o.position));
        let area = sim.world.area;
        let placed = sim
            .world
            .by_tag(&shot.stage)
            .filter(|o| o.kind == ObjectKind::Stage && o.area == area)
            .min_by(|a, b| {
                let d = |o: &&thedas_sim::world::Object| {
                    near.map_or(0.0, |n| n.distance(Vec3::from_array(o.position)))
                };
                d(a).total_cmp(&d(b))
            })?;
        Some(place_camera(placed.position, placed.facing, cam, base))
    }
}

/// Stage camera `cam` on a stage the area places at `position`, turned `facing` degrees
/// about Z (as objects are placed: [`thedas_render::Prop::placement`]).
pub fn place_camera(position: [f32; 3], facing: f32, cam: &StageCamera, base: &Camera) -> Camera {
    let stage = thedas_render::Prop::placement(position, facing);
    let eye = stage.transform_point3(Vec3::from_array(cam.position));
    let [x, y, z, w] = cam.rotation;
    let local = Quat::from_xyzw(x, y, z, w).normalize();
    // A stage camera looks along its local +Y.
    let forward = stage
        .transform_vector3(local * Vec3::Y)
        .normalize_or(Vec3::Y);
    Camera {
        position: eye,
        yaw: forward.y.atan2(forward.x),
        pitch: forward.z.clamp(-1.0, 1.0).asin(),
        fov_y: cam.fov.to_radians(),
        ..*base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_the_stage_camera_in_the_world() {
        // Half a turn about Z: the camera looks along -Y in the stage.
        let cam = StageCamera {
            name: "c".into(),
            position: [0.0, 1.0, 1.5],
            rotation: [0.0, 0.0, 1.0, 0.0],
            fov: 30.0,
        };
        let base = Camera::new(Vec3::ZERO, 0.0, 0.0);
        // The stage stands at (10, 20, 3), turned a quarter turn: stage +Y is world -X.
        let c = place_camera([10.0, 20.0, 3.0], 90.0, &cam, &base);
        assert!(
            (c.position - Vec3::new(9.0, 20.0, 4.5)).length() < 1e-4,
            "{:?}",
            c.position
        );
        assert!((c.forward() - Vec3::X).length() < 1e-4, "{:?}", c.forward());
        assert!((c.fov_y - 30f32.to_radians()).abs() < 1e-6);
        assert_eq!(c.far, base.far);
    }
}
