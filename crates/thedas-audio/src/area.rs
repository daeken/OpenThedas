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

/// A reverb trigger: a `TriggerList` entry of `Type` 3 (DAOrigins.exe's
/// CCTriggerAudioReverb, which reads `ReverbPreset` and `Priority`, 0x7999f8..0x799a5f) or 4
/// (CCTriggerAudioVolume, a subclass: its loader 0x79a5d0 runs the reverb one first, and
/// both share the enter handler 0x79b3d0; the trigger factory 0x67a6b4 maps the types).
/// Entering its outline sets its preset, an empty one turning the reverb off.
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
    /// The preset the area starts with (DAOrigins.exe 0x6866f8..0x686a32): its own if it
    /// has one, else the preset the most reverb triggers use (ties: the first by name, the
    /// exe's map order; triggers with no preset are not counted, 0x6867b6), else none.
    ///
    /// Provisional: the exe counts a trigger for which a virtual (+0x1d8) holds as 0.15
    /// instead of 1; that check is not traced, so every trigger counts 1. It also loads the
    /// next two most-used presets into secondary reverb instances, which only events that
    /// name a preset use; those are not modelled.
    pub fn initial(&self) -> &str {
        if !self.preset.is_empty() {
            return &self.preset;
        }
        let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
        for t in self.triggers.iter().filter(|t| !t.preset.is_empty()) {
            *counts.entry(t.preset.as_str()).or_default() += 1;
        }
        let mut best: Option<(&str, usize)> = None;
        for (name, n) in counts {
            if best.is_none_or(|b| n > b.1) {
                best = Some((name, n));
            }
        }
        best.map_or("", |b| b.0)
    }

    /// The triggers `p` is inside, by index.
    pub fn inside(&self, p: [f32; 3]) -> Vec<bool> {
        self.triggers.iter().map(|t| t.contains(p)).collect()
    }

    /// The preset to switch to as the party leader moves from where `was` says it was to
    /// `p`: the preset of a trigger it has just entered (the last listed, if several), as
    /// entering a reverb trigger sets its preset (0x79b3d0 -> 0x5b7790 -> 0x5c35f0, with the
    /// default fade). Leaving a trigger changes nothing: the reverb stays until another is
    /// entered. `Priority` is read (0x799a5f) but nothing uses it.
    pub fn entered(&self, was: &[bool], p: [f32; 3]) -> (Vec<bool>, Option<&str>) {
        let now = self.inside(p);
        let mut preset = None;
        for (i, t) in self.triggers.iter().enumerate() {
            if now[i] && !was.get(i).copied().unwrap_or(false) {
                preset = Some(t.preset.as_str());
            }
        }
        (now, preset)
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
            let kind = num(t, "Type");
            if get(t, "ReverbPreset").is_none() || (kind != 3.0 && kind != 4.0) {
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
        let trigger = |preset: &str, kind: u8, x: f32, points: Vec<u32>| Struct {
            id: 1,
            fields: vec![
                f("Geometry", Value::List(points)),
                f("Priority", Value::Byte(128)),
                f("ReverbPreset", Value::ExoString(preset.as_bytes().to_vec())),
                f("Type", Value::Byte(kind)),
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
                        f("TriggerList", Value::List(vec![1, 2, 7, 8])),
                    ],
                },
                trigger("Cave", 3, 10.0, vec![3, 4, 5, 6]),
                // An audio volume (type 4) is a reverb trigger too.
                trigger("Hallway", 4, 12.0, vec![3, 4, 5, 6]),
                point(-2.0, -2.0),
                point(2.0, -2.0),
                point(2.0, 2.0),
                point(-2.0, 2.0),
                // A music volume (type 5) is not; an empty preset is kept (it turns the
                // reverb off) but not counted for the starting preset.
                trigger("Arena", 5, 40.0, vec![3, 4, 5, 6]),
                trigger("", 4, 50.0, vec![3, 4, 5, 6]),
            ],
        };
        let r = area_reverb(&g.write().unwrap());
        assert_eq!(r.preset, "Forest");
        assert_eq!(r.triggers.len(), 3);
        let (_, set) = r.entered(&[], [50.0, 0.0, 0.0]);
        assert_eq!(set, Some(""));
        assert_eq!(r.initial(), "Forest");
        // Entering a trigger sets its preset; where two cover, the last listed entered.
        let (was, set) = r.entered(&[], [9.0, 1.0, 0.0]);
        assert_eq!(set, Some("Cave"));
        let (was, set) = r.entered(&was, [11.0, 0.0, 0.0]);
        assert_eq!(set, Some("Hallway"));
        // Leaving changes nothing.
        let (_, set) = r.entered(&was, [0.0, 0.0, 0.0]);
        assert_eq!(set, None);
        // Without its own preset an area starts with its triggers' most used.
        let mut r2 = r.clone();
        r2.preset.clear();
        r2.triggers.pop();
        r2.triggers.push(ReverbTrigger {
            preset: "Hallway".into(),
            ..r.triggers[0].clone()
        });
        assert_eq!(r2.initial(), "Hallway");
        r2.triggers.pop();
        assert_eq!(r2.initial(), "Cave");
        assert_eq!(area_reverb(b"junk"), AreaReverb::default());
    }
}
