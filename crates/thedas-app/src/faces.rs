//! Lip sync: the speaker of a voiced conversation line moves its face with the line's
//! FaceFX animation (see [`thedas_formats::facefx`]).
//!
//! The line's animation has the voice sample's name (`<strref>_f.fxe` for a female hero
//! when there is one, else `<strref>_m.fxe`, as the voice-over picks its sample) and sits
//! in the voice-over folders' `facialanimations` archives
//! (`audio/vo/<language>/facialanimations/*.erf` of the core package, the DLC `core` folders
//! and the campaign's module, as [`thedas_audio::library::Library::install_dirs`] orders
//! them). The speaker's face rig is the one its race's base head names (field 6248 of
//! `<race><gender>_uhm_basa_0.mmh`: `hm_` → `humanmale.fxa`), the race and gender read from
//! the prefix of its head morph (`hm_duncan`), else of its models. The rig's bone offsets
//! are laid over the speaker's pose as an additive layer, so they move the face bones of
//! whichever of its models has them (the head).
//!
//! The lines animated: the open conversation's, a cutscene's own voiced lines (said by the
//! object drawn for the line's actor), and lines said in passing, each by its speaker at
//! once, until its animation's last key.
//!
//! Provisional: the animation runs from when the line comes up, on the frame clock, not
//! the voice's own playback; which of a creature's models names its rig in the original is
//! not traced (the race's base head is taken); the animation's blend in and out are not
//! applied.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use thedas_formats::anim::{Animation, Channel, Key, Track};
use thedas_formats::facefx::{FaceActor, FaceAnim};
use thedas_formats::gff4::Gff4;
use thedas_render::Clip;
use thedas_script::vm::ObjectId;
use thedas_vfs::{Vfs, join_ci};

use thedas_audio::library::Library;

/// The voice-over language folder (`audio/vo/<language>`), as the app's flag sets it.
static LANGUAGE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Sets the voice-over language the facial animations are read for (once, at start).
pub fn set_language(language: &str) {
    let _ = LANGUAGE.set(language.to_string());
}

/// The model field naming a model's FaceFX rig (`humanmale.fxa`).
const FIELD_FX_ACTOR: u32 = 6248;

/// The line being animated.
struct Playing {
    key: (String, u32, ObjectId),
    anim: Option<Arc<FaceAnim>>,
    time: f32,
}

#[derive(Default)]
pub struct Faces {
    /// The facial animation archives, opened when first needed.
    archives: Option<Vfs>,
    actors: HashMap<String, Option<Arc<FaceActor>>>,
    /// By race prefix (`hm`): its rig's name.
    rigs: HashMap<String, Option<String>>,
    playing: Option<Playing>,
    /// Lines said in passing, each by its own speaker.
    passing: Vec<Playing>,
}

/// How long a line said in passing stays animated after its last key (seconds).
const PASSING_TAIL: f32 = 0.5;

/// What a line asks of the faces: who says it, and its voice sample's two names.
pub struct SpokenLine {
    pub speaker: ObjectId,
    pub dialog: String,
    pub strref: u32,
    pub female: bool,
}

/// The facial animation archives of an install, lowest priority first.
fn archives(install: &Path, module: &str, language: &str) -> Vfs {
    let mut vfs = Vfs::new();
    for dir in Library::install_dirs(install, module, language) {
        // `audio/vo/<language>/vo` beside `audio/vo/<language>/facialanimations`.
        let Some(parent) = dir.parent() else {
            continue;
        };
        let faces = join_ci(parent, "facialanimations");
        let Ok(entries) = std::fs::read_dir(&faces) else {
            continue;
        };
        let mut erfs: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("erf")))
            .collect();
        erfs.sort();
        for erf in erfs {
            if let Err(e) = vfs.push_archive(&erf, erf.display().to_string()) {
                log::warn!("faces: {}: {e}", erf.display());
            }
        }
    }
    vfs
}

impl Faces {
    /// Follows the line on screen (`None` when no voiced conversation or cutscene line is),
    /// and the lines said in passing, `dt` seconds on.
    pub fn step(&mut self, install: &Path, module: &str, line: Option<SpokenLine>, dt: f32) {
        for p in &mut self.passing {
            p.time += dt;
        }
        // A line said in passing lasts as long as its animation (or a moment without one).
        self.passing
            .retain(|p| p.time <= p.anim.as_ref().map_or(0.0, |a| a.length()) + PASSING_TAIL);
        let Some(line) = line else {
            self.playing = None;
            return;
        };
        let key = (line.dialog.to_ascii_lowercase(), line.strref, line.speaker);
        if let Some(p) = self.playing.as_mut()
            && p.key == key
        {
            p.time += dt;
            return;
        }
        self.playing = Some(self.start(install, module, &line));
    }

