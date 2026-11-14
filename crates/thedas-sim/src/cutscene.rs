//! Cutscenes scripts load with `LoadCutscene`: `.cut` files (GFF4 `CUT `). The simulation
//! needs their length and the lines said in them, which exist nowhere else: a scripted
//! cutscene's speech is not in any conversation the player walks through.
//!
//! Provisional, from the files' contents: the root's field 5000 is the length in seconds and
//! field 5200 lists the actors (`ACTR`, name in field 2), each with a generic list of tracks
//! (field 5206). A spoken line is an `ASLN` track: its text (field 22, a talk-table
//! reference), when it starts and ends (fields 5301 and 5302, seconds into the cutscene) and
//! the conversation it comes from (field 5562). A line without its own text says a line of
//! that conversation ([`dialog_refs`]): provisional, the k-th such line (by start time)
//! says the k-th line the conversation walks, from the line that played the cutscene (or
//! from the conversation's start when another conversation's line played it; e.g.
//! `bhn100cs_intro` speaks `bhn100dg_introduction`'s six lines in this order).
//!
//! A movie played full screen is an `APBM` track (track type 42 in field 5300), the
//! executable's `EC::Cutscene::Actor::ActionPlayMovie` (vtable 0xb23608): the Bink movie's
//! name is field 5740 (no extension; `packages/core/data/movies`), and it plays from the
//! track's start (field 5301) until the track's end (5302, 1e10 for "to the end").
//! Its update (0x7ce740) starts the movie once the cutscene's clock reaches the start
//! (the renderer's movie player at `[0xc31650]`+0xc0, its `Play` 0x469390, which stops any
//! movie playing first), ducks the audio (`audio_categories/ducking/OnMovie`) and hands the
//! player the cutscene's time every tick; its exit (0x7ce880) stops it. Nothing in the
//! simulation reads the movies: [`movies`] is for the presentation.

use thedas_formats::Gff4;
use thedas_formats::gff4::{List, StructId, Value};

const FIELD_SECONDS: u32 = 5000;
const FIELD_END_SCRIPT: u32 = 5001;
const FIELD_ACTORS: u32 = 5200;
const FIELD_ACTOR_NAME: u32 = 2;
const FIELD_TRACKS: u32 = 5206;
const FIELD_LINE_TEXT: u32 = 22;
const FIELD_LINE_START: u32 = 5301;
const FIELD_LINE_END: u32 = 5302;
const FIELD_MOVIE: u32 = 5740;
const FIELD_SOUND_EVENT: u32 = 5670;
const FIELD_LINE_CONVERSATION: u32 = 5562;
const FIELD_LINE_VO: u32 = 5564;

/// The longest a cutscene is taken to run (seconds), whatever its file says.
pub const MAX_SECONDS: f32 = 600.0;

/// A line said in a cutscene.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CutsceneLine {
    /// Seconds into the cutscene it shows, and stops showing.
    pub start: f32,
    pub end: f32,
    pub strref: u32,
    /// The actor saying it, as the cutscene names it.
    pub actor: String,
    /// The conversation the line comes from (field 5562): its voice-over is in that
    /// conversation's FMOD project.
    #[serde(default)]
    pub conversation: Option<String>,
    /// Field 5564. Taken to say whether the line's own voice-over plays: 0 plays it, 2 is a
    /// line whose voice is mixed into one of the cutscene's sound tracks (the intro's
    /// narration: `game_intro`'s lines are 2 but for three later additions, which are 0).
    /// Provisional.
    #[serde(default)]
    pub vo_mode: u8,
}

/// A cutscene, and how far it has played.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Cutscene {
    /// The resource, e.g. `pre100cs_intro.cut`.
    pub name: String,
    pub seconds: f32,
    /// By start time.
    pub lines: Vec<CutsceneLine>,
    /// Milliseconds it has played.
    #[serde(default)]
    pub elapsed_ms: u64,
    /// A conversation line played it (see [`crate::dialog::Node::cutscene`]).
    #[serde(default)]
    pub from_dialog: bool,
    /// The objects its actors took, put back or left at their end poses when it ends (see
    /// [`crate::cutactors`]).
    #[serde(default)]
    pub bound: Vec<crate::cutactors::Bound>,
    /// Where it plays, when a conversation placed it (see [`crate::cuttracks::Placement`]).
    #[serde(default)]
    pub placement: Option<crate::cuttracks::Placement>,
}

