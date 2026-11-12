//! The game's sound: the start menu's and character creation's music, the interface's sounds
//! and conversation voice-over, played through [`thedas_audio`] on the system output.
//!
//! - Music: the front end plays `music/_menu/main` (DAOrigins.exe names the `music/_menu`
//!   theme, 0xafed94); character creation `music/_menu/chargen`; both stop when the game
//!   proper starts. In-game music (area themes and combat) is not done yet.
//! - Interface: the movies' `PlayGUISound <id> ...` plays `sound_gui.gda`'s event for the id.
//! - Voice-over: a conversation line plays its dialog project's `VO/VOconversation` event
//!   (`VO/VOambient` for an ambient line) with the line's own sample as the programmer sound:
//!   `<strref>_f` for a female hero when the bank has it, else `<strref>_m` (DAOrigins.exe
//!   builds `%ls/VO/%ld` and the suffix at 0x7f2be9). The line stays up for the length of its
//!   voice.
//!
//! Provisional: voice-over is not positioned at the speaker yet, and the options screen's
//! volumes map linearly (0..10 to silent..full).

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use thedas_audio::area::{AreaReverb, AreaSound, area_audio, area_reverb};
use thedas_audio::{AudioEngine, EventHandle, Library, PlayOptions, Volumes};
use thedas_formats::anim::AnimEventText;
use thedas_vfs::Vfs;

/// The voice-over a conversation line wants.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceLine {
    pub dialog: String,
    pub strref: u32,
    pub female: bool,
    pub ambient: bool,
}

/// An attack's blow: where the target stands, the `COMBAT_RESULT_*`, and the base items of
/// the attacker's weapon and the target's armour and shield.
pub type Impact = ([f32; 3], i32, Option<i32>, Option<i32>, Option<i32>);

/// A creature's body armour, main-hand and off-hand base items (`BaseItem`), and whether
/// it is fighting.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Gear {
    pub armour: Option<i32>,
    pub main: Option<i32>,
    pub off: Option<i32>,
    /// The creature's footstep column in `sound_movement_footsteps.gda` (a column hash:
    /// its appearance's footstep type).
    pub footstep: Option<u32>,
    /// The ground under it: sound material (low nibble), vegetation (high nibble).
    pub surface: u8,
    /// Its size column (`Small`, `Medium`, `Large` as a column hash: its appearance's
    /// `apr_base.gda` column 0xc7cb6561, 0x6a0a1f).
    pub size: Option<u32>,
}

/// A sound set's lines (`sse_<set>.gda`: row -> strref) and each type's number of them
/// (`sst_<set>.gda`: type -> count).
type SoundSetLines = (
    std::collections::HashMap<i64, i64>,
    std::collections::HashMap<i64, i64>,
);

/// A party-picker member: id, template, sound set, position.
pub type PickerMember = (u32, String, Option<String>, [f32; 3]);

/// What the game is doing, as the sound follows it.
#[derive(Clone, Debug, Default)]
pub struct AudioState {
    /// The start menu (or the screens it opens) is up, before a game.
    pub front_end: bool,
    /// Character creation is running.
    pub chargen: bool,
    /// The interface's sound requests since the last frame (`PlayGUISound` arguments).
    pub gui_sounds: Vec<String>,
    pub line: Option<VoiceLine>,
    /// The cutscene playing (resource name) and how far it has played (seconds).
    pub cutscene: Option<(String, f32)>,
    /// Each drawn creature's animation now: id, `.ani` resource, seconds into it, its
    /// length, position (see `Live::animation_clocks`).
    pub animations: Vec<(u32, String, f32, f32, [f32; 3])>,
    /// What the animated creatures wear and wield, for their animations' foley and swings:
    /// id -> (body armour, main-hand, off-hand base items, footstep column, ground).
    pub gear: std::collections::HashMap<u32, Gear>,
    /// Attacks' blows: (target position, result, attacker's weapon, target's armour and
    /// shield base items).
    pub impacts: Vec<Impact>,
    /// Sound set lines to say: (set, `ss_types.gda` type, probability override (0: the
    /// type's own), where the speaker stands (none: 2D)).
    pub sound_sets: Vec<(String, i32, f32, Option<[f32; 3]>)>,
    /// The party picker while it is open: each member picked (id, template, sound set,
    /// position).
    pub picker: Option<Vec<PickerMember>>,
    /// The pool's followers (to say an unpicked one's line): id -> (id, template, sound set,
    /// position).
    pub picker_left: std::collections::HashMap<u32, PickerMember>,
    /// Events scripts played (`PlaySound`), with where the object stands (none: 2D).
    pub script_sounds: Vec<(String, Option<[f32; 3]>)>,
    /// Scripts' `SetMusicIntensity`, the latest this frame.
    pub music_intensity: Option<i32>,
    /// A boot movie playing (its file) and how far it has played (milliseconds): its own
    /// sound track plays with it.
    pub movie: Option<(std::path::PathBuf, f64)>,
    /// The area the party is in (its resource), in play.
    pub area: Option<String>,
    /// The party is fighting / has fallen.
    pub combat: bool,
    pub dead: bool,
    /// Where the listener is (between the party leader and the camera) and its right.
    pub listener: Option<([f32; 3], [f32; 3])>,
    /// Where the party leader stands (reverb triggers follow it).
    pub leader: Option<[f32; 3]>,
    /// Whether the hero is female.
    pub hero_female: bool,
    /// The area's sound objects the simulation has switched off (`ActivateSoundByTag`), by
    /// tag (lowercase).
    pub inactive_sounds: Vec<String>,
    pub volumes: Volumes,
}

/// A movie's sound track: decoded by ffmpeg on a thread, then played from where the
/// movie is.
struct MovieSound {
    path: std::path::PathBuf,
    decoding: Option<std::sync::mpsc::Receiver<Option<thedas_audio::Pcm>>>,
    voice: Option<thedas_audio::VoiceId>,
}

