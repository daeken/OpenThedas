//! The game's sound, played through [`thedas_audio`] on the system output.
//!
//! - Music: the front end plays `music/_menu/main` (DAOrigins.exe names the `music/_menu`
//!   theme, 0xafed94); character creation `music/_menu/chargen`. In the game, the area's
//!   theme (`MusicPreset`) plays `main`, `combat` or `Death` with its stingers (see
//!   `follow_area`).
//! - Interface: the movies' `PlayGUISound <id> ...` plays `sound_gui.gda`'s event for the id.
//! - Voice-over: a conversation line plays its dialog project's `VO/VOconversation` event
//!   (`VO/VOambient` for an ambient line) with the line's own sample as the programmer sound:
//!   `<strref>_f` for a female hero when the bank has it, else `<strref>_m` (DAOrigins.exe
//!   builds `%ls/VO/%ld` and the suffix at 0x7f2be9). The line stays up for the length of its
//!   voice.
//! - The world: area ambience and placed sounds, script sounds, sound sets, animation sound
//!   events (footsteps, foley, weapon actions, blows, bodyfalls, vegetation) and the world
//!   reverb, each documented where it is played.
//! - Volumes: the options screen's sliders map to dB as DAOrigins.exe maps them (see
//!   [`volumes`]).
//!
//! Provisional: voice-over is not positioned at the speaker.

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
    /// Each hand's (main, off) attack in progress, with a result: (`COMBAT_RESULT_*`, the
    /// target's sound material, where the target stands).
    pub hands: [Option<(i32, i64, [f32; 3])>; 2],
    /// The attack in progress's target: its sound material and where it stands.
    pub attack_target: Option<(i64, [f32; 3])>,
    /// Its hands' sound material when they hold no weapon (`apr_base.gda` column
    /// 0x7d86f95a: 19 bare hands, 20 claws, 27 large claws; 0x6a0b05..0x6a0b53).
    pub unarmed: Option<i64>,
    /// For a placeable: its type, the row of `placeable_types.gda` (worksheet 93, the
    /// placeable's +0x4f0) that names its sounds.
    pub placeable: Option<i64>,
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
    /// The simulation's game mode (`GM_*` in `script.ldf`: 0 explore, 1 combat, 2 dead,
    /// 4 dialog, 5 cutscene, 9 loading, ...).
    pub game_mode: i32,
    /// The container whose inventory is open for looting: its id and placeable type (its
    /// `placeable_types.gda` row).
    pub container: Option<(u32, i64)>,
    /// The game is paused (the player's pause).
    pub paused: bool,
    /// A menu the world waits behind is up (the game's `GM_GUI`).
    pub gui: bool,
    /// An area is loading (the load screen is up): the game mode is `GM_LOADING` for the
    /// sound, whatever the simulation's says.
    pub loading: bool,
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
    /// A change of track waiting for its moment: the track wanted, when to change (seconds
    /// into the playing track), where that track was last frame, and seconds waited.
    switch: Option<(&'static str, f64, f64, f32)>,
    sounds: Vec<AreaSound>,
    /// Per sound: playing (`Some(Some)`), played through while in reach (`Some(None)`: it
    /// plays again once the listener has left and come back), or not started.
    playing: Vec<Option<Option<EventHandle>>>,
    reverb: AreaReverb,
    /// The reverb preset applied (`None` until the first).
    preset: Option<String>,
    /// Which reverb triggers the party leader was inside last frame.
    inside: Vec<bool>,
    /// Which music volumes the party leader was inside last frame.
    music_inside: Vec<bool>,
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
    /// The music of an area just left, kept playing until the next area says whether it
    /// keeps the same theme: (theme, track, its event).
    carried: Option<(String, &'static str, EventHandle)>,
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
    /// `sound_weapon_hits.gda` (rows: the weapon's sound material; columns: the struck
    /// material by label).
    weapon_hits: Option<thedas_formats::Gda>,
    bodyfall_foley: Option<thedas_formats::Gda>,
    /// `placeable_types.gda`: each placeable type's sounds, by column (`ClosedToOpened`,
    /// `OpenedToClosed`, `Destroyed`, `Used`, `Locked`, `Hit` and others).
    placeable_types: Option<thedas_formats::Gda>,
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
    /// A music volume's request waiting for its delay: the intensity and the seconds left
    /// (see `AreaReverb::music_entered`).
    music_request: Option<(i32, f32)>,
    /// The game mode's ducking event playing (see [`mode_duck`]): its path and handle.
    mode_duck: Option<(&'static str, Option<EventHandle>)>,
    /// The container open last frame (see [`AudioState::container`]).
    container: Option<u32>,
    /// The pause's ducking event playing (see [`pause_duck`]).
    pause_duck: Option<Option<EventHandle>>,
    /// Paused last frame, and whether that pause holds the voice categories
    /// ([`PAUSED_VOICES`]).
    paused: Option<bool>,
    /// The music director (see [`MusicDirector`]).
    director: MusicDirector,
    /// The music track's ambience ducking event playing (see [`music_duck`]).
    music_duck: Option<(&'static str, Option<EventHandle>)>,
    /// The simulation's music intensity last seen (scripts' `SetMusicIntensity`).
    script_intensity: Option<i32>,
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

/// When a music change takes effect, in seconds into the playing track at `pos` with markers
/// (sync points) `markers`: the first marker more than 0.4 s ahead and less than 15 s ahead,
/// else 0.4 s on (DAOrigins.exe 0x5c4b10 state 4: 0x190 ms, 0x3a98 ms).
fn music_switch_at(pos: f64, markers: &[f64]) -> f64 {
    markers
        .iter()
        .copied()
        .find(|m| *m > pos + 0.4 && *m - pos < 15.0)
        .unwrap_or(pos + 0.4)
}

/// `apr_base.gda`'s footstep type column (DAOrigins.exe 0x6a098a).
pub const APR_FOOTSTEP: u32 = 0xda48_e219;
/// `apr_base.gda`'s sound size column (0x6a0a1f): `Small`, `Medium`, `Large`.
pub const APR_SIZE: u32 = 0xc7cb_6561;
/// `apr_base.gda`'s unarmed hands' sound material column (0x6a0b12).
pub const APR_UNARMED: u32 = 0x7d86_f95a;
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

/// A blow's event (DAOrigins.exe 0x5b8d2e): `sound_weapon_hits.gda`'s cell in the row of
/// the striking hand's sound material and the column of the struck material's label
/// (`labels`: sound material -> label hash, from `sound_item_materials.gda`).
fn blow_event(
    hits: &thedas_formats::Gda,
    labels: &std::collections::HashMap<i64, u32>,
    weapon: Option<i64>,
    struck: i64,
) -> Option<String> {
    table_event(hits, weapon?, *labels.get(&struck)?)
}

/// The ambience ducking event for the music track playing (DAOrigins.exe 0x5bccd0, run
/// as the music changes, and 0x5bc6f6..0x5bc7bf, which stops both when the music volume
/// falls to nothing): `OnExploreMusic` while `main` plays and `OnCombatMusic` while
/// `combat` does, each only while the music volume is above 0; none otherwise. The explore
/// one lowers ambient one-shots 4 dB; the combat one lowers ambience and one-shots 4 dB and
/// silences `vfxamb_combat`.
fn music_duck(track: Option<&str>, music_volume: f32) -> Option<&'static str> {
    if music_volume <= 0.0 {
        return None;
    }
    match track? {
        "main" => Some("audio_categories/ducking/OnExploreMusic"),
        "combat" => Some("audio_categories/ducking/OnCombatMusic"),
        _ => None,
    }
}

/// `placeable_types.gda`'s column of the sound a container makes as its inventory opens
/// (DAOrigins.exe 0x74c2f7).
const COL_CONTAINER_OPEN: u32 = 0x9809_4859;

const PAUSE_DUCK: &str = "audio_categories/ducking/OnGamePause";

/// Whether the pause plays `OnGamePause` (DAOrigins.exe's pause toggle 0x5d6742: pausing
/// in explore, combat, dead, dialog or cutscene mode plays it, 0x5d67fc..0x5d683c;
/// unpausing stops it, 0x5d68fb). It lowers music, ambience and placeables 6 to 9 dB and
/// pauses movement, footsteps, effects and ambient voices (see `Duck`).
fn pause_duck(paused: bool, mode: i32) -> bool {
    paused
        && matches!(
            mode,
            GM_EXPLORE | GM_COMBAT | GM_DEAD | GM_DIALOG | GM_CUTSCENE
        )
}

/// The voice categories the pause holds when it begins in `GM_GUI` (DAOrigins.exe's pause
/// toggle 0x5d684e..0x5d68e3: each through 0x5b7540, which pauses the FMOD category,
/// strings 0xafeb60, 0xafeb74, 0xafeb2c). Unpausing in any mode resumes them
/// (0x5d6932..0x5d69c2).
const PAUSED_VOICES: [&str; 3] = ["VOambient", "VOconversation", "VOnarration"];

/// DAOrigins.exe's music director (0x5bb630, run each frame with the frame time): when the
/// game mode changes it cues the theme's track for the new mode (music vtable slot 0x8,
/// 0x5bfdf0, which the music update then switches to at the playing track's next marker).
///
/// - Explore and pregame cue `main` (0x5bb90c, string 0xafebd8).
/// - Combat cues `combat` (0xafed3c), but coming from `GM_GUI`, dialog or a cutscene only
///   after 6 s (0xafed38) still in combat (0x5bb726..0x5bb76e: the cue waits at +0x5c with
///   its time at +0x7c, counted down at 0x5bb95c; any mode change drops it, 0x5bb6d6).
/// - Dialog and cutscenes cue `main`, except when one follows the other (0x5bb773,
///   0x5bb7d6).
/// - Death cues `death` (0xafed2c), character creation `chargen` (0xafed1c).
/// - The interface, loading and the rest leave the track as it is.
#[derive(Clone, Debug, PartialEq)]
struct MusicDirector {
    mode: i32,
    track: &'static str,
    /// A combat cue waiting: seconds left.
    delayed: Option<f32>,
}

impl MusicDirector {
    fn new() -> MusicDirector {
        MusicDirector {
            mode: GM_LOADING,
            track: "main",
            delayed: None,
        }
    }

    /// The track to play in `mode`, `dt` seconds after the last call.
    fn update(&mut self, mode: i32, dt: f32) -> &'static str {
        if mode != self.mode {
            let old = std::mem::replace(&mut self.mode, mode);
            self.delayed = None;
            match mode {
                GM_COMBAT if matches!(old, GM_GUI | GM_DIALOG | GM_CUTSCENE) => {
                    self.delayed = Some(6.0)
                }
                GM_COMBAT => self.track = "combat",
                GM_CUTSCENE if old == GM_DIALOG => {}
                GM_DIALOG if old == GM_CUTSCENE => {}
                GM_EXPLORE | GM_PREGAME | GM_DIALOG | GM_CUTSCENE => self.track = "main",
                GM_DEAD => self.track = "Death",
                GM_CHARGEN => self.track = "chargen",
                _ => {}
            }
        } else if let Some(left) = self.delayed.as_mut() {
            *left -= dt;
            if *left <= 0.0 {
                self.delayed = None;
                self.track = "combat";
            }
        }
        self.track
    }
}

const GM_EXPLORE: i32 = 0;
const GM_COMBAT: i32 = 1;
const GM_DEAD: i32 = 2;
const GM_GUI: i32 = 3;
const GM_DIALOG: i32 = 4;
const GM_CUTSCENE: i32 = 5;
const GM_PREGAME: i32 = 8;
const GM_LOADING: i32 = 9;
const GM_CHARGEN: i32 = 11;

/// The game mode as the sound sees it (`GM_*` in `script.ldf`): loading while the load
/// screen is up, dead once the party has fallen, a cutscene while one plays outside a
/// conversation, the simulation's own mode when it says dialog or another special one,
/// `GM_GUI` while a menu stops the world, combat while the party fights, else explore.
///
/// Provisional: the simulation does not keep combat or the interface as game modes, so
/// those come from the party's fighting and the menus up.
fn game_mode(state: &AudioState) -> i32 {
    if state.loading {
        GM_LOADING
    } else if state.dead {
        GM_DEAD
    } else if state.cutscene.is_some() && state.game_mode != GM_DIALOG {
        GM_CUTSCENE
    } else if !matches!(state.game_mode, GM_EXPLORE | GM_COMBAT) {
        state.game_mode
    } else if state.gui {
        GM_GUI
    } else if state.combat {
        GM_COMBAT
    } else {
        GM_EXPLORE
    }
}

/// The ducking event a game mode plays while it lasts (DAOrigins.exe's mode-change handler
/// 0x5bd220, tables 0x5bd618 and 0x5bd638): `OnDeath` for `GM_DEAD`, `OnCutscene` for
/// `GM_DIALOG` and `GM_CUTSCENE` (one event across a change between the two), and
/// `OnGameLoading` for `GM_LOADING`. The loading one turns world sounds (movement,
/// footsteps, effects, ambience, placeables) down 60 dB over 0.2 s and leaves music, voice
/// and the interface alone; the cutscene one lowers ambience and effects and the music by
/// 3 dB.
fn mode_duck(mode: i32) -> Option<&'static str> {
    match mode {
        GM_DEAD => Some("audio_categories/ducking/OnDeath"),
        GM_DIALOG | GM_CUTSCENE => Some("audio_categories/ducking/OnCutscene"),
        GM_LOADING => Some("audio_categories/ducking/OnGameLoading"),
        _ => None,
    }
}

/// The `placeable_types.gda` column a placeable sound event (kind 27) names: its text
/// trimmed of spaces, tabs and the string's terminator, hashed as a column name.
fn placeable_sound_column(label: &str) -> u32 {
    thedas_formats::gda::column_hash(label.trim_matches(|c| matches!(c, ' ' | '\t' | '\0')))
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
const COL_SHEATH: u32 = 0xf8eb_75ab;
const COL_UNSHEATH: u32 = 0x8dfc_4c37;
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

/// Items' sound materials: `BITM_base.gda`'s column 0x7e0505a1 gives each base item a
/// sound material (`sound_item_materials.gda`: 1 sword, 2 axe, ... 10-13 leather, cloth,
/// chain and metal armour; DAOrigins.exe reads it at 0x6a0b89).
#[derive(Default)]
struct Impacts {
    materials: std::collections::HashMap<i64, i64>,
}

const COL_SOUND_MATERIAL: u32 = 0x7e05_05a1;

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
        out
    }

    /// A base item's sound material.
    fn material(&self, base: Option<i32>) -> Option<i64> {
        base.and_then(|b| self.materials.get(&(b as i64)).copied())
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
            carried: None,
            movie: None,
            vfs: vfs.clone(),
            anim_events: Default::default(),
            anim_clocks: Default::default(),
            footsteps: read_gda(vfs, "sound_movement_footsteps.gda"),
            scuffs: read_gda(vfs, "sound_movement_scuffing.gda"),
            vegetation: read_gda(vfs, "sound_vegetation_type.gda"),
            bodyfalls: read_gda(vfs, "sound_bodyfall_creat_size.gda"),
            weapon_hits: read_gda(vfs, "sound_weapon_hits.gda"),
            bodyfall_foley: read_gda(vfs, "sound_bodyfall_foley.gda"),
            placeable_types: read_gda(vfs, "placeable_types.gda"),
            item_material_labels: item_material_labels(vfs),
            rustles: Default::default(),
            impacts: Impacts::read(vfs),
            foley: Foley::read(vfs),
            music_state: 1.0,
            music_request: None,
            mode_duck: None,
            script_intensity: None,
            music_duck: None,
            pause_duck: None,
            paused: None,
            director: MusicDirector::new(),
            container: None,
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
                    // Weapon actions (0x69e8e0..0x69e9a0, request type 3):
                    // `sound_weapon_actions.gda`'s column for the event kind, in the row of
                    // the hand's sound material (+0x938 main, +0x93c off: its weapon's, the
                    // appearance's unarmed material without one). Kinds 3 and 4 take the
                    // `Sheath` column (its events are the unsheathing sounds), 5 and 6
                    // `Unsheath` (sheathing), for the main and the off hand; 9, 10 and 19
                    // (`Swish`) the hand the event's text names (`|Left`: off, 0x69e690).
                    (3..=6 | 9 | 10 | 19, text) => {
                        let g = gear.get(&id).copied().unwrap_or_default();
                        let left = match ev.kind {
                            3 | 5 => false,
                            4 | 6 => true,
                            _ => text.as_deref().is_some_and(|t| t.contains("|Left")),
                        };
                        let column = match ev.kind {
                            3 | 4 => COL_SHEATH,
                            5 | 6 => COL_UNSHEATH,
                            9 => 0x5ff7_e447,
                            10 => 0x85e1_df32,
                            _ => COL_SWISH,
                        };
                        let w = if left { g.off } else { g.main };
                        let m = self.impacts.material(w).or(g.unarmed);
                        self.foley.cell("weapon", m, column)
                    }
                    // Armour foley (0x69e7f0): `sound_movement_foley.gda`'s column the event
                    // names (`Small`, `Medium`, `Large`) for the armour's sound material
                    // (none: row 0).
                    // A placeable's sound (kind 27, the placeable's handler 0x74c360): the
                    // event's text, trimmed of spaces and tabs (0x4c3310), names a column of
                    // `placeable_types.gda` (worksheet 93), read in the row of the
                    // placeable's type; door animations name `ClosedToOpened` and
                    // `OpenedToClosed`. Creatures ignore the kind (0x6b62fb).
                    // Provisional: the exe plays it at the centre of the model's bounds
                    // (0x74c4e2..0x74c686); we play it where the placeable stands.
                    (27, Some(label)) => gear.get(&id).and_then(|g| g.placeable).and_then(|row| {
                        let col = placeable_sound_column(label);
                        let e = self
                            .placeable_types
                            .as_ref()
                            .and_then(|t| table_event(t, row, col));
                        log::debug!("audio: placeable {id} {label:?}: {e:?}");
                        e
                    }),
                    (17, Some(size)) => {
                        let g = gear.get(&id).copied().unwrap_or_default();
                        let col = thedas_formats::gda::column_hash(size.trim());
                        let m = self.impacts.material(g.armour).unwrap_or(0);
                        self.foley.cell("movement", Some(m), col)
                    }
                    // A blow (kind 1, 0x6a04a0; request type 4, 0x5b8d2e): at the attack
                    // animation's hit frame, the hand the event names (`|Left`: off) sounds
                    // its attack's result unless it is invalid, a miss or 8 (0x6a04dc):
                    // `sound_weapon_hits.gda`'s event in the row of the hand's sound
                    // material, the column of the target's (by label, 0x5b6f60). Kind 7
                    // (0x6a5b00) does the same for the attack's target when the event says
                    // `|PlayHit` (0x69e720), whatever the result.
                    // Provisional: placeable targets (table 93) are not sounded, and the
                    // sound plays where the target stands.
                    (1 | 7, text) => {
                        let g = gear.get(&id).copied().unwrap_or_default();
                        let left = text.as_deref().is_some_and(|t| t.contains("|Left"));
                        log::trace!(
                            "audio: hit frame kind {} {key} creature {id}: hands {:?} target {:?}",
                            ev.kind,
                            g.hands,
                            g.attack_target
                        );
                        let target = if ev.kind == 1 {
                            g.hands[left as usize]
                                .filter(|(r, ..)| !matches!(r, 0 | 3 | 8))
                                .map(|(_, m, at)| (m, at))
                        } else if text.as_deref().is_some_and(|t| t.contains("|PlayHit")) {
                            g.attack_target
                        } else {
                            None
                        };
                        if let Some((struck, at)) = target {
                            let w = if left { g.off } else { g.main };
                            let weapon = self.impacts.material(w).or(g.unarmed);
                            let event = self.weapon_hits.as_ref().and_then(|t| {
                                blow_event(t, &self.item_material_labels, weapon, struck)
                            });
                            if let Some(e) = event {
                                log::debug!("audio: blow {e} (creature {id}, struck {struck})");
                                self.engine.play(
                                    &e,
                                    PlayOptions {
                                        position: Some(at),
                                        ..Default::default()
                                    },
                                );
                            }
                        }
                        None
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
                        let body = self
                            .bodyfalls
                            .as_ref()
                            .and_then(|t| table_event(t, row, size));
                        log::debug!(
                            "audio: bodyfall {body:?} (creature {id}, ground {row}, armour {armour})"
                        );
                        body
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
        // The same theme carries on across areas (DAOrigins.exe's SetTheme does nothing when
        // the theme it is given is the one playing, 0x5c0ec9); another replaces it once the
        // load is over (see `follow_area`), so the old music plays under the load screen.
        let mut track = None;
        if let Some((carried, name, h)) = self.carried.take() {
            if theme
                .as_deref()
                .is_some_and(|t| t.eq_ignore_ascii_case(&carried))
            {
                track = Some((name, h));
            } else {
                self.carried = Some((carried, name, h));
            }
        }
        self.area = Some(AreaPlay {
            resref: resref.to_string(),
            theme,
            track,
            switch: None,
            sounds,
            playing: vec![None; n],
            reverb,
            preset: None,
            inside: Vec::new(),
            music_inside: Vec::new(),
        });
    }

    fn end_area(&mut self) {
        if let Some(a) = self.area.take() {
            if a.preset.is_some() {
                self.engine
                    .set_reverb(thedas_formats::fev::ReverbProperties::OFF, 0.0);
            }
            for h in a.playing.into_iter().flatten().flatten() {
                self.engine.stop(h, false);
            }
            if let Some(h) = self.carried.take() {
                self.engine.stop(h.2, false);
            }
            match (a.theme, a.track) {
                (Some(theme), Some((name, h))) => self.carried = Some((theme, name, h)),
                (_, Some((_, h))) => self.engine.stop(h, false),
                _ => {}
            }
        }
    }

    /// The game mode's ducking (see [`mode_duck`]): on a change of mode the old mode's event
    /// stops (its ducks ramp back over their fade-outs) and the new one's starts, unless both
    /// modes use the same event, which then plays on.
    fn follow_mode(&mut self, mode: i32) {
        let want = mode_duck(mode);
        if self.mode_duck.as_ref().map(|d| d.0) == want {
            return;
        }
        if let Some((path, Some(h))) = self.mode_duck.take() {
            log::debug!("audio: game mode duck {path} ends");
            self.engine.stop(h, false);
        }
        self.mode_duck = want.map(|path| {
            log::debug!("audio: game mode {mode}: duck {path}");
            (path, self.engine.play(path, PlayOptions::default()))
        });
    }

    /// The playing music track's ambience ducking (see [`music_duck`]): when it changes the
    /// new event starts and the old one stops.
    fn follow_music_duck(&mut self, state: &AudioState) {
        let track = self.area.as_ref().and_then(|a| a.track).map(|t| t.0);
        let want = music_duck(track, state.volumes.music);
        if self.music_duck.as_ref().map(|d| d.0) == want {
            return;
        }
        if let Some(path) = want {
            log::debug!("audio: music {track:?}: duck {path}");
        }
        let new = want.map(|path| (path, self.engine.play(path, PlayOptions::default())));
        if let Some((path, Some(h))) = self.music_duck.take() {
            log::debug!("audio: music duck {path} ends");
            self.engine.stop(h, false);
        }
        self.music_duck = new;
    }

    /// Sets the music's intensity, `music_state` on the area track (DAOrigins.exe's
    /// SetMusicIntensity, 0x5c0150, sets it on the theme's main, combat and third events).
    fn set_music_state(&mut self, s: f32) {
        if s == self.music_state {
            return;
        }
        log::debug!("audio: music state {s}");
        self.music_state = s;
        if let Some((_, h)) = self.area.as_ref().and_then(|a| a.track) {
            self.engine.set_parameter(h, "music_state", s);
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
    /// A change of track waits for the playing track's next marker, as DAOrigins.exe's music
    /// update does (0x5c4b10, states 4 to 6; see [`music_switch_at`]); then the new track
    /// starts and the old one stops over its event's fade-out.
    ///
    /// Provisional: when the stinger plays (taken as at the change; the exe waits for it in
    /// state 7), and cutscenes and conversations keeping the track (they only duck it, see
    /// [`mode_duck`]). Scripts' `PlayMusic` and `StopMusic` do nothing in the exe.
    fn follow_area(&mut self, state: &AudioState, dt: f32) {
        if state.area.is_none() {
            self.end_area();
            return;
        }
        // A theme set during a load is applied when the load ends (DAOrigins.exe: SetTheme,
        // music slot 0x8 at 0x5bfdf0, queues it at +0xd8; leaving `GM_LOADING`, the director
        // 0x5b7070 calls slot 0x10, 0x5c4060, which starts it). Until then the last area's
        // music plays on and the new area's waits.
        if !state.loading
            && let Some((_, _, h)) = self.carried.take()
        {
            self.engine.stop(h, false);
        }
        let Some(a) = self.area.as_mut() else {
            return;
        };
        if let Some(theme) = a.theme.clone().filter(|_| !state.loading) {
            // The track the music director cues for the game mode (see `MusicDirector`).
            let want = self.director.track;
            // A change waits for the playing track's next marker (DAOrigins.exe 0x5c4b10, states
            // 4 and 5): the first sync point more than 0.4 s ahead (0x190 ms) and less than
            // 15 s ahead (0x3a98 ms), else 0.4 s on. The first track of an area starts at once.
            let mut now = a.track.is_none();
            match a.track {
                Some((playing, _)) if playing == want => a.switch = None,
                Some((_, h)) => match (a.switch, self.engine.position(h)) {
                    (Some((w, at, last, waited)), Some((pos, _))) if w == want => {
                        // Due, past the end of a loop, or waited past the longest wait.
                        if pos >= at || pos + 0.001 < last || waited > 15.5 {
                            now = true;
                        } else {
                            a.switch = Some((w, at, pos, waited + dt));
                        }
                    }
                    (_, Some((pos, markers))) => {
                        let at = music_switch_at(pos, &markers);
                        log::debug!("audio: music to {want} at {at:.2} s (now {pos:.2} s)");
                        a.switch = Some((want, at, pos, 0.0));
                    }
                    // Not playing a sample yet: change at once.
                    (_, None) => now = true,
                },
                None => {}
            }
            if now && a.track.map(|t| t.0) != Some(want) {
                a.switch = None;
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
            // A music volume entered replaces any request still waiting.
            let (now, request) = a.reverb.music_entered(&a.music_inside, p);
            if let Some((s, delay)) = request {
                log::debug!("audio: music volume asks for music state {s} after {delay} s");
                self.music_request = Some((s, delay));
            }
            a.music_inside = now;
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
            // An audio volume's sound follows the listener (see `SoundRegion`): at the
            // listener while the party leader is inside the volume, else at the volume's
            // nearest point, when within the volume's radius of the event's reach.
            // Provisional: re-placed every frame rather than every 0.06 s after 0.25 m.
            let position = match &s.region {
                Some(r) if state.leader.is_some_and(|p| r.contains(p)) => listener,
                Some(r) => r.nearest(listener),
                None => s.position,
            };
            let d = {
                let v = [
                    position[0] - listener[0],
                    position[1] - listener[1],
                    position[2] - listener[2],
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
                            position: Some(position),
                            // `Volume` above 0 dB is left out (0x7c25c0..0x7c25cd).
                            volume: 10f32.powf(s.volume.min(0.0) / 20.0),
                            distance_scale: s.max_distance_mult.max(1e-3),
                            // The placed sound's own pitch offset and fade times
                            // (DAOrigins.exe 0x7c261d..0x7c2637).
                            pitch: s.pitch,
                            fade_in_ms: Some(s.fade_in_ms),
                            fade_out_ms: Some(s.fade_out_ms),
                            // Its cone (see `AreaSound::forward`); a volume's sound faces
                            // the listener, so has none.
                            cone: (s.region.is_none() && s.cone_inside < 360.0).then_some(
                                thedas_audio::engine::Cone {
                                    forward: s.forward,
                                    inside: s.cone_inside,
                                    outside: s.cone_outside,
                                    outside_gain: s.cone_outside_gain,
                                },
                            ),
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
                (Some(Some(h)), true) if s.region.is_some() => {
                    self.engine.set_position(h, position);
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
        // Back at the menus: an area's music still carried stops.
        if (state.front_end || state.chargen)
            && let Some((_, _, h)) = self.carried.take()
        {
            self.engine.stop(h, false);
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
        // Scripts' SetMusicIntensity (command 486, handler 0x65d070) asks the music director
        // for that intensity with no delay (0x5b6cd0), as a music volume does: it replaces a
        // request still waiting and applies on the director's next update. The tracks' sounds
        // sit around 1, so 0 silences the music (pre100pt_light_beacon) and cutscene endings
        // (gen00cs_cs_end_music_int_*) set 0 to 4. The simulation keeps the latest call's
        // value, so a change of it is a call (a repeat of the same value is not seen).
        if let Some(i) = state.music_intensity.take()
            && self.script_intensity != Some(i)
        {
            self.script_intensity = Some(i);
            self.music_request = Some((i, 0.0));
        }
        // A music volume's request (see `AreaReverb::music_entered`) counts down on the
        // music director's update (0x5bb630, given the frame's seconds as the music update
        // is) and then sets the intensity (0x5b6df0 -> the music object's SetMusicIntensity,
        // vtable slot 0x1c). A delay of 0 applies on the next update.
        // The director skips the countdown while loading (`GM_LOADING`, 0x5bb669).
        if let Some((s, left)) = self.music_request.as_mut().filter(|_| !state.loading) {
            *left -= dt;
            if *left <= 0.0 {
                let s = *s as f32;
                self.music_request = None;
                self.set_music_state(s);
            }
        }
        let mode = game_mode(&state);
        self.follow_mode(mode);
        let (was, before) = (self.director.mode, self.director.track);
        let track = self.director.update(mode, dt);
        if mode != was || track != before {
            let waiting = if self.director.delayed.is_some() {
                " (combat cue in 6 s)"
            } else {
                ""
            };
            log::info!("audio: game mode {was} -> {mode}: music track {track}{waiting}");
        }
        self.follow_area(&state, dt);
        self.follow_music_duck(&state);
        // The pause's voices (see `PAUSED_VOICES`): held when the pause begins in GUI mode,
        // resumed when it ends.
        if state.paused != self.paused.is_some() {
            let hold = state.paused && mode == GM_GUI;
            for c in PAUSED_VOICES {
                self.engine.set_category_paused(c, hold);
            }
            self.paused = state.paused.then_some(hold);
        }
        let pause = pause_duck(state.paused, mode);
        if pause != self.pause_duck.is_some() {
            if let Some(Some(h)) = self.pause_duck.take() {
                log::debug!("audio: pause duck ends");
                self.engine.stop(h, false);
            }
            if pause {
                log::debug!("audio: paused: duck {PAUSE_DUCK}");
                self.pause_duck = Some(self.engine.play(PAUSE_DUCK, PlayOptions::default()));
            }
        }
        // A container's inventory opening (DAOrigins.exe 0x7314ee -> 0x74c2b0) plays its
        // placeable type's container column (`placeable_types.gda` 0x98094859, the wooden
        // chest's `glo_gui/gui/gui_chest1`) in 2D (0x5bc7f0 with no position).
        let opened = state.container.map(|c| c.0);
        if opened != self.container
            && let Some((id, row)) = state.container
        {
            let e = self
                .placeable_types
                .as_ref()
                .and_then(|t| table_event(t, row, COL_CONTAINER_OPEN));
            log::debug!("audio: container {id} opens: {e:?}");
            if let Some(e) = e {
                self.engine.play(&e, PlayOptions::default());
            }
        }
        self.container = opened;
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

    /// The voice-over sample a line plays, if it has one: `<strref>_f` for a female hero
    /// when the dialog's bank has it, else `<strref>_m` (DAOrigins.exe 0x7f2be9), from the
    /// bank named after the dialog.
    fn voice_sample(&self, line: &VoiceLine) -> Option<String> {
        let lib = &self.engine.library;
        let bank = line.dialog.to_ascii_lowercase();
        let female = format!("{}_f", line.strref);
        let male = format!("{}_m", line.strref);
        if line.female && lib.find_sample(&bank, &female).is_some() {
            Some(female)
        } else if lib.find_sample(&bank, &male).is_some() {
            Some(male)
        } else {
            None
        }
    }

    /// Whether a conversation line is voiced and how long its voice-over runs
    /// (milliseconds; `Some(None)` when voiced but its length is unknown), known as soon as
    /// the line is chosen, before it plays: the sample is looked up in the dialog's bank
    /// headers, nothing is decoded.
    pub fn line_voice(&self, line: &VoiceLine) -> Option<Option<f64>> {
        let sample = self.voice_sample(line)?;
        let bank = line.dialog.to_ascii_lowercase();
        Some(
            self.engine
                .library
                .sample_seconds(&bank, &sample)
                .map(|s| s * 1000.0),
        )
    }

    /// Whether voice-over can be heard: the options' Dialog volume is above silence (a
    /// disabled Dialog, or a slider at 0, is -60 dB, silent: DAOrigins.exe 0x5b61b0 ->
    /// 0x5b6110). Lines still play silently, timed as usual; whether the exe also counts
    /// such a line as unvoiced for its subtitles (`Conversation.HasVO`, the line's +0xd4
    /// read at 0x690193) is not traced.
    #[allow(dead_code)] // For the conversation screen's subtitles (UI-203).
    pub fn dialog_audible(&self) -> bool {
        self.engine.volumes.dialog > 0.0
    }

    fn speak(&mut self, line: VoiceLine) -> (VoiceLine, Option<EventHandle>, Option<f64>) {
        let project = line.dialog.to_ascii_lowercase();
        let event = if line.ambient {
            "VOambient"
        } else {
            "VOconversation"
        };
        let Some(sample) = self.voice_sample(&line) else {
            return (line, None, None);
        };
        let ms = self.line_voice(&line).flatten();
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
    fn music_changes_wait_for_the_next_marker() {
        let bars = [0.0, 4.0, 8.0, 12.0, 16.0, 40.0];
        // The next marker more than 0.4 s ahead.
        assert_eq!(music_switch_at(1.0, &bars), 4.0);
        assert_eq!(music_switch_at(3.7, &bars), 8.0);
        // None within 15 s (or none at all): 0.4 s on.
        assert!((music_switch_at(17.0, &bars) - 17.4).abs() < 1e-9);
        assert!((music_switch_at(2.0, &[]) - 2.4).abs() < 1e-9);
    }

    #[test]
    fn blows_look_up_weapon_by_struck_material() {
        use thedas_formats::gda::{Cell, Column, Gda, column_hash};
        let col = |name: &str| Column {
            hash: column_hash(name),
            kind: None,
        };
        let hits = Gda {
            version: thedas_formats::FourCC(*b"V0.2"),
            columns: vec![col("ID"), col("Label"), col("Armor-chain"), col("Flesh")],
            rows: vec![
                vec![
                    Cell::Int(1),
                    Cell::String("Sword".into()),
                    Cell::String("w/sword/chain".into()),
                    Cell::String("w/sword/flesh".into()),
                ],
                vec![
                    Cell::Int(19),
                    Cell::String("BareHands".into()),
                    Cell::Empty,
                    Cell::String("w/fist/flesh".into()),
                ],
            ],
        };
        let labels: std::collections::HashMap<i64, u32> =
            [(12, column_hash("Armor-chain")), (16, column_hash("Flesh"))].into();
        assert_eq!(
            blow_event(&hits, &labels, Some(1), 12).as_deref(),
            Some("w/sword/chain")
        );
        assert_eq!(
            blow_event(&hits, &labels, Some(19), 16).as_deref(),
            Some("w/fist/flesh")
        );
        // An empty cell, an unknown struck material or no weapon material: no blow.
        assert_eq!(blow_event(&hits, &labels, Some(19), 12), None);
        assert_eq!(blow_event(&hits, &labels, Some(1), 99), None);
        assert_eq!(blow_event(&hits, &labels, None, 16), None);
    }

    #[test]
    fn music_tracks_duck_the_ambience_while_heard() {
        assert_eq!(
            music_duck(Some("main"), 1.0),
            Some("audio_categories/ducking/OnExploreMusic")
        );
        assert_eq!(
            music_duck(Some("combat"), 0.2),
            Some("audio_categories/ducking/OnCombatMusic")
        );
        assert_eq!(music_duck(Some("Death"), 1.0), None);
        assert_eq!(music_duck(None, 1.0), None);
        // Music turned off: no ducking.
        assert_eq!(music_duck(Some("main"), 0.0), None);
    }

    #[test]
    fn the_pause_ducks_outside_menus() {
        assert!(pause_duck(true, 0));
        assert!(pause_duck(true, 1));
        assert!(pause_duck(true, GM_DIALOG));
        assert!(!pause_duck(false, 0));
        // GM_GUI (3) and loading: not this event.
        assert!(!pause_duck(true, 3));
        assert!(!pause_duck(true, GM_LOADING));
    }

    #[test]
    fn the_director_cues_a_track_per_game_mode() {
        let mut d = MusicDirector::new();
        // Out of the load: main; a fight: combat at once.
        assert_eq!(d.update(GM_EXPLORE, 0.1), "main");
        assert_eq!(d.update(GM_COMBAT, 0.1), "combat");
        // A conversation in the fight: main, and through a cutscene it leads into.
        assert_eq!(d.update(GM_DIALOG, 0.1), "main");
        d.track = "combat";
        assert_eq!(d.update(GM_CUTSCENE, 0.1), "combat");
        assert_eq!(d.update(GM_DIALOG, 0.1), "combat");
        // Back to the fight from dialog: combat only after 6 s still fighting.
        d.track = "main";
        assert_eq!(d.update(GM_COMBAT, 0.1), "main");
        assert_eq!(d.update(GM_COMBAT, 5.0), "main");
        assert_eq!(d.update(GM_COMBAT, 1.0), "combat");
        // The interface keeps the track; a mode change drops a waiting cue.
        assert_eq!(d.update(GM_GUI, 0.1), "combat");
        d.track = "main";
        d.update(GM_COMBAT, 0.1);
        d.update(GM_GUI, 0.1);
        assert_eq!(d.update(GM_GUI, 10.0), "main");
        assert_eq!(d.update(GM_DEAD, 0.1), "Death");
        assert_eq!(d.update(GM_LOADING, 0.1), "Death");
        assert_eq!(d.update(GM_PREGAME, 0.1), "main");
        assert_eq!(d.update(GM_CHARGEN, 0.1), "chargen");
    }

    #[test]
    fn game_modes_duck_through_their_events() {
        assert_eq!(mode_duck(0), None);
        assert_eq!(mode_duck(1), None);
        assert_eq!(mode_duck(GM_DIALOG), mode_duck(GM_CUTSCENE));
        assert_eq!(
            mode_duck(GM_LOADING),
            Some("audio_categories/ducking/OnGameLoading")
        );
        assert_eq!(mode_duck(GM_DEAD), Some("audio_categories/ducking/OnDeath"));
    }

    #[test]
    fn placeable_sounds_name_a_column_of_their_type() {
        use thedas_formats::gda::{Cell, Column, Gda, column_hash};
        let col = |name: &str| Column {
            hash: column_hash(name),
            kind: None,
        };
        let types = Gda {
            version: thedas_formats::FourCC(*b"V0.2"),
            columns: vec![col("ID"), col("ClosedToOpened"), col("Used")],
            rows: vec![vec![
                Cell::Int(1),
                Cell::String("plc/doors/small_open ".into()),
                Cell::Empty,
            ]],
        };
        // The event's text, as `.evt` files store it (with its terminator), or padded.
        let open = placeable_sound_column("ClosedToOpened\0");
        assert_eq!(open, placeable_sound_column(" \tClosedToOpened "));
        assert_eq!(
            table_event(&types, 1, open).as_deref(),
            Some("plc/doors/small_open")
        );
        // An empty cell or another type: nothing.
        assert_eq!(table_event(&types, 1, placeable_sound_column("Used")), None);
        assert_eq!(table_event(&types, 2, open), None);
    }

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
