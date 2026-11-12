//! The cutscene camera: while a cutscene plays, the view is its camera actor's, as its
//! tracks place it on the cutscene's clock (see [`thedas_sim::cuttracks`]): the executable's
//! `GameModeCutscene` hands the camera the active camera actor's world position, rotation
//! and field of view every tick (0x7dd6b8). The actor's world pose is its tracks' placed as
//! the cutscene is (a conversation line's cutscene plays on the line's area stage), or the
//! camera of the cutscene's stage file a jump puts it on (`ActionJumpToCamera`, see
//! [`thedas_sim::cuttracks::CameraJump`]). A cutscene without camera cuts (a movie's)
//! leaves the game camera alone.
//!
//! Presentation only: reads the playing cutscene's name, clock and placement, its `.cut`
//! file and its stage file.
//!
//! Provisional: the camera looks along the actor's local +Y with +Z up and its field of
//! view is taken as vertical; the camera's roll (the rotation track's third curve turns
//! about the view axis) is dropped, as the renderer's camera has none; the original's
//! camera blend between cuts (0x6c45f0's blend time) is not followed: cuts are cuts.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Quat, Vec3};
use thedas_formats::stage::Stage;
use thedas_render::Camera;
use thedas_sim::Sim;
use thedas_sim::cuttracks::{Placement, Pose, Tracks};

/// Cutscenes' tracks and stage files read so far, by resource name.
#[derive(Default)]
pub struct CutsceneCamera {
    tracks: HashMap<String, Arc<Tracks>>,
    stages: HashMap<String, Option<Arc<Stage>>>,
}

impl CutsceneCamera {
    /// The playing cutscene's tracks, once read.
    pub fn tracks(&mut self, sim: &Sim) -> Option<Arc<Tracks>> {
        let name = &sim.world.cutscene.as_ref()?.name;
        Some(
            self.tracks
                .entry(name.clone())
                .or_insert_with(|| {
                    let t = sim
                        .vfs
                        .read(name)
                        .map(|(data, _)| Tracks::parse(&data))
                        .unwrap_or_default();
                    log::info!(
                        "cutscene {name}: {} actors, camera cuts {:?}",
                        t.actors.len(),
                        t.cuts
                            .iter()
                            .map(|(at, id)| format!(
                                "{} at {at:.2}s",
                                t.actors
                                    .iter()
                                    .find(|a| a.id == *id)
                                    .map_or("?", |a| a.name.as_str())
                            ))
                            .collect::<Vec<_>>()
                    );
                    Arc::new(t)
                })
                .clone(),
        )
    }

    /// The view of the cutscene `sim` is playing, once one of its cuts has made a camera;
    /// `base` supplies the clip planes.
    pub fn camera(&mut self, sim: &Sim, base: &Camera) -> Option<Camera> {
        let c = sim.world.cutscene.as_ref()?;
        let t = c.elapsed_ms as f32 / 1000.0;
        let placement = c.placement.unwrap_or(Placement::NONE);
        let tracks = self.tracks(sim)?;
        let stage = self.stage(sim, tracks.stage_file.as_deref());
        let lookup = |name: &str, by_place: bool| stage_camera(stage.as_deref()?, name, by_place);
        let pose = tracks.camera_at(t, placement, &lookup)?;
        Some(pose_camera(&pose, base))
    }

    fn stage(&mut self, sim: &Sim, file: Option<&str>) -> Option<Arc<Stage>> {
        let file = file?.to_ascii_lowercase();
        let file = if file.ends_with(".stg") {
            file
        } else {
            format!("{file}.stg")
        };
        self.stages
            .entry(file.clone())
            .or_insert_with(|| {
                let (data, _) = sim.vfs.read(&file).ok()?;
                Stage::parse(&data).ok().map(Arc::new)
            })
            .clone()
    }
}

/// A camera of `stage` in the stage's space, as a jump finds it (see
/// [`Stage::jump_camera`]).
pub fn stage_camera(stage: &Stage, name: &str, by_place: bool) -> Option<Pose> {
    let c = stage.jump_camera(name, by_place)?;
    Some(Pose {
        position: c.position,
        rotation: c.rotation,
        fov: c.fov,
    })
}

/// A camera actor's pose as the renderer's camera.
pub fn pose_camera(pose: &Pose, base: &Camera) -> Camera {
    let [x, y, z, w] = pose.rotation;
    let forward = (Quat::from_xyzw(x, y, z, w).normalize() * Vec3::Y).normalize_or(Vec3::Y);
    Camera {
        position: Vec3::from_array(pose.position),
        yaw: forward.y.atan2(forward.x),
        pitch: forward.z.clamp(-1.0, 1.0).asin(),
        fov_y: pose.fov.clamp(1.0, 170.0).to_radians(),
        ..*base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_along_the_actors_y() {
        // A quarter turn about Z: local +Y is world -X.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let pose = Pose {
            position: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, h, h],
            fov: 40.0,
        };
        let base = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let c = pose_camera(&pose, &base);
        assert_eq!(c.position, Vec3::new(1.0, 2.0, 3.0));
        assert!(
            (c.forward() - Vec3::NEG_X).length() < 1e-5,
            "{:?}",
            c.forward()
        );
        assert!((c.fov_y - 40f32.to_radians()).abs() < 1e-6);
        assert_eq!(c.far, base.far);
    }

    #[test]
    fn finds_stage_cameras_by_exact_name_or_by_place() {
        use thedas_formats::stage::{Place, StageCamera};
        let stage = Stage {
            places: vec![Place {
                name: "pc".into(),
                position: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                default_camera: Some("wide".into()),
            }],
            cameras: vec![StageCamera {
                name: "wide".into(),
                position: [1.0, 2.0, 10.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                fov: 45.0,
            }],
        };
        let p = stage_camera(&stage, "wide", false).unwrap();
        assert_eq!(p.position, [1.0, 2.0, 10.0]);
        assert_eq!(p.fov, 45.0);
        assert_eq!(stage_camera(&stage, "pc", true), Some(p));
        assert_eq!(stage_camera(&stage, "pc", false), None);
        assert_eq!(stage_camera(&stage, "WIDE", false), None);
    }
}
