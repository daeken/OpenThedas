//! Conversation staging: stages (`.stg`, GFF4 `STAG`) and the shot each conversation line
//! takes from one.
//!
//! A stage lays out a conversation around a point an area places (an area's `StageList`
//! entry, found by tag): the places its actors stand at and the cameras that frame them, in
//! the stage's own space (metres; +Y forward, Z up). Each place names its default camera.
//!
//! A conversation (`.dlg`) line names its stage's tag (12100) and carries its cinematics as a
//! cutscene (12211, a `CUT ` struct): the stage file it was made with (5017) and its actors
//! (5200). The "Primary Camera" actor's tracks include an `AJSC` entry naming the stage place
//! whose default camera frames the line.
//!
//! Its actors' `AJSP` tracks put each at a stage place. A line staged without cinematics of
//! its own (an empty cut) takes the camera of the place its speaker stands at, and the
//! actors' places, from the nearest line on the same stage with cinematics that places the
//! speaker (actors move between places from line to line).
//!
//! A line's cut also gives how long its cinematics run (5000, seconds); a stage's `decision`
//! camera frames the player choosing a reply.
//!
//! Provisional. Field names are xoreos's (read for the format only): 11000 places, 11001
//! cameras, 11003 camera field of view, 11004 a place's default camera; 1 a name, 4 a
//! position, 5 a rotation quaternion. In the conversations checked, the camera actor's own
//! transform (5221/5222) equals the named stage camera, its field-of-view track always reads
//! the default 45 and its offset tracks are zero, so the stage camera alone is taken.

use crate::gff4::{FourCC, Gff4, StructId};
use crate::typed::{Doc, TypedResult};

pub mod field {
    //! GFF4 field labels for stages and line shots. Provisional (see the module docs).
    pub const NAME: u32 = 1;
    pub const POSITION: u32 = 4;
    pub const ROTATION: u32 = 5;
    pub const STAGE_PLACES: u32 = 11000;
    pub const STAGE_CAMERAS: u32 = 11001;
    pub const CAMERA_FOV: u32 = 11003;
    pub const PLACE_DEFAULT_CAMERA: u32 = 11004;
    pub const CONV_LINES: u32 = 12002;
    pub const LINE_SPEAKER: u32 = 12202;
    pub const LINE_STAGE: u32 = 12100;
    pub const LINE_CUT: u32 = 12211;
    pub const CUT_STAGE_FILE: u32 = 5017;
    pub const CUT_SECONDS: u32 = 5000;
    pub const CUT_ACTORS: u32 = 5200;
    pub const ACTOR_NAME: u32 = 2;
    pub const ACTOR_TRACKS: u32 = 5206;
    pub const JUMP_BY_PLACE: u32 = 5570;
}

/// The cutscene track that puts a camera at a stage place's camera.
const SET_CAMERA: FourCC = FourCC(*b"AJSC");
/// The cutscene track that puts an actor at a stage place.
const SET_PLACE: FourCC = FourCC(*b"AJSP");
/// The cutscene actor that is the line's camera.
const PRIMARY_CAMERA: &str = "Primary Camera";

/// Where an actor stands on a stage, and the camera that frames it.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub name: String,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub default_camera: Option<String>,
}

/// A stage camera: where it is and how it turns (local +Y is its view direction) in the
/// stage's space, and its field of view in degrees.
#[derive(Clone, Debug, PartialEq)]
pub struct StageCamera {
    pub name: String,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub fov: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stage {
    pub places: Vec<Place>,
    pub cameras: Vec<StageCamera>,
}

const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

impl Stage {
    pub fn parse(data: &[u8]) -> TypedResult<Stage> {
        Self::from_gff4(&Gff4::parse(data)?)
    }

