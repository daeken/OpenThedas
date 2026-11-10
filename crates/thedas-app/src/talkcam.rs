//! The conversation camera and stage marks: while a conversation line with staged
//! cinematics is up, the view is that line's stage camera (see [`thedas_formats::stage`]),
//! placed where the area puts the line's stage, and the line's actors are drawn standing at
//! their stage places. Lines without a shot (replies, unstaged lines) keep the last one; a
//! conversation without any leaves the game camera and the creatures alone.
//!
//! Presentation only: reads the simulation and the dialog and stage resources. The
//! creatures are drawn at their marks; the simulation keeps them where they were, and they
//! are drawn there again when the conversation ends.
//!
//! Once a line with its own cinematics has run its length (the cut's, 5000) and the player
//! has replies to choose, the view moves to the stage's `decision` camera.
//!
//! Provisional: the stage is the area stage with the line's tag nearest the conversation's
//! owner (a tag can be placed more than once); the camera's field of view is taken as
//! vertical; whether the original moves the creatures themselves (so that they stay on
//! their marks afterwards) is not traced; the original's camera moves between and within
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
    /// Each actor drawn at its mark: world position and facing (degrees, as objects').
    pub marks: HashMap<ObjectId, ([f32; 3], f32)>,
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
    pub fn camera(&mut self, sim: &Sim, base: &Camera) -> Option<Camera> {
        let choosing = sim.conversation_line().is_some_and(|o| o.choosing);
        let now = self.clock_ms;
        let shown = self.shown.clone();
        let staged = self.staging(sim, base)?.clone();
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
        let stage = self.stage(sim, &shot.stage_file)?;
        let cam = stage.camera_for(&shot.place)?;
        let near = sim
            .world
            .get(open.owner)
            .map(|o| Vec3::from_array(o.position));
        let area = sim.world.area;
        let placed = sim
            .world
            .by_tag(&shot.stage)
            .filter(|o| o.kind == ObjectKind::Stage && o.area == area)
            .min_by(|a, b| {
                let d =
                    |o: &&Object| near.map_or(0.0, |n| n.distance(Vec3::from_array(o.position)));
                d(a).total_cmp(&d(b))
            })?;
        let mut marks = HashMap::new();
        for (actor, place) in &shot.actors {
            let (Some(id), Some(p)) = (
                actor_object(sim, actor, open),
                stage
                    .places
                    .iter()
                    .find(|p| p.name.eq_ignore_ascii_case(place)),
            ) else {
                continue;
            };
            marks.insert(
                id,
                place_mark(placed.position, placed.facing, p.position, p.rotation),
            );
        }
        Some(Staged {
            camera: place_camera(placed.position, placed.facing, cam, base),
            decision: stage
                .camera("decision")
                .map(|d| place_camera(placed.position, placed.facing, d, base)),
            line_ms: shot.seconds.map(|s| s as f64 * 1000.0),
            marks,
        })
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

/// A stage place (`position`, rotation quaternion `rotation`, in the stage's space) in the
/// world, on a stage the area places at `stage_at` turned `stage_facing` degrees: a
/// position and an object facing (degrees about Z).
pub fn place_mark(
    stage_at: [f32; 3],
    stage_facing: f32,
    position: [f32; 3],
    rotation: [f32; 4],
) -> ([f32; 3], f32) {
    let stage = thedas_render::Prop::placement(stage_at, stage_facing);
    let at = stage.transform_point3(Vec3::from_array(position));
    // About Z, as objects' quaternions are read (see the simulation's area loading).
    let turn = (2.0 * rotation[2].atan2(rotation[3])).to_degrees();
    (at.to_array(), (stage_facing + turn).rem_euclid(360.0))
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
    fn puts_actors_on_their_marks() {
        // A quarter-turned stage: stage +Y is world -X; the place turns a further half turn.
        let (at, facing) = place_mark(
            [10.0, 20.0, 3.0],
            90.0,
            [0.0, 2.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        );
        assert!(
            (Vec3::from_array(at) - Vec3::new(8.0, 20.0, 3.0)).length() < 1e-4,
            "{at:?}"
        );
        assert!((facing - 270.0).abs() < 1e-3, "{facing}");
    }
}
