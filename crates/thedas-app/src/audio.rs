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
    pub volumes: Volumes,
}

pub struct GameAudio {
    engine: AudioEngine,
    _output: thedas_audio::device::Output,
    gui: Vec<Option<String>>,
    music: Option<(&'static str, EventHandle)>,
    voice: Option<(VoiceLine, Option<EventHandle>, Option<f64>)>,
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
    pub fn open(install: &Path, vfs: &Vfs, language: &str) -> Option<GameAudio> {
        let output = thedas_audio::device::Output::open()?;
        let library = Arc::new(Library::new(Library::install_dirs(install, language)));
        let engine = AudioEngine::new(library, output.mixer.clone());
        Some(GameAudio {
            engine,
            _output: output,
            gui: gui_table(vfs),
            music: None,
            voice: None,
            last: Instant::now(),
        })
    }

    /// How long the current conversation line's voice-over runs (milliseconds), once known.
    pub fn voice_ms(&self) -> Option<f64> {
        self.voice.as_ref().and_then(|v| v.2)
    }

    pub fn frame(&mut self, state: AudioState) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.25);
        self.last = now;
        self.engine.volumes = state.volumes;
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
        for s in state.gui_sounds {
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