    pub fn from_gff4(g: &Gff4) -> TypedResult<Stage> {
        let d = Doc::new(g, "stg");
        let name =
            |id| -> TypedResult<String> { Ok(d.opt_str(id, field::NAME)?.unwrap_or_default()) };
        let mut places = Vec::new();
        for p in d.structs(g.root, field::STAGE_PLACES)? {
            places.push(Place {
                name: name(p)?,
                position: d.opt_vec3(p, field::POSITION)?.unwrap_or_default(),
                rotation: d.opt_vec4(p, field::ROTATION, 1.0)?.unwrap_or(IDENTITY),
                default_camera: d
                    .opt_str(p, field::PLACE_DEFAULT_CAMERA)?
                    .filter(|c| !c.is_empty()),
            });
        }
        let mut cameras = Vec::new();
        for c in d.structs(g.root, field::STAGE_CAMERAS)? {
            cameras.push(StageCamera {
                name: name(c)?,
                position: d.opt_vec3(c, field::POSITION)?.unwrap_or_default(),
                rotation: d.opt_vec4(c, field::ROTATION, 1.0)?.unwrap_or(IDENTITY),
                fov: d
                    .opt_f32(c, field::CAMERA_FOV)?
                    .filter(|f| f.is_finite() && *f > 0.0)
                    .unwrap_or(45.0),
            });
        }
        Ok(Stage { places, cameras })
    }

