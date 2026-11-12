//! Character creation's stage: while chargen.gfx is up, the world view shows the creature
//! being made on a set of its own, framed by the creation screen's camera, as
//! DAOrigins.exe's `GUIPaperdoll` does in its stage mode.
//!
//! What the executable does:
//! - The client keeps a stage area named `char_stage` (the string 0xb159ac, copied at
//!   0x6057e8 into the client's +0x350 and built into the stage object at its +0x388 by
//!   0x7072e0). `char_stage.are` (designerareas.erf) lays it out on `oth999d`, the party
//!   picker's hall.
//! - The paperdoll in stage mode (`GUIPaperdoll` +0x140 == 1, 0x7bb890) draws into the stage's
//!   scene and stands its creature at the stage's waypoint `char_chargen` (0xb21db4, matched
//!   at 0x7bb9ba): the waypoint's position and orientation become the creature's
//!   (0x7bb9d6-0x7bba2d). It then applies camera preset 0 (0x7bbc4d).
//! - The creature is the one in character creation (`GUICharGen`'s `CharGenStart` handler,
//!   0x736680, hands it to the paperdoll, 0x7b8870). The game made it from the
//!   `default_player` template (0x69d953): a human male (appearance 15) in common clothes,
//!   so it shows before any choice is made. The simulation builds its hero the same way.
//! - Camera presets (0x7b38a0, also `Paperdoll.SetCameraPreset`, handler 0x7b49b0): in stage
//!   mode the main game camera is moved (0x6c4b20 field of view, 0x6c45f0 position,
//!   0x6c4ec0 orientation) rather than the paperdoll's own `SceneCamera`. See [`camera`].
//!
//! Provisional: the move between presets is a cut here (the executable blends when
//! +0x1c4 is not set, 0x7b4057); the head height of the face preset is sampled when the
//! preset is first framed for the creature's current look.

use glam::{Quat, Vec3};
use thedas_render::Camera;
use thedas_script::vm::ObjectId;
use thedas_sim::Sim;
use thedas_vfs::Vfs;

/// The stage area (0xb159ac).
pub const STAGE_AREA: &str = "char_stage";
/// The waypoint the creature stands at (0xb21db4).
pub const MARK: &str = "char_chargen";

/// Camera preset 0: the whole body (character creation's first pages). Any other preset
/// frames the face (chargen.gfx's appearance page sends `Paperdoll.SetCameraPreset 1`).
pub const PRESET_BODY: i32 = 0;

/// The stage camera's field of view under `GameModeCharGen` (game mode 0xb, set at
/// 0x719243): 45 degrees (0xc0fd7c; 40, 0xc0fd78, in other modes), checked at 0x7b3913.
pub const FOV_DEGREES: f32 = 45.0;
/// Its orientation: yaw 180 degrees (0xc0fd84) and pitch -10 (0xc0fd80, kept at +0x164),
/// built as yaw about Z, then pitch about X, then roll about Y (0x4ee200).
pub const YAW_DEGREES: f32 = 180.0;
pub const PITCH_DEGREES: f32 = -10.0;
/// Body preset: the height framed under `GameModeCharGen` (1.9 m, 0xb219e0), with this much
/// room around it (1.1, 0xc0fd64), and the camera stepped this far along its right (-0.7 m,
/// 0xc0fd68, in stage mode only) so the creature stands left of centre.
pub const BODY_HEIGHT: f32 = 1.9;
pub const BODY_ROOM: f32 = 1.1;
pub const BODY_SIDE: f32 = -0.7;
/// Face preset: this far from the face (0.65 m, 0xc0fd6c), level with the head bone less
/// 0.05 m (0xc0fd74), stepped along its right by -0.19 m (0xc0fd70, stage mode only).
pub const FACE_DISTANCE: f32 = 0.65;
pub const FACE_DROP: f32 = 0.05;
pub const FACE_SIDE: f32 = -0.19;

/// The stage: its level and where the creature stands on it.
#[derive(Clone, Debug, PartialEq)]
pub struct Stage {
    pub layout: String,
    pub mark: [f32; 3],
    /// The game's facing (degrees about Z, as the simulation keeps it).
    pub facing: f32,
}

impl Stage {
    /// Reads [`STAGE_AREA`]'s area file: its layout and its [`MARK`] waypoint. `None` when
    /// either is missing.
    pub fn load(vfs: &Vfs) -> Option<Stage> {
        use thedas_formats::gff3::{Gff3, Struct, Value};
        fn field<'a>(s: &'a Struct, label: &str) -> Option<&'a Value> {
            s.fields.iter().find(|f| f.label == label).map(|f| &f.value)
        }
        let float = |s: &Struct, label: &str| match field(s, label) {
            Some(Value::Float(v)) => *v,
            _ => 0.0,
        };
        let text = |s: &Struct, label: &str| match field(s, label) {
            Some(Value::ExoString(b) | Value::ResRef(b)) => Some(
                String::from_utf8_lossy(b)
                    .trim_end_matches('\0')
                    .trim()
                    .to_string(),
            ),
            _ => None,
        };
        let (data, _) = vfs.read(&format!("{STAGE_AREA}.are")).ok()?;
        let g = Gff3::parse(&data).ok()?;
        let root = g.structs.first()?;
        let layout = text(root, "Layout").filter(|l| !l.is_empty())?;
        let Some(Value::List(waypoints)) = field(root, "WaypointList") else {
            return None;
        };
        let w = waypoints
            .iter()
            .filter_map(|&i| g.structs.get(i as usize))
            .find(|w| text(w, "Tag").is_some_and(|t| t.eq_ignore_ascii_case(MARK)))?;
        let (z, wq) = (float(w, "ZOrientation"), float(w, "WOrientation"));
        Some(Stage {
            layout,
            mark: [
                float(w, "XPosition"),
                float(w, "YPosition"),
                float(w, "ZPosition"),
            ],
            // About Z, as the simulation reads a waypoint's quaternion.
            facing: (2.0 * z.atan2(wq)).to_degrees().rem_euclid(360.0),
        })
    }
}

