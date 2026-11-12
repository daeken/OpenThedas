//! FMOD Designer event files (`.fev`, `FEV1`, version `0x3a0000`): the events the game plays
//! by path (`music/_menu/main`, `ss_alistair/VO/VOconversation`), what each event plays (layers
//! of sound instances placed on a parameter, shaped by envelopes) and the sound definitions
//! that name the waveforms in the FSB banks ([`crate::fsb`]).
//!
//! Layout, in file order: a header (`FEV1`, version, two sizes), the project name, the wave
//! banks the project uses, the category tree, the event groups (nested; each holds events),
//! sound-definition templates, sound definitions (each a list of waveforms: a bank sample, an
//! oscillator, silence or a programmer-supplied sound), reverb definitions and a music-system
//! chunk (empty in DA:O: its music is ordinary events switched by a `music_state` parameter).
//!
//! Every field and its order was read off `fmod_event.dll`'s loader as DA:O ships it
//! (`EventSystemI::load` 0x10025ba0, event groups 0x1000bca0, complex events 0x10019070,
//! simple events 0x1001ad20, sound instances 0x10020f30, envelopes 0x10017840, sound
//! definition templates 0x10022ef0, sound definitions 0x10026aa1), for this one version; every
//! one of the install's 9,020 event files parses to its end. Field *meanings* the loader does
//! not reveal (it only stores them) carry the name of the member offset they land in (`f28`)
//! and are provisional.

use thiserror::Error;

/// The only version DA:O ships.
pub const VERSION: u32 = 0x003a_0000;