    /// The camera named `name`, ignoring case (`decision` frames the player choosing).
    pub fn camera(&self, name: &str) -> Option<&StageCamera> {
        self.cameras
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// The camera a line's `AJSC` track names: a place's default camera, or else the camera
    /// of that name (lines put the player's choice under the `decision` camera this way).
    /// Names ignore case.
    pub fn camera_for(&self, name: &str) -> Option<&StageCamera> {
        match self
            .places
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
        {
            Some(p) => self.camera(p.default_camera.as_deref()?),
            None => self.camera(name),
        }
    }

    /// The camera an `ActionJumpToCamera` track (`AJSC`) puts its actor on (0x7d1ea0 ->
    /// 0x7d1bf0): the camera of exactly that name, or, when the track's 5570 is set
    /// (`by_place`), the default camera of the place of exactly that name (0x7d1e10).
    pub fn jump_camera(&self, name: &str, by_place: bool) -> Option<&StageCamera> {
        let name = if by_place {
            self.places
                .iter()
                .find(|p| p.name == name)?
                .default_camera
                .as_deref()?
        } else {
            name
        };
        self.cameras.iter().find(|c| c.name == name)
    }
}

/// The shot a conversation line takes: the stage (an area stage's tag), the stage file
/// its cinematics were made with (without `.stg`), the camera that frames it (see
/// [`Stage::jump_camera`]: `place` names the camera, or with `by_place` the place whose
/// default camera it is) and the place each actor stands at (actor as the cutscene names
/// it: `OWNER`, `PLAYER`, `HENCH:<tag>` or a creature's tag; place).
#[derive(Clone, Debug, PartialEq)]
pub struct LineShot {
    pub stage: String,
    pub stage_file: String,
    pub place: String,
    pub by_place: bool,
    pub actors: Vec<(String, String)>,
    /// How long the line's cinematics run (seconds, 5000), when the line has its own.
    pub seconds: Option<f32>,
}

/// Each line of a conversation (in the order of its line list, 12002, which the dialog's
/// node indices follow) with the shot it takes, if it is staged.
pub fn line_shots(g: &Gff4) -> TypedResult<Vec<Option<LineShot>>> {
    let d = Doc::new(g, "dlg");
    let mut staged = Vec::new();
    for line in d.structs(g.root, field::CONV_LINES)? {
        staged.push(staging(&d, line)?);
    }
    let mut out = Vec::with_capacity(staged.len());
    for (i, s) in staged.iter().enumerate() {
        let Some(s) = s else {
            out.push(None);
            continue;
        };
        if let (Some(file), Some((place, by_place))) = (&s.stage_file, &s.camera) {
            out.push(Some(LineShot {
                stage: s.stage.clone(),
                stage_file: file.clone(),
                place: place.clone(),
                by_place: *by_place,
                actors: s.places.clone(),
                seconds: s.seconds,
            }));
            continue;
        }
        // The speaker's place on the nearest line of the same stage that places them.
        let mut order: Vec<usize> = (0..staged.len()).filter(|&j| j != i).collect();
        order.sort_by_key(|&j| j.abs_diff(i));
        let found = order.into_iter().find_map(|j| {
            let o = staged[j].as_ref()?;
            if !o.stage.eq_ignore_ascii_case(&s.stage) {
                return None;
            }
            let (_, place) = o
                .places
                .iter()
                .find(|(actor, _)| actor.eq_ignore_ascii_case(&s.speaker))?;
            Some(LineShot {
                stage: s.stage.clone(),
                stage_file: o.stage_file.clone()?,
                place: place.clone(),
                by_place: true,
                actors: o.places.clone(),
                seconds: s.seconds,
            })
        });
        out.push(found);
    }
    Ok(out)
}

/// What one line's cinematics say about its staging.
struct Staging {
    stage: String,
    speaker: String,
    stage_file: Option<String>,
    /// The camera that frames the line: the `AJSC` track's name and its 5570.
    camera: Option<(String, bool)>,
    /// Each actor's place (actor name, place).
    places: Vec<(String, String)>,
    seconds: Option<f32>,
}

fn staging(d: &Doc, line: StructId) -> TypedResult<Option<Staging>> {
    let Some(stage) = d
        .opt_str(line, field::LINE_STAGE)?
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    let mut s = Staging {
        stage,
        speaker: d.opt_str(line, field::LINE_SPEAKER)?.unwrap_or_default(),
        stage_file: None,
        camera: None,
        places: Vec::new(),
        seconds: None,
    };
    let Some(cut) = d.opt_struct(line, field::LINE_CUT)? else {
        return Ok(Some(s));
    };
    s.seconds = d
        .opt_f32(cut, field::CUT_SECONDS)?
        .filter(|t| t.is_finite() && *t > 0.0);
    s.stage_file = d
        .opt_str(cut, field::CUT_STAGE_FILE)?
        .filter(|f| !f.is_empty())
        .map(|f| {
            let f = f.to_ascii_lowercase();
            f.strip_suffix(".stg").map(str::to_string).unwrap_or(f)
        });
    for actor in d.structs(cut, field::CUT_ACTORS)? {
        let name = d.opt_str(actor, field::ACTOR_NAME)?.unwrap_or_default();
        let is_camera = name.eq_ignore_ascii_case(PRIMARY_CAMERA);
        for track in d.structs(actor, field::ACTOR_TRACKS)? {
            let label = d.label(track);
            let Some(place) = d.opt_str(track, field::NAME)?.filter(|p| !p.is_empty()) else {
                continue;
            };
            if is_camera && label == SET_CAMERA && s.camera.is_none() {
                let by_place = d
                    .opt_i64(track, field::JUMP_BY_PLACE)?
                    .is_some_and(|v| v != 0);
                s.camera = Some((place, by_place));
            } else if label == SET_PLACE {
                s.places.push((name.clone(), place));
            }
        }
    }
    Ok(Some(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gff4::{GenericSlot, Value};
    use crate::typed::testutil::{Builder, generic, s, structs};

    #[test]
    fn reads_places_and_cameras() {
        let mut b = Builder::new(b"STG ");
        let place = b.add(
            b"PLAC",
            vec![
                (field::NAME, s("node1_mother")),
                (field::POSITION, Value::Vec3([0.5, -0.25, 0.0])),
                (field::ROTATION, Value::Quat([0.0, 0.0, 0.0, 1.0])),
                (field::PLACE_DEFAULT_CAMERA, s("cam1")),
            ],
        );
        let cam = b.add(
            b"CAMR",
            vec![
                (field::NAME, s("cam1")),
                (field::POSITION, Value::Vec3([0.0, 1.5, 1.6])),
                (field::ROTATION, Value::Quat([0.0, 0.0, 1.0, 0.0])),
                (field::CAMERA_FOV, Value::F32(25.0)),
            ],
        );
        let other = b.add(b"CAMR", vec![(field::NAME, s("group"))]);
        let root = b.add(
            b"STAG",
            vec![
                (field::STAGE_PLACES, structs(&[place])),
                (field::STAGE_CAMERAS, structs(&[cam, other])),
            ],
        );
        let st = Stage::from_gff4(&b.finish(root)).unwrap();
        let c = st.camera_for("NODE1_MOTHER").unwrap();
        assert_eq!(c.position, [0.0, 1.5, 1.6]);
        assert_eq!(c.fov, 25.0);
        assert_eq!(st.cameras[1].fov, 45.0);
        assert!(st.camera_for("nobody").is_none());
        // A name that is no place's is a camera's.
        assert_eq!(st.camera_for("Group").unwrap().name, "group");
        // A jump finds a camera by its exact name, or a place's default camera.
        assert_eq!(st.jump_camera("group", false).unwrap().name, "group");
        assert!(st.jump_camera("Group", false).is_none());
        assert_eq!(st.jump_camera("node1_mother", true).unwrap().name, "cam1");
        assert!(st.jump_camera("node1_mother", false).is_none());
    }

    #[test]
    fn finds_each_lines_shot() {
        let mut b = Builder::new(b"CONV");
        let set = b.add(
            b"AJSC",
            vec![
                (field::NAME, s("node1_mother")),
                (field::JUMP_BY_PLACE, Value::U8(255)),
            ],
        );
        let track = b.add(b"TRAK", vec![]);
        let camera = b.add(
            b"ACTR",
            vec![
                (field::ACTOR_NAME, s("Primary Camera")),
                (field::ACTOR_TRACKS, generic(&[track, set])),
            ],
        );
        let stand = b.add(b"AJSP", vec![(field::NAME, s("node2_player"))]);
        let player = b.add(
            b"ACTR",
            vec![
                (field::ACTOR_NAME, s("PLAYER")),
                (field::ACTOR_TRACKS, generic(&[stand])),
            ],
        );
        let cut = b.add(
            b"CUT ",
            vec![
                (field::CUT_STAGE_FILE, s("BHN_Stage_1.stg")),
                (field::CUT_SECONDS, Value::F32(2.5)),
                (field::CUT_ACTORS, structs(&[player, camera])),
            ],
        );
        let staged = b.add(
            b"NTRY",
            vec![
                (field::LINE_STAGE, s("bhn_stage")),
                (
                    field::LINE_CUT,
                    Value::Generic(Box::new(GenericSlot {
                        type_code: 0,
                        flags: 0,
                        value: Some(Value::Struct(Some(cut))),
                    })),
                ),
            ],
        );
        let plain = b.add(b"RPLY", vec![]);
        let bare = b.add(
            b"NTRY",
            vec![
                (field::LINE_STAGE, s("bhn_stage")),
                (field::LINE_SPEAKER, s("player")),
            ],
        );
        let elsewhere = b.add(
            b"NTRY",
            vec![
                (field::LINE_STAGE, s("other_stage")),
                (field::LINE_SPEAKER, s("PLAYER")),
            ],
        );
        let root = b.add(
            b"CONV",
            vec![(
                field::CONV_LINES,
                generic(&[staged, plain, bare, elsewhere]),
            )],
        );
        let shots = line_shots(&b.finish(root)).unwrap();
        assert_eq!(
            shots,
            vec![
                Some(LineShot {
                    stage: "bhn_stage".into(),
                    stage_file: "bhn_stage_1".into(),
                    place: "node1_mother".into(),
                    by_place: true,
                    actors: vec![("PLAYER".into(), "node2_player".into())],
                    seconds: Some(2.5),
                }),
                None,
                Some(LineShot {
                    stage: "bhn_stage".into(),
                    stage_file: "bhn_stage_1".into(),
                    place: "node2_player".into(),
                    by_place: true,
                    actors: vec![("PLAYER".into(), "node2_player".into())],
                    seconds: None,
                }),
                None,
            ]
        );
    }
}
