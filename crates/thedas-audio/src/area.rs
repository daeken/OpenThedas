//! What an area's `.are` says about its sound: the music theme (`MusicPreset`), the
//! sounds placed in it (`SoundList`: an FMOD event at a position, its volume in dB, a
//! multiplier on its reach and whether it starts on), and its reverb (`ReverbPreset`, and
//! reverb triggers: regions with their own preset).

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

/// A reverb trigger (`TriggerList` entry with a `ReverbPreset`, `Type` 3; DAOrigins.exe's
/// CCTriggerAudioReverb reads `ReverbPreset` and `Priority`, 0x7999f8..0x799a5f): inside
/// its outline the reverb is its preset.
#[derive(Clone, Debug, PartialEq)]
pub struct ReverbTrigger {
    pub preset: String,
    pub priority: i32,
    pub position: [f32; 3],
    /// Rotation about Z, radians.
    pub yaw: f32,
    /// The outline, relative to `position` before rotation.
    pub outline: Vec<[f32; 2]>,
}

impl ReverbTrigger {
    /// Whether `p` is inside the outline (on the ground plane).
    pub fn contains(&self, p: [f32; 3]) -> bool {
        if self.outline.len() < 3 {
            return false;
        }
        let (dx, dy) = (p[0] - self.position[0], p[1] - self.position[1]);
        let (s, c) = (-self.yaw).sin_cos();
        let (x, y) = (dx * c - dy * s, dx * s + dy * c);
        let mut inside = false;
        let n = self.outline.len();
        for i in 0..n {
            let [xi, yi] = self.outline[i];
            let [xj, yj] = self.outline[(i + n - 1) % n];
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                inside = !inside;
            }
        }
        inside
    }
}

/// An area's reverb: its own preset (`ReverbPreset`, empty for none) and its reverb
/// triggers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AreaReverb {
    pub preset: String,
    pub triggers: Vec<ReverbTrigger>,
}

impl AreaReverb {
    /// The preset at `p`: the highest-priority trigger containing it (the first listed on a
    /// tie), else the area's. Provisional: how DAOrigins.exe weighs overlapping triggers
    /// (it sets the preset on entering one, 0x5c35f0) is not traced.
    pub fn preset_at(&self, p: [f32; 3]) -> &str {
        let mut best: Option<&ReverbTrigger> = None;
        for t in &self.triggers {
            if t.contains(p) && best.is_none_or(|b| t.priority > b.priority) {
                best = Some(t);
            }
        }
        best.map_or(self.preset.as_str(), |t| t.preset.as_str())
    }
}

/// The area's reverb, from its `.are`.
pub fn area_reverb(data: &[u8]) -> AreaReverb {
    use thedas_formats::gff3::{Gff3, Struct, Value};
    let Ok(g) = Gff3::parse(data) else {
        return AreaReverb::default();
    };
    fn get<'a>(s: &'a Struct, l: &str) -> Option<&'a Value> {
        s.fields.iter().find(|f| f.label == l).map(|f| &f.value)
    }
    let text = |s: &Struct, l: &str| match get(s, l) {
        Some(Value::ExoString(b) | Value::ResRef(b)) => String::from_utf8_lossy(b)
            .trim_end_matches('\0')
            .trim()
            .to_string(),
        _ => String::new(),
    };
    let num = |s: &Struct, l: &str| match get(s, l) {
        Some(Value::Float(v)) => *v,
        Some(Value::Byte(v)) => *v as f32,
        Some(Value::Int(v)) => *v as f32,
        Some(Value::Dword(v)) => *v as f32,
        Some(Value::Word(v)) => *v as f32,
        _ => 0.0,
    };
    let root = g.root();
    let mut out = AreaReverb {
        preset: text(root, "ReverbPreset"),
        triggers: Vec::new(),
    };
    if let Some(Value::List(items)) = get(root, "TriggerList") {
        for &i in items {
            let Some(t) = g.structs.get(i as usize) else {
                continue;
            };
            let preset = text(t, "ReverbPreset");
            if get(t, "ReverbPreset").is_none() || num(t, "Type") != 3.0 {
                continue;
            }
            let mut outline = Vec::new();
            if let Some(Value::List(points)) = get(t, "Geometry") {
                for &p in points {
                    if let Some(p) = g.structs.get(p as usize) {
                        outline.push([num(p, "XPoint"), num(p, "YPoint")]);
                    }
                }
            }
            out.triggers.push(ReverbTrigger {
                preset,
                priority: num(t, "Priority") as i32,
                position: [
                    num(t, "XPosition"),
                    num(t, "YPosition"),
                    num(t, "ZPosition"),
                ],
                yaw: 2.0 * num(t, "ZOrientation").atan2(num(t, "WOrientation")),
                outline,
            });
        }
    }
    out
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

    #[test]
    fn reads_the_reverb_and_its_triggers() {
        let point = |x: f32, y: f32| Struct {
            id: 3,
            fields: vec![
                f("XPoint", Value::Float(x)),
                f("YPoint", Value::Float(y)),
                f("ZPoint", Value::Float(0.0)),
            ],
        };
        let trigger = |preset: &str, priority: u8, x: f32, points: Vec<u32>| Struct {
            id: 1,
            fields: vec![
                f("Geometry", Value::List(points)),
                f("Priority", Value::Byte(priority)),
                f("ReverbPreset", Value::ExoString(preset.as_bytes().to_vec())),
                f("Type", Value::Byte(3)),
                f("XPosition", Value::Float(x)),
                f("YPosition", Value::Float(0.0)),
                f("ZPosition", Value::Float(0.0)),
                f("ZOrientation", Value::Float(0.0)),
                f("WOrientation", Value::Float(1.0)),
            ],
        };
        let g = Gff3 {
            file_type: *b"ARE ",
            file_version: *b"V3.2",
            structs: vec![
                Struct {
                    id: u32::MAX,
                    fields: vec![
                        f("ReverbPreset", Value::ExoString(b"Forest".to_vec())),
                        f("TriggerList", Value::List(vec![1, 2])),
                    ],
                },
                trigger("Cave", 128, 10.0, vec![3, 4, 5, 6]),
                trigger("Hallway", 200, 12.0, vec![3, 4, 5, 6]),
                point(-2.0, -2.0),
                point(2.0, -2.0),
                point(2.0, 2.0),
                point(-2.0, 2.0),
            ],
        };
        let r = area_reverb(&g.write().unwrap());
        assert_eq!(r.preset, "Forest");
        assert_eq!(r.triggers.len(), 2);
        assert_eq!(r.preset_at([0.0, 0.0, 0.0]), "Forest");
        assert_eq!(r.preset_at([9.0, 1.0, 0.0]), "Cave");
        // Where both cover, the higher priority wins.
        assert_eq!(r.preset_at([11.0, 0.0, 0.0]), "Hallway");
        assert_eq!(area_reverb(b"junk"), AreaReverb::default());
    }
}