impl Cutscene {
    /// The resource name a script gives (`pre100cs_intro` or `PRE100CS_INTRO.cut`).
    pub fn resource(name: &str) -> String {
        let n = name.trim().to_ascii_lowercase();
        if n.ends_with(".cut") {
            n
        } else {
            format!("{n}.cut")
        }
    }

    pub fn parse(data: &[u8], name: &str) -> Option<Self> {
        Self::from_gff4(&Gff4::parse(data).ok()?, name)
    }

    pub fn from_gff4(g: &Gff4, name: &str) -> Option<Self> {
        let root = cut_root(g)?;
        let seconds = g
            .field(root, FIELD_SECONDS)
            .and_then(Value::as_f64)
            .map(|s| s as f32)
            .filter(|s| s.is_finite() && *s >= 0.0)?
            .min(MAX_SECONDS);
        let mut lines = Vec::new();
        for actor in structs(g, root, FIELD_ACTORS) {
            let name = g
                .field(actor, FIELD_ACTOR_NAME)
                .and_then(Value::as_str)
                .map(|s| s.trim_end_matches('\0').to_string())
                .unwrap_or_default();
            for track in structs(g, actor, FIELD_TRACKS) {
                if g.template_of(track).label.0 != *b"ASLN" {
                    continue;
                }
                let Some(Value::Tlk(t)) = g.field(track, FIELD_LINE_TEXT) else {
                    continue;
                };
                let time = |l| {
                    g.field(track, l)
                        .and_then(Value::as_f64)
                        .map(|s| s as f32)
                        .filter(|s| s.is_finite())
                };
                let (Some(start), Some(end)) = (time(FIELD_LINE_START), time(FIELD_LINE_END))
                else {
                    continue;
                };
                if t.id == u32::MAX || end < start {
                    continue;
                }
                let conversation = g
                    .field(track, FIELD_LINE_CONVERSATION)
                    .and_then(Value::as_str)
                    .map(|s| s.trim_end_matches('\0').trim().to_string())
                    .filter(|s| !s.is_empty());
                let vo_mode = g
                    .field(track, FIELD_LINE_VO)
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0) as u8;
                lines.push(CutsceneLine {
                    start: start.clamp(0.0, seconds),
                    end: end.clamp(0.0, seconds),
                    strref: t.id,
                    actor: name.clone(),
                    conversation,
                    vo_mode,
                });
            }
        }
        lines.sort_by(|a, b| a.start.total_cmp(&b.start));
        Some(Self {
            name: Self::resource(name),
            seconds,
            lines,
            elapsed_ms: 0,
            from_dialog: false,
            bound: Vec::new(),
            placement: None,
        })
    }

    /// The line being said now, as far as it has played.
    pub fn line_now(&self) -> Option<&CutsceneLine> {
        self.line_at(self.elapsed_ms as f32 / 1000.0)
    }

    /// The line showing `seconds` into the cutscene, if any.
    pub fn line_at(&self, seconds: f32) -> Option<&CutsceneLine> {
        self.lines
            .iter()
            .rev()
            .find(|l| l.start <= seconds && seconds < l.end)
    }
}

/// The completion script named by the cutscene, including DA2's LINE wrapper. The
/// DA2 loader (0x59d15a) reads field 5001 into the cutscene's string member at +0xf0.
/// For example, the voyage names a script that performs the transition to the Gallows.
/// Its execution context and ordering are still provisional; see `Sim::close_cutscene`.
pub fn end_script(g: &Gff4) -> Option<String> {
    let root = cut_root(g)?;
    g.field(root, FIELD_END_SCRIPT)
        .and_then(Value::as_str)
        .map(|s| s.trim_end_matches('\0').trim().to_string())
        .filter(|s| !s.is_empty())
}

/// A spoken line of a cutscene that says a conversation's line rather than its own text:
/// an `ASLN` track without a talk-table reference (22) naming a conversation (5562).
#[derive(Clone, Debug, PartialEq)]
pub struct DialogRef {
    pub start: f32,
    pub end: f32,
    pub actor: String,
    /// The conversation (resource name without `.dlg`).
    pub dialog: String,
    /// The track's field 5564 (see [`CutsceneLine::vo_mode`]).
    pub vo_mode: u8,
}

