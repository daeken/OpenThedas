//! What an area's `.are` says about its sound: the music theme (`MusicPreset`), the
//! sounds placed in it (`SoundList`: an FMOD event at a position, its volume in dB, a
//! multiplier on its reach and whether it starts on), and its reverb (`ReverbPreset`, and
//! reverb triggers: regions with their own preset), and its music volumes (regions that
//! set the music's intensity).

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
    /// Added to the event's pitch (DAOrigins.exe's sound start 0x7c261d: +0x480, read from
    /// `Pitch` at 0x7c22b1).
    pub pitch: f32,
    /// Fade-in and fade-out times (ms) that replace the event's (0x7c2623..0x7c2637: +0x484
    /// `FadeIn`, +0x488 `FadeOut`). Most sounds fade out over 34 ms.
    pub fade_in_ms: u32,
    pub fade_out_ms: u32,
    /// The audio volume it belongs to, for a sound of a volume's own `SoundList` (see
    /// [`SoundRegion`]); `None` for the area's.
    pub region: Option<SoundRegion>,
}

/// The outline of an audio volume (`TriggerList` `Type` 4, CCTriggerAudioVolume) whose
/// own sounds follow the listener (DAOrigins.exe's sound update 0x7c2ee0 for a sound with
/// an owning volume at +0x4b8).
///
/// The volume's loader (0x79a000) makes each `SoundList` entry an area sound placed at the
/// volume's position (0x695540 with the volume's +0x120); adding the volume to the area
/// adds them with the area's own (0x79b7d0 -> 0x7c3110). Each frame such a sound, when the
/// listener is within the volume's radius (its farthest outline point plus 4 m, 0x79a490)
/// of the event's reach, is put at the listener while the party is inside the volume, and
/// otherwise at the point of the volume nearest the listener at the listener's height,
/// re-placed every 0.06 s once the listener has moved 0.25 m (0x7c2ffc..0x7c30fc).
///
/// Provisional: the nearest point is taken on the outline (the exe's 0x798de0 is not
/// traced past its bounds test), and inside is the party leader in the outline.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundRegion {
    pub position: [f32; 3],
    /// Rotation about Z, radians.
    pub yaw: f32,
    /// The outline, relative to `position` before rotation.
    pub outline: Vec<[f32; 2]>,
}

impl SoundRegion {
    /// Whether `p` is inside the outline (on the ground plane).
    pub fn contains(&self, p: [f32; 3]) -> bool {
        outline_contains(self.position, self.yaw, &self.outline, p)
    }

