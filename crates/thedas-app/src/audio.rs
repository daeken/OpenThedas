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

use thedas_audio::area::{AreaSound, area_audio};
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
    /// Attacks' blows: (target position, result, attacker's weapon, target's armour and
    /// shield base items).
    pub impacts: Vec<Impact>,
    /// Sound set lines to say: (set, `ss_types.gda` type, probability override (0: the
    /// type's own), where the speaker stands (none: 2D)).
    pub sound_sets: Vec<(String, i32, f32, Option<[f32; 3]>)>,
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
    /// `sound_movement_footsteps.gda` row 0 (no surface known): the humanoid and dog
    /// footstep events.
    footsteps: (Option<String>, Option<String>),
    /// Weapon impacts: each base item's sound material (`BITM_base.gda`), and each weapon
    /// material's impact events (`sound_weapon_hits.gda`).
    impacts: Impacts,
    /// The music's `music_state` (scripts' `SetMusicIntensity`; 1 until one sets it).
    music_state: f32,
    /// `ss_types.gda`: each sound set type's first entry row and default probability.
    ss_types: std::collections::HashMap<i32, (i64, f32)>,
    /// Each sound set's entries (`sse_<set>.gda`: entry row -> line strref), read once.
    sound_set_entries: std::collections::HashMap<String, Arc<Vec<(i64, i64)>>>,
    rng: u64,
    /// Creature templates' sound sets (`.utc` `SoundSet`), for creatures that do not carry
    /// theirs (from a save made before the simulation kept it).
    template_sets: std::collections::HashMap<String, Option<String>>,
    last: Instant,
}

/// `sound_movement_footsteps.gda`'s first row (surface unknown): the small humanoid's and the
/// dog's footstep events. Provisional: the walkmesh's surface and the creature's footstep
/// type (the other rows and columns) are not chosen yet.
fn footstep_events(vfs: &Vfs) -> (Option<String>, Option<String>) {
    let Ok((data, _)) = vfs.read("sound_movement_footsteps.gda") else {
        return (None, None);
    };
    let Ok(gda) = thedas_formats::Gda::parse(&data) else {
        return (None, None);
    };
    let Some(row) = gda.row_by_id(0) else {
        return (None, None);
    };
    let events: Vec<String> = row
        .iter()
        .filter_map(|c| match c {
            thedas_formats::gda::Cell::String(s) if s.contains('/') => {
                Some(s.trim_end_matches('\0').trim().to_string())
            }
            _ => None,
        })
        .collect();
    let find = |part: &str| events.iter().find(|e| e.contains(part)).cloned();
    (find("/humanoid/small/"), find("/dog/"))
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
            footsteps: footstep_events(vfs),
            impacts: Impacts::read(vfs),
            music_state: 1.0,
            ss_types: ss_types(vfs),
            sound_set_entries: Default::default(),
            rng: 0x2545_f491_4f6c_dd1d,
            template_sets: Default::default(),
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

    /// A line of sound set `set` of type `entry` (`ss_types.gda`): one of the set's entries
    /// for that type (`sse_<set>.gda` rows from the type's first row on, below the next
    /// type's) at random, said with the type's probability (or the override). It plays the
    /// set's `VO/VOambient_ss` event with the line's sample (`<strref>_m`, else `_f`).
    /// Provisional: which event a set's lines use, and engine-side throttling (the
    /// `sound_settings.gda` playback limits) beyond the event's own max playbacks.
    fn say_sound_set(&mut self, set: &str, entry: i32, probability: f32, at: Option<[f32; 3]>) {
        let Some(&(base, p)) = self.ss_types.get(&entry) else {
            log::debug!("audio: no sound set type {entry}");
            return;
        };
        let chance = if probability > 0.0 { probability } else { p };
        if (self.random() % 10_000) as f32 / 10_000.0 >= chance {
            return;
        }
        let key = set.to_ascii_lowercase();
        let entries = match self.sound_set_entries.get(&key) {
            Some(e) => e.clone(),
            None => {
                use thedas_formats::gda::Cell;
                let rows: Vec<(i64, i64)> = self
                    .vfs
                    .read(&format!("sse_{key}.gda"))
                    .ok()
                    .and_then(|(d, _)| thedas_formats::Gda::parse(&d).ok())
                    .map(|t| {
                        t.rows
                            .iter()
                            .filter_map(|r| match (r.first(), r.get(1)) {
                                (Some(Cell::Int(id)), Some(Cell::Int(s))) => Some((*id, *s)),
                                _ => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let rows = Arc::new(rows);
                self.sound_set_entries.insert(key.clone(), rows.clone());
                rows
            }
        };
        let lines: Vec<i64> = entries
            .iter()
            .filter(|(id, s)| *id >= base && *id < base + 100 && *s > 0)
            .map(|(_, s)| *s)
            .collect();
        if lines.is_empty() {
            return;
        }
        let strref = lines[(self.random() % lines.len() as u64) as usize];
        let lib = self.engine.library.clone();
        let sample = [format!("{strref}_m"), format!("{strref}_f")]
            .into_iter()
            .find(|s| lib.find_sample(&key, s).is_some());
        let Some(sample) = sample else {
            log::debug!("audio: sound set {key} has no sample for {strref}");
            return;
        };
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
    /// footstep (see [`footstep_events`]) where the creature stands. Provisional: events on
    /// the frame an animation starts are those at its time 0 onward; footsteps sound for
    /// humanoids and dogs only.
    fn follow_animations(&mut self, clocks: Vec<(u32, String, f32, f32, [f32; 3])>) {
        let mut seen = std::collections::HashMap::new();
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
                    (14, Some(tags)) if tags.contains("Left") => {
                        if key.starts_with("c_dog") {
                            self.footsteps.1.clone()
                        } else if is_humanoid(&key) {
                            self.footsteps.0.clone()
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                if let Some(event) = event {
                    log::trace!(
                        "audio: {key} {:?} -> {t:.3} (len {length:.3}): event at {:.3}",
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
        log::info!(
            "audio: area {resref}: music {theme:?}, {} placed sounds",
            sounds.len()
        );
        let n = sounds.len();
        self.area = Some(AreaPlay {
            resref: resref.to_string(),
            theme,
            track: None,
            sounds,
            playing: vec![None; n],
        });
    }

    fn end_area(&mut self) {
        if let Some(a) = self.area.take() {
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
        self.follow_animations(std::mem::take(&mut state.animations));
        self.follow_movie(state.movie.take());
        // Scripts' SetMusicIntensity sets the area music's `music_state` (DAOrigins.exe logs
        // it as "music state= %2.1f"): the tracks' sounds sit around 1, so 0 silences the
        // music (pre100pt_light_beacon) and cutscene endings (gen00cs_cs_end_music_int_*)
        // set 0 to 4. Provisional: read from the data and the scripts, not traced.
        if let Some(i) = state.music_intensity.take() {
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

/// Whether an animation resource is a humanoid's: the shared humanoid animations are
/// `mh.*` / `fh.*` (male, female) and their variants `mh_*`, `fh_*`, and cutscene actors
/// `cs_*`. Provisional (from the resource names).
fn is_humanoid(ani: &str) -> bool {
    ["mh.", "mh_", "fh.", "fh_", "cs_", "csmale"]
        .iter()
        .any(|p| ani.starts_with(p))
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
    fn humanoid_animations_by_name() {
        assert!(is_humanoid("mh.walk.ani"));
        assert!(is_humanoid("fh_c.attk_v1.ani"));
        assert!(!is_humanoid("c_dog.cwalkf.ani"));
        assert!(!is_humanoid("ip_dbldoor.open.ani"));
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
