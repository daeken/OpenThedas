//! The conversation camera: while a conversation line with staged cinematics is up, the
//! view is that line's stage camera (see [`thedas_formats::stage`]), placed where the area
//! puts the line's stage. A line without a shot gets the executable's default framing of its
//! speaker (see [`default_framing`]). The line's actors are drawn by its own cutscene (see
//! [`crate::cutactors`]), and the simulation leaves them on their marks when the line ends
//! (see [`thedas_sim::cutactors`]).
//!
//! Presentation only: reads the simulation and the dialog and stage resources.
//!
//! Once a line with its own cinematics has run its length (the cut's, 5000) and the player
//! has replies to choose, the view moves to the stage's `decision` camera.
//!
//! Provisional: the camera's field of view is taken as
//! vertical; the original's camera moves between and within
//! lines and lines staged at the speaker's
//! current location (`StageAtCurrentLocation`) are not followed.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Quat, Vec3};
use thedas_formats::stage::{LineShot, Stage, StageCamera};
use thedas_render::Camera;
use thedas_script::vm::ObjectId;
use thedas_sim::Sim;
use thedas_sim::dialog::OpenLine;
use thedas_sim::world::{Object, ObjectKind};

/// Where a staged line puts its camera and actors.
#[derive(Clone, Debug, PartialEq)]
pub struct Staged {
    pub camera: Camera,
    /// The stage's `decision` camera, which frames the player choosing a reply.
    pub decision: Option<Camera>,
    /// How long the line's cinematics run (milliseconds), when it has its own.
    pub line_ms: Option<f64>,
}

/// Which conversation line: the conversation (dialog name, owner) and the node shown.
type LineKey = ((String, ObjectId), Option<usize>);

/// Line shots and stages read so far, and the staging in use.
#[derive(Default)]
pub struct TalkCamera {
    shots: HashMap<String, Option<Arc<Vec<Option<LineShot>>>>>,
    stages: HashMap<String, Option<Arc<Stage>>>,
    /// The line whose staging is held (the last staged line) and that staging.
    held: Option<(LineKey, Staged)>,
    /// The presentation clock (milliseconds; it runs while the world is stopped too).
    clock_ms: f64,
    /// The line shown and when it came up on the clock.
    shown: Option<(LineKey, f64)>,
}

impl TalkCamera {
    /// The staging of the conversation `sim` has open, if one of its lines has given a shot;
    /// `base` supplies the camera's clip planes.
    pub fn staging(&mut self, sim: &Sim, base: &Camera) -> Option<&Staged> {
        let Some(open) = sim.conversation_line() else {
            self.held = None;
            self.shown = None;
            return None;
        };
        let conversation = (open.dialog.to_ascii_lowercase(), open.owner);
        if self
            .held
            .as_ref()
            .is_some_and(|((c, _), _)| *c != conversation)
        {
            self.held = None;
        }
        let line = (conversation, open.line);
        if self.shown.as_ref().is_none_or(|(l, _)| *l != line) {
            self.shown = Some((line.clone(), self.clock_ms));
        }
        let shot = open.line.and_then(|l| {
            let shots = self.shots(sim, &line.0.0)?;
            shots.get(l).cloned().flatten()
        });
        if shot.is_none() {
            log::debug!("line {:?} of {}: no shot", open.line, line.0.0);
        }
        if let Some(shot) = shot
            && let Some(staged) = self.stage_line(sim, &shot, &open, base)
        {
            self.held = Some((line, staged));
        }
        self.held.as_ref().map(|(_, s)| s)
    }

    /// Advances the presentation clock.
    pub fn advance(&mut self, dt_ms: f64) {
        self.clock_ms += dt_ms.max(0.0);
    }

    /// The camera for the open conversation: the line's shot, or the stage's decision camera
    /// once a line with its own cinematics has run its length and the player has replies
    /// to choose. Provisional: the original's line length is its voice-over's; the cut's
    /// length (5000) is taken instead, and lines without one keep their shot.
    ///
    /// A line without a shot of its own is framed as the executable frames a line that
    /// plays no cinematics (see [`default_framing`]); `face` gives a creature's eyes as
    /// drawn, offset from where it stands.
    pub fn camera(
        &mut self,
        sim: &Sim,
        base: &Camera,
        face: &dyn Fn(ObjectId) -> Option<Vec3>,
    ) -> Option<Camera> {
        let open = sim.conversation_line()?;
        let choosing = open.choosing;
        let line: LineKey = ((open.dialog.to_ascii_lowercase(), open.owner), open.line);
        let now = self.clock_ms;
        let staging = self.staging(sim, base).cloned();
        let shown = self.shown.clone();
        let held_here = self.held.as_ref().is_some_and(|(held, _)| *held == line);
        if !held_here && line.1.is_some() {
            return self.unstaged(sim, base, face);
        }
        let staged = staging?;
        let line_done = match (&self.held, shown) {
            (Some((held, _)), Some((line, since))) if *held == line => {
                staged.line_ms.is_some_and(|ms| now - since >= ms)
            }
            _ => false,
        };
        Some(match staged.decision {
            Some(d) if choosing && line_done => d,
            _ => staged.camera,
        })
    }