    /// A line said in passing (`SpokenLine::speaker` says it at once), animated beside the
    /// open conversation's, replacing what that speaker was saying.
    pub fn say(&mut self, install: &Path, module: &str, line: &SpokenLine) {
        let started = self.start(install, module, line);
        self.passing.retain(|p| p.key.2 != line.speaker);
        self.passing.push(started);
    }

    /// A line's animation, read, from its start.
    fn start(&mut self, install: &Path, module: &str, line: &SpokenLine) -> Playing {
        let key = (line.dialog.to_ascii_lowercase(), line.strref, line.speaker);
        let archives = self.archives.get_or_insert_with(|| {
            let language = LANGUAGE.get().map_or("en-us", String::as_str);
            archives(install, module, language)
        });
        let read = |name: &str| {
            let (data, _) = archives.read(name).ok()?;
            FaceAnim::parse(&data)
                .map_err(|e| log::warn!("faces: {name}: {e}"))
                .ok()
        };
        let female = line
            .female
            .then(|| read(&format!("{}_f.fxe", line.strref)))
            .flatten();
        let anim = female
            .or_else(|| read(&format!("{}_m.fxe", line.strref)))
            .map(Arc::new);
        log::debug!(
            "faces: line {} of {} by {}: {}",
            line.strref,
            line.dialog,
            line.speaker,
            anim.as_ref().map_or("no animation", |a| a.name.as_str())
        );
        Playing {
            key,
            anim,
            time: 0.0,
        }
    }

    /// The face layer for creature `id`, whose head morph and models are `models` (the
    /// first whose race prefix has a rig gives it), if it is speaking a line with an
    /// animation.
    pub fn layer<'a>(
        &mut self,
        vfs: &Vfs,
        id: ObjectId,
        models: impl IntoIterator<Item = &'a String>,
    ) -> Option<Clip> {
        let p = self
            .playing
            .iter()
            .chain(&self.passing)
            .find(|p| p.key.2 == id)?;
        let anim = p.anim.clone()?;
        let time = p.time;
        let actor = models.into_iter().find_map(|m| self.actor(vfs, m));
        let actor = actor?;
        let values = actor.evaluate(|node| anim.curve(node).map(|c| c.at(time)));
        let mut tracks = Vec::new();
        for o in actor.bone_offsets(&values) {
            tracks.push(Track {
                bone: o.bone.clone(),
                channel: Channel::Rotation,
                keys: vec![Key {
                    time: 0.0,
                    value: o.rotation,
                }],
            });
            let [x, y, z] = o.translation;
            tracks.push(Track {
                bone: o.bone,
                channel: Channel::Translation,
                keys: vec![Key {
                    time: 0.0,
                    value: [x, y, z, 0.0],
                }],
            });
        }
        if tracks.is_empty() {
            return None;
        }
        Some(Clip {
            inline: Some(Arc::new(Animation {
                name: anim.name.clone(),
                length: 0.0,
                additive: true,
                tracks,
                scalars: Vec::new(),
            })),
            ..Default::default()
        })
    }

    /// The rig of the race and gender model `model` is named for (`hm_duncan` → the
    /// `hm` rig), read once.
    fn actor(&mut self, vfs: &Vfs, model: &str) -> Option<Arc<FaceActor>> {
        let prefix = model.split('_').next()?.to_ascii_lowercase();
        let rig = self
            .rigs
            .entry(prefix.clone())
            .or_insert_with(|| {
                let (data, _) = vfs.read(&format!("{prefix}_uhm_basa_0.mmh")).ok()?;
                let g = Gff4::parse(&data).ok()?;
                let name = g.field(g.root, FIELD_FX_ACTOR)?.as_str()?;
                let name = name.trim_end_matches('\0').to_ascii_lowercase();
                (!name.is_empty()).then_some(name)
            })
            .clone()?;
        self.actors
            .entry(rig.clone())
            .or_insert_with(|| {
                let (data, _) = vfs.read(&rig).ok()?;
                FaceActor::parse(&data)
                    .map_err(|e| log::warn!("faces: {rig}: {e}"))
                    .ok()
                    .map(Arc::new)
            })
            .clone()
    }
}
