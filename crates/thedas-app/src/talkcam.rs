//! The conversation camera: while a conversation line with staged cinematics is up, the
//! view is that line's stage camera (see [`thedas_formats::stage`]), placed where the area
//! puts the line's stage. A line without a shot gets the executable's default framing of its
//! speaker (see [`default_framing`]). The line's actors are drawn by its own cutscene (see
//! [`crate::cutactors`]), and the simulation leaves them on their marks when the line ends
//! (see [`thedas_sim::cutactors`]).
//!
//! Presentation only: reads the simulation and the dialog and stage resources.
//!
//! A line whose own cutscene cuts between camera actors is seen through that cutscene's
//! camera as it plays (see [`line_cut_view`], traced).
//!
//! Once a line with its own cinematics has run its length (the cut's, 5000) and the player
//! has replies to choose, the view moves to the stage's `decision` camera. Provisional, and
//! our own invention: the executable has no `decision` string and no such fallback was found
//! in `GameModeDialog`; what the original shows while the player chooses is not traced.
//!
//! Provisional: the camera's field of view is taken as vertical; camera moves between
//! lines, and within a line whose cutscene has no camera cuts, are not followed; nor are
//! lines staged at the speaker's current location (`StageAtCurrentLocation`).

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Quat, Vec3};
use thedas_formats::stage::{LineShot, Stage, StageCamera};
use thedas_render::Camera;
use thedas_script::vm::ObjectId;
use thedas_sim::Sim;
use thedas_sim::cutactors::{Bound, LineCut};
use thedas_sim::cuttracks::{Finders, Placement, Tracks};
use thedas_sim::dialog::OpenLine;
use thedas_sim::world::{Object, ObjectKind};