/// A cutscene's lines that say a conversation's lines, by start time.
pub fn dialog_refs(g: &Gff4) -> Vec<DialogRef> {
    let Some(root) = cut_root(g) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for actor in structs(g, root, FIELD_ACTORS) {
        let name = g
            .field(actor, FIELD_ACTOR_NAME)
            .and_then(Value::as_str)
            .map(|s| s.trim_end_matches('\0').to_string())
            .unwrap_or_default();
        for track in structs(g, actor, FIELD_TRACKS) {
            if g.template_of(track).label.0 != *b"ASLN"
                || matches!(g.field(track, FIELD_LINE_TEXT), Some(Value::Tlk(t)) if t.id != u32::MAX)
            {
                continue;
            }
            let Some(dialog) = g
                .field(track, FIELD_LINE_CONVERSATION)
                .and_then(Value::as_str)
                .map(|s| s.trim_end_matches('\0').trim().to_string())
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let time = |l| {
                g.field(track, l)
                    .and_then(Value::as_f64)
                    .map(|s| s as f32)
                    .filter(|s| s.is_finite())
            };
            let (Some(start), Some(end)) = (time(FIELD_LINE_START), time(FIELD_LINE_END)) else {
                continue;
            };
            if end < start {
                continue;
            }
            let vo_mode = g
                .field(track, FIELD_LINE_VO)
                .and_then(Value::as_f64)
                .unwrap_or(0.0) as u8;
            out.push(DialogRef {
                start,
                end,
                actor: name.clone(),
                dialog,
                vo_mode,
            });
        }
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// A movie a cutscene plays full screen (an `APBM` track).
#[derive(Clone, Debug, PartialEq)]
pub struct CutsceneMovie {
    /// Seconds into the cutscene it starts, and stops (at most the cutscene's length).
    pub start: f32,
    pub end: f32,
    /// The Bink movie's name, without its extension (`game_introduction`).
    pub name: String,
}

/// The movies a cutscene's file plays, by start time (none when it is not a cutscene).
pub fn movies(data: &[u8]) -> Vec<CutsceneMovie> {
    match Gff4::parse(data) {
        Ok(g) => movies_of(&g),
        Err(_) => Vec::new(),
    }
}

/// The movies a parsed cutscene plays, by start time.
pub fn movies_of(g: &Gff4) -> Vec<CutsceneMovie> {
    let Some(root) = cut_root(g) else {
        return Vec::new();
    };
    let seconds = g
        .field(root, FIELD_SECONDS)
        .and_then(Value::as_f64)
        .map(|s| s as f32)
        .filter(|s| s.is_finite() && *s >= 0.0)
        .unwrap_or(0.0)
        .min(MAX_SECONDS);
    let mut out = Vec::new();
    for actor in structs(g, root, FIELD_ACTORS) {
        for track in structs(g, actor, FIELD_TRACKS) {
            if g.template_of(track).label.0 != *b"APBM" {
                continue;
            }
            let name = g
                .field(track, FIELD_MOVIE)
                .and_then(Value::as_str)
                .map(|s| s.trim_end_matches('\0').trim().to_string())
                .unwrap_or_default();
            let time = |l| {
                g.field(track, l)
                    .and_then(Value::as_f64)
                    .map(|s| s as f32)
                    .filter(|s| !s.is_nan())
            };
            let (Some(start), Some(end)) = (time(FIELD_LINE_START), time(FIELD_LINE_END)) else {
                continue;
            };
            if name.is_empty() || end <= start {
                continue;
            }
            out.push(CutsceneMovie {
                start: start.clamp(0.0, seconds),
                end: end.clamp(0.0, seconds),
                name,
            });
        }
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// A sound a cutscene plays (an `ACSP` track, track type 31): an FMOD event by path, from
/// the track's start until its end.
#[derive(Clone, Debug, PartialEq)]
pub struct CutsceneSound {
    /// Seconds into the cutscene it starts, and stops (at most the cutscene's length).
    pub start: f32,
    pub end: f32,
    /// The event (`game_intro/game_intro/game_intro`).
    pub event: String,
    /// The actor the track is on (a 3D sound plays from it). Provisional.
    pub actor: String,
}

/// The sounds a cutscene's file plays, by start time (none when it is not a cutscene).
/// Nothing in the simulation reads them: they are for the presentation.
pub fn sounds(data: &[u8]) -> Vec<CutsceneSound> {
    match Gff4::parse(data) {
        Ok(g) => sounds_of(&g),
        Err(_) => Vec::new(),
    }
}

/// The sounds a parsed cutscene plays, by start time.
pub fn sounds_of(g: &Gff4) -> Vec<CutsceneSound> {
    let Some(root) = cut_root(g) else {
        return Vec::new();
    };
    let seconds = g
        .field(root, FIELD_SECONDS)
        .and_then(Value::as_f64)
        .map(|s| s as f32)
        .filter(|s| s.is_finite() && *s >= 0.0)
        .unwrap_or(0.0)
        .min(MAX_SECONDS);
    let mut out = Vec::new();
    for actor in structs(g, root, FIELD_ACTORS) {
        let actor_name = g
            .field(actor, FIELD_ACTOR_NAME)
            .and_then(Value::as_str)
            .map(|s| s.trim_end_matches('\0').to_string())
            .unwrap_or_default();
        for track in structs(g, actor, FIELD_TRACKS) {
            if g.template_of(track).label.0 != *b"ACSP" {
                continue;
            }
            let event = g
                .field(track, FIELD_SOUND_EVENT)
                .and_then(Value::as_str)
                .map(|s| s.trim_end_matches('\0').trim().to_string())
                .unwrap_or_default();
            let time = |l| {
                g.field(track, l)
                    .and_then(Value::as_f64)
                    .map(|s| s as f32)
                    .filter(|s| !s.is_nan())
            };
            let (Some(start), Some(end)) = (time(FIELD_LINE_START), time(FIELD_LINE_END)) else {
                continue;
            };
            if event.is_empty() || end <= start {
                continue;
            }
            out.push(CutsceneSound {
                start: start.clamp(0.0, seconds),
                end: end.min(seconds.max(start)),
                event,
                actor: actor_name.clone(),
            });
        }
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// The structs of a list field, whether a struct list or a generic one.
/// A Dragon Age II line file's cutscene: the root `LINE` struct's field 31100.
const FIELD_LINE_CUT: u32 = 31100;

/// The cutscene struct (`CUT `) a file holds, if it is a cutscene: the root of a `.cut` file
/// (GFF4 type `CUT `), or, in Dragon Age II, the struct a `LINE` file wraps. DA2 stores both
/// its cutscenes (`.cut`) and every conversation line (`.cl`) as a `LINE` file: a root that
/// holds the same `CUT ` struct in field 31100, beside 31101 (a number), 31102 (the line's
/// talk-table string, -1 for a cutscene) and 31103. Provisional for DA2: read from the
/// shipped files, not traced in the executable.
pub fn cut_root(g: &Gff4) -> Option<StructId> {
    match &g.file_type.0 {
        b"CUT " => Some(g.root),
        b"LINE" => {
            let cut = match g.field(g.root, FIELD_LINE_CUT)? {
                Value::Struct(Some(id)) => *id,
                Value::Generic(slot) => match slot.value.as_ref()? {
                    Value::Struct(Some(id)) => *id,
                    _ => return None,
                },
                _ => return None,
            };
            (g.template_of(cut).label.0 == *b"CUT ").then_some(cut)
        }
        _ => None,
    }
}

pub(crate) fn structs(g: &Gff4, id: StructId, label: u32) -> Vec<StructId> {
    match g.field(id, label) {
        Some(Value::List(Some(List::Struct(v)))) => v.iter().flatten().copied().collect(),
        Some(Value::List(Some(List::Generic(slots)))) => slots
            .iter()
            .filter_map(|s| match &s.value {
                Some(Value::Struct(Some(id))) => Some(*id),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use thedas_formats::gff4::{
        FieldDef, FourCC, GenericSlot, StructData, StructTemplate, TlkString, Version,
    };

    use super::*;

    fn template(label: &[u8; 4], fields: &[u32]) -> StructTemplate {
        StructTemplate {
            label: FourCC::new(label),
            fields: fields
                .iter()
                .map(|&label| FieldDef {
                    label,
                    type_code: 0,
                    flags: 0,
                    offset: 0,
                })
                .collect(),
            size: 0,
        }
    }

    fn slot(id: usize) -> GenericSlot {
        GenericSlot {
            type_code: 0,
            flags: 0,
            value: Some(Value::Struct(Some(StructId(id)))),
        }
    }

    fn line(strref: u32, start: f32, end: f32) -> StructData {
        StructData {
            template: 2,
            values: vec![
                Value::Tlk(TlkString {
                    id: strref,
                    text: None,
                }),
                Value::F32(start),
                Value::F32(end),
            ],
        }
    }

    fn movie(name: &str, start: f32, end: f32) -> StructData {
        StructData {
            template: 4,
            values: vec![
                Value::String(Some(name.into())),
                Value::F32(start),
                Value::F32(end),
            ],
        }
    }

    /// A cutscene of `seconds` whose one actor says two lines (the later one listed first)
    /// and has a track that is not a line.
    fn cut(file_type: &[u8; 4], seconds: f32) -> Gff4 {
        Gff4 {
            version: Version::V40,
            platform: FourCC::new(b"PC  "),
            file_type: FourCC::new(file_type),
            file_version: FourCC::new(b"V1.0"),
            templates: vec![
                template(b"CUT ", &[FIELD_SECONDS, FIELD_ACTORS]),
                template(b"ACTR", &[FIELD_ACTOR_NAME, FIELD_TRACKS]),
                template(
                    b"ASLN",
                    &[FIELD_LINE_TEXT, FIELD_LINE_START, FIELD_LINE_END],
                ),
                template(b"TRAK", &[]),
                template(b"APBM", &[FIELD_MOVIE, FIELD_LINE_START, FIELD_LINE_END]),
                template(
                    b"ACSP",
                    &[FIELD_SOUND_EVENT, FIELD_LINE_START, FIELD_LINE_END],
                ),
            ],
            structs: vec![
                StructData {
                    template: 0,
                    values: vec![
                        Value::F32(seconds),
                        Value::List(Some(List::Struct(vec![Some(StructId(1))]))),
                    ],
                },
                StructData {
                    template: 1,
                    values: vec![
                        Value::String(Some("duncan\0".into())),
                        Value::List(Some(List::Generic(vec![
                            slot(2),
                            slot(3),
                            slot(4),
                            slot(5),
                            slot(6),
                            slot(7),
                        ]))),
                    ],
                },
                line(200, 4.0, 6.0),
                StructData {
                    template: 3,
                    values: vec![],
                },
                line(100, 0.5, 3.5),
                movie("later\0", 2.0, 1e10),
                movie("first\0", 0.0, 2.0),
                StructData {
                    template: 5,
                    values: vec![
                        Value::String(Some("game_intro/game_intro/game_intro\0".into())),
                        Value::F32(0.0),
                        Value::F32(1e10),
                    ],
                },
            ],
            root: StructId(0),
            shared_strings: None,
        }
    }

    #[test]
    fn reads_the_length_and_the_lines_said() {
        let c = Cutscene::from_gff4(&cut(b"CUT ", 10.0), "PRE100CS_Intro").unwrap();
        assert_eq!(c.name, "pre100cs_intro.cut");
        assert_eq!(c.seconds, 10.0);
        let said: Vec<(u32, f32, &str)> = c
            .lines
            .iter()
            .map(|l| (l.strref, l.start, l.actor.as_str()))
            .collect();
        assert_eq!(said, [(100, 0.5, "duncan"), (200, 4.0, "duncan")]);
        assert_eq!(c.line_at(1.0).map(|l| l.strref), Some(100));
        assert_eq!(c.line_at(3.8), None);
        assert_eq!(c.line_at(5.0).map(|l| l.strref), Some(200));
        // A runaway length is held to the cap; other files are not cutscenes.
        assert_eq!(
            Cutscene::from_gff4(&cut(b"CUT ", 1e9), "x")
                .unwrap()
                .seconds,
            MAX_SECONDS
        );
        assert!(Cutscene::from_gff4(&cut(b"CONV", 10.0), "x").is_none());
        assert!(Cutscene::from_gff4(&cut(b"CUT ", f32::NAN), "x").is_none());
        assert!(Cutscene::parse(b"junk", "x").is_none());
        assert_eq!(Cutscene::resource(" A.CUT "), "a.cut");
    }

    #[test]
    fn reads_the_movies_played() {
        let m = movies_of(&cut(b"CUT ", 10.0));
        let got: Vec<(&str, f32, f32)> = m
            .iter()
            .map(|m| (m.name.as_str(), m.start, m.end))
            .collect();
        // By start; "to the end" is held to the cutscene's length.
        assert_eq!(got, [("first", 0.0, 2.0), ("later", 2.0, 10.0)]);
        assert!(movies_of(&cut(b"CONV", 10.0)).is_empty());
        assert!(movies(b"junk").is_empty());
    }

    #[test]
    fn reads_the_sounds_played() {
        let s = sounds_of(&cut(b"CUT ", 10.0));
        assert_eq!(
            s,
            [CutsceneSound {
                start: 0.0,
                end: 10.0,
                event: "game_intro/game_intro/game_intro".into(),
                actor: "duncan".into(),
            }]
        );
        assert!(sounds_of(&cut(b"CONV", 10.0)).is_empty());
        assert!(sounds(b"junk").is_empty());
    }

    /// The same cutscene as [`cut`], wrapped as Dragon Age II wraps one: a `LINE` file whose
    /// root holds the `CUT ` struct in field 31100.
    fn line_file(seconds: f32) -> Gff4 {
        let mut g = cut(b"LINE", seconds);
        g.templates.push(template(b"LINE", &[FIELD_LINE_CUT]));
        g.structs.push(StructData {
            template: (g.templates.len() - 1) as u32,
            values: vec![Value::Struct(Some(StructId(0)))],
        });
        g.root = StructId(g.structs.len() - 1);
        g
    }

    #[test]
    fn a_dragon_age_2_line_file_reads_as_the_cutscene_it_wraps() {
        let plain = cut(b"CUT ", 10.0);
        let wrapped = line_file(10.0);
        assert_eq!(cut_root(&plain), Some(StructId(0)));
        assert_eq!(cut_root(&wrapped), Some(StructId(0)));
        assert_eq!(
            Cutscene::from_gff4(&wrapped, "x"),
            Cutscene::from_gff4(&plain, "x")
        );
        assert!(Cutscene::from_gff4(&wrapped, "x").is_some());
        assert_eq!(movies_of(&wrapped), movies_of(&plain));
        assert_eq!(sounds_of(&wrapped), sounds_of(&plain));
        assert_eq!(dialog_refs(&wrapped), dialog_refs(&plain));
        assert!(!movies_of(&wrapped).is_empty());

        // A LINE file without a cutscene struct in 31100 is not a cutscene.
        let mut empty = line_file(10.0);
        let root = empty.root.0;
        empty.structs[root].values = vec![Value::Struct(None)];
        assert_eq!(cut_root(&empty), None);
        assert!(Cutscene::from_gff4(&empty, "x").is_none());
        assert!(movies_of(&empty).is_empty());
        // Nor is a LINE file whose 31100 points at some other struct.
        let mut other = line_file(10.0);
        other.structs[root].values = vec![Value::Struct(Some(StructId(1)))];
        assert_eq!(cut_root(&other), None);
        // Any other file type is not a cutscene, as before.
        assert_eq!(cut_root(&cut(b"DLG ", 10.0)), None);
    }

    #[test]
    fn completion_script_comes_from_the_wrapped_cutscene() {
        let mut g = line_file(1.0);
        assert_eq!(end_script(&g), None);
        g.templates[0].fields.push(FieldDef {
            label: FIELD_END_SCRIPT,
            type_code: 14,
            flags: 0,
            offset: 0,
        });
        g.structs[0]
            .values
            .push(Value::String(Some(" voyage_end\0".into())));
        assert_eq!(end_script(&g).as_deref(), Some("voyage_end"));
        *g.structs[0].values.last_mut().unwrap() = Value::String(Some("\0".into()));
        assert_eq!(end_script(&g), None);
    }
}