/// The creature the stage shows: the one in character creation (`StartCharGen` mode 0),
/// while it is.
pub fn subject(sim: &Sim) -> Option<ObjectId> {
    let id = sim.world.chargen.filter(|_| sim.world.chargen_mode == 0)?;
    sim.world.get(id).map(|o| o.id)
}

/// The camera's orientation (0x4ee200 with yaw 180, pitch -10, roll 0: Z, then X, then Y).
fn orientation() -> Quat {
    Quat::from_rotation_z(YAW_DEGREES.to_radians())
        * Quat::from_rotation_x(PITCH_DEGREES.to_radians())
}

/// The stage camera for camera preset `preset` (0x7b38a0 in stage mode, under
/// `GameModeCharGen`), for a creature standing at `feet` whose head bone is at `head`;
/// `base` gives the clip planes. The camera looks along its +Y, turned by [`orientation`];
/// its right is its +X (0x4cbce0).
/// - Preset 0: it frames [`BODY_HEIGHT`] with [`BODY_ROOM`]: it looks at half that height
///   above the feet (+0x190, 0x7b3f85) from half the height over tan(fov/2), times the
///   room, behind (0x7b3c9f-0x7b3cff, 0x7b3edb-0x7b3f2f), stepped [`BODY_SIDE`] along its
///   right (0x7b3f36-0x7b3f7e).
/// - Any other preset: from the feet raised to the head bone (`head`, 0x6a1ef0) less
///   [`FACE_DROP`] (0x7b39de-0x7b3a07), [`FACE_DISTANCE`] behind (0x7b3a2c), stepped
///   [`FACE_SIDE`] along its right (0x7b3a52-0x7b3a96). Without a head, 1.6 m up
///   (provisional).
///
/// The field of view is [`FOV_DEGREES`], taken as vertical (provisional).
pub fn camera(preset: i32, feet: Vec3, head: Option<Vec3>, base: &Camera) -> Camera {
    let q = orientation();
    let forward = q * Vec3::Y;
    let right = q * Vec3::X;
    let fov = FOV_DEGREES.to_radians();
    let eye = if preset == PRESET_BODY {
        let half = BODY_HEIGHT * 0.5;
        let distance = half / (fov * 0.5).tan() * BODY_ROOM;
        let target = feet + Vec3::Z * half;
        target - forward * distance - right * BODY_SIDE
    } else {
        let z = head.map_or(feet.z + 1.6, |h| h.z) - FACE_DROP;
        let target = Vec3::new(feet.x, feet.y, z);
        target - forward * FACE_DISTANCE - right * FACE_SIDE
    };
    let mut c = Camera::new(
        eye,
        forward.y.atan2(forward.x),
        forward.z.clamp(-1.0, 1.0).asin(),
    );
    c.fov_y = fov;
    c.near = base.near.min(0.05);
    c.far = base.far;
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_preset_frames_the_creature_from_in_front_and_to_its_left() {
        let base = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let feet = Vec3::new(-2.0, -3.0, 1.0);
        let c = camera(PRESET_BODY, feet, None, &base);
        // Looking along -Y and a little down, from +Y of the creature.
        assert!(
            (c.yaw.to_degrees() + 90.0).abs() < 1e-3 || (c.yaw.to_degrees() - 270.0).abs() < 1e-3
        );
        assert!((c.pitch.to_degrees() + 10.0).abs() < 1e-3);
        assert!(c.position.y > feet.y + 2.0);
        // The camera's right is -X; it steps 0.7 m that way, so the creature is left of
        // centre.
        assert!((c.position.x - (feet.x - 0.7)).abs() < 1e-4);
        // Its line of sight passes half the framed height above the feet.
        let to = Vec3::new(c.position.x, feet.y, 0.0);
        let along = (c.position.y - to.y) / c.forward().y.abs();
        let z = c.position.z + c.forward().z * along;
        assert!((z - (feet.z + BODY_HEIGHT * 0.5)).abs() < 1e-3);
        assert!((c.fov_y.to_degrees() - 45.0).abs() < 1e-4);
    }

    #[test]
    fn the_face_preset_looks_at_the_head_from_close() {
        let base = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let feet = Vec3::new(0.0, 0.0, 0.0);
        let head = Vec3::new(0.0, 0.05, 1.7);
        let c = camera(1, feet, Some(head), &base);
        let back = c.position - Vec3::new(feet.x - 0.19, feet.y, 1.65);
        assert!((back.length() - FACE_DISTANCE).abs() < 1e-4);
        assert!(c.position.z > 1.65);
    }
}