/// Decodes a movie's audio to interleaved stereo `f32` at `rate` with `ffmpeg`.
fn decode_movie_audio(path: &Path, rate: u32) -> Option<thedas_audio::Pcm> {
    use std::io::Read;
    let ffmpeg = crate::intro::find_ffmpeg()?;
    let mut child = std::process::Command::new(ffmpeg)
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args([
            "-vn",
            "-f",
            "f32le",
            "-ac",
            "2",
            "-ar",
            &rate.to_string(),
            "-",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut bytes = Vec::new();
    child.stdout.take()?.read_to_end(&mut bytes).ok()?;
    let _ = child.wait();
    let samples: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    (!samples.is_empty()).then_some(thedas_audio::Pcm {
        rate,
        channels: 2,
        samples,
    })
}

/// An area's sound, playing.
struct AreaPlay {
    resref: String,
    theme: Option<String>,
    track: Option<(&'static str, EventHandle)>,
    sounds: Vec<AreaSound>,
    /// Per sound: playing (`Some(Some)`), played through while in reach (`Some(None)`: it
    /// plays again once the listener has left and come back), or not started.
    playing: Vec<Option<Option<EventHandle>>>,
    reverb: AreaReverb,
    /// The reverb preset applied (`None` until the first).
    preset: Option<String>,
    /// Which reverb triggers the party leader was inside last frame.
    inside: Vec<bool>,
}

/// A cutscene's sound tracks and which of them are playing.
struct CutsceneSounds {
    name: String,
    tracks: Vec<thedas_sim::cutscene::CutsceneSound>,
    playing: Vec<Option<EventHandle>>,
    done: Vec<bool>,
}

pub struct GameAudio {
    engine: AudioEngine,
    /// The system output (none when recording: the mix is rendered on demand).
    _output: Option<thedas_audio::device::Output>,
    /// What a recording renders (see [`GameAudio::recording`]).
    pub recorded: Vec<f32>,
    gui: Vec<Option<String>>,
    music: Option<(&'static str, EventHandle)>,
    voice: Option<(VoiceLine, Option<EventHandle>, Option<f64>)>,
    cutscene: Option<CutsceneSounds>,
    area: Option<AreaPlay>,
    movie: Option<MovieSound>,
    /// The resources animation events are read from.
    vfs: Arc<Vfs>,
    /// Animation events by `.ani` resource (lowercase), read once.
    anim_events: std::collections::HashMap<String, Arc<Vec<AnimEventText>>>,
    /// Each creature's animation and time last frame.
    anim_clocks: std::collections::HashMap<u32, (String, f32)>,
    /// `sound_movement_footsteps.gda` (rows: ground material; columns: footstep type) and
    /// `sound_movement_scuffing.gda` (rows: ground material).
    footsteps: Option<thedas_formats::Gda>,
    scuffs: Option<thedas_formats::Gda>,
    /// `sound_bodyfall_creat_size.gda` (rows: ground material; columns: size),
    /// `sound_bodyfall_foley.gda` (rows: ground material; columns: armour material by
    /// label) and `sound_item_materials.gda`'s labels (sound material -> that column).
    bodyfalls: Option<thedas_formats::Gda>,
    bodyfall_foley: Option<thedas_formats::Gda>,
    item_material_labels: std::collections::HashMap<i64, u32>,
    /// `sound_vegetation_type.gda`, worksheet 155 "Sound Movement Vegetation" (rows:
    /// vegetation; columns: size `small`/`medium`/`large`).
    vegetation: Option<thedas_formats::Gda>,
    /// Per creature: where it last rustled through vegetation and the squared distance it
    /// must move from there to rustle again.
    rustles: std::collections::HashMap<u32, ([f32; 3], f32)>,
    /// Weapon impacts: each base item's sound material (`BITM_base.gda`), and each weapon
    /// material's impact events (`sound_weapon_hits.gda`).
    impacts: Impacts,
    /// Swing and armour-foley events (animation event kinds 19 and 17).
    foley: Foley,
    /// The music's `music_state` (scripts' `SetMusicIntensity`; 1 until one sets it).
    music_state: f32,
    /// `ss_types.gda`: each sound set type's first entry row and default probability.
    ss_types: std::collections::HashMap<i32, (i64, f32)>,
    /// Each sound set's entries (`sse_<set>.gda`: entry row -> line strref), read once.
    sound_set_entries: std::collections::HashMap<String, Arc<SoundSetLines>>,
    /// Per (set, type): the lines not yet drawn from its bag.
    sound_set_bags: std::collections::HashMap<(String, i32), Vec<i64>>,
    /// Whether the hero is female (sound set lines prefer their `_f` events).
    hero_female: bool,
    rng: u64,
    /// `partypicker.gda`: each follower template's picked and unpicked sound set entries
    /// ("default" for any other).
    picker_lines: std::collections::HashMap<String, (i32, i32)>,
    /// Who the open party picker had picked last frame, and when it last spoke.
    picked: Option<Vec<u32>>,
    /// Seconds of frame time since the picker last spoke.
    picker_spoke: f32,
    /// Creature templates' sound sets (`.utc` `SoundSet`), for creatures that do not carry
    /// theirs (from a save made before the simulation kept it).
    template_sets: std::collections::HashMap<String, Option<String>>,
    last: Instant,
}

/// `apr_base.gda`'s footstep type column (DAOrigins.exe 0x6a098a).
pub const APR_FOOTSTEP: u32 = 0xda48_e219;
/// `apr_base.gda`'s sound size column (0x6a0a1f): `Small`, `Medium`, `Large`.
pub const APR_SIZE: u32 = 0xc7cb_6561;
/// `sound_movement_scuffing.gda`'s event column (0x5b8d5f).
const COL_SCUFF: u32 = 0xb449_8545;

/// A creature's footstep column: its appearance's footstep type (`HUMANOID_SMALL`, `DOG`,
/// ...), trimmed, `Unknown` when the row has none, as a column hash (DAOrigins.exe
/// 0x6a096e..0x6a09dc).
pub fn footstep_column(cell: Option<&thedas_formats::gda::Cell>) -> u32 {
    let name = match cell {
        Some(thedas_formats::gda::Cell::String(s)) => {
            s.trim_matches(|c| c == '\0' || c == ' ' || c == '\t')
        }
        _ => "Unknown",
    };
    thedas_formats::gda::column_hash(name)
}

/// `sound_item_materials.gda`: each sound material's label (`Armor-leather`, ...) as a
/// column hash, the column DAOrigins.exe's bodyfall reads its armour sweetener from
/// (0x5b6f60: the row's `Label`, hashed).
fn item_material_labels(vfs: &Vfs) -> std::collections::HashMap<i64, u32> {
    use thedas_formats::gda::Cell;
    let mut out = std::collections::HashMap::new();
    let Some(t) = read_gda(vfs, "sound_item_materials.gda") else {
        return out;
    };
    let Some(lc) = t.columns.iter().position(|c| c.hash == 0xb49c_a166) else {
        return out;
    };
    for row in &t.rows {
        if let (Some(Cell::Int(id)), Some(Cell::String(label))) = (row.first(), row.get(lc)) {
            let label = label.trim_end_matches('\0').trim();
            out.insert(*id, thedas_formats::gda::column_hash(label));
        }
    }
    out
}

fn read_gda(vfs: &Vfs, name: &str) -> Option<thedas_formats::Gda> {
    vfs.read(name)
        .ok()
        .and_then(|(d, _)| thedas_formats::Gda::parse(&d).ok())
}

/// The event in row `row`, column `col` (by hash) of a sound table, if any.
fn table_event(t: &thedas_formats::Gda, row: i64, col: u32) -> Option<String> {
    let c = t.columns.iter().position(|c| c.hash == col)?;
    match t.row_by_id(row)?.get(c)? {
        thedas_formats::gda::Cell::String(s) => {
            let s = s.trim_end_matches('\0').trim();
            (!s.is_empty()).then(|| s.to_string())
        }
        _ => None,
    }
}

/// `ss_types.gda`: each sound set type (1 COMBAT ATTACK, 2 COMBAT BATTLE CRY, ...) with the
/// row its entries start at in a set's `sse_<set>.gda` (100, 200, ...) and the chance a
/// line of it is said.
fn ss_types(vfs: &Vfs) -> std::collections::HashMap<i32, (i64, f32)> {
    use thedas_formats::gda::Cell;
    let mut out = std::collections::HashMap::new();
    let Some(t) = vfs
        .read("ss_types.gda")
        .ok()
        .and_then(|(d, _)| thedas_formats::Gda::parse(&d).ok())
    else {
        return out;
    };
    for row in &t.rows {
        let nums: Vec<f64> = row
            .iter()
            .filter_map(|c| match c {
                Cell::Int(i) => Some(*i as f64),
                Cell::Float(f) => Some(*f),
                _ => None,
            })
            .collect();
        // ID, first entry row, probability, ...
        if let [id, base, p, ..] = nums[..] {
            out.insert(id as i32, (base as i64, p as f32));
        }
    }
    out
}

/// `sound_weapon_actions.gda` and `sound_movement_foley.gda`: events by (row, column hash).
#[derive(Default)]
struct Foley {
    tables: std::collections::HashMap<&'static str, std::collections::HashMap<(i64, u32), String>>,
}

const COL_SWISH: u32 = 0xa334_7e2b;
/// `sound_movement_footsteps.gda`'s `HUMANOID_SMALL` column.
const COL_HUMANOID_SMALL: u32 = 0x787a_7bc5;

impl Foley {
    fn read(vfs: &Vfs) -> Foley {
        use thedas_formats::gda::Cell;
        let mut out = Foley::default();
        for (key, name) in [
            ("weapon", "sound_weapon_actions.gda"),
            ("movement", "sound_movement_foley.gda"),
        ] {
            let Some(t) = vfs
                .read(name)
                .ok()
                .and_then(|(d, _)| thedas_formats::Gda::parse(&d).ok())
            else {
                continue;
            };
            let mut cells = std::collections::HashMap::new();
            for row in &t.rows {
                let Some(Cell::Int(id)) = row.first() else {
                    continue;
                };
                for (c, cell) in t.columns.iter().zip(row) {
                    if let Cell::String(s) = cell {
                        let s = s.trim_end_matches('\0').trim();
                        if s.contains('/') {
                            cells.insert((*id, c.hash), s.to_string());
                        }
                    }
                }
            }
            out.tables.insert(key, cells);
        }
        out
    }

    fn cell(&self, table: &str, row: Option<i64>, column: u32) -> Option<String> {
        self.tables.get(table)?.get(&(row?, column)).cloned()
    }
}

/// `partypicker.gda`: per follower template (column 0x88578372), the sound set entries said
/// when picked (0x50853e6b: 47 YES) and unpicked (0xfd7493a7: 54 CUSS), as the picker's
/// code reads them (DAOrigins.exe 0x767a2a, 0x767a45); the `Default` row serves the rest.
fn picker_lines(vfs: &Vfs) -> std::collections::HashMap<String, (i32, i32)> {
    use thedas_formats::gda::Cell;
    let mut out = std::collections::HashMap::new();
    let Some(t) = vfs
        .read("partypicker.gda")
        .ok()
        .and_then(|(d, _)| thedas_formats::Gda::parse(&d).ok())
    else {
        return out;
    };
    let col = |h: u32| t.columns.iter().position(|c| c.hash == h);
    let (Some(tc), Some(lc), Some(sc), Some(uc)) = (
        col(0x8857_8372),
        col(0xb49c_a166),
        col(0x5085_3e6b),
        col(0xfd74_93a7),
    ) else {
        return out;
    };
    for row in &t.rows {
        let text = |i: usize| match row.get(i) {
            Some(Cell::String(s)) => s.trim_end_matches('\0').trim().to_ascii_lowercase(),
            _ => String::new(),
        };
        let num = |i: usize| match row.get(i) {
            Some(Cell::Int(v)) => *v as i32,
            _ => 0,
        };
        let key = if text(lc) == "default" {
            "default".to_string()
        } else {
            text(tc)
        };
        if !key.is_empty() && num(sc) > 0 {
            out.insert(key, (num(sc), num(uc)));
        }
    }
    out
}

/// What an attack's blow sounds like.
///
/// `BITM_base.gda`'s column 0x7e0505a1 gives each base item a sound material
/// (`sound_item_materials.gda`: 1 sword, 2 axe, 3 blunt, 4 dagger, 5/6 wood/steel shield,
/// 7 staff, 8 bow, 9 crossbow, 10-13 leather/cloth/chain/metal armour, ...); found by its
/// values (every weapon kind matches), the column's name is not known. A blow plays
/// `sound_weapon_hits.gda`'s event for the weapon's material (bare hands without one) and
/// what it struck: the target's armour (flesh without), its shield when blocked (wood or
/// metal). Misses play nothing here: the swing's whoosh is the animation's own event.
/// Provisional: creatures' hides (fur, scale, bone, stone) are taken as flesh.
#[derive(Default)]
struct Impacts {
    materials: std::collections::HashMap<i64, i64>,
    hits: std::collections::HashMap<i64, Vec<String>>,
}

const COL_SOUND_MATERIAL: u32 = 0x7e05_05a1;
const BARE_HANDS: i64 = 19;

impl Impacts {
    fn read(vfs: &Vfs) -> Impacts {
        let table = |name: &str| {
            vfs.read(name)
                .ok()
                .and_then(|(d, _)| thedas_formats::Gda::parse(&d).ok())
        };
        let mut out = Impacts::default();
        if let Some(t) = table("BITM_base.gda")
            && let Some(col) = t.columns.iter().position(|c| c.hash == COL_SOUND_MATERIAL)
        {
            for row in &t.rows {
                let id = match row.first() {
                    Some(thedas_formats::gda::Cell::Int(i)) => *i,
                    _ => continue,
                };
                if let Some(thedas_formats::gda::Cell::Int(m)) = row.get(col) {
                    out.materials.insert(id, *m);
                }
            }
        }
        if let Some(t) = table("sound_weapon_hits.gda") {
            for row in &t.rows {
                let id = match row.first() {
                    Some(thedas_formats::gda::Cell::Int(i)) => *i,
                    _ => continue,
                };
                let events = row
                    .iter()
                    .filter_map(|c| match c {
                        thedas_formats::gda::Cell::String(s) if s.contains('/') => {
                            Some(s.trim_end_matches('\0').trim().to_string())
                        }
                        _ => None,
                    })
                    .collect();
                out.hits.insert(id, events);
            }
        }
        out
    }

    /// A base item's sound material.
    fn material(&self, base: Option<i32>) -> Option<i64> {
        base.and_then(|b| self.materials.get(&(b as i64)).copied())
    }

    /// The event a blow plays, if any.
    fn event(
        &self,
        result: i32,
        weapon: Option<i32>,
        armour: Option<i32>,
        shield: Option<i32>,
    ) -> Option<String> {
        const HIT: i32 = 1;
        const CRITICAL: i32 = 2;
        const DEATHBLOW: i32 = 4;
        const BLOCKED: i32 = 5;
        const BACKSTAB: i32 = 9;
        let material = |b: Option<i32>| b.and_then(|b| self.materials.get(&(b as i64)).copied());
        let w = material(weapon).filter(|m| self.hits.get(m).is_some_and(|e| !e.is_empty()));
        let events = self.hits.get(&w.unwrap_or(BARE_HANDS))?;
        let ending = match result {
            BLOCKED => match material(shield) {
                Some(6) => "/impact_materials/metal",
                Some(_) => "/impact_materials/wood",
                None => return None,
            },
            HIT | CRITICAL | DEATHBLOW | BACKSTAB => match material(armour) {
                Some(10) => "/impact_armor/leather_arm",
                Some(11) => "/impact_armor/cloth_arm",
                Some(12) => "/impact_armor/chain_arm",
                Some(13) => "/impact_armor/metal_arm",
                _ => "/impact_armor/flesh",
            },
            _ => return None,
        };
        events.iter().find(|e| e.ends_with(ending)).cloned()
    }
}

/// `sound_gui.gda`: the interface sound ids' events.
fn gui_table(vfs: &Vfs) -> Vec<Option<String>> {
    let Ok((data, _)) = vfs.read("sound_gui.gda") else {
        log::warn!("audio: no sound_gui.gda; interface sounds are off");
        return Vec::new();
    };
    let Ok(gda) = thedas_formats::Gda::parse(&data) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for id in 0..512i64 {
        let Some(row) = gda.row_by_id(id) else {
            continue;
        };
        let event = row.iter().rev().find_map(|c| match c {
            thedas_formats::gda::Cell::String(s) if s.contains('/') => {
                Some(s.trim_end_matches('\0').trim().to_string())
            }
            _ => None,
        });
        if out.len() <= id as usize {
            out.resize(id as usize + 1, None);
        }
        out[id as usize] = event;
    }
    out
}

impl GameAudio {
    /// The game's sound on the default output; `None` (logged) with no output device.
    pub fn open(install: &Path, module: &str, vfs: &Arc<Vfs>, language: &str) -> Option<GameAudio> {
        let output = thedas_audio::device::Output::open()?;
        let mixer = output.mixer.clone();
        Some(Self::with_mixer(
            install,
            module,
            vfs,
            language,
            mixer,
            Some(output),
        ))
    }

    /// The game's sound mixed into [`GameAudio::recorded`] (stereo at `rate`) instead of the
    /// speakers, `frame_dt`'s worth a frame: for unattended runs (`--audio-wav`). Decoding is
    /// done in step, so the recording does not depend on timing.
    pub fn recording(
        install: &Path,
        module: &str,
        vfs: &Arc<Vfs>,
        language: &str,
        rate: u32,
    ) -> GameAudio {
        let mixer = Arc::new(std::sync::Mutex::new(thedas_audio::Mixer::new(rate)));
        let mut a = Self::with_mixer(install, module, vfs, language, mixer, None);
        a.engine.synchronous = true;
        a
    }

    fn with_mixer(
        install: &Path,
        module: &str,
        vfs: &Arc<Vfs>,
        language: &str,
        mixer: Arc<std::sync::Mutex<thedas_audio::Mixer>>,
        output: Option<thedas_audio::device::Output>,
    ) -> GameAudio {
        let library = Arc::new(Library::new(Library::install_dirs(
            install, module, language,
        )));
        let engine = AudioEngine::new(library, mixer);
        GameAudio {
            engine,
            _output: output,
            recorded: Vec::new(),
            gui: gui_table(vfs),
            music: None,
            voice: None,
            cutscene: None,
            area: None,
            movie: None,
            vfs: vfs.clone(),
            anim_events: Default::default(),
            anim_clocks: Default::default(),
            footsteps: read_gda(vfs, "sound_movement_footsteps.gda"),
            scuffs: read_gda(vfs, "sound_movement_scuffing.gda"),
            vegetation: read_gda(vfs, "sound_vegetation_type.gda"),
            bodyfalls: read_gda(vfs, "sound_bodyfall_creat_size.gda"),
            bodyfall_foley: read_gda(vfs, "sound_bodyfall_foley.gda"),
            item_material_labels: item_material_labels(vfs),
            rustles: Default::default(),
            impacts: Impacts::read(vfs),
            foley: Foley::read(vfs),
            music_state: 1.0,
            ss_types: ss_types(vfs),
            sound_set_entries: Default::default(),
            sound_set_bags: Default::default(),
            hero_female: false,
            rng: 0x2545_f491_4f6c_dd1d,
            template_sets: Default::default(),
            picker_lines: picker_lines(vfs),
            picked: None,
            picker_spoke: f32::INFINITY,
            last: Instant::now(),
        }
    }

    /// A creature template's sound set (its `.utc`'s `SoundSet`), read once.
    pub fn template_sound_set(&mut self, vfs: &Vfs, template: &str) -> Option<String> {
        use thedas_formats::gff3::{Gff3, Value};
        let key = template.to_ascii_lowercase();
        if let Some(s) = self.template_sets.get(&key) {
            return s.clone();
        }
        let set = vfs
            .read(&format!("{key}.utc"))
            .ok()
            .and_then(|(d, _)| Gff3::parse(&d).ok())
            .and_then(|g| {
                g.root()
                    .fields
                    .iter()
                    .find(|f| f.label == "SoundSet")
                    .and_then(|f| match &f.value {
                        Value::ResRef(b) | Value::ExoString(b) => {
                            let s = String::from_utf8_lossy(b)
                                .trim_end_matches('\0')
                                .trim()
                                .to_ascii_lowercase();
                            (!s.is_empty()).then_some(s)
                        }
                        _ => None,
                    })
            });
        self.template_sets.insert(key, set.clone());
        set
    }

    fn random(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    /// A line of sound set `set` of type `entry` (`ss_types.gda`), as DAOrigins.exe's
    /// sound-set player says it (0x6b5740):
    /// - `sst_<set>.gda` gives the type's number of lines `n` (column 0x1bd4c5a2, row = the
    ///   type); none, nothing is said. The lines are `sse_<set>.gda` rows `100 * type + k`
    ///   for `k < n` (column 0xd7bb7367: the line's strref).
    /// - With more than one, the next is drawn from a bag of the type's lines that refills
    ///   once all have been drawn (0x5b8890, 0x5b8970), whether or not it is then said.
    /// - It is said with probability `probability` (0: the type's, `ss_types.gda` column
    ///   0xa3000ead), against `(rand() % 100 + 1) / 100`.
    /// - The line is named as a conversation line is (0x7f2b70): `<set>/VO/<strref>` with
    ///   `_f` for a female hero when the event `<set>/VO/<strref>_f` exists, else `_m`. An
    ///   event of that name (creature sets: `ss_wolf/VO/179451_m`, which picks among the
    ///   line's variants) plays as itself; otherwise the set's `VO/VOambient_ss` plays the
    ///   sample of that name (voiced sets: `179996_m`).
    ///
    /// Provisional: the bag's draw order (taken as random), a per-creature multiplier on
    /// the probability for some types (table 0x119, column 0x29f4e2fb, when the type's
    /// column 0x6ddd6b8b is set) and a creature flag (+0x969) that makes it certain; how
    /// the name is played (the event when it exists, else the sample) is inferred from the
    /// data, as the builder returns both.
    fn say_sound_set(&mut self, set: &str, entry: i32, probability: f32, at: Option<[f32; 3]>) {
        let Some(&(_, p)) = self.ss_types.get(&entry) else {
            log::debug!("audio: no sound set type {entry}");
            return;
        };
        let key = set.to_ascii_lowercase();
        let entries = match self.sound_set_entries.get(&key) {
            Some(e) => e.clone(),
            None => {
                use thedas_formats::gda::Cell;
                let rows: std::collections::HashMap<i64, i64> =
                    read_gda(&self.vfs, &format!("sse_{key}.gda"))
                        .map(|t| {
                            let col = t
                                .columns
                                .iter()
                                .position(|c| c.hash == 0xd7bb_7367)
                                .unwrap_or(1);
                            t.rows
                                .iter()
                                .filter_map(|r| match (r.first(), r.get(col)) {
                                    (Some(Cell::Int(id)), Some(Cell::Int(s))) => Some((*id, *s)),
                                    _ => None,
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                let counts: std::collections::HashMap<i64, i64> =
                    read_gda(&self.vfs, &format!("sst_{key}.gda"))
                        .map(|t| {
                            let col = t
                                .columns
                                .iter()
                                .position(|c| c.hash == 0x1bd4_c5a2)
                                .unwrap_or(1);
                            t.rows
                                .iter()
                                .filter_map(|r| match (r.first(), r.get(col)) {
                                    (Some(Cell::Int(id)), Some(Cell::Int(n))) => Some((*id, *n)),
                                    _ => None,
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                let e = Arc::new((rows, counts));
                self.sound_set_entries.insert(key.clone(), e.clone());
                e
            }
        };
        let (rows, counts) = &*entries;
        let n = counts.get(&(entry as i64)).copied().unwrap_or(0);
        if n <= 0 {
            return;
        }
        let first = 100 * entry as i64;
        let strref = if n == 1 {
            rows.get(&first).copied()
        } else {
            let bag_key = (key.clone(), entry);
            if self
                .sound_set_bags
                .get(&bag_key)
                .is_none_or(|b| b.is_empty())
            {
                let all: Vec<i64> = (0..n)
                    .filter_map(|k| rows.get(&(first + k)).copied())
                    .collect();
                self.sound_set_bags.insert(bag_key.clone(), all);
            }
            let r = self.random();
            self.sound_set_bags
                .get_mut(&bag_key)
                .and_then(|b| (!b.is_empty()).then(|| b.swap_remove((r % b.len() as u64) as usize)))
        };
        let Some(strref) = strref.filter(|s| *s > 0) else {
            return;
        };
        let chance = if probability > 0.0 { probability } else { p };
        let roll = (self.random() % 100 + 1) as f32 / 100.0;
        if chance < roll {
            return;
        }
        let project = self.engine.library.project(&key);
        let has_event = |name: &str| project.as_ref().is_some_and(|p| p.event(name).is_some());
        let female_event = format!("{key}/VO/{strref}_f");
        let suffix = if self.hero_female && has_event(&female_event) {
            "f"
        } else {
            "m"
        };
        let event = format!("{key}/VO/{strref}_{suffix}");
        if has_event(&event) {
            self.engine.play(
                &event,
                PlayOptions {
                    position: at,
                    ..Default::default()
                },
            );
            return;
        }
        let sample = format!("{strref}_{suffix}");
        if self.engine.library.find_sample(&key, &sample).is_none() {
            log::debug!("audio: sound set {key} has no event or sample for {strref}_{suffix}");
            return;
        }
        self.engine.play(
            &format!("{key}/VO/VOambient_ss"),
            PlayOptions {
                programmer_sound: Some(sample),
                position: at,
                ..Default::default()
            },
        );
    }

    /// Animations' sound events: as a creature's animation passes an event, kind 20 plays the
    /// event it names (weapon swings, creature noises: `glo_fly_wep/...`) and kind 14 a
    /// footstep for the ground under the creature where it stands.
    ///
    /// DAOrigins.exe gates footsteps and armour foley (0x7c4a80): a small or medium
    /// creature (+0x934 `Small`/`Medium`) whose model is at its lowest level of detail
    /// (AnimBase +0x150 == 3: four levels, 0x617020 falls back between them) makes neither,
    /// except in dialog or a cutscene (game modes 4, 5). Only drawn creatures' animations
    /// reach here, which stands for that. Provisional: events on the frame an animation
    /// starts are those at its time 0 onward.
    fn follow_animations(
        &mut self,
        clocks: Vec<(u32, String, f32, f32, [f32; 3])>,
        gear: &std::collections::HashMap<u32, Gear>,
    ) {
        let mut seen = std::collections::HashMap::new();
        // Vegetation (DAOrigins.exe 0x6a5510, run as a creature moves): once it is a
        // random 0.89..1.09 m (squared: `rand() % 100 * 0.004 + 0.8`) from where it last
        // rustled, standing in vegetation (the ground's high nibble) plays
        // `sound_vegetation_type.gda`'s (worksheet 155) event for that vegetation in its size
        // column.
        // Provisional: a creature first seen here starts from where it stands (the exe
        // starts every creature from the origin with 1.0, 0x6b42f2).
        let mut rustled = std::collections::HashMap::new();
        for (id, _, _, _, pos) in &clocks {
            let g = gear.get(id).copied().unwrap_or_default();
            let (last, reach) = self.rustles.get(id).copied().unwrap_or((*pos, 1.0));
            let d2: f32 = (0..3).map(|i| (pos[i] - last[i]).powi(2)).sum();
            let veg = (g.surface >> 4) as i64;
            if d2 < reach || veg == 0 {
                rustled.insert(*id, (last, reach));
                continue;
            }
            let event = match (&self.vegetation, g.size) {
                (Some(t), Some(col)) => table_event(t, veg, col),
                _ => None,
            };
            match &event {
                Some(event) => {
                    log::debug!(
                        "audio: vegetation rustle {event} (creature {id}, vegetation {veg})"
                    );
                    self.engine.play(
                        event,
                        PlayOptions {
                            position: Some(*pos),
                            ..Default::default()
                        },
                    );
                }
                None => log::debug!(
                    "audio: vegetation {veg} under creature {id} has no rustle (size {:?})",
                    g.size
                ),
            }
            let reach = (self.random() % 100) as f32 * 0.004 + 0.8;
            rustled.insert(*id, (*pos, reach));
        }
        self.rustles = rustled;
        for (id, ani, t, length, pos) in clocks {
            let key = ani.to_ascii_lowercase();
            let prev = self.anim_clocks.get(&id).cloned();
            seen.insert(id, (key.clone(), t));
            let events = match self.anim_events.get(&key) {
                Some(e) => e.clone(),
                None => {
                    let stem = key.strip_suffix(".ani").unwrap_or(&key);
                    let e = Arc::new(
                        self.vfs
                            .read(&format!("{stem}.evt"))
                            .ok()
                            .and_then(|(d, _)| {
                                thedas_formats::anim::parse_events_with_text(&d).ok()
                            })
                            .unwrap_or_default(),
                    );
                    self.anim_events.insert(key.clone(), e.clone());
                    e
                }
            };
            if events.is_empty() {
                continue;
            }
            // Which stretch of the animation passed since last frame.
            let crossed = |e: f32| match &prev {
                Some((k, p)) if *k == key => {
                    if t >= *p {
                        e > *p && e <= t
                    } else {
                        // Wrapped (a loop).
                        e > *p && e <= length || e <= t
                    }
                }
                // A new animation: the events at its start, if it has just started.
                _ => e <= t && t < 0.25,
            };
            for ev in events.iter().filter(|e| crossed(e.time)) {
                let event = match (ev.kind, &ev.text) {
                    (20, Some(path)) => Some(path.clone()),
                    // A swing (DAOrigins.exe 0x69e940): `sound_weapon_actions.gda`'s Swish
                    // for the weapon's sound material, the off hand's when the event says
                    // `Left`.
                    (19, text) => {
                        let g = gear.get(&id).copied().unwrap_or_default();
                        let left = text.as_deref().is_some_and(|t| t.contains("Left"));
                        let w = if left { g.off } else { g.main };
                        let m = self.impacts.material(w);
                        self.foley.cell("weapon", m, COL_SWISH)
                    }
                    // Armour foley (0x69e7f0): `sound_movement_foley.gda`'s column the event
                    // names (`Small`, `Medium`, `Large`) for the armour's sound material
                    // (none: row 0).
                    (17, Some(size)) => {
                        let g = gear.get(&id).copied().unwrap_or_default();
                        let col = thedas_formats::gda::column_hash(size.trim());
                        let m = self.impacts.material(g.armour).unwrap_or(0);
                        self.foley.cell("movement", Some(m), col)
                    }
                    // A bodyfall (kind 18: 0x6a5ad0 -> 0x5b8cc1): the body's event from
                    // `sound_bodyfall_creat_size.gda` (the ground's material, the creature's
                    // size), and its armour's from `sound_bodyfall_foley.gda` (the ground's
                    // material, the armour's sound material by label), where there is one.
                    // Provisional: the creature's armour material (+0x930) is taken from
                    // its chest item, as for its foley.
                    (18, _) => {
                        let g = gear.get(&id).copied().unwrap_or_default();
                        let row = (g.surface & 0xf) as i64;
                        let armour = self.impacts.material(g.armour).unwrap_or(0);
                        log::trace!(
                            "audio: bodyfall: armour item {:?} material {armour}",
                            g.armour
                        );
                        let sweetener = self.item_material_labels.get(&armour).and_then(|col| {
                            self.bodyfall_foley
                                .as_ref()
                                .and_then(|t| table_event(t, row, *col))
                        });
                        if let Some(e) = sweetener {
                            self.engine.play(
                                &e,
                                PlayOptions {
                                    position: Some(pos),
                                    ..Default::default()
                                },
                            );
                        }
                        let size = g
                            .size
                            .unwrap_or(thedas_formats::gda::column_hash("Unknown"));
                        self.bodyfalls
                            .as_ref()
                            .and_then(|t| table_event(t, row, size))
                    }
                    // A footstep (0x6aaaa0 -> 0x5b8d51): the ground's material under the
                    // creature picks the row; an event marked `|Scuff` plays the scuffing
                    // table's event, any other the footstep table's in the creature's
                    // footstep column.
                    // A material the table has no row for (13, Metal) plays nothing, as
                    // the exe's lookup fails (0x5b8d92).
                    (14, text) => {
                        let g = gear.get(&id).copied().unwrap_or_default();
                        let row = (g.surface & 0xf) as i64;
                        if text.as_deref().is_some_and(|t| t.contains("|Scuff")) {
                            self.scuffs
                                .as_ref()
                                .and_then(|t| table_event(t, row, COL_SCUFF))
                        } else {
                            let col = g.footstep.unwrap_or(COL_HUMANOID_SMALL);
                            self.footsteps
                                .as_ref()
                                .and_then(|t| table_event(t, row, col))
                        }
                    }
                    _ => None,
                };
                if let Some(event) = event {
                    log::trace!(
                        "audio: {key} {:?} -> {t:.3} (len {length:.3}): {event} at {:.3}",
                        prev.as_ref().map(|p| p.1),
                        ev.time
                    );
                    self.engine.play(
                        &event,
                        PlayOptions {
                            position: Some(pos),
                            ..Default::default()
                        },
                    );
                }
            }
        }
        self.anim_clocks = seen;
    }

    /// A boot movie's own sound track plays along with it (DA:O's logos carry theirs in the
    /// Bink file; the cutscenes' movies are silent and their sound is the cutscene's).
    /// Provisional: played at full volume, outside the categories.
    fn follow_movie(&mut self, movie: Option<(std::path::PathBuf, f64)>) {
        let same = matches!((&self.movie, &movie), (Some(m), Some((p, _))) if &m.path == p);
        if !same {
            if let Some(v) = self.movie.take().and_then(|m| m.voice)
                && let Ok(mut mx) = self.engine.mixer.lock()
            {
                mx.fade(v, 0.0, 0.05, true);
            }
            if let Some((path, _)) = &movie {
                let (tx, rx) = std::sync::mpsc::channel();
                let p = path.clone();
                let rate = self.engine.mixer.lock().map(|m| m.rate()).unwrap_or(48000);
                std::thread::spawn(move || {
                    let _ = tx.send(decode_movie_audio(&p, rate));
                });
                self.movie = Some(MovieSound {
                    path: path.clone(),
                    decoding: Some(rx),
                    voice: None,
                });
            }
        }
        let (Some(m), Some((_, ms))) = (self.movie.as_mut(), movie) else {
            return;
        };
        if let Some(rx) = &m.decoding
            && let Ok(pcm) = rx.try_recv()
        {
            m.decoding = None;
            if let (Some(pcm), Ok(mut mx)) = (pcm, self.engine.mixer.lock()) {
                let start = (ms / 1000.0 * pcm.rate as f64) as usize;
                if start < pcm.frames() {
                    let gain = self.engine.volumes.sfx;
                    m.voice = Some(mx.play_from(
                        Arc::new(pcm),
                        thedas_audio::VoiceParams {
                            gain,
                            ..Default::default()
                        },
                        start,
                    ));
                }
            }
        }
    }

    /// Whether area `resref`'s sound (its `.are`) is needed (the party has just arrived).
    pub fn wants_area(&self, resref: &str) -> bool {
        self.area.as_ref().is_none_or(|a| a.resref != resref)
    }

    pub fn set_area(&mut self, resref: &str, data: &[u8]) {
        self.end_area();
        let (theme, sounds) = area_audio(data);
        let reverb = area_reverb(data);
        log::info!(
            "audio: area {resref}: music {theme:?}, {} placed sounds, reverb {:?} and {} \
             reverb triggers",
            sounds.len(),
            reverb.preset,
            reverb.triggers.len()
        );
        let n = sounds.len();
        self.area = Some(AreaPlay {
            resref: resref.to_string(),
            theme,
            track: None,
            sounds,
            playing: vec![None; n],
            reverb,
            preset: None,
            inside: Vec::new(),
        });
    }

    fn end_area(&mut self) {
        if let Some(a) = self.area.take() {
            if a.preset.is_some() {
                self.engine
                    .set_reverb(thedas_formats::fev::ReverbProperties::OFF, 0.0);
            }
            for h in a
                .playing
                .into_iter()
                .flatten()
                .flatten()
                .chain(a.track.map(|t| t.1))
            {
                self.engine.stop(h, false);
            }
        }
    }

    /// The area's music and placed sounds follow the game.
    ///
    /// Music: the area's theme plays `main` while exploring, `combat` while the party
    /// fights and `Death` once it has fallen, each with `music_state` 1 (their sounds are
    /// placed around 1 on that parameter), and the theme's `stingers/from_main`,
    /// `to_main` or `to_death` marks the change. Placed sounds play their events where they
    /// stand while the listener is within their reach and the simulation has them on.
    ///
    /// Provisional, not traced in DAOrigins.exe: when the music changes (it waits for
    /// markers in the original; `sound_settings.gda` "max time between markers"), the
    /// stinger choice, cutscenes and conversations leaving the music alone, and scripts'
    /// `PlayMusic`/`SetMusicIntensity` (ignored).
    fn follow_area(&mut self, state: &AudioState) {
        if state.area.is_none() {
            self.end_area();
            return;
        }
        let Some(a) = self.area.as_mut() else {
            return;
        };
        if let Some(theme) = a.theme.clone() {
            let want = if state.dead {
                "Death"
            } else if state.combat {
                "combat"
            } else {
                "main"
            };
            if a.track.map(|t| t.0) != Some(want) {
                let from = a.track.map(|t| t.0);
                if let Some((_, h)) = a.track.take() {
                    self.engine.stop(h, false);
                }
                let stinger = match (from, want) {
                    (Some(_), "Death") => Some("to_death"),
                    (Some("main"), "combat") => Some("from_main"),
                    (Some("combat"), "main") => Some("to_main"),
                    _ => None,
                };
                if let Some(st) = stinger {
                    self.engine
                        .play(&format!("{theme}/stingers/{st}"), PlayOptions::default());
                }
                let h = self.engine.play(
                    &format!("{theme}/{want}"),
                    PlayOptions {
                        parameters: vec![("music_state".into(), self.music_state)],
                        ..Default::default()
                    },
                );
                a.track = h.map(|h| (want, h));
                if a.track.is_none() {
                    // No such track: try again only when the state changes.
                    log::debug!("audio: no music {theme}/{want}");
                }
            }
        }
        // The reverb (see `AreaReverb`): the area's starting preset at once on arrival,
        // then each reverb trigger the party leader enters sets its own, faded over 0.33 s
        // (DAOrigins.exe 0xaff1c8).
        let mut want = a
            .preset
            .clone()
            .unwrap_or_else(|| a.reverb.initial().to_string());
        if let Some(p) = state.leader.or(state.listener.map(|l| l.0)) {
            let (now, entered) = a.reverb.entered(&a.inside, p);
            if let Some(e) = entered {
                want = e.to_string();
            }
            a.inside = now;
        }
        let want = want.as_str();
        if a.preset.as_deref() != Some(want) {
            let props = self.engine.reverb_preset(want).unwrap_or_else(|| {
                log::debug!("audio: no reverb preset {want:?}");
                thedas_formats::fev::ReverbProperties::OFF
            });
            let fade = if a.preset.is_some() { 0.33 } else { 0.0 };
            log::debug!("audio: reverb {want:?} over {fade} s");
            self.engine.set_reverb(props, fade);
            a.preset = Some(want.to_string());
        }
        let Some((listener, _)) = state.listener else {
            return;
        };
        for i in 0..a.sounds.len() {
            let s = &a.sounds[i];
            let on = s.active && !state.inactive_sounds.contains(&s.tag);
            let d = {
                let v = [
                    s.position[0] - listener[0],
                    s.position[1] - listener[1],
                    s.position[2] - listener[2],
                ];
                (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
            };
            // Within reach: the event's max distance, scaled, with a margin; events read
            // lazily (an unreachable sound costs nothing).
            let reach = {
                let project = s.event.split('/').next().unwrap_or("");
                self.engine
                    .library
                    .project(project)
                    .and_then(|f| f.event(&s.event).map(|e| e.max_distance))
                    .unwrap_or(0.0)
                    * s.max_distance_mult.max(0.0)
                    * 1.1
            };
            let near = d <= reach.max(1.0);
            match (a.playing[i], on && near) {
                (None, true) => {
                    a.playing[i] = Some(self.engine.play(
                        &s.event,
                        PlayOptions {
                            position: Some(s.position),
                            volume: 10f32.powf(s.volume / 20.0),
                            distance_scale: s.max_distance_mult.max(1e-3),
                            ..Default::default()
                        },
                    ));
                }
                (Some(p), false) => {
                    if let Some(h) = p {
                        self.engine.stop(h, false);
                    }
                    a.playing[i] = None;
                }
                (Some(Some(h)), true) if !self.engine.is_playing(h) => {
                    a.playing[i] = Some(None);
                }
                _ => {}
            }
        }
    }

    /// Whether the sound tracks of cutscene `name` are needed (it has just started).
    pub fn wants_cutscene(&self, name: &str) -> bool {
        self.cutscene.as_ref().is_none_or(|c| c.name != name)
    }

    /// The sound tracks (`ACSP`) of cutscene `name`, read when it starts.
    pub fn set_cutscene(&mut self, name: &str, tracks: Vec<thedas_sim::cutscene::CutsceneSound>) {
        self.end_cutscene();
        if !tracks.is_empty() {
            log::info!(
                "cutscene {name}: sounds {:?}",
                tracks
                    .iter()
                    .map(|t| format!("{} at {:.2}s", t.event, t.start))
                    .collect::<Vec<_>>()
            );
        }
        let n = tracks.len();
        self.cutscene = Some(CutsceneSounds {
            name: name.to_string(),
            tracks,
            playing: vec![None; n],
            done: vec![false; n],
        });
    }

    fn end_cutscene(&mut self) {
        if let Some(c) = self.cutscene.take() {
            for h in c.playing.into_iter().flatten() {
                self.engine.stop(h, false);
            }
        }
    }

    /// Starts and stops the cutscene's sound tracks at their times: an `ACSP` track plays
    /// its event from its start until its end, or until the cutscene ends (or is skipped).
    /// Provisional: a track whose start was missed (a cutscene joined late) does not play.
    fn follow_cutscene(&mut self, cutscene: Option<(String, f32)>) {
        let Some((name, t)) = cutscene else {
            self.end_cutscene();
            return;
        };
        let Some(c) = self.cutscene.as_mut().filter(|c| c.name == name) else {
            return;
        };
        for i in 0..c.tracks.len() {
            let tr = &c.tracks[i];
            if c.done[i] {
                continue;
            }
            if c.playing[i].is_none() && t >= tr.start {
                if t > tr.start + 1.0 {
                    c.done[i] = true;
                    continue;
                }
                c.playing[i] = self.engine.play(&tr.event, PlayOptions::default());
                if c.playing[i].is_none() {
                    log::warn!("cutscene {name}: no sound event {}", tr.event);
                    c.done[i] = true;
                }
            } else if let Some(h) = c.playing[i]
                && t >= tr.end
            {
                self.engine.stop(h, false);
                c.playing[i] = None;
                c.done[i] = true;
            }
        }
    }

    /// How long the current conversation line's voice-over runs (milliseconds), once known.
    pub fn voice_ms(&self) -> Option<f64> {
        self.voice.as_ref().and_then(|v| v.2)
    }

    /// The game's sound this frame, on the system output's clock.
    pub fn frame(&mut self, state: AudioState) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.25);
        self.last = now;
        self.frame_dt(state, dt);
    }

    /// A recording's frame: `dt` seconds of the game, mixed into [`GameAudio::recorded`].
    pub fn record(&mut self, state: AudioState, dt: f32) {
        self.frame_dt(state, dt);
        let Ok(mut m) = self.engine.mixer.lock() else {
            return;
        };
        let n = (dt * m.rate() as f32).round() as usize;
        let mut buf = vec![0.0; n * 2];
        m.render(&mut buf);
        self.recorded.extend(buf);
    }

    fn frame_dt(&mut self, mut state: AudioState, dt: f32) {
        self.engine.volumes = state.volumes;
        self.hero_female = state.hero_female;
        if let Some((pos, right)) = state.listener {
            self.engine.set_listener(pos, right);
        }
        // Menu music.
        let want = if state.chargen {
            Some("music/_menu/chargen")
        } else if state.front_end {
            Some("music/_menu/main")
        } else {
            None
        };
        if self.music.map(|m| m.0) != want {
            if let Some((_, h)) = self.music.take() {
                self.engine.stop(h, false);
            }
            if let Some(path) = want {
                self.music = self
                    .engine
                    .play(path, PlayOptions::default())
                    .map(|h| (path, h));
            }
        }
        // Interface sounds.
        for s in std::mem::take(&mut state.gui_sounds) {
            let id = s
                .split_whitespace()
                .next()
                .and_then(|t| t.parse::<usize>().ok());
            match id.and_then(|i| self.gui.get(i).cloned().flatten()) {
                Some(event) => {
                    self.engine.play(&event, PlayOptions::default());
                }
                None => log::debug!("audio: GUI sound {s:?} has no event"),
            }
        }
        for (at, result, weapon, armour, shield) in std::mem::take(&mut state.impacts) {
            if let Some(event) = self.impacts.event(result, weapon, armour, shield) {
                self.engine.play(
                    &event,
                    PlayOptions {
                        position: Some(at),
                        ..Default::default()
                    },
                );
            }
        }
        // Sound set lines come from scripts (`PlaySoundSet`; rules and AI scripts pass the
        // grunt and pain types themselves) and character creation's preview. DAOrigins.exe
        // calls its sound set player (0x6b5740) only from the script command (0x6625c6), two
        // message handlers (0x60ba68, 0x6b75a0) and one timed system (0x7660f2, 0x766242);
        // nothing adds barks around each blow or on selection.
        // The party picker (DAOrigins.exe 0x766040, 0x7661b0): picking a follower has it say
        // its picked line, unpicking its unpicked line, at most one every 0.1 s
        // (0xc0d718), always (probability 1).
        self.picker_spoke += dt;
        match state.picker.take() {
            Some(members) => {
                let ids: Vec<u32> = members.iter().map(|m| m.0).collect();
                if let Some(before) = self.picked.replace(ids.clone()) {
                    let changed = members
                        .iter()
                        .filter(|m| !before.contains(&m.0))
                        .map(|m| (m, true))
                        .chain(
                            before
                                .iter()
                                .filter(|b| !ids.contains(b))
                                .filter_map(|b| state.picker_left.get(b).map(|m| (m, false))),
                        )
                        .next();
                    if let Some(((_, template, set, at), picked)) = changed
                        && self.picker_spoke > 0.1
                        && let Some(set) = set
                    {
                        let lines = self
                            .picker_lines
                            .get(&template.to_ascii_lowercase())
                            .or_else(|| self.picker_lines.get("default"))
                            .copied();
                        if let Some((yes, no)) = lines {
                            let entry = if picked { yes } else { no };
                            state.sound_sets.push((set.clone(), entry, 1.0, Some(*at)));
                            self.picker_spoke = 0.0;
                        }
                    }
                }
            }
            None => self.picked = None,
        }
        for (set, entry, probability, at) in std::mem::take(&mut state.sound_sets) {
            self.say_sound_set(&set, entry, probability, at);
        }
        for (event, position) in std::mem::take(&mut state.script_sounds) {
            if self
                .engine
                .play(
                    &event,
                    PlayOptions {
                        position,
                        ..Default::default()
                    },
                )
                .is_none()
            {
                log::debug!("audio: script sound {event} has no event");
            }
        }
        let gear = std::mem::take(&mut state.gear);
        self.follow_animations(std::mem::take(&mut state.animations), &gear);
        self.follow_movie(state.movie.take());
        // Scripts' SetMusicIntensity sets the area music's `music_state` (DAOrigins.exe logs
        // it as "music state= %2.1f"): the tracks' sounds sit around 1, so 0 silences the
        // music (pre100pt_light_beacon) and cutscene endings (gen00cs_cs_end_music_int_*)
        // set 0 to 4. Provisional: read from the data and the scripts, not traced.
        if let Some(i) = state.music_intensity.take()
            && i as f32 != self.music_state
        {
            self.music_state = i as f32;
            if let Some((_, h)) = self.area.as_ref().and_then(|a| a.track) {
                self.engine
                    .set_parameter(h, "music_state", self.music_state);
            }
        }
        self.follow_area(&state);
        self.follow_cutscene(state.cutscene.clone());
        // Voice-over.
        if self.voice.as_ref().map(|v| &v.0) != state.line.as_ref() {
            if let Some((_, Some(h), _)) = self.voice.take() {
                self.engine.stop(h, true);
            }
            self.voice = state.line.map(|line| self.speak(line));
        }
        self.engine.update(dt);
    }

    fn speak(&mut self, line: VoiceLine) -> (VoiceLine, Option<EventHandle>, Option<f64>) {
        let project = line.dialog.to_ascii_lowercase();
        let event = if line.ambient {
            "VOambient"
        } else {
            "VOconversation"
        };
        let lib = self.engine.library.clone();
        let bank = project.clone();
        let female = format!("{}_f", line.strref);
        let male = format!("{}_m", line.strref);
        let sample = if line.female && lib.find_sample(&bank, &female).is_some() {
            Some(female)
        } else if lib.find_sample(&bank, &male).is_some() {
            Some(male)
        } else {
            None
        };
        let Some(sample) = sample else {
            return (line, None, None);
        };
        let ms = lib.sample_seconds(&bank, &sample).map(|s| s * 1000.0);
        let h = self.engine.play(
            &format!("{project}/VO/{event}"),
            PlayOptions {
                programmer_sound: Some(sample),
                ..Default::default()
            },
        );
        (line, h, ms)
    }
}

/// The options screen's volumes (`ClientOptions.AudioOptions.*`, 0..10, with their disable
/// switches) as gains, as DAOrigins.exe turns them into category volumes: a slider at `x`
/// is `-3 * (10 - x)` dB (0x5b61b0: 10 and above full, below 0.01 or disabled -60 dB), then
/// `10^(dB / 20)` with anything under 0.001 silent (0x5b6110). So 5 is -15 dB, not half.
pub fn volumes(option: impl Fn(&str) -> Option<f64>) -> Volumes {
    let v = |vol: &str, off: &str| {
        let x = option(vol).unwrap_or(10.0) as f32;
        let db = if option(off).unwrap_or(0.0) != 0.0 {
            -60.0
        } else {
            slider_db(x)
        };
        db_gain(db)
    };
    Volumes {
        music: v("AudioOptions.MusicVolume", "AudioOptions.MusicDisabled"),
        sfx: v("AudioOptions.SoundFXVolume", "AudioOptions.SFXDisabled"),
        dialog: v("AudioOptions.DialogVolume", "AudioOptions.DialogDisabled"),
    }
}

/// A volume slider's 0..10 as dB (DAOrigins.exe 0x5b61b0).
fn slider_db(x: f32) -> f32 {
    if x > 9.99 {
        0.0
    } else if x < 0.01 {
        -60.0
    } else {
        -3.0 * (10.0 - x)
    }
}

/// dB as a gain, silent at 0.001 (-60 dB) and below (DAOrigins.exe 0x5b6110).
fn db_gain(db: f32) -> f32 {
    let g = 10f32.powf(db / 20.0);
    if db <= -60.0 || g <= 0.001 {
        0.0
    } else {
        g.min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footstep_columns_come_from_the_appearance_type() {
        use thedas_formats::gda::Cell;
        let col = |v: &str| footstep_column(Some(&Cell::String(v.into())));
        assert_eq!(col("HUMANOID_SMALL\t "), COL_HUMANOID_SMALL);
        assert_eq!(col("DOG"), thedas_formats::gda::column_hash("dog"));
        assert_eq!(
            footstep_column(None),
            thedas_formats::gda::column_hash("Unknown")
        );
    }

    #[test]
    fn volumes_follow_the_options() {
        let opts = |n: &str| match n {
            "AudioOptions.MusicVolume" => Some(5.0),
            "AudioOptions.SFXDisabled" => Some(1.0),
            _ => None,
        };
        let v = volumes(opts);
        // 5 is -15 dB; disabled is silent; the default 10 is full.
        assert!(
            (v.music - 10f32.powf(-15.0 / 20.0)).abs() < 1e-6,
            "{}",
            v.music
        );
        assert_eq!((v.sfx, v.dialog), (0.0, 1.0));
        assert_eq!(slider_db(0.0), -60.0);
        assert_eq!(db_gain(-60.0), 0.0);
    }
}

#[cfg(test)]
mod impact_tests {
    use super::*;

    #[test]
    fn blows_sound_by_weapon_and_what_they_strike() {
        let ev = |w: &str, t: &str| format!("glo_fly_wep/weapon_impacts/{w}/{t}");
        let mut i = Impacts::default();
        i.materials
            .extend([(2, 1), (3, 4), (60, 13), (61, 11), (70, 6)]);
        i.hits.insert(
            1,
            vec![
                ev("sword", "impact_materials/wood"),
                ev("sword", "impact_materials/metal"),
                ev("sword", "impact_armor/metal_arm"),
                ev("sword", "impact_armor/cloth_arm"),
                ev("sword", "impact_armor/flesh"),
            ],
        );
        i.hits
            .insert(19, vec![ev("barehand", "impact_armor/flesh")]);
        assert_eq!(
            i.event(1, Some(2), Some(60), None),
            Some(ev("sword", "impact_armor/metal_arm"))
        );
        assert_eq!(
            i.event(2, Some(2), Some(61), None),
            Some(ev("sword", "impact_armor/cloth_arm"))
        );
        assert_eq!(
            i.event(5, Some(2), None, Some(70)),
            Some(ev("sword", "impact_materials/metal"))
        );
        // No weapon (or one without impacts): bare hands; a miss sounds nothing here.
        assert_eq!(
            i.event(1, None, None, None),
            Some(ev("barehand", "impact_armor/flesh"))
        );
        assert_eq!(
            i.event(1, Some(3), None, None),
            Some(ev("barehand", "impact_armor/flesh"))
        );
        assert_eq!(i.event(3, Some(2), None, None), None);
    }
}
