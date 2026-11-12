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
use thedas_vfs::Vfs;

/// The voice-over a conversation line wants.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceLine {
    pub dialog: String,
    pub strref: u32,
    pub female: bool,
    pub ambient: bool,
}

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
    _output: thedas_audio::device::Output,
    gui: Vec<Option<String>>,
    music: Option<(&'static str, EventHandle)>,
    voice: Option<(VoiceLine, Option<EventHandle>, Option<f64>)>,
    cutscene: Option<CutsceneSounds>,
    area: Option<AreaPlay>,
    last: Instant,
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
    pub fn open(install: &Path, module: &str, vfs: &Vfs, language: &str) -> Option<GameAudio> {
        let output = thedas_audio::device::Output::open()?;
        let library = Arc::new(Library::new(Library::install_dirs(
            install, module, language,
        )));
        let engine = AudioEngine::new(library, output.mixer.clone());
        Some(GameAudio {
            engine,
            _output: output,
            gui: gui_table(vfs),
            music: None,
            voice: None,
            cutscene: None,
            area: None,
            last: Instant::now(),
        })
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
                        parameters: vec![("music_state".into(), 1.0)],
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

    pub fn frame(&mut self, mut state: AudioState) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.25);
        self.last = now;
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
/// switches) as gains.
pub fn volumes(option: impl Fn(&str) -> Option<f64>) -> Volumes {
    let v = |vol: &str, off: &str| {
        if option(off).unwrap_or(0.0) != 0.0 {
            0.0
        } else {
            (option(vol).unwrap_or(10.0) / 10.0).clamp(0.0, 1.0) as f32
        }
    };
    Volumes {
        music: v("AudioOptions.MusicVolume", "AudioOptions.MusicDisabled"),
        sfx: v("AudioOptions.SoundFXVolume", "AudioOptions.SFXDisabled"),
        dialog: v("AudioOptions.DialogVolume", "AudioOptions.DialogDisabled"),
    }
}