#[derive(Debug, Error)]
pub enum FevError {
    #[error("not an FEV1 file")]
    Magic,
    #[error("FEV version {0:#x} is not supported (only {VERSION:#x})")]
    Version(u32),
    #[error("FEV truncated at {at:#x} reading {what}")]
    Truncated { at: usize, what: &'static str },
    #[error("FEV: {what} {value} at {at:#x} is out of range")]
    Range {
        at: usize,
        what: &'static str,
        value: u64,
    },
}

pub type FevResult<T> = Result<T, FevError>;

struct Reader<'a> {
    d: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize, what: &'static str) -> FevResult<&'a [u8]> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.d.len());
        let Some(end) = end else {
            return Err(FevError::Truncated { at: self.at, what });
        };
        let s = &self.d[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u32(&mut self, what: &'static str) -> FevResult<u32> {
        let b = self.bytes(4, what)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i32(&mut self, what: &'static str) -> FevResult<i32> {
        Ok(self.u32(what)? as i32)
    }
    fn u16(&mut self, what: &'static str) -> FevResult<u16> {
        let b = self.bytes(2, what)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn f32(&mut self, what: &'static str) -> FevResult<f32> {
        Ok(f32::from_bits(self.u32(what)?))
    }
    /// A count, sanity-checked against the bytes left (each item needs at least `min` bytes).
    fn count(&mut self, what: &'static str, min: usize) -> FevResult<usize> {
        let at = self.at;
        let n = self.u32(what)? as usize;
        if n.saturating_mul(min.max(1)) > self.d.len() - self.at {
            return Err(FevError::Range {
                at,
                what,
                value: n as u64,
            });
        }
        Ok(n)
    }
    /// A length-prefixed string (the length counts the terminating NUL).
    fn string(&mut self, what: &'static str) -> FevResult<String> {
        let n = self.count(what, 1)?;
        let b = self.bytes(n, what)?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        Ok(String::from_utf8_lossy(&b[..end]).into_owned())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WaveBank {
    /// Load flags (0x80: stream from disk, 0x200: decompress into memory; provisional).
    pub flags: u32,
    pub max_streams: u32,
    /// The bank's file stem (`music_bank_strm_new` for `music_bank_strm_new.fsb`).
    pub name: String,
}

/// A volume category (`master`, `music`, `VOconversation`, ...). Events name theirs.
#[derive(Clone, Debug, PartialEq)]
pub struct Category {
    pub name: String,
    pub volume: f32,
    pub pitch: f32,
    pub unknown: [u32; 2],
    pub children: Vec<Category>,
}

impl Category {
    /// Depth-first search by name.
    pub fn find(&self, name: &str) -> Option<&Category> {
        if self.name.eq_ignore_ascii_case(name) {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(name))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Property {
    Int(i32),
    Float(f32),
    String(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct EventGroup {
    pub name: String,
    pub properties: Vec<(String, Property)>,
    pub groups: Vec<EventGroup>,
    pub events: Vec<Event>,
}

/// `FMOD_MODE` bits in [`Event::mode`].
pub mod mode {
    pub const MODE_2D: u32 = 0x8;
    pub const MODE_3D: u32 = 0x10;
    pub const HEAD_RELATIVE: u32 = 0x4_0000;
    pub const WORLD_RELATIVE: u32 = 0x8_0000;
    pub const LOG_ROLLOFF: u32 = 0x10_0000;
    pub const LINEAR_ROLLOFF: u32 = 0x20_0000;
    pub const CUSTOM_ROLLOFF: u32 = 0x400_0000;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// 8: a complex (layered) event, 0x10: a simple event (one sound instance).
    pub kind_flags: u32,
    pub name: String,
    pub guid: [u8; 16],
    /// Linear gain (FMOD Designer stores the dB slider as a factor: 0.501 is -6 dB).
    pub volume: f32,
    /// Pitch, raw: 1.0 is 4 octaves (`fmod_event.dll` `EventI::getPitch`, 0x100103e0:
    /// octaves = raw x 4, semitones x 48, tones x 24).
    pub pitch: f32,
    /// Random pitch, raw like `pitch`: each play is offset uniformly within plus or minus
    /// this (0x1000f790).
    pub pitch_randomization: f32,
    /// Random volume, linear: each play's gain is scaled by 1 - v .. 1 (0x10010184).
    pub volume_randomization: f32,
    pub priority: u32,
    pub max_playbacks: u32,
    pub steal_priority: u32,
    /// `FMOD_MODE` bits ([`mode`]): 2D or 3D, rolloff, relative positioning.
    pub mode: u32,
    pub min_distance: f32,
    pub max_distance: f32,
    pub flags: u32,
    /// 2D speaker levels: L, R, C, LFE, back L, back R, side L, side R.
    pub speaker_levels: [f32; 8],
    pub cone_inside_angle: f32,
    pub cone_outside_angle: f32,
    pub cone_outside_volume: f32,
    pub max_playbacks_behavior: u32,
    pub doppler: f32,
    pub reverb_dry: f32,
    pub reverb_wet: f32,
    pub speaker_spread: f32,
    pub fade_in_ms: u32,
    pub fade_out_ms: u32,
    pub spawn_intensity: f32,
    pub spawn_intensity_randomization: f32,
    pub pan_level_3d: f32,
    pub position_randomization: u32,
    pub body: EventBody,
    /// Category names (normally one).
    pub categories: Vec<String>,
}

impl Event {
    pub fn is_3d(&self) -> bool {
        self.mode & mode::MODE_3D != 0
    }

    /// Every sound instance the event can play, in layer order.
    pub fn sounds(&self) -> Vec<&SoundInstance> {
        match &self.body {
            EventBody::Simple { sound, .. } => vec![sound],
            EventBody::Complex { layers, .. } => layers.iter().flat_map(|l| &l.sounds).collect(),
        }
    }

    pub fn parameter(&self, name: &str) -> Option<&Parameter> {
        match &self.body {
            EventBody::Complex { parameters, .. } => parameters
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(name)),
            EventBody::Simple { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EventBody {
    Simple {
        unknown: u32,
        sound: SoundInstance,
    },
    Complex {
        layers: Vec<Layer>,
        parameters: Vec<Parameter>,
        properties: Vec<(String, Property)>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub flags: u16,
    pub priority: i16,
    /// Index into the event's parameters of the one its sounds are placed on.
    pub control_parameter: u16,
    pub sounds: Vec<SoundInstance>,
    pub envelopes: Vec<Envelope>,
}

/// A sound definition placed on a layer, over a range of the layer's control parameter.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundInstance {
    /// Index into [`Fev::sound_defs`].
    pub sound_def: u16,
    /// Where the instance starts and how long it is, in the control parameter's units.
    pub start: f32,
    pub length: f32,
    pub unknown0: u32,
    /// 0: plays its waveforms in a loop (the default), 1 or 2: other FMOD loop modes
    /// (oneshot / loop and cut; provisional).
    pub loop_mode: u32,
    /// -1: loop forever.
    pub loop_count: i32,
    pub unknown1: u32,
    pub f28: f32,
    pub f2c: f32,
    pub f30: f32,
    pub f1c: f32,
    pub f20: f32,
    pub f24: f32,
    pub flags_a: u32,
    pub flags_b: u32,
}

/// What an [`Envelope`] changes ([`Envelope::flags`]); otherwise a DSP effect's parameter.
pub mod envelope {
    pub const VOLUME: u32 = 0x8;
    pub const PITCH: u32 = 0x10;
    pub const PAN: u32 = 0x20;
    pub const TIME_OFFSET: u32 = 0x40;
    pub const SURROUND_PAN: u32 = 0x80;
    pub const SPEAKER_SPREAD_3D: u32 = 0x100;
    pub const REVERB_LEVEL: u32 = 0x200;
    pub const PAN_LEVEL_3D: u32 = 0x400;
    pub const REVERB_BALANCE: u32 = 0x800;
    pub const SPAWN_INTENSITY: u32 = 0x1000;
}

/// A curve over the layer's control parameter driving one property or effect parameter.
#[derive(Clone, Debug, PartialEq)]
pub struct Envelope {
    /// Index of the envelope this one chains from, or -1.
    pub parent: i32,
    /// For an effect, its name (`FMOD Lowpass Simple`); empty for a built-in property.
    pub effect: String,
    /// Which parameter of the effect.
    pub effect_parameter: u32,
    /// Which property ([`envelope`] bits), plus flags.
    pub flags: u32,
    pub flags2: u32,
    pub points: Vec<EnvelopePoint>,
    pub unknown: u32,
    pub muted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvelopePoint {
    /// Position along the control parameter, normalised 0..=1 over its range.
    pub x: f32,
    pub y: f32,
    /// Curve shape to the next point (1: linear; provisional).
    pub flags: u32,
}

impl Envelope {
    /// The envelope's value at normalised position `x` (linear between points, held flat
    /// outside them).
    pub fn value(&self, x: f32) -> Option<f32> {
        let first = self.points.first()?;
        if x <= first.x {
            return Some(first.y);
        }
        for w in self.points.windows(2) {
            let (a, b) = (w[0], w[1]);
            if x <= b.x {
                let t = if b.x > a.x {
                    (x - a.x) / (b.x - a.x)
                } else {
                    1.0
                };
                return Some(a.y + (b.y - a.y) * t);
            }
        }
        self.points.last().map(|p| p.y)
    }
}

/// Built-in parameter names (FMOD sets these itself).
pub const PARAM_DISTANCE: &str = "(distance)";
pub const PARAM_LISTENER_ANGLE: &str = "(listener angle)";
pub const PARAM_EVENT_ANGLE: &str = "(event angle)";

#[derive(Clone, Debug, PartialEq)]
pub struct Parameter {
    pub name: String,
    pub velocity: f32,
    pub min: f32,
    pub max: f32,
    pub flags: u32,
    pub seek_speed: f32,
    pub f2c: f32,
    pub sustain_points: Vec<f32>,
}

/// Sound-definition play modes ([`SoundDefTemplate::play_mode`]).
pub mod play_mode {
    pub const SEQUENTIAL: u32 = 0;
    pub const RANDOM: u32 = 1;
    pub const RANDOM_NO_REPEAT: u32 = 2;
    pub const SEQUENTIAL_NO_REPEAT: u32 = 3;
    pub const SHUFFLE: u32 = 4;
    /// The game supplies the sound (DA:O's voice-over: the line's own sample).
    pub const PROGRAMMER_SELECTED: u32 = 5;
}

/// Settings shared by sound definitions.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundDefTemplate {
    pub play_mode: u32,
    pub spawn_time_min_ms: u32,
    pub spawn_time_max_ms: u32,
    pub max_spawned: u32,
    /// Linear gain (`fmod_event.dll` reads it at +0x10, 0x1002e9f9).
    pub volume: f32,
    /// 1: random volume from `f1c` up to 1; otherwise a range `f14`..`f18` (0x1002ea00).
    pub volume_randomization_method: u32,
    pub f14: f32,
    pub f18: f32,
    /// The lowest random gain, linear (+0x1c, 0x1002ea0e): each play is `f1c` .. 1.
    pub f1c: f32,
    /// Pitch, raw like the event's: 1 is 4 octaves (+0x20, 0x1002eb1e).
    pub pitch: f32,
    /// 1: random pitch within plus or minus `f2c`; otherwise a range `f24`..`f28`.
    pub pitch_randomization_method: u32,
    pub f24: f32,
    pub f28: f32,
    /// Random pitch, raw: each play is offset within plus or minus this (+0x2c, 0x1002eb2b).
    pub f2c: f32,
    pub position_randomization: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SoundDef {
    /// `/main_theme`.
    pub name: String,
    pub template: u32,
    pub waveforms: Vec<Waveform>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Waveform {
    /// Relative chance under random play modes (100 by default).
    pub weight: u32,
    pub kind: WaveformKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WaveformKind {
    /// A sample in a bank: its source file name, the bank's name, its index in the bank and
    /// its length in milliseconds.
    Wave {
        file: String,
        bank: String,
        index: u32,
        length_ms: u32,
    },
    Oscillator {
        shape: u32,
        frequency: f32,
    },
    DontPlay,
    Programmer,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReverbDef {
    pub name: String,
    /// The 33 words as stored; [`ReverbDef::properties`] names them.
    pub raw: Vec<u32>,
}

/// FMOD Ex's reverb properties (`FMOD_REVERB_PROPERTIES`, I3DL2 style). Levels are
/// millibels; times seconds; `diffusion` and `density` percent; references Hz.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReverbProperties {
    pub instance: i32,
    /// The preset's environment number (FMOD's preset list index; DA:O's own presets 0).
    pub environment: i32,
    pub env_size: f32,
    pub env_diffusion: f32,
    pub room: i32,
    pub room_hf: i32,
    pub room_lf: i32,
    pub decay_time: f32,
    pub decay_hf_ratio: f32,
    pub decay_lf_ratio: f32,
    pub reflections: i32,
    pub reflections_delay: f32,
    pub reflections_pan: [f32; 3],
    pub reverb: i32,
    pub reverb_delay: f32,
    pub reverb_pan: [f32; 3],
    pub echo_time: f32,
    pub echo_depth: f32,
    pub modulation_time: f32,
    pub modulation_depth: f32,
    pub air_absorption_hf: f32,
    pub hf_reference: f32,
    pub lf_reference: f32,
    pub room_rolloff_factor: f32,
    pub diffusion: f32,
    pub density: f32,
    pub flags: u32,
}

impl ReverbProperties {
    /// No reverb (FMOD's `FMOD_PRESET_OFF`, as DAOrigins.exe 0x5c2440 builds it for an
    /// empty preset name).
    pub const OFF: ReverbProperties = ReverbProperties {
        instance: 0,
        environment: -1,
        env_size: 7.5,
        env_diffusion: 1.0,
        room: -10000,
        room_hf: -10000,
        room_lf: 0,
        decay_time: 1.0,
        decay_hf_ratio: 1.0,
        decay_lf_ratio: 1.0,
        reflections: -2602,
        reflections_delay: 0.007,
        reflections_pan: [0.0; 3],
        reverb: 200,
        reverb_delay: 0.011,
        reverb_pan: [0.0; 3],
        echo_time: 0.25,
        echo_depth: 0.0,
        modulation_time: 0.25,
        modulation_depth: 0.0,
        air_absorption_hf: -5.0,
        hf_reference: 5000.0,
        lf_reference: 250.0,
        room_rolloff_factor: 0.0,
        diffusion: 0.0,
        density: 0.0,
        flags: 0x33f,
    };

    /// FMOD's `FMOD_PRESET_GENERIC`, which DAOrigins.exe (0x5c252b) uses for the preset
    /// name `force_generic`.
    pub const GENERIC: ReverbProperties = ReverbProperties {
        environment: 0,
        room: -1000,
        room_hf: -100,
        decay_time: 1.49,
        decay_hf_ratio: 0.83,
        diffusion: 100.0,
        density: 100.0,
        flags: 0x3f,
        ..ReverbProperties::OFF
    };
}

impl ReverbProperties {
    /// These properties with any non-finite value (a damaged or modded project's) replaced
    /// by [`ReverbProperties::OFF`]'s.
    pub fn finite(mut self) -> ReverbProperties {
        let d = ReverbProperties::OFF;
        let fix = |v: &mut f32, d: f32| {
            if !v.is_finite() {
                *v = d;
            }
        };
        fix(&mut self.env_size, d.env_size);
        fix(&mut self.env_diffusion, d.env_diffusion);
        fix(&mut self.decay_time, d.decay_time);
        fix(&mut self.decay_hf_ratio, d.decay_hf_ratio);
        fix(&mut self.decay_lf_ratio, d.decay_lf_ratio);
        fix(&mut self.reflections_delay, d.reflections_delay);
        fix(&mut self.reverb_delay, d.reverb_delay);
        for (v, d) in self.reflections_pan.iter_mut().zip(d.reflections_pan) {
            fix(v, d);
        }
        for (v, d) in self.reverb_pan.iter_mut().zip(d.reverb_pan) {
            fix(v, d);
        }
        fix(&mut self.echo_time, d.echo_time);
        fix(&mut self.echo_depth, d.echo_depth);
        fix(&mut self.modulation_time, d.modulation_time);
        fix(&mut self.modulation_depth, d.modulation_depth);
        fix(&mut self.air_absorption_hf, d.air_absorption_hf);
        fix(&mut self.hf_reference, d.hf_reference);
        fix(&mut self.lf_reference, d.lf_reference);
        fix(&mut self.room_rolloff_factor, d.room_rolloff_factor);
        fix(&mut self.diffusion, d.diffusion);
        fix(&mut self.density, d.density);
        self
    }
}

impl ReverbDef {
    /// The properties, in the order `fmod_event.dll` reads them (0x10026f80..0x100272ef):
    /// room, room HF, room rolloff, decay time, decay HF ratio, reflections and delay,
    /// reverb and delay, diffusion, density, HF reference, room LF, LF reference, instance,
    /// environment, environment size and diffusion, room LF again, decay LF ratio, the
    /// reflections and reverb pans, echo time and depth, modulation time and depth, air
    /// absorption, LF reference again, flags. `None` if the definition is short.
    pub fn properties(&self) -> Option<ReverbProperties> {
        let w = |i: usize| self.raw.get(i).copied();
        let i = |n: usize| w(n).map(|v| v as i32);
        let f = |n: usize| w(n).map(f32::from_bits);
        Some(
            ReverbProperties {
                room: i(0)?,
                room_hf: i(1)?,
                room_rolloff_factor: f(2)?,
                decay_time: f(3)?,
                decay_hf_ratio: f(4)?,
                reflections: i(5)?,
                reflections_delay: f(6)?,
                reverb: i(7)?,
                reverb_delay: f(8)?,
                diffusion: f(9)?,
                density: f(10)?,
                hf_reference: f(11)?,
                instance: i(14)?,
                environment: i(15)?,
                env_size: f(16)?,
                env_diffusion: f(17)?,
                room_lf: i(18)?,
                decay_lf_ratio: f(19)?,
                reflections_pan: [f(20)?, f(21)?, f(22)?],
                reverb_pan: [f(23)?, f(24)?, f(25)?],
                echo_time: f(26)?,
                echo_depth: f(27)?,
                modulation_time: f(28)?,
                modulation_depth: f(29)?,
                air_absorption_hf: f(30)?,
                lf_reference: f(31)?,
                flags: w(32)?,
            }
            .finite(),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fev {
    pub version: u32,
    pub header: [u32; 2],
    /// The project name: the first element of event paths.
    pub name: String,
    pub wave_banks: Vec<WaveBank>,
    pub category: Category,
    pub groups: Vec<EventGroup>,
    pub templates: Vec<SoundDefTemplate>,
    pub sound_defs: Vec<SoundDef>,
    pub reverbs: Vec<ReverbDef>,
    /// The music system's chunk data (`comp` and below), unparsed.
    pub music_data: Vec<u8>,
}

const REVERB_WORDS: usize = 33;

fn properties(r: &mut Reader) -> FevResult<Vec<(String, Property)>> {
    let n = r.count("property count", 9)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.string("property name")?;
        let at = r.at;
        let v = match r.u32("property type")? {
            0 => Property::Int(r.i32("property")?),
            1 => Property::Float(r.f32("property")?),
            2 => Property::String(r.string("property")?),
            t => {
                return Err(FevError::Range {
                    at,
                    what: "property type",
                    value: t as u64,
                });
            }
        };
        out.push((name, v));
    }
    Ok(out)
}

fn category(r: &mut Reader, depth: usize) -> FevResult<Category> {
    let at = r.at;
    if depth > 64 {
        return Err(FevError::Range {
            at,
            what: "category depth",
            value: depth as u64,
        });
    }
    let name = r.string("category name")?;
    let volume = r.f32("category volume")?;
    let pitch = r.f32("category pitch")?;
    let unknown = [r.u32("category")?, r.u32("category")?];
    let n = r.count("subcategories", 24)?;
    let children = (0..n)
        .map(|_| category(r, depth + 1))
        .collect::<FevResult<_>>()?;
    Ok(Category {
        name,
        volume,
        pitch,
        unknown,
        children,
    })
}

fn sound_instance(r: &mut Reader) -> FevResult<SoundInstance> {
    Ok(SoundInstance {
        sound_def: r.u16("sound instance")?,
        start: r.f32("sound instance")?,
        length: r.f32("sound instance")?,
        unknown0: r.u32("sound instance")?,
        loop_mode: r.u32("sound instance")?,
        loop_count: r.i32("sound instance")?,
        unknown1: r.u32("sound instance")?,
        f28: r.f32("sound instance")?,
        f2c: r.f32("sound instance")?,
        f30: r.f32("sound instance")?,
        f1c: r.f32("sound instance")?,
        f20: r.f32("sound instance")?,
        f24: r.f32("sound instance")?,
        flags_a: r.u32("sound instance")?,
        flags_b: r.u32("sound instance")?,
    })
}

fn envelope(r: &mut Reader) -> FevResult<Envelope> {
    let parent = r.i32("envelope")?;
    let effect = r.string("envelope effect")?;
    let effect_parameter = r.u32("envelope")?;
    let flags = r.u32("envelope")?;
    let flags2 = r.u32("envelope")?;
    let n = r.count("envelope points", 12)?;
    let points = (0..n)
        .map(|_| {
            Ok(EnvelopePoint {
                x: r.f32("envelope point")?,
                y: r.f32("envelope point")?,
                flags: r.u32("envelope point")?,
            })
        })
        .collect::<FevResult<_>>()?;
    Ok(Envelope {
        parent,
        effect,
        effect_parameter,
        flags,
        flags2,
        points,
        unknown: r.u32("envelope")?,
        muted: r.u32("envelope")? != 0,
    })
}

fn layer(r: &mut Reader) -> FevResult<Layer> {
    let flags = r.u16("layer")?;
    let priority = r.u16("layer")? as i16;
    let control_parameter = r.u16("layer")?;
    let sounds = r.u16("layer")? as usize;
    let envelopes = r.u16("layer")? as usize;
    Ok(Layer {
        flags,
        priority,
        control_parameter,
        sounds: (0..sounds)
            .map(|_| sound_instance(r))
            .collect::<FevResult<_>>()?,
        envelopes: (0..envelopes)
            .map(|_| envelope(r))
            .collect::<FevResult<_>>()?,
    })
}

fn event(r: &mut Reader) -> FevResult<Event> {
    let kind_at = r.at;
    let kind_flags = r.u32("event kind")?;
    let name = r.string("event name")?;
    let mut guid = [0u8; 16];
    guid.copy_from_slice(r.bytes(16, "event guid")?);
    let f = |r: &mut Reader| r.f32("event");
    let u = |r: &mut Reader| r.u32("event");
    let volume = f(r)?;
    let pitch = f(r)?;
    let pitch_randomization = f(r)?;
    let volume_randomization = f(r)?;
    let priority = u(r)?;
    let max_playbacks = u(r)?;
    let steal_priority = u(r)?;
    let mode = u(r)?;
    let min_distance = f(r)?;
    let max_distance = f(r)?;
    let flags = u(r)?;
    let mut speaker_levels = [0.0; 8];
    for s in &mut speaker_levels {
        *s = f(r)?;
    }
    let cone_inside_angle = f(r)?;
    let cone_outside_angle = f(r)?;
    let cone_outside_volume = f(r)?;
    let max_playbacks_behavior = u(r)?;
    let doppler = f(r)?;
    let reverb_dry = f(r)?;
    let reverb_wet = f(r)?;
    let speaker_spread = f(r)?;
    let fade_in_ms = u(r)?;
    let fade_out_ms = u(r)?;
    let spawn_intensity = f(r)?;
    let spawn_intensity_randomization = f(r)?;
    let pan_level_3d = f(r)?;
    let position_randomization = u(r)?;
    let body = match kind_flags {
        0x10 => EventBody::Simple {
            unknown: u(r)?,
            sound: sound_instance(r)?,
        },
        8 => {
            let n = r.count("layers", 10)?;
            let layers = (0..n).map(|_| layer(r)).collect::<FevResult<_>>()?;
            let n = r.count("parameters", 32)?;
            let mut parameters = Vec::with_capacity(n);
            for _ in 0..n {
                let name = r.string("parameter name")?;
                let velocity = f(r)?;
                let min = f(r)?;
                let max = f(r)?;
                let flags = u(r)?;
                let seek_speed = f(r)?;
                let f2c = f(r)?;
                let k = r.count("sustain points", 4)?;
                let sustain_points = (0..k).map(|_| f(r)).collect::<FevResult<_>>()?;
                parameters.push(Parameter {
                    name,
                    velocity,
                    min,
                    max,
                    flags,
                    seek_speed,
                    f2c,
                    sustain_points,
                });
            }
            EventBody::Complex {
                layers,
                parameters,
                properties: properties(r)?,
            }
        }
        k => {
            return Err(FevError::Range {
                at: kind_at,
                what: "event kind",
                value: k as u64,
            });
        }
    };
    let n = r.count("event categories", 4)?;
    let categories = (0..n)
        .map(|_| r.string("event category"))
        .collect::<FevResult<_>>()?;
    Ok(Event {
        kind_flags,
        name,
        guid,
        volume,
        pitch,
        pitch_randomization,
        volume_randomization,
        priority,
        max_playbacks,
        steal_priority,
        mode,
        min_distance,
        max_distance,
        flags,
        speaker_levels,
        cone_inside_angle,
        cone_outside_angle,
        cone_outside_volume,
        max_playbacks_behavior,
        doppler,
        reverb_dry,
        reverb_wet,
        speaker_spread,
        fade_in_ms,
        fade_out_ms,
        spawn_intensity,
        spawn_intensity_randomization,
        pan_level_3d,
        position_randomization,
        body,
        categories,
    })
}

fn group(r: &mut Reader, depth: usize) -> FevResult<EventGroup> {
    if depth > 64 {
        return Err(FevError::Range {
            at: r.at,
            what: "group depth",
            value: depth as u64,
        });
    }
    let name = r.string("group name")?;
    let properties = properties(r)?;
    let ng = r.count("subgroups", 16)?;
    let ne = r.count("events", 100)?;
    let groups = (0..ng)
        .map(|_| group(r, depth + 1))
        .collect::<FevResult<_>>()?;
    let events = (0..ne).map(|_| event(r)).collect::<FevResult<_>>()?;
    Ok(EventGroup {
        name,
        properties,
        groups,
        events,
    })
}

impl Fev {
    pub fn parse(data: &[u8]) -> FevResult<Fev> {
        let mut r = Reader { d: data, at: 0 };
        if r.bytes(4, "magic").map_err(|_| FevError::Magic)? != b"FEV1" {
            return Err(FevError::Magic);
        }
        let version = r.u32("version")?;
        if version != VERSION {
            return Err(FevError::Version(version));
        }
        let header = [r.u32("header")?, r.u32("header")?];
        let name = r.string("project name")?;
        let n = r.count("wave banks", 12)?;
        let mut wave_banks = Vec::with_capacity(n);
        for _ in 0..n {
            wave_banks.push(WaveBank {
                flags: r.u32("wave bank")?,
                max_streams: r.u32("wave bank")?,
                name: r.string("wave bank name")?,
            });
        }
        let category = category(&mut r, 0)?;
        let n = r.count("event groups", 16)?;
        let groups = (0..n).map(|_| group(&mut r, 0)).collect::<FevResult<_>>()?;
        let n = r.count("templates", 60)?;
        let mut templates = Vec::with_capacity(n);
        for _ in 0..n {
            let u = |r: &mut Reader| r.u32("template");
            let f = |r: &mut Reader| r.f32("template");
            templates.push(SoundDefTemplate {
                play_mode: u(&mut r)?,
                spawn_time_min_ms: u(&mut r)?,
                spawn_time_max_ms: u(&mut r)?,
                max_spawned: u(&mut r)?,
                volume: f(&mut r)?,
                volume_randomization_method: u(&mut r)?,
                f14: f(&mut r)?,
                f18: f(&mut r)?,
                f1c: f(&mut r)?,
                pitch: f(&mut r)?,
                pitch_randomization_method: u(&mut r)?,
                f24: f(&mut r)?,
                f28: f(&mut r)?,
                f2c: f(&mut r)?,
                position_randomization: f(&mut r)?,
            });
        }
        let n = r.count("sound definitions", 12)?;
        let mut sound_defs = Vec::with_capacity(n);
        for _ in 0..n {
            let name = r.string("sound definition name")?;
            let template = r.u32("sound definition")?;
            let k = r.count("waveforms", 8)?;
            let mut waveforms = Vec::with_capacity(k);
            for _ in 0..k {
                let at = r.at;
                let kind = r.u32("waveform type")?;
                let weight = r.u32("waveform weight")?;
                let kind = match kind {
                    0 => WaveformKind::Wave {
                        file: r.string("waveform file")?,
                        bank: r.string("waveform bank")?,
                        index: r.u32("waveform")?,
                        length_ms: r.u32("waveform")?,
                    },
                    1 => WaveformKind::Oscillator {
                        shape: r.u32("oscillator")?,
                        frequency: r.f32("oscillator")?,
                    },
                    2 => WaveformKind::DontPlay,
                    3 => WaveformKind::Programmer,
                    t => {
                        return Err(FevError::Range {
                            at,
                            what: "waveform type",
                            value: t as u64,
                        });
                    }
                };
                waveforms.push(Waveform { weight, kind });
            }
            sound_defs.push(SoundDef {
                name,
                template,
                waveforms,
            });
        }
        let n = r.count("reverbs", 4 * REVERB_WORDS)?;
        let mut reverbs = Vec::with_capacity(n);
        for _ in 0..n {
            let name = r.string("reverb name")?;
            let raw = (0..REVERB_WORDS)
                .map(|_| r.u32("reverb"))
                .collect::<FevResult<_>>()?;
            reverbs.push(ReverbDef { name, raw });
        }
        let music_data = data[r.at..].to_vec();
        Ok(Fev {
            version,
            header,
            name,
            wave_banks,
            category,
            groups,
            templates,
            sound_defs,
            reverbs,
            music_data,
        })
    }

    /// The event at `path`: group names then the event name, `/`-separated, optionally led
    /// by the project name (`music/_menu/main` or `_menu/main`). Case-insensitive.
    pub fn event(&self, path: &str) -> Option<&Event> {
        let mut parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if parts.len() > 1 && parts[0].eq_ignore_ascii_case(&self.name) {
            parts.remove(0);
        }
        let (event, groups) = parts.split_last()?;
        let (first, rest) = groups.split_first()?;
        let mut g = self
            .groups
            .iter()
            .find(|g| g.name.eq_ignore_ascii_case(first))?;
        for name in rest {
            g = g
                .groups
                .iter()
                .find(|s| s.name.eq_ignore_ascii_case(name))?;
        }
        g.events.iter().find(|e| e.name.eq_ignore_ascii_case(event))
    }

    /// Every event with its path (without the project name), depth first.
    pub fn events(&self) -> Vec<(String, &Event)> {
        fn walk<'a>(g: &'a EventGroup, prefix: &str, out: &mut Vec<(String, &'a Event)>) {
            let p = format!("{prefix}{}/", g.name);
            for e in &g.events {
                out.push((format!("{p}{}", e.name), e));
            }
            for s in &g.groups {
                walk(s, &p, out);
            }
        }
        let mut out = Vec::new();
        for g in &self.groups {
            walk(g, "", &mut out);
        }
        out
    }

    pub fn sound_def(&self, instance: &SoundInstance) -> Option<&SoundDef> {
        self.sound_defs.get(instance.sound_def as usize)
    }

    pub fn template(&self, def: &SoundDef) -> Option<&SoundDefTemplate> {
        self.templates.get(def.template as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct W(Vec<u8>);
    impl W {
        fn u32(&mut self, v: u32) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn u16(&mut self, v: u16) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn f(&mut self, v: f32) -> &mut Self {
            self.u32(v.to_bits())
        }
        fn s(&mut self, v: &str) -> &mut Self {
            self.u32(v.len() as u32 + 1);
            self.0.extend_from_slice(v.as_bytes());
            self.0.push(0);
            self
        }
    }

    fn event_header(w: &mut W, kind: u32, name: &str) {
        w.u32(kind).s(name);
        w.0.extend_from_slice(&[7; 16]);
        w.f(0.5)
            .f(0.0)
            .f(0.0)
            .f(0.0)
            .u32(41)
            .u32(1)
            .u32(10000)
            .u32(0x0018_0008);
        w.f(1.0).f(10000.0).u32(0);
        for _ in 0..8 {
            w.f(1.0);
        }
        w.f(360.0)
            .f(360.0)
            .f(1.0)
            .u32(1)
            .f(1.0)
            .f(0.0)
            .f(0.0)
            .f(0.0);
        w.u32(1500).u32(1500).f(1.0).f(0.0).f(1.0).u32(0);
    }

    fn sound(w: &mut W, def: u16) {
        w.u16(def).f(0.0).f(1.0).u32(0).u32(2).u32(u32::MAX).u32(0);
        w.f(0.0).f(0.0).f(0.0).f(1.0).f(-1.0).f(-1.0).u32(2).u32(2);
    }

    fn file() -> Vec<u8> {
        let mut w = W(Vec::new());
        w.0.extend_from_slice(b"FEV1");
        w.u32(VERSION).u32(0).u32(0).s("music");
        w.u32(1).u32(0x80).u32(4).s("music_bank_strm_new");
        // master { music }
        w.s("master").f(1.0).f(0.0).u32(0).u32(0).u32(1);
        w.s("music").f(1.0).f(0.0).u32(0).u32(0).u32(0);
        // group _menu: no properties, 1 subgroup, 1 event
        w.u32(1).s("_menu").u32(0).u32(1).u32(1);
        // subgroup "stingers": one property, a simple event
        w.s("stingers")
            .u32(1)
            .s("note")
            .u32(2)
            .s("hi")
            .u32(0)
            .u32(1);
        event_header(&mut w, 0x10, "hit");
        w.u32(1);
        sound(&mut w, 0);
        w.u32(1).s("music");
        // complex event "main"
        event_header(&mut w, 8, "main");
        w.u32(1).u16(2).u16(0xffff).u16(0).u16(1).u16(1);
        sound(&mut w, 1);
        w.u32(u32::MAX).s("").u32(0).u32(8).u32(0).u32(2);
        w.f(0.0).f(1.0).u32(1).f(1.0).f(0.0).u32(1).u32(0).u32(0);
        w.u32(1)
            .s("music_state")
            .f(0.0)
            .f(-1.0)
            .f(2.0)
            .u32(3)
            .f(0.0)
            .f(0.0)
            .u32(0);
        w.u32(0).u32(1).s("music");
        // one template, two sound definitions
        w.u32(1)
            .u32(3)
            .u32(0)
            .u32(0)
            .u32(1)
            .f(0.3)
            .u32(1)
            .f(1.0)
            .f(1.0)
            .f(1.0);
        w.f(0.0).u32(1).f(0.0).f(0.0).f(0.0).f(0.0);
        w.u32(2);
        w.s("/hit").u32(0).u32(1).u32(1).u32(100).u32(0).f(440.0);
        w.s("/main_theme").u32(0).u32(2);
        w.u32(0)
            .u32(100)
            .s("gui/Main_Theme.wav")
            .s("music_bank_strm_new")
            .u32(125)
            .u32(170269);
        w.u32(3).u32(100);
        // no reverbs, an empty music chunk
        w.u32(0).u32(8);
        w.0.extend_from_slice(b"comp");
        w.0
    }

    #[test]
    fn parses_events_definitions_and_paths() {
        let fev = Fev::parse(&file()).unwrap();
        assert_eq!(fev.name, "music");
        assert_eq!(fev.wave_banks[0].name, "music_bank_strm_new");
        assert!(fev.category.find("MUSIC").is_some());
        let main = fev.event("music/_menu/main").unwrap();
        assert_eq!(main.fade_in_ms, 1500);
        assert!(!main.is_3d());
        let sounds = main.sounds();
        let def = fev.sound_def(sounds[0]).unwrap();
        assert_eq!(def.name, "/main_theme");
        assert_eq!(fev.template(def).unwrap().play_mode, 3);
        assert!(matches!(
            &def.waveforms[0].kind,
            WaveformKind::Wave { index: 125, bank, .. } if bank == "music_bank_strm_new"
        ));
        assert_eq!(def.waveforms[1].kind, WaveformKind::Programmer);
        assert_eq!(main.parameter("music_state").unwrap().max, 2.0);
        let EventBody::Complex { layers, .. } = &main.body else {
            panic!("complex")
        };
        let env = &layers[0].envelopes[0];
        assert_eq!(env.flags & envelope::VOLUME, envelope::VOLUME);
        assert_eq!(env.value(0.25), Some(0.75));
        assert_eq!(env.value(2.0), Some(0.0));
        let hit = fev.event("_menu/stingers/hit").unwrap();
        assert!(matches!(hit.body, EventBody::Simple { .. }));
        assert_eq!(fev.events().len(), 2);
        assert_eq!(fev.music_data, b"\x08\0\0\0comp");
        assert!(fev.event("_menu/nope").is_none());
    }

    #[test]
    fn rejects_bad_input_without_panicking() {
        let f = file();
        for cut in [0, 3, 8, 20, 60, 200, 400, f.len() - 20] {
            assert!(Fev::parse(&f[..cut]).is_err(), "cut {cut}");
        }
        let mut bad = f.clone();
        bad[4] = 0x39;
        assert!(matches!(Fev::parse(&bad), Err(FevError::Version(_))));
        // A huge count fails cleanly instead of allocating.
        let mut huge = f.clone();
        let at = 4 + 4 + 8 + 4 + 6;
        huge[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Fev::parse(&huge).is_err());
    }

    #[test]
    fn reverb_words_name_the_properties_in_the_dlls_order() {
        // "Generic" as master_reverb.fev stores it.
        let words: [f32; 33] = [
            -1000.0, -100.0, 0.0, 1.49, 0.83, -2602.0, 0.007, 200.0, 0.011, 100.0, 100.0, 5000.0,
            0.0, 250.0, 0.0, 0.0, 7.5, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.25, 0.0,
            0.25, 0.0, -5.0, 250.0, 63.0,
        ];
        let ints = [0, 1, 5, 7, 12, 14, 15, 18, 32];
        let raw: Vec<u32> = words
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                if ints.contains(&i) {
                    v as i32 as u32
                } else {
                    v.to_bits()
                }
            })
            .collect();
        let mut def = ReverbDef {
            name: "Generic".into(),
            raw,
        };
        let p = def.properties().unwrap();
        assert_eq!(
            (p.room, p.room_hf, p.reflections, p.reverb),
            (-1000, -100, -2602, 200)
        );
        assert_eq!(
            (p.decay_time, p.decay_hf_ratio, p.hf_reference),
            (1.49, 0.83, 5000.0)
        );
        assert_eq!(
            (p.lf_reference, p.air_absorption_hf, p.flags),
            (250.0, -5.0, 63)
        );
        // A damaged value falls back; a short definition is none.
        def.raw[3] = f32::NAN.to_bits();
        assert_eq!(
            def.properties().unwrap().decay_time,
            ReverbProperties::OFF.decay_time
        );
        def.raw.truncate(20);
        assert_eq!(def.properties(), None);
    }
}
