//! What an area's `.are` says about its sound: the music theme (`MusicPreset`) and the
//! sounds placed in it (`SoundList`: an FMOD event at a position, its volume in dB, a
//! multiplier on its reach and whether it starts on).

/// A sound placed in an area (`SoundList`): an FMOD event at a position.
#[derive(Clone, Debug, PartialEq)]
pub struct AreaSound {
    pub tag: String,
    pub event: String,
    pub position: [f32; 3],
    /// dB.
    pub volume: f32,
    pub max_distance_mult: f32,
    pub active: bool,
}

/// An area's music theme (`MusicPreset`, `music\\Prelude_kwilds`, as an event group path
/// `music/Prelude_kwilds`) and placed sounds, from its `.are`.
pub fn area_audio(data: &[u8]) -> (Option<String>, Vec<AreaSound>) {
    use thedas_formats::gff3::{Gff3, Struct, Value};
    let Ok(g) = Gff3::parse(data) else {
        return (None, Vec::new());
    };
    fn get<'a>(s: &'a Struct, l: &str) -> Option<&'a Value> {
        s.fields.iter().find(|f| f.label == l).map(|f| &f.value)
    }
    let text = |s: &Struct, l: &str| match get(s, l) {
        Some(Value::ExoString(b) | Value::ResRef(b)) => {
            let t = String::from_utf8_lossy(b)
                .trim_end_matches('\0')
                .trim()
                .to_string();
            (!t.is_empty()).then_some(t)
        }
        _ => None,
    };
    let num = |s: &Struct, l: &str| match get(s, l) {
        Some(Value::Float(v)) => Some(*v),
        Some(Value::Byte(v)) => Some(*v as f32),
        Some(Value::Int(v)) => Some(*v as f32),
        Some(Value::Dword(v)) => Some(*v as f32),
        Some(Value::Word(v)) => Some(*v as f32),
        _ => None,
    };
    let root = g.root();
    let music = text(root, "MusicPreset").map(|m| m.replace('\\', "/"));
    let mut sounds = Vec::new();
    if let Some(Value::List(items)) = get(root, "SoundList") {
        for &i in items {
            let Some(s) = g.structs.get(i as usize) else {
                continue;
            };
            let Some(event) = text(s, "Sound") else {
                continue;
            };
            sounds.push(AreaSound {
                tag: text(s, "Tag").unwrap_or_default().to_ascii_lowercase(),
                event,
                position: [
                    num(s, "XPosition").unwrap_or(0.0),
                    num(s, "YPosition").unwrap_or(0.0),
                    num(s, "ZPosition").unwrap_or(0.0),
                ],
                volume: num(s, "Volume").unwrap_or(0.0),
                max_distance_mult: num(s, "MaxDistanceMult").unwrap_or(1.0),
                active: num(s, "Active").unwrap_or(1.0) != 0.0,
            });
        }
    }
    (music, sounds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use thedas_formats::gff3::{Field, Gff3, Struct, Value};

    fn f(label: &str, value: Value) -> Field {
        Field {
            label: label.into(),
            value,
        }
    }

    #[test]
    fn reads_the_music_theme_and_placed_sounds() {
        let g = Gff3 {
            file_type: *b"ARE ",
            file_version: *b"V3.2",
            structs: vec![
                Struct {
                    id: u32::MAX,
                    fields: vec![
                        f(
                            "MusicPreset",
                            Value::ExoString(b"music\\Prelude_kwilds".to_vec()),
                        ),
                        f("SoundList", Value::List(vec![1])),
                    ],
                },
                Struct {
                    id: 6,
                    fields: vec![
                        f("Active", Value::Byte(0)),
                        f("MaxDistanceMult", Value::Float(0.5)),
                        f("Sound", Value::ExoString(b"kw/3D/amb_crow".to_vec())),
                        f("Tag", Value::ExoString(b"Amb_Crow".to_vec())),
                        f("Volume", Value::Float(-4.0)),
                        f("XPosition", Value::Float(1.0)),
                        f("YPosition", Value::Float(2.0)),
                        f("ZPosition", Value::Float(3.0)),
                    ],
                },
            ],
        };
        let (music, sounds) = area_audio(&g.write().unwrap());
        assert_eq!(music.as_deref(), Some("music/Prelude_kwilds"));
        assert_eq!(
            sounds,
            [AreaSound {
                tag: "amb_crow".into(),
                event: "kw/3D/amb_crow".into(),
                position: [1.0, 2.0, 3.0],
                volume: -4.0,
                max_distance_mult: 0.5,
                active: false,
            }]
        );
        assert_eq!(area_audio(b"junk"), (None, Vec::new()));
    }
}