    /// The point of the outline nearest `p` on the ground plane, at `p`'s height.
    pub fn nearest(&self, p: [f32; 3]) -> [f32; 3] {
        if self.outline.is_empty() {
            return [self.position[0], self.position[1], p[2]];
        }
        let (dx, dy) = (p[0] - self.position[0], p[1] - self.position[1]);
        let (s, c) = (-self.yaw).sin_cos();
        let (x, y) = (dx * c - dy * s, dx * s + dy * c);
        let n = self.outline.len();
        let mut best = (f32::MAX, self.outline[0]);
        for i in 0..n {
            let [ax, ay] = self.outline[i];
            let [bx, by] = self.outline[(i + 1) % n];
            let (ex, ey) = (bx - ax, by - ay);
            let len2 = ex * ex + ey * ey;
            let t = if len2 > 0.0 {
                (((x - ax) * ex + (y - ay) * ey) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let q = [ax + ex * t, ay + ey * t];
            let d2 = (q[0] - x).powi(2) + (q[1] - y).powi(2);
            if d2 < best.0 {
                best = (d2, q);
            }
        }
        let [qx, qy] = best.1;
        let (s, c) = self.yaw.sin_cos();
        [
            self.position[0] + qx * c - qy * s,
            self.position[1] + qx * s + qy * c,
            p[2],
        ]
    }

    /// The volume's radius: its farthest outline point from its position, plus 4 m
    /// (DAOrigins.exe 0x79a490, +0x534 = 4.0 at 0xadef1c).
    pub fn radius(&self) -> f32 {
        self.outline
            .iter()
            .map(|[x, y]| (x * x + y * y).sqrt())
            .fold(0.0, f32::max)
            + 4.0
    }
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
    let pos = |s: &Struct| {
        [
            num(s, "XPosition").unwrap_or(0.0),
            num(s, "YPosition").unwrap_or(0.0),
            num(s, "ZPosition").unwrap_or(0.0),
        ]
    };
    let mut add = |list: Option<&Value>, region: Option<SoundRegion>| {
        let Some(Value::List(items)) = list else {
            return;
        };
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
                // A volume's sounds stand at the volume (0x79a114 -> 0x695540).
                position: region.as_ref().map(|r| r.position).unwrap_or(pos(s)),
                volume: num(s, "Volume").unwrap_or(0.0),
                max_distance_mult: num(s, "MaxDistanceMult").unwrap_or(1.0),
                active: num(s, "Active").unwrap_or(1.0) != 0.0,
                pitch: num(s, "Pitch").unwrap_or(0.0),
                fade_in_ms: num(s, "FadeIn").unwrap_or(0.0).max(0.0) as u32,
                fade_out_ms: num(s, "FadeOut").unwrap_or(0.0).max(0.0) as u32,
                region: region.clone(),
            });
        }
    };
    add(get(root, "SoundList"), None);
    // Audio volumes' own sounds (0x79a5d0 -> 0x79a000).
    if let Some(Value::List(triggers)) = get(root, "TriggerList") {
        for &t in triggers {
            let Some(t) = g.structs.get(t as usize) else {
                continue;
            };
            if num(t, "Type") != Some(4.0) {
                continue;
            }
            let mut outline = Vec::new();
            if let Some(Value::List(points)) = get(t, "Geometry") {
                for &p in points {
                    if let Some(p) = g.structs.get(p as usize) {
                        outline.push([
                            num(p, "XPoint").unwrap_or(0.0),
                            num(p, "YPoint").unwrap_or(0.0),
                        ]);
                    }
                }
            }
            let yaw = 2.0
                * num(t, "ZOrientation")
                    .unwrap_or(0.0)
                    .atan2(num(t, "WOrientation").unwrap_or(1.0));
            let region = SoundRegion {
                position: pos(t),
                yaw,
                outline,
            };
            add(get(t, "SoundList"), Some(region));
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
    /// An audio volume (`Type` 4) rather than a plain reverb trigger (3).
    pub volume: bool,
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
        outline_contains(self.position, self.yaw, &self.outline, p)
    }
}

/// Whether `p` is inside a trigger's `outline` (relative to `position`, rotated by `yaw`
/// about Z), on the ground plane.
fn outline_contains(position: [f32; 3], yaw: f32, outline: &[[f32; 2]], p: [f32; 3]) -> bool {
    if outline.len() < 3 {
        return false;
    }
    let (dx, dy) = (p[0] - position[0], p[1] - position[1]);
    let (s, c) = (-yaw).sin_cos();
    let (x, y) = (dx * c - dy * s, dx * s + dy * c);
    let mut inside = false;
    let n = outline.len();
    for i in 0..n {
        let [xi, yi] = outline[i];
        let [xj, yj] = outline[(i + n - 1) % n];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
    }
    inside
}

/// A music volume: a `TriggerList` entry of `Type` 5 (DAOrigins.exe's
/// CCTriggerAudioMusicVolume; its loader 0x799ad0 reads `EnterState`, `ExitState`,
/// `EnterStateDelay` and `ExitStateDelay`, the delays in milliseconds scaled by 0.001 to
/// seconds).
///
/// Entering it asks for the music intensity `state` after `delay` seconds (the enter
/// handler 0x799380 -> 0x5b6e80 -> 0x5b6cd0 stores both on the music director, replacing
/// any request still waiting). Leaving does nothing: the handler ignores it, so
/// `ExitState` and `ExitStateDelay` are read but unused.
#[derive(Clone, Debug, PartialEq)]
pub struct MusicTrigger {
    pub state: i32,
    /// Seconds.
    pub delay: f32,
    pub position: [f32; 3],
    /// Rotation about Z, radians.
    pub yaw: f32,
    /// The outline, relative to `position` before rotation.
    pub outline: Vec<[f32; 2]>,
}

impl MusicTrigger {
    /// Whether `p` is inside the outline (on the ground plane).
    pub fn contains(&self, p: [f32; 3]) -> bool {
        outline_contains(self.position, self.yaw, &self.outline, p)
    }
}

/// An area's reverb: its own preset (`ReverbPreset`, empty for none) and its reverb
/// triggers; and its music volumes, which are triggers of the same kind.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AreaReverb {
    pub preset: String,
    pub triggers: Vec<ReverbTrigger>,
    pub music: Vec<MusicTrigger>,
}

