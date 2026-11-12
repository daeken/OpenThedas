//! The cutscene camera: while a cutscene plays, the view is its camera actor's, as its
//! tracks place it on the cutscene's clock (see [`thedas_sim::cuttracks`]): the executable's
//! `GameModeCutscene` hands the camera the active camera actor's position, rotation and
//! field of view every tick (0x7dd6b8). A cutscene without camera cuts (a movie's) leaves
//! the game camera alone.
//!
//! Presentation only: reads the playing cutscene's name and clock and its `.cut` file.
//!
//! Provisional: the camera looks along the actor's local +Y with +Z up and its field of
//! view is taken as vertical; the camera's roll (the rotation track's third curve turns
//! about the view axis) is dropped, as the renderer's camera has none; the original's
//! camera blend between cuts (0x6c45f0's blend time) is not followed: cuts are cuts.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Quat, Vec3};
use thedas_render::Camera;
use thedas_sim::Sim;
use thedas_sim::cuttracks::{Pose, Tracks};

/// Cutscenes' tracks read so far, by resource name.
#[derive(Default)]
pub struct CutsceneCamera {
    tracks: HashMap<String, Arc<Tracks>>,
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
        let t = sim.world.cutscene.as_ref()?.elapsed_ms as f32 / 1000.0;
        let pose = self.tracks(sim)?.camera_at(t)?;
        Some(pose_camera(&pose, base))
    }
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
}
