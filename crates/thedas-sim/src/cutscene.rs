//! Cutscenes scripts load with `LoadCutscene`: `.cut` files (GFF4 `CUT `). The simulation
//! needs their length and the lines said in them, which exist nowhere else: a scripted
//! cutscene's speech is not in any conversation the player walks through.
//!
//! Provisional, from the files' contents: the root's field 5000 is the length in seconds and
//! field 5200 lists the actors (`ACTR`, name in field 2), each with a generic list of tracks
//! (field 5206). A spoken line is an `ASLN` track: its text (field 22, a talk-table
//! reference), when it starts and ends (fields 5301 and 5302, seconds into the cutscene) and
//! the conversation it comes from (field 5562).

use thedas_formats::Gff4;
use thedas_formats::gff4::{List, StructId, Value};

const FIELD_SECONDS: u32 = 5000;
const FIELD_ACTORS: u32 = 5200;
const FIELD_ACTOR_NAME: u32 = 2;
const FIELD_TRACKS: u32 = 5206;
const FIELD_LINE_TEXT: u32 = 22;
const FIELD_LINE_START: u32 = 5301;
const FIELD_LINE_END: u32 = 5302;

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
}

/// A cutscene playing: the presentation shows it and then finishes it
/// ([`crate::sim::Sim::finish_cutscene`]).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Cutscene {
    /// The resource, e.g. `pre100cs_intro.cut`.
    pub name: String,
    pub seconds: f32,
    /// By start time.
    pub lines: Vec<CutsceneLine>,
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
        if g.file_type.0 != *b"CUT " {
            return None;
        }
        let seconds = g
            .field(g.root, FIELD_SECONDS)
            .and_then(Value::as_f64)
            .map(|s| s as f32)
            .filter(|s| s.is_finite() && *s >= 0.0)?
            .min(MAX_SECONDS);
        let mut lines = Vec::new();
        for actor in structs(g, g.root, FIELD_ACTORS) {
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
                lines.push(CutsceneLine {
                    start: start.clamp(0.0, seconds),
                    end: end.clamp(0.0, seconds),
                    strref: t.id,
                    actor: name.clone(),
                });
            }
        }
        lines.sort_by(|a, b| a.start.total_cmp(&b.start));
        Some(Self {
            name: Self::resource(name),
            seconds,
            lines,
        })
    }

    /// The line showing `seconds` into the cutscene, if any.
    pub fn line_at(&self, seconds: f32) -> Option<&CutsceneLine> {
        self.lines
            .iter()
            .rev()
            .find(|l| l.start <= seconds && seconds < l.end)
    }
}

/// The structs of a list field, whether a struct list or a generic one.
fn structs(g: &Gff4, id: StructId, label: u32) -> Vec<StructId> {
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
                        Value::List(Some(List::Generic(vec![slot(2), slot(3), slot(4)]))),
                    ],
                },
                line(200, 4.0, 6.0),
                StructData {
                    template: 3,
                    values: vec![],
                },
                line(100, 0.5, 3.5),
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
}