impl AreaReverb {
    /// The preset the area starts with (DAOrigins.exe 0x6866f8..0x686a32): its own if it
    /// has one, else the preset the most reverb triggers use (ties: the first by name in the
    /// exe's map order, a case-sensitive compare of UTF-16 code units (0x413800), which is
    /// byte order for these ASCII names; triggers with no preset are not counted, 0x6867b6),
    /// else none.
    ///
    /// A reverb trigger counts 1 and an audio volume 0.15 (0x6867d1..0x6867e7: the
    /// trigger's +0x1d8 virtual, its cast to an audio volume, 0x79af30 for type 4 and null,
    /// 0x4137b0, for type 3).
    /// The sums are `f32`, added in trigger order as the exe adds them (each sum stored back
    /// to a float, 0x686811..0x68682b), so near-ties such as twenty volumes (`0.15 * 20`)
    /// against three triggers round the same way; an exact tie goes to the first by name.
    ///
    /// Provisional: the exe also loads the next two most-used presets into secondary reverb
    /// instances, which only events that name a preset use; those are not modelled.
    pub fn initial(&self) -> &str {
        if !self.preset.is_empty() {
            return &self.preset;
        }
        let mut counts: std::collections::BTreeMap<&str, f32> = Default::default();
        for t in self.triggers.iter().filter(|t| !t.preset.is_empty()) {
            *counts.entry(t.preset.as_str()).or_default() += if t.volume { 0.15 } else { 1.0 };
        }
        let mut best: Option<(&str, f32)> = None;
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
    /// `p`: the preset of a trigger it has just entered, as entering a reverb trigger sets
    /// its preset (0x79b3d0 -> 0x5b7790 -> 0x5c35f0, with the default fade). Leaving a
    /// trigger changes nothing: the reverb stays until another is entered. `Priority` is
    /// read (0x799a5f) but nothing uses it.
    ///
    /// Provisional: that the party leader's entering is what counts (the caller of the
    /// enter handler is not traced), and that of several triggers entered in one step the
    /// last listed wins.
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

    /// The music request as the party leader moves from where `was` says it was to `p`:
    /// the intensity and delay of a music volume it has just entered (see
    /// [`MusicTrigger`]), and which volumes it is inside.
    ///
    /// Provisional, as for [`AreaReverb::entered`]: that the party leader's entering is
    /// what counts, and that of several entered in one step the last listed wins.
    pub fn music_entered(&self, was: &[bool], p: [f32; 3]) -> (Vec<bool>, Option<(i32, f32)>) {
        let now: Vec<bool> = self.music.iter().map(|t| t.contains(p)).collect();
        let mut request = None;
        for (i, t) in self.music.iter().enumerate() {
            if now[i] && !was.get(i).copied().unwrap_or(false) {
                request = Some((t.state, t.delay));
            }
        }
        (now, request)
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
        music: Vec::new(),
    };
    if let Some(Value::List(items)) = get(root, "TriggerList") {
        for &i in items {
            let Some(t) = g.structs.get(i as usize) else {
                continue;
            };
            let kind = num(t, "Type");
            let mut outline = Vec::new();
            if let Some(Value::List(points)) = get(t, "Geometry") {
                for &p in points {
                    if let Some(p) = g.structs.get(p as usize) {
                        outline.push([num(p, "XPoint"), num(p, "YPoint")]);
                    }
                }
            }
            let position = [
                num(t, "XPosition"),
                num(t, "YPosition"),
                num(t, "ZPosition"),
            ];
            let yaw = 2.0 * num(t, "ZOrientation").atan2(num(t, "WOrientation"));
            if kind == 5.0 {
                out.music.push(MusicTrigger {
                    state: num(t, "EnterState") as i32,
                    // The loader's `fild` and `fmul` by the double 0.001 (0xae7118), stored as a
                    // float.
                    delay: (num(t, "EnterStateDelay") as f64 * 0.0010000000474974513) as f32,
                    position,
                    yaw,
                    outline,
                });
                continue;
            }
            if get(t, "ReverbPreset").is_none() || (kind != 3.0 && kind != 4.0) {
                continue;
            }
            out.triggers.push(ReverbTrigger {
                preset: text(t, "ReverbPreset"),
                volume: kind == 4.0,
                priority: num(t, "Priority") as i32,
                position,
                yaw,
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
        let pt = |x: f32, y: f32| Struct {
            id: 3,
            fields: vec![f("XPoint", Value::Float(x)), f("YPoint", Value::Float(y))],
        };
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
                        f("TriggerList", Value::List(vec![2])),
                    ],
                },
                Struct {
                    id: 6,
                    fields: vec![
                        f("Active", Value::Byte(0)),
                        f("FadeOut", Value::Dword(34)),
                        f("MaxDistanceMult", Value::Float(0.5)),
                        f("Pitch", Value::Float(-0.05)),
                        f("Sound", Value::ExoString(b"kw/3D/amb_crow".to_vec())),
                        f("Tag", Value::ExoString(b"Amb_Crow".to_vec())),
                        f("Volume", Value::Float(-4.0)),
                        f("XPosition", Value::Float(1.0)),
                        f("YPosition", Value::Float(2.0)),
                        f("ZPosition", Value::Float(3.0)),
                    ],
                },
                // An audio volume: a 4 m square at (10, 0), turned 90 degrees, with a sound.
                Struct {
                    id: 1,
                    fields: vec![
                        f("Type", Value::Byte(4)),
                        f("Geometry", Value::List(vec![3, 4, 5, 6])),
                        f("SoundList", Value::List(vec![7])),
                        f("XPosition", Value::Float(10.0)),
                        f("YPosition", Value::Float(0.0)),
                        f("ZPosition", Value::Float(1.0)),
                        f(
                            "ZOrientation",
                            Value::Float(std::f32::consts::FRAC_1_SQRT_2),
                        ),
                        f(
                            "WOrientation",
                            Value::Float(std::f32::consts::FRAC_1_SQRT_2),
                        ),
                    ],
                },
                pt(-2.0, -2.0),
                pt(2.0, -2.0),
                pt(2.0, 2.0),
                pt(-2.0, 2.0),
                Struct {
                    id: 6,
                    fields: vec![
                        f("Sound", Value::ExoString(b"t/int/amb_rumble_lp".to_vec())),
                        f("Tag", Value::ExoString(b"rumble".to_vec())),
                        f("Volume", Value::Float(-6.0)),
                        f("XPosition", Value::Float(99.0)),
                    ],
                },
            ],
        };
        let (music, sounds) = area_audio(&g.write().unwrap());
        assert_eq!(music.as_deref(), Some("music/Prelude_kwilds"));
        assert_eq!(
            sounds[0],
            AreaSound {
                tag: "amb_crow".into(),
                event: "kw/3D/amb_crow".into(),
                position: [1.0, 2.0, 3.0],
                volume: -4.0,
                max_distance_mult: 0.5,
                active: false,
                pitch: -0.05,
                fade_in_ms: 0,
                fade_out_ms: 34,
                region: None,
            }
        );
        // The volume's sound stands at the volume and follows the listener.
        assert_eq!(sounds.len(), 2);
        let v = &sounds[1];
        assert_eq!(
            (v.tag.as_str(), v.position, v.volume),
            ("rumble", [10.0, 0.0, 1.0], -6.0)
        );
        let r = v.region.as_ref().unwrap();
        assert!(r.contains([11.0, 1.0, 0.0]));
        assert!(!r.contains([13.0, 0.0, 0.0]));
        let n = r.nearest([20.0, 1.0, 5.0]);
        assert!(
            (n[0] - 12.0).abs() < 1e-4 && (n[1] - 1.0).abs() < 1e-4 && n[2] == 5.0,
            "{n:?}"
        );
        assert!((r.radius() - (8f32.sqrt() + 4.0)).abs() < 1e-4);
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
        let mut g = g;
        g.structs[7].fields.push(f("EnterState", Value::Int(4)));
        g.structs[7]
            .fields
            .push(f("EnterStateDelay", Value::Int(2000)));
        g.structs[7].fields.push(f("ExitState", Value::Int(1)));
        let r = area_reverb(&g.write().unwrap());
        // The music volume asks for its state on entering, after its delay in seconds;
        // leaving asks for nothing.
        assert_eq!(r.music.len(), 1);
        assert_eq!((r.music[0].state, r.music[0].delay), (4, 2.0));
        let (was, req) = r.music_entered(&[], [39.0, 1.0, 0.0]);
        assert_eq!(req, Some((4, 2.0)));
        let (was, req) = r.music_entered(&was, [40.0, 0.0, 0.0]);
        assert_eq!(req, None);
        let (was, req) = r.music_entered(&was, [0.0, 0.0, 0.0]);
        assert_eq!((was, req), (vec![false], None));
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
        // Audio volumes count 0.15 each: six of them (0.9) lose to one reverb trigger.
        for _ in 0..5 {
            r2.triggers.push(r.triggers[1].clone());
        }
        assert_eq!(r2.initial(), "Cave");
        r2.triggers.push(r.triggers[1].clone());
        assert_eq!(r2.initial(), "Hallway");
        assert_eq!(area_reverb(b"junk"), AreaReverb::default());
    }
}