use crate::cutactors::{DrawnRoots, drawn_object, stage_place};

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
    /// Each conversation's lines' own cutscenes, by conversation.
    cuts: HashMap<String, Option<Arc<Vec<LineCut>>>>,
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
        let shot = open.line.and_then(|l| {
            let shots = self.shots(sim, &line.0.0)?;
            shots.get(l).cloned().flatten()
        });
        if self.shown.as_ref().is_none_or(|(l, _)| *l != line) {
            self.shown = Some((line.clone(), self.clock_ms));
            log::debug!(
                "conversation {} line {:?} (speaker {:?}) at {:.0} ms: shot {:?}",
                line.0.0,
                open.line,
                sim.dialog_state().map(|d| d.speaker),
                self.clock_ms,
                shot.as_ref()
                    .map(|s| (&s.stage, &s.stage_file, &s.place, s.by_place, s.seconds))
            );
        }
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
    /// to choose. Provisional: the decision camera as a fallback is our own invention (see
    /// the module docs); the original's line length is its voice-over's; the cut's
    /// length (5000) is taken instead, and lines without one keep their shot.
    ///
    /// A line without a shot of its own is framed as the executable frames a line that
    /// plays no cinematics (see [`default_framing`]); `face` gives a creature's eyes as
    /// drawn, offset from where it stands.
    ///
    /// A line that plays its own cutscene with camera cuts is seen through that cutscene's
    /// camera as it plays (see [`line_cut_view`]); `roots` gives where the drawn objects
    /// are, which cameras linked to them follow.
    pub fn camera(
        &mut self,
        sim: &Sim,
        base: &Camera,
        face: &dyn Fn(ObjectId) -> Option<Vec3>,
        roots: &DrawnRoots,
    ) -> Option<Camera> {
        let open = sim.conversation_line()?;
        let choosing = open.choosing;
        let line: LineKey = ((open.dialog.to_ascii_lowercase(), open.owner), open.line);
        let now = self.clock_ms;
        let staging = self.staging(sim, base).cloned();
        let shown = self.shown.clone();
        if let Some(view) = self.line_cut_camera(sim, &line, now, base, roots) {
            return Some(view);
        }
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

    /// The frame buffer effects the shown line's own cutscene has running (its kind-19
    /// tracks: depth of field, fades), when it is the line the simulation staged; on the
    /// clock its camera runs on (see [`Self::line_cut_camera`]).
    pub fn line_cut_effects(&mut self, sim: &Sim) -> Vec<thedas_sim::cuttracks::FrameEffectState> {
        let Some(open) = sim.conversation_line() else {
            return Vec::new();
        };
        let line: LineKey = ((open.dialog.to_ascii_lowercase(), open.owner), open.line);
        let Some((dlg, at, _, _)) = sim.line_staging() else {
            return Vec::new();
        };
        if !dlg.eq_ignore_ascii_case(&line.0.0) || Some(at) != line.1 {
            return Vec::new();
        }
        let tracks = self
            .line_cuts(sim, &line.0.0)
            .and_then(|cuts| cuts.get(at)?.tracks.clone());
        let Some(tracks) = tracks else {
            return Vec::new();
        };
        let now = self.clock_ms;
        let since = match &self.shown {
            Some((l, since)) if *l == line => *since,
            _ => now,
        };
        tracks.frame_effects_at(((now - since) / 1000.0) as f32)
    }

    /// The view of the shown line's own cutscene (see [`line_cut_view`]), when it is the
    /// line the simulation staged and its cutscene has camera cuts.
    fn line_cut_camera(
        &mut self,
        sim: &Sim,
        line: &LineKey,
        now: f64,
        base: &Camera,
        roots: &DrawnRoots,
    ) -> Option<Camera> {
        let (dlg, at, placement, bound) = sim.line_staging()?;
        if !dlg.eq_ignore_ascii_case(&line.0.0) || Some(at) != line.1 {
            return None;
        }
        let tracks = self.line_cuts(sim, &line.0.0)?.get(at)?.tracks.clone()?;
        if tracks.cuts.is_empty() {
            return None;
        }
        let since = match &self.shown {
            Some((l, since)) if l == line => *since,
            _ => now,
        };
        let t = ((now - since) / 1000.0) as f32;
        let stage = tracks.stage_file.as_deref().and_then(|f| {
            let f = f.trim_end_matches('\0').to_ascii_lowercase();
            let f = f.strip_suffix(".stg").unwrap_or(&f).to_string();
            self.stage(sim, &f)
        });
        let view = line_cut_view(&tracks, t, placement, stage.as_deref(), bound, roots, base);
        if view.is_none() {
            log::debug!(
                "line {at} of {dlg}: its cutscene gives no camera {t:.2}s in (stage {:?})",
                tracks.stage_file
            );
        }
        view
    }

    fn line_cuts(&mut self, sim: &Sim, dlg: &str) -> Option<Arc<Vec<LineCut>>> {
        self.cuts
            .entry(dlg.to_string())
            .or_insert_with(|| {
                let (data, _) = sim.vfs.read(&format!("{dlg}.dlg")).ok()?;
                let g = thedas_formats::Gff4::parse(&data).ok()?;
                Some(Arc::new(thedas_sim::cutactors::line_cuts(&g)))
            })
            .clone()
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
        let Some(cam) = stage.jump_camera(&shot.place, shot.by_place) else {
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

/// The view of a conversation line's own cutscene (`tracks`, its 12211 `CUT ` struct) `t`
/// seconds after the line came up: its camera actor's world pose, as for any cutscene
/// (see [`crate::cutcam`]). `None` when the cutscene has no camera cuts, or its camera
/// jumps to a camera `stage` (the cutscene's stage file) does not have.
///
/// Traced in DAOrigins.exe: as a line comes up, `GameModeDialog` builds the line's
/// cutscene (0x68e53c -> 0x7f0960: from the `CUTR` reference's resource, else from the
/// inline `CUT ` struct, 0x7f0a4d -> 0x7e2a70) and places it on the line's stage
/// (0x68e584 -> 0x7d2aa0); a line with neither gets the default framing (0x68e541 ->
/// 0x68e700, see [`default_framing`]). Each update ticks that cutscene (0x68df79 ->
/// 0x7e3d30), and its tick hands the camera the active camera actor's world pose (0x7e41b1
/// .. 0x7e41ca, as `GameModeCutscene` does): so the camera cuts between camera actors
/// (`ActionChangeActiveCamera`, kind 11, e.g. a "Primary Camera" at 0 s and a "Secondary
/// Camera" at 2.5 s) and jumps from stage camera to stage camera within the line.
///
/// Provisional: once the cutscene has run its length the view holds its last pose (nothing
/// else in `GameModeDialog` sets the camera that was found; not traced to the end).
pub fn line_cut_view(
    tracks: &Tracks,
    t: f32,
    placement: Placement,
    stage: Option<&Stage>,
    bound: &[Bound],
    roots: &DrawnRoots,
    base: &Camera,
) -> Option<Camera> {
    if tracks.cuts.is_empty() {
        return None;
    }
    let camera = |name: &str, by_place: bool| crate::cutcam::stage_camera(stage?, name, by_place);
    let place = |name: &str| stage_place(stage, name);
    let root = |i: usize| roots(drawn_object(tracks, bound, i)?);
    let find = Finders {
        camera: &camera,
        place: &place,
        root: &root,
    };
    let pose = tracks.camera_at(t.max(0.0), placement, &find)?;
    Some(crate::cutcam::pose_camera(&pose, base))
}

/// The open conversation's line's speaker and who it speaks to, when a line is up that is
/// not said in passing. Provisional: the listener is the party's leader (0x68d270 asks for
/// the player's creature), or the conversation's owner when the leader speaks.
pub fn line_speaker_and_listener(sim: &Sim) -> Option<(ObjectId, ObjectId)> {
    let open = sim.conversation_line()?;
    if open.ambient {
        return None;
    }
    let said = sim.dialog_state()?.speaker;
    let said = said.trim_end_matches('\0');
    let speaker = if said.is_empty() {
        open.owner
    } else {
        actor_object(sim, said, &open)?
    };
    let leader = sim.world.controlled().unwrap_or(open.player);
    let listener = if speaker == leader {
        open.owner
    } else {
        leader
    };
    Some((speaker, listener))
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
    use std::f32::consts::FRAC_1_SQRT_2;
    use thedas_sim::cuttracks::{Actor, CameraJump};

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

    /// A camera actor of a line's cutscene that jumps to stage camera `camera` (or, with
    /// `by_place`, to that place's default camera) for the whole line.
    fn camera_actor(id: i64, name: &str, camera: &str, by_place: bool) -> Actor {
        Actor {
            id,
            name: name.into(),
            creature: None,
            model: None,
            tag: None,
            end_position: [0.0; 3],
            end_rotation: [0.0, 0.0, 0.0, 1.0],
            snaps: false,
            pose: None,
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            motion: Vec::new(),
            turn: Vec::new(),
            fov: Vec::new(),
            animations: Vec::new(),
            places: Vec::new(),
            camera_jumps: vec![CameraJump {
                start: 0.0,
                end: 1e10,
                camera: camera.into(),
                by_place,
            }],
            links: Vec::new(),
            head_tracks: Vec::new(),
            visibility: Vec::new(),
            weapon_draws: Vec::new(),
        }
    }

    #[test]
    fn a_lines_own_cutscene_cuts_between_its_cameras() {
        use thedas_formats::stage::Place;
        // A line made like Irving's in the Harrowing: the "Primary Camera" (id 3) is on the
        // speaker's place's default camera, the "Secondary Camera" (id 5) on a wide camera
        // from 0.5 s.
        let stage = Stage {
            places: vec![Place {
                name: "speaker".into(),
                position: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                default_camera: Some("close".into()),
            }],
            cameras: vec![
                StageCamera {
                    name: "close".into(),
                    position: [0.0, -1.0, 1.6],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    fov: 30.0,
                },
                StageCamera {
                    name: "wide".into(),
                    position: [4.0, 0.0, 1.5],
                    // A quarter turn about Z: it looks along stage -X.
                    rotation: [0.0, 0.0, FRAC_1_SQRT_2, FRAC_1_SQRT_2],
                    fov: 45.0,
                },
            ],
        };
        let tracks = Tracks {
            actors: vec![
                camera_actor(3, "Primary Camera", "speaker", true),
                camera_actor(5, "Secondary Camera", "wide", false),
            ],
            stage_file: Some("line.stg".into()),
            seconds: 7.7,
            cuts: vec![(0.0, 3), (0.5, 5)],
            ..Default::default()
        };
        // The cutscene plays on a stage the area puts at (10, 20, 0), a quarter turn round.
        let placement = Placement::facing([10.0, 20.0, 0.0], 90.0);
        let base = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let none: &DrawnRoots = &|_| None;
        let view =
            |t: f32| line_cut_view(&tracks, t, placement, Some(&stage), &[], none, &base).unwrap();
        // First the speaker's close camera: stage (0, -1) is world (11, 20), looking along
        // stage +Y = world -X.
        let c = view(0.2);
        assert!(
            (c.position - Vec3::new(11.0, 20.0, 1.6)).length() < 1e-4,
            "{:?}",
            c.position
        );
        assert!((c.forward() - Vec3::NEG_X).length() < 1e-4);
        assert!((c.fov_y - 30f32.to_radians()).abs() < 1e-6);
        // From 0.5 s the wide camera: stage (4, 0) is world (10, 24), looking along stage
        // -X = world -Y.
        for t in [0.5, 3.0, 20.0] {
            let c = view(t);
            assert!(
                (c.position - Vec3::new(10.0, 24.0, 1.5)).length() < 1e-4,
                "{t}: {:?}",
                c.position
            );
            assert!((c.forward() - Vec3::NEG_Y).length() < 1e-4, "{t}");
            assert!((c.fov_y - 45f32.to_radians()).abs() < 1e-6);
        }
        // A cutscene without camera cuts frames nothing, nor does a jump to a camera the
        // stage does not have.
        let uncut = Tracks {
            cuts: Vec::new(),
            ..tracks.clone()
        };
        assert!(line_cut_view(&uncut, 1.0, placement, Some(&stage), &[], none, &base).is_none());
        assert!(line_cut_view(&tracks, 1.0, placement, None, &[], none, &base).is_none());
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