    /// The default framing of the open conversation's current line (its speaker, seen from
    /// its listener's side).
    fn unstaged(
        &mut self,
        sim: &Sim,
        base: &Camera,
        face: &dyn Fn(ObjectId) -> Option<Vec3>,
    ) -> Option<Camera> {
        let open = sim.conversation_line()?;
        let said = sim.dialog_state()?.speaker;
        let said = said.trim_end_matches('\0');
        let Some(speaker) = (if said.is_empty() {
            Some(open.owner)
        } else {
            actor_object(sim, said, &open)
        }) else {
            log::debug!("unstaged line: no speaker {said:?}");
            return None;
        };
        // Provisional: the listener is the party's leader (0x68d270 asks for the player's
        // creature), or the conversation's owner when the leader speaks.
        let leader = sim.world.controlled().unwrap_or(open.player);
        let listener = if speaker == leader {
            open.owner
        } else {
            leader
        };
        let area = sim.world.area;
        let here = |id: ObjectId| {
            sim.world
                .get(id)
                .filter(|o| o.area.is_some() && o.area == area)
        };
        let Some(s) = here(speaker) else {
            log::debug!("unstaged line: speaker {said:?} is not in the area");
            return None;
        };
        let listener = here(listener)
            .filter(|o| o.id != speaker)
            .map(|o| Vec3::from_array(o.position));
        log::debug!(
            "unstaged line: speaker {} ({said}) at {:?} facing {}, eyes {:?}, listener at {:?}",
            s.tag,
            s.position,
            s.facing,
            face(speaker),
            listener
        );
        Some(default_framing(
            Vec3::from_array(s.position),
            s.facing,
            face(speaker),
            listener,
            base,
        ))
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

    fn stage_line(
        &mut self,
        sim: &Sim,
        shot: &LineShot,
        open: &OpenLine,
        base: &Camera,
    ) -> Option<Staged> {
        let Some(stage) = self.stage(sim, &shot.stage_file) else {
            log::debug!("staged line: no stage file {}", shot.stage_file);
            return None;
        };
        let Some(cam) = stage.camera_for(&shot.place) else {
            log::debug!(
                "staged line: stage {} has no camera for {}",
                shot.stage_file,
                shot.place
            );
            return None;
        };
        // The stage nearest the player's creature, on the ground plane (0x7d2aa0 ->
        // 0x67cdd0), as the line's cutscene is placed.
        let near = sim
            .world
            .get(open.player)
            .map(|o| Vec3::new(o.position[0], o.position[1], 0.0));
        let area = sim.world.area;
        let placed = sim
            .world
            .by_tag(&shot.stage)
            .filter(|o| o.kind == ObjectKind::Stage && o.area == area)
            .min_by(|a, b| {
                let d = |o: &&Object| {
                    near.map_or(0.0, |n| {
                        n.distance(Vec3::new(o.position[0], o.position[1], 0.0))
                    })
                };
                d(a).total_cmp(&d(b))
            });
        let Some(placed) = placed else {
            log::debug!(
                "staged line: no stage tagged {} in the area ({} placed elsewhere)",
                shot.stage,
                sim.world.by_tag(&shot.stage).count()
            );
            return None;
        };
        log::debug!(
            "staged line: stage {} ({}) place {} at {:?} facing {} (owner at {:?}), camera {:?} {:?}",
            shot.stage,
            shot.stage_file,
            shot.place,
            placed.position,
            placed.facing,
            near,
            cam.position,
            cam.rotation
        );
        Some(Staged {
            camera: place_camera(placed.position, placed.facing, cam, base),
            decision: stage
                .camera("decision")
                .map(|d| place_camera(placed.position, placed.facing, d, base)),
            line_ms: shot.seconds.map(|s| s as f64 * 1000.0),
        })
    }
}

/// How far from the speaker's eyes the default framing's camera stands (metres;
/// 0x68d4b0's 0.9).
const FRAMING_DISTANCE: f32 = 0.9;
/// How far the camera stands off the line from the speaker to the listener (degrees,
/// counter-clockwise; 0x68d4b0's cos/sin of 15°).
const FRAMING_TURN: f32 = 15.0;
/// How far below the eyes the camera and its aim are (metres; 0x68d4b0's 0.06).
const FRAMING_DROP: f32 = 0.06;
/// The default framing's field of view (degrees): 0x68d4b0 sets 40 divided by a value
/// the renderer gives (untraced), taken as 1.
const FRAMING_FOV: f32 = 40.0;

/// The executable's framing of a conversation line that plays no cinematics
/// (`GameModeDialog` 0x68d4b0, called as each such line comes up, 0x68e0d1 and 0x68e700):
/// the speaker standing at `at` (facing `facing` degrees, as objects face), with eyes at
/// `eyes` from there (the `eye_Left` bone in the original; (0, 0, 1) without a model),
/// seen from 0.9 m in front of its eyes towards the listener at `listener` (along its
/// facing without one), turned 15° counter-clockwise off that line and 6 cm below the
/// eyes, looking back at them (0x6c45f0 places the camera, 0x6c4a00 aims it at the
/// speaker with an offset), with a field of view of 40°. The camera cuts there (blend
/// time 0).
///
/// Provisional: the eyes are the drawn eye meshes' centre rather than the left eye bone;
/// the field of view's divisor is taken as 1 and the field of view as vertical; the
/// listener (see [`TalkCamera::camera`]).
pub fn default_framing(
    at: Vec3,
    facing: f32,
    eyes: Option<Vec3>,
    listener: Option<Vec3>,
    base: &Camera,
) -> Camera {
    let heading = thedas_sim::world::heading_of(facing).to_radians();
    let forward = Vec3::new(heading.cos(), heading.sin(), 0.0);
    let eyes = eyes.unwrap_or(Vec3::Z);
    // Towards the listener on the ground plane, from the speaker's eyes (0x451a30
    // normalises; a vanishing direction is +X).
    let toward = match listener {
        Some(l) => {
            let d = Vec3::new(l.x - at.x - eyes.x, l.y - at.y - eyes.y, 0.0);
            if d.length_squared() > 1e-12 {
                d.normalize()
            } else {
                Vec3::X
            }
        }
        None => forward,
    } * FRAMING_DISTANCE;
    let (s, c) = FRAMING_TURN.to_radians().sin_cos();
    let offset = Vec3::new(
        c * toward.x - s * toward.y,
        s * toward.x + c * toward.y,
        toward.z,
    );
    let look = at + eyes - Vec3::Z * FRAMING_DROP;
    let eye = look + offset;
    // The aim: 0x68d4b0's tiny shift along and across the offset (0.00061 and 0.0349).
    let aim = look
        + Vec3::new(
            0.000_609_159_5 * offset.x + 0.034_899_496 * offset.y,
            0.000_609_159_5 * offset.y - 0.034_899_496 * offset.x,
            0.0,
        );
    let dir = (aim - eye).normalize_or(-offset.normalize_or(Vec3::X));
    Camera {
        position: eye,
        yaw: dir.y.atan2(dir.x),
        pitch: dir.z.clamp(-1.0, 1.0).asin(),
        fov_y: FRAMING_FOV.to_radians(),
        ..*base
    }
}

/// The creature a cutscene actor names: `OWNER` (the conversation's owner), `PLAYER`
/// (its player character), `HENCH:<tag>` or a tag (a creature in the current area).
fn actor_object(sim: &Sim, actor: &str, open: &OpenLine) -> Option<ObjectId> {
    if actor.eq_ignore_ascii_case("OWNER") {
        return Some(open.owner);
    }
    if actor.eq_ignore_ascii_case("PLAYER") {
        return Some(open.player);
    }
    let tag = match actor.split_once(':') {
        Some((kind, tag)) if kind.eq_ignore_ascii_case("HENCH") => tag,
        Some(_) => return None,
        None => actor,
    };
    let area = sim.world.area;
    sim.world
        .by_tag(tag)
        .find(|o| o.kind == ObjectKind::Creature && o.area == area)
        .map(|o| o.id)
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

    #[test]
    fn frames_an_unstaged_speaker_from_the_listener_side() {
        let base = Camera::new(Vec3::ZERO, 0.0, 0.0);
        // The speaker at the origin, eyes 1.6 m up; the listener 3 m along +X.
        let c = default_framing(
            Vec3::ZERO,
            0.0,
            Some(Vec3::new(0.0, 0.0, 1.6)),
            Some(Vec3::new(3.0, 0.0, 0.0)),
            &base,
        );
        let t = 15f32.to_radians();
        let want = Vec3::new(0.9 * t.cos(), 0.9 * t.sin(), 1.6 - 0.06);
        assert!((c.position - want).length() < 1e-4, "{:?}", c.position);
        // It looks back at the eyes.
        let back = (Vec3::new(0.0, 0.0, 1.54) - c.position).normalize();
        assert!(c.forward().dot(back) > 0.999, "{:?}", c.forward());
        assert!((c.fov_y - 40f32.to_radians()).abs() < 1e-6);
        // Without a listener, along the speaker's facing (facing 0 looks along +Y).
        let c = default_framing(Vec3::ZERO, 0.0, None, None, &base);
        let want = Vec3::new(-0.9 * t.sin(), 0.9 * t.cos(), 1.0 - 0.06);
        assert!((c.position - want).length() < 1e-4, "{:?}", c.position);
    }
}
