//! Playing FMOD events: what the game asks for by path (`music/_menu/main`,
//! `glo_gui/gui/gui_button2`, a conversation line's `.../VO/VOconversation` with the line's own
//! sample) turned into mixer voices.
//!
//! What an event does is read from its `.fev` ([`thedas_formats::fev`]): each layer's sound
//! instances play their sound definition's waveforms (picked by the definition's play mode, or
//! supplied by the game for programmer-selected definitions such as voice-over), at the event's
//! volume times the definition's, the category's and the player's volume, faded in and out
//! over the event's fade times, looped or not by the instance's loop mode, respawned on the
//! definition's spawn interval, and for 3D events attenuated by distance (the layer's
//! `(distance)` volume envelope, or the event's rolloff between its min and max distance) and
//! panned towards the source.
//!
//! Provisional, not traced in `fmod_event.dll`'s playback code: the loop-mode meanings (0 and 2
//! loop, 1 plays once), spawn timing, envelope interpolation (linear), the stereo pan of 3D
//! sources, and the mapping of the options screen's 0..10 volumes to gain (linear).

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use thedas_formats::fev::{
    Category, Event, EventBody, Fev, PARAM_DISTANCE, SoundDef, SoundDefTemplate, WaveformKind,
    envelope, mode, play_mode,
};

use crate::decode::Pcm;
use crate::library::Library;
use crate::mixer::{Bus, Mixer, VoiceId, VoiceParams};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EventHandle(pub u64);

/// What to play and how.
#[derive(Clone, Debug)]
pub struct PlayOptions {
    /// The sample (by name, e.g. `179996_m`) a programmer-selected sound definition plays: a
    /// conversation line's voice-over.
    pub programmer_sound: Option<String>,
    /// World position of a 3D event.
    pub position: Option<[f32; 3]>,
    /// Parameter values to start with.
    pub parameters: Vec<(String, f32)>,
    /// A gain on top of the event's own (a placed sound's `Volume`).
    pub volume: f32,
    /// Scales the event's distances (a placed sound's `MaxDistanceMult`): the source is
    /// heard as though it were this much closer (below 1: farther).
    pub distance_scale: f32,
}

impl Default for PlayOptions {
    fn default() -> Self {
        PlayOptions {
            programmer_sound: None,
            position: None,
            parameters: Vec::new(),
            volume: 1.0,
            distance_scale: 1.0,
        }
    }
}

/// The player's volume settings (the options screen's 0..=10 sliders, as 0..=1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Volumes {
    pub music: f32,
    pub sfx: f32,
    pub dialog: f32,
}

impl Default for Volumes {
    fn default() -> Self {
        Volumes {
            music: 1.0,
            sfx: 1.0,
            dialog: 1.0,
        }
    }
}

struct Listener {
    position: [f32; 3],
    right: [f32; 3],
}

struct Slot {
    /// Which sound instance of the event (layer, index), and its layer's control parameter.
    layer: usize,
    instance: usize,
    voices: Vec<VoiceId>,
    pending: usize,
    cursor: usize,
    /// Seconds until the next spawn (spawning definitions), or none.
    next_spawn: Option<f32>,
    /// Played its one shot (oneshot instances) or cannot play.
    finished: bool,
    started: bool,
    /// The sample it last picked (bank, index).
    sample: Option<(String, usize)>,
    /// Each voice's own gain factor (its sound definition's random volume).
    gains: HashMap<VoiceId, f32>,
}

struct Instance {
    handle: EventHandle,
    project: Arc<Fev>,
    path: String,
    /// Group indices from the top, then the event's index in its group.
    event: (Vec<usize>, usize),
    options: PlayOptions,
    slots: Vec<Slot>,
    /// 0..=1 fade multiplier and its per-second change.
    fade: f32,
    fade_rate: f32,
    stopping: bool,
    params: HashMap<String, f32>,
    category_gain: f32,
    /// The event's category, lowercase.
    category: String,
    /// What the event ducks while it plays (its `#duck:` parameters).
    ducks: Vec<Duck>,
    bus: Bus,
    /// Linear level sent to the world reverb.
    reverb_send: f32,
    /// Playback-rate factor from the event's pitch and its random offset.
    rate: f32,
    age: f32,
}

impl Instance {
    /// The instance's event (found when it started, so always there).
    fn event(&self) -> Option<&Event> {
        resolve(&self.project, &self.event)
    }
}

/// A parameter's value in its range, 0..=1 (sound instances and envelopes are placed on it).
fn normalized(p: &thedas_formats::fev::Parameter, params: &HashMap<String, f32>) -> f32 {
    let v = *params.get(&p.name.to_ascii_lowercase()).unwrap_or(&p.min);
    if p.max > p.min {
        ((v - p.min) / (p.max - p.min)).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Find an event by path (`project/group/.../event`, case-insensitive) as indices.
fn locate(fev: &Fev, path: &str) -> Option<(Vec<usize>, usize)> {
    let mut parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() > 1 && parts[0].eq_ignore_ascii_case(&fev.name) {
        parts.remove(0);
    }
    let (event, groups) = parts.split_last()?;
    let mut idx = Vec::new();
    let mut list = &fev.groups;
    let mut cur = None;
    for g in groups {
        let i = list.iter().position(|x| x.name.eq_ignore_ascii_case(g))?;
        idx.push(i);
        cur = Some(&list[i]);
        list = &list[i].groups;
    }
    let e = cur?
        .events
        .iter()
        .position(|e| e.name.eq_ignore_ascii_case(event))?;
    Some((idx, e))
}

fn resolve<'a>(fev: &'a Fev, at: &(Vec<usize>, usize)) -> Option<&'a Event> {
    let (first, rest) = at.0.split_first()?;
    let mut g = fev.groups.get(*first)?;
    for i in rest {
        g = g.groups.get(*i)?;
    }
    g.events.get(at.1)
}

/// A decode finished on the loader thread.
struct Loaded {
    handle: EventHandle,
    slot: usize,
    pcm: Option<Arc<Pcm>>,
    looping: bool,
    /// This play's sound-definition rate and gain factors (see [`template_variation`]).
    rate: f32,
    gain: f32,
}

pub struct AudioEngine {
    pub library: Arc<Library>,
    pub mixer: Arc<Mutex<Mixer>>,
    instances: Vec<Instance>,
    next: u64,
    /// Category volumes (with their parents' applied), by lowercase name.
    categories: HashMap<String, f32>,
    /// Each category's parent (lowercase), for ducking a category and those under it.
    parents: HashMap<String, String>,
    /// Ducking now applied per category: dB now, the fade-out it releases over (ms) and the
    /// duck's depth (dB, positive), which the ramps cover in their fade times.
    ducking: HashMap<String, (f32, f32, f32)>,
    pub volumes: Volumes,
    listener: Listener,
    rng: u64,
    /// Decode on the calling thread (offline renders) instead of a loader thread.
    pub synchronous: bool,
    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
}

fn bus_for(category: &str) -> Bus {
    let c = category.to_ascii_lowercase();
    if c == "music" || c == "stinger" {
        Bus::Music
    } else if c.starts_with("vo") {
        Bus::Dialog
    } else if c == "gui" {
        Bus::Ui
    } else if c.contains("ambient") || c == "amb_oneshots" {
        Bus::Ambient
    } else {
        Bus::Sfx
    }
}

/// A `#duck:<category>,<fade in ms>,<?>,<fade out ms>,<dB>` parameter: while its event plays,
/// the category (and those under it) is turned down by dB, ramping in and out over the fade
/// times. DA:O's events name them (the intro: `#duck:music,3000,0,5000,-60`). Provisional:
/// the third number's meaning, the dB ramp (linear in dB), and `#pause:` (not done).
#[derive(Clone, Debug, PartialEq)]
pub struct Duck {
    pub category: String,
    pub fade_in_ms: f32,
    pub fade_out_ms: f32,
    pub db: f32,
}

impl Duck {
    pub fn parse(name: &str) -> Option<Duck> {
        let rest = name.trim().strip_prefix("#duck:")?;
        let parts: Vec<&str> = rest.split(',').map(str::trim).collect();
        let num = |i: usize| parts.get(i).and_then(|p| p.parse::<f32>().ok());
        Some(Duck {
            category: parts.first()?.to_ascii_lowercase(),
            fade_in_ms: num(1).unwrap_or(0.0),
            fade_out_ms: num(3).unwrap_or(0.0),
            db: num(4)?,
        })
    }
}

fn parent_map(c: &Category, out: &mut HashMap<String, String>) {
    for k in &c.children {
        out.insert(k.name.to_ascii_lowercase(), c.name.to_ascii_lowercase());
        parent_map(k, out);
    }
}

fn flatten(c: &Category, parent: f32, out: &mut HashMap<String, f32>) {
    let v = parent * c.volume;
    out.insert(c.name.to_ascii_lowercase(), v);
    for k in &c.children {
        flatten(k, v, out);
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

impl AudioEngine {
    pub fn new(library: Arc<Library>, mixer: Arc<Mutex<Mixer>>) -> AudioEngine {
        let mut categories = HashMap::new();
        let mut parents = HashMap::new();
        if let Some(cats) = library.project("audio_categories") {
            flatten(&cats.category, 1.0, &mut categories);
            parent_map(&cats.category, &mut parents);
        }
        let (tx, rx) = channel();
        AudioEngine {
            library,
            mixer,
            instances: Vec::new(),
            next: 1,
            categories,
            parents,
            ducking: HashMap::new(),
            volumes: Volumes::default(),
            listener: Listener {
                position: [0.0; 3],
                right: [1.0, 0.0, 0.0],
            },
            rng: 0x9e37_79b9_7f4a_7c15,
            synchronous: false,
            tx,
            rx,
        }
    }

    fn random(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    /// Start the event at `path` (`project/group/.../event`). `None` when the project or
    /// event does not exist.
    pub fn play(&mut self, path: &str, options: PlayOptions) -> Option<EventHandle> {
        let project_name = path.split('/').find(|s| !s.is_empty())?;
        let project = self.library.project(project_name)?;
        let index = locate(&project, path).or_else(|| {
            log::debug!("audio: no event {path}");
            None
        })?;
        let event = resolve(&project, &index)?;
        // The event's playback limit (FMOD Designer's max playbacks and what happens past it:
        // 1 steals the oldest, 2 the newest, 3 the quietest (taken as the oldest), 4 and 5
        // fail).
        let playing: Vec<EventHandle> = self
            .instances
            .iter()
            .filter(|i| !i.stopping && Arc::ptr_eq(&i.project, &project) && i.event == index)
            .map(|i| i.handle)
            .collect();
        if playing.len() >= event.max_playbacks.max(1) as usize {
            let steal = match event.max_playbacks_behavior {
                1 | 3 => playing.first().copied(),
                2 => playing.last().copied(),
                _ => None,
            };
            let h = steal?;
            self.stop(h, true);
        }
        let handle = EventHandle(self.next);
        self.next += 1;
        let category = event.categories.first().cloned().unwrap_or_default();
        // Categories are global by name: `audio_categories` (loaded first, as the game does)
        // defines them; a category it lacks takes the event's own project's definition.
        let category_gain = match self.categories.get(&category.to_ascii_lowercase()) {
            Some(v) => *v,
            None => {
                let mut own = HashMap::new();
                flatten(&project.category, 1.0, &mut own);
                *own.get(&category.to_ascii_lowercase()).unwrap_or(&1.0)
            }
        };
        let mut slots = Vec::new();
        match &event.body {
            EventBody::Simple { .. } => slots.push((0, 0)),
            EventBody::Complex { layers, .. } => {
                for (li, l) in layers.iter().enumerate() {
                    for si in 0..l.sounds.len() {
                        slots.push((li, si));
                    }
                }
            }
        }
        let mut params: HashMap<String, f32> = HashMap::new();
        let mut ducks = Vec::new();
        if let EventBody::Complex { parameters, .. } = &event.body {
            ducks = parameters
                .iter()
                .filter_map(|p| Duck::parse(&p.name))
                .collect();
            for p in parameters {
                params.insert(p.name.to_ascii_lowercase(), p.min);
            }
        }
        for (k, v) in &options.parameters {
            params.insert(k.to_ascii_lowercase(), *v);
        }
        // The world reverb: events in the `gui`, `music` and `master` categories send
        // nothing (DAOrigins.exe 0x5bf217..0x5bf255 names them; 0x5be6a0 mutes their
        // sends), the rest send at the event's reverb wet level (dB).
        let leaf = category
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let reverb_send = if matches!(leaf.as_str(), "gui" | "music" | "master") {
            0.0
        } else {
            db(event.reverb_wet)
        };
        let category_gain = category_gain * db(event.reverb_dry);
        let (rate, random_gain) = {
            let u = |e: &mut Self| (e.random() % 32768) as f32 / 32767.0;
            let (u1, u2) = (u(self), u(self));
            event_variation(
                event.pitch,
                event.pitch_randomization,
                event.volume_randomization,
                event.flags,
                (u1, u2),
            )
        };
        let category_gain = category_gain * random_gain;
        let fade_in = event.fade_in_ms as f32 / 1000.0;
        let inst = Instance {
            handle,
            project: project.clone(),
            path: path.to_string(),
            event: index,
            options,
            slots: slots
                .into_iter()
                .map(|(layer, instance)| Slot {
                    layer,
                    instance,
                    voices: Vec::new(),
                    pending: 0,
                    cursor: 0,
                    next_spawn: None,
                    finished: false,
                    started: false,
                    sample: None,
                    gains: HashMap::new(),
                })
                .collect(),
            fade: if fade_in > 0.0 { 0.0 } else { 1.0 },
            fade_rate: if fade_in > 0.0 { 1.0 / fade_in } else { 0.0 },
            stopping: false,
            params,
            category_gain,
            category: category.to_ascii_lowercase(),
            ducks,
            bus: bus_for(&category),
            reverb_send,
            rate,
            age: 0.0,
        };
        log::debug!(
            "audio: play {path} (pitch x{rate:.3} = {:+.2} semitones, random gain x{random_gain:.3})",
            12.0 * rate.log2()
        );
        self.instances.push(inst);
        self.update(0.0);
        Some(handle)
    }

    /// Fade the world reverb to `preset` over `seconds` (0: at once). The preset by
    /// name comes from [`AudioEngine::reverb_preset`].
    pub fn set_reverb(&mut self, preset: thedas_formats::fev::ReverbProperties, seconds: f32) {
        if let Ok(mut m) = self.mixer.lock() {
            m.reverb.set(preset, seconds);
        }
    }

    /// The reverb preset named `name` in `master_reverb.fev`, as DAOrigins.exe looks it up
    /// (0x5c23e0): an empty name is no reverb, `force_generic` FMOD's generic preset, any
    /// other name the project's preset of that name (case-insensitively; `None` if none).
    pub fn reverb_preset(&self, name: &str) -> Option<thedas_formats::fev::ReverbProperties> {
        use thedas_formats::fev::ReverbProperties;
        let name = name.trim();
        if name.is_empty() {
            return Some(ReverbProperties::OFF);
        }
        if name.eq_ignore_ascii_case("force_generic") {
            return Some(ReverbProperties::GENERIC);
        }
        let project = self.library.project("master_reverb")?;
        project
            .reverbs
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(name))
            .and_then(|r| r.properties())
    }

    /// Where a playing event is in its (first playing) sample, and that sample's sync points
    /// (music markers), in seconds; `None` until a voice plays.
    pub fn position(&self, handle: EventHandle) -> Option<(f64, Vec<f64>)> {
        let inst = self.instances.iter().find(|i| i.handle == handle)?;
        let mixer = self.mixer.lock().ok()?;
        for slot in &inst.slots {
            for v in &slot.voices {
                if let Some(at) = mixer.position(*v) {
                    let markers = slot
                        .sample
                        .as_ref()
                        .map(|(b, i)| self.library.sync_points(b, *i))
                        .unwrap_or_default();
                    return Some((at, markers));
                }
            }
        }
        None
    }

    /// Stop an event: faded over its fade-out time, or at once.
    pub fn stop(&mut self, handle: EventHandle, immediate: bool) {
        let Some(i) = self.instances.iter_mut().find(|i| i.handle == handle) else {
            return;
        };
        let fade_out = i.event().map_or(0, |e| e.fade_out_ms) as f32 / 1000.0;
        if immediate || fade_out <= 0.0 {
            i.fade = 0.0;
        }
        i.stopping = true;
        i.fade_rate = if fade_out > 0.0 {
            -1.0 / fade_out
        } else {
            -1e9
        };
        self.update(0.0);
    }

    pub fn stop_all(&mut self) {
        let hs: Vec<_> = self.instances.iter().map(|i| i.handle).collect();
        for h in hs {
            self.stop(h, true);
        }
    }

    pub fn is_playing(&self, handle: EventHandle) -> bool {
        self.instances.iter().any(|i| i.handle == handle)
    }

    /// The path an event was started with.
    pub fn path(&self, handle: EventHandle) -> Option<&str> {
        self.instances
            .iter()
            .find(|i| i.handle == handle)
            .map(|i| i.path.as_str())
    }

    pub fn set_parameter(&mut self, handle: EventHandle, name: &str, value: f32) {
        if let Some(i) = self.instances.iter_mut().find(|i| i.handle == handle) {
            i.params.insert(name.to_ascii_lowercase(), value);
        }
    }

    pub fn set_position(&mut self, handle: EventHandle, position: [f32; 3]) {
        if let Some(i) = self.instances.iter_mut().find(|i| i.handle == handle) {
            i.options.position = Some(position);
        }
    }

    /// The listener (camera) position and its right-hand direction.
    pub fn set_listener(&mut self, position: [f32; 3], right: [f32; 3]) {
        self.listener = Listener { position, right };
    }

    pub fn active_events(&self) -> usize {
        self.instances.len()
    }

    /// Pick the waveform a slot plays next: (bank, sample index, looping).
    fn pick(
        &mut self,
        fev: &Fev,
        def: &SoundDef,
        programmer: Option<&str>,
        cursor: usize,
    ) -> Option<(String, usize)> {
        let mode = fev
            .template(def)
            .map(|t| t.play_mode)
            .unwrap_or(play_mode::SEQUENTIAL);
        let waves: Vec<_> = def
            .waveforms
            .iter()
            .filter(|w| !matches!(w.kind, WaveformKind::DontPlay))
            .collect();
        let programmer_bank = || {
            fev.wave_banks
                .first()
                .map(|b| b.name.clone())
                .unwrap_or_else(|| fev.name.clone())
        };
        if mode == play_mode::PROGRAMMER_SELECTED
            || waves
                .iter()
                .all(|w| matches!(w.kind, WaveformKind::Programmer))
        {
            let name = programmer?;
            // The line's sample lives in the project's own bank (or one of its banks).
            for b in fev
                .wave_banks
                .iter()
                .map(|b| b.name.clone())
                .chain([programmer_bank()])
            {
                if let Some(i) = self.library.find_sample(&b, name) {
                    return Some((b, i));
                }
            }
            log::debug!("audio: no sample {name} in {}", fev.name);
            return None;
        }
        if waves.is_empty() {
            return None;
        }
        let w = match mode {
            play_mode::RANDOM | play_mode::RANDOM_NO_REPEAT | play_mode::SHUFFLE => {
                let total: u64 = waves.iter().map(|w| w.weight.max(1) as u64).sum();
                let mut r = self.random() % total.max(1);
                let mut pick = waves[0];
                for w in &waves {
                    let wt = w.weight.max(1) as u64;
                    if r < wt {
                        pick = w;
                        break;
                    }
                    r -= wt;
                }
                pick
            }
            _ => waves[cursor % waves.len()],
        };
        match &w.kind {
            WaveformKind::Wave { bank, index, .. } => Some((bank.clone(), *index as usize)),
            WaveformKind::Programmer => {
                let name = programmer?;
                let b = programmer_bank();
                self.library.find_sample(&b, name).map(|i| (b, i))
            }
            _ => None,
        }
    }

    /// Start one waveform in a slot (decoded here or on a loader thread).
    fn spawn(&mut self, ii: usize, si: usize) {
        let draws = (
            (self.random() % 32768) as f32 / 32767.0,
            (self.random() % 32768) as f32 / 32767.0,
        );
        let inst = &self.instances[ii];
        let fev = inst.project.clone();
        let Some(ev) = inst.event() else {
            return;
        };
        let slot = &inst.slots[si];
        let sound = match &ev.body {
            EventBody::Simple { sound, .. } => sound,
            EventBody::Complex { layers, .. } => &layers[slot.layer].sounds[slot.instance],
        };
        let Some(def) = fev.sound_def(sound) else {
            self.instances[ii].slots[si].finished = true;
            return;
        };
        let loops = sound.loop_mode != 1;
        let single = def.waveforms.len() <= 1;
        let spawning = fev.template(def).is_some_and(|t| t.spawn_time_max_ms > 0);
        let (rate, gain) = match fev.template(def) {
            Some(t) => template_variation(t, draws),
            None => (1.0, 1.0),
        };
        let programmer = inst.options.programmer_sound.clone();
        let cursor = slot.cursor;
        let handle = inst.handle;
        let def = def.clone();
        let picked = self.pick(&fev, &def, programmer.as_deref(), cursor);
        let slot = &mut self.instances[ii].slots[si];
        slot.cursor += 1;
        slot.started = true;
        let Some((bank, index)) = picked else {
            slot.finished = true;
            return;
        };
        slot.sample = Some((bank.clone(), index));
        // A lone looping waveform loops in the mixer; several play one after another.
        let looping = loops && single && !spawning;
        slot.pending += 1;
        let tx = self.tx.clone();
        if self.synchronous {
            let pcm = self.library.sample(&bank, index);
            let _ = tx.send(Loaded {
                handle,
                slot: si,
                pcm,
                looping,
                rate,
                gain,
            });
            self.receive();
        } else {
            let lib = self.library.clone();
            std::thread::spawn(move || {
                let pcm = lib.sample(&bank, index);
                let _ = tx.send(Loaded {
                    handle,
                    slot: si,
                    pcm,
                    looping,
                    rate,
                    gain,
                });
            });
        }
    }

    fn receive(&mut self) {
        while let Ok(l) = self.rx.try_recv() {
            let Some(inst) = self.instances.iter_mut().find(|i| i.handle == l.handle) else {
                continue;
            };
            let Some(slot) = inst.slots.get_mut(l.slot) else {
                continue;
            };
            slot.pending = slot.pending.saturating_sub(1);
            let Some(pcm) = l.pcm else {
                slot.finished = true;
                continue;
            };
            let frames = pcm.frames();
            let id = self.mixer.lock().map(|mut m| {
                m.play(
                    pcm,
                    VoiceParams {
                        gain: 0.0,
                        looping: l.looping.then_some((0, frames)),
                        bus: inst.bus,
                        reverb_send: inst.reverb_send,
                        pitch: inst.rate * l.rate,
                        ..Default::default()
                    },
                )
            });
            if let Ok(id) = id {
                log::debug!(
                    "audio: voice of {} (sound pitch x{:.3}, sound gain x{:.3})",
                    inst.path,
                    l.rate,
                    l.gain
                );
                slot.voices.push(id);
                slot.gains.insert(id, l.gain);
            }
        }
    }

    /// Advance by `dt` seconds: start, respawn and retire voices and set their gains.
    pub fn update(&mut self, dt: f32) {
        self.receive();
        let mut spawns = Vec::new();
        let mut stops = Vec::new();
        let mut rng = self.rng;
        let rate = self.mixer.lock().map(|m| m.rate()).unwrap_or(48000) as f32;
        for ii in 0..self.instances.len() {
            let inst = &mut self.instances[ii];
            inst.age += dt;
            inst.fade = (inst.fade + inst.fade_rate * dt).clamp(0.0, 1.0);
            if !inst.stopping && inst.fade >= 1.0 {
                inst.fade_rate = 0.0;
            }
            // Drop voices the mixer has finished.
            if let Ok(m) = self.mixer.lock() {
                for s in &mut inst.slots {
                    s.voices.retain(|v| m.is_playing(*v));
                }
            }
            if inst.stopping {
                continue;
            }
            let fev = inst.project.clone();
            let Some(ev) = resolve(&fev, &inst.event) else {
                continue;
            };
            for (si, s) in inst.slots.iter_mut().enumerate() {
                let sound = match &ev.body {
                    EventBody::Simple { sound, .. } => sound,
                    EventBody::Complex { layers, .. } => &layers[s.layer].sounds[s.instance],
                };
                let Some(def) = fev.sound_def(sound) else {
                    s.finished = true;
                    continue;
                };
                let t = fev.template(def);
                let spawn_max = t.map(|t| t.spawn_time_max_ms).unwrap_or(0);
                let max_spawned = t.map(|t| t.max_spawned.max(1)).unwrap_or(1) as usize;
                // A sound instance plays while its layer's control parameter is within it.
                if let EventBody::Complex {
                    layers, parameters, ..
                } = &ev.body
                {
                    let p = parameters.get(layers[s.layer].control_parameter as usize);
                    let x = p.map(|p| normalized(p, &inst.params)).unwrap_or(0.0);
                    let covered = x >= sound.start - 1e-4 && x <= sound.start + sound.length + 1e-4;
                    if !covered {
                        if s.started {
                            stops.append(&mut s.voices);
                            s.started = false;
                            s.finished = false;
                            s.next_spawn = None;
                        }
                        continue;
                    }
                }
                if s.finished {
                    continue;
                }
                if !s.started {
                    spawns.push((ii, si));
                    if spawn_max > 0 {
                        let min = t.map(|t| t.spawn_time_min_ms).unwrap_or(0) as f32;
                        s.next_spawn = Some(min.max(1.0) / 1000.0);
                    }
                    continue;
                }
                if let Some(ns) = &mut s.next_spawn {
                    *ns -= dt;
                    if *ns <= 0.0 && s.voices.len() + s.pending < max_spawned {
                        let min = t.map(|t| t.spawn_time_min_ms).unwrap_or(0) as f32;
                        let span = (spawn_max as f32 - min).max(0.0);
                        rng ^= rng << 13;
                        rng ^= rng >> 7;
                        rng ^= rng << 17;
                        let r = (rng % 10_000) as f32 / 10_000.0;
                        *ns = (min + span * r).max(1.0) / 1000.0;
                        spawns.push((ii, si));
                    }
                    continue;
                }
                if s.voices.is_empty() && s.pending == 0 {
                    if sound.loop_mode != 1 && def.waveforms.len() > 1 {
                        spawns.push((ii, si));
                    } else {
                        s.finished = true;
                    }
                }
            }
        }
        self.rng = rng;
        if let Ok(mut m) = self.mixer.lock() {
            for v in stops {
                m.fade(v, 0.0, 0.05, true);
            }
        }
        for (ii, si) in spawns {
            self.spawn(ii, si);
        }
        // Ducking: each category moves towards the deepest duck its playing events ask for,
        // over the duck's fade-in, and back over its fade-out once they stop.
        let mut targets: HashMap<String, (f32, f32, f32)> = HashMap::new();
        for inst in &self.instances {
            if inst.stopping {
                continue;
            }
            for d in &inst.ducks {
                let e =
                    targets
                        .entry(d.category.clone())
                        .or_insert((0.0, d.fade_in_ms, d.fade_out_ms));
                if d.db < e.0 {
                    *e = (d.db, d.fade_in_ms, d.fade_out_ms);
                }
            }
        }
        for (k, (db, _, fout)) in &targets {
            let e = self.ducking.entry(k.clone()).or_insert((0.0, 0.0, 0.0));
            e.1 = *fout;
            e.2 = db.abs().max(e.2);
        }
        for (k, e) in self.ducking.iter_mut() {
            let (target, fade_ms) = match targets.get(k) {
                Some((db, fin, _)) => (*db, *fin),
                None => (0.0, e.1),
            };
            let span = e.2.max(1.0);
            let step = if fade_ms > 0.0 {
                span * dt * 1000.0 / fade_ms
            } else {
                f32::INFINITY
            };
            e.0 = if target < e.0 {
                (e.0 - step).max(target)
            } else {
                (e.0 + step).min(target)
            };
        }
        self.ducking
            .retain(|k, d| d.0 < -1e-3 || targets.contains_key(k));
        // Gains and pans.
        let listener = &self.listener;
        let mut mixer = match self.mixer.lock() {
            Ok(m) => m,
            Err(_) => return,
        };
        let ramp = dt.max(64.0 / rate);
        for inst in &mut self.instances {
            let fev = inst.project.clone();
            let Some(ev) = resolve(&fev, &inst.event) else {
                continue;
            };
            let user = match inst.bus {
                Bus::Music => self.volumes.music,
                Bus::Dialog => self.volumes.dialog,
                _ => self.volumes.sfx,
            };
            // The category's and its parents' ducking.
            let mut duck_db = 0.0;
            let mut c = Some(inst.category.clone());
            for _ in 0..32 {
                let Some(name) = c else {
                    break;
                };
                duck_db += self.ducking.get(&name).map(|d| d.0).unwrap_or(0.0);
                c = self.parents.get(&name).cloned();
            }
            let duck_gain = 10f32.powf(duck_db / 20.0);
            let mut gain3d = 1.0;
            let mut pan = 0.0;
            let distance = inst.options.position.filter(|_| ev.is_3d()).map(|p| {
                let d = sub(p, listener.position);
                let len = dot(d, d).sqrt();
                if len > 1e-3 {
                    pan = (dot(d, listener.right) / len).clamp(-1.0, 1.0) * ev.pan_level_3d;
                }
                len / inst.options.distance_scale.max(1e-3)
            });
            if let Some(d) = distance {
                inst.params.insert(PARAM_DISTANCE.to_string(), d);
                if ev.mode & mode::CUSTOM_ROLLOFF == 0 {
                    let (min, max) = (ev.min_distance.max(1e-3), ev.max_distance.max(1e-3));
                    let dd = d.clamp(min, max);
                    gain3d = if ev.mode & mode::LINEAR_ROLLOFF != 0 {
                        1.0 - (dd - min) / (max - min).max(1e-3)
                    } else {
                        min / dd
                    };
                }
            }
            for s in &inst.slots {
                let (sound, env_gain) = match &ev.body {
                    EventBody::Simple { sound, .. } => (sound, 1.0),
                    EventBody::Complex {
                        layers, parameters, ..
                    } => {
                        let layer = &layers[s.layer];
                        let p = parameters.get(layer.control_parameter as usize);
                        let x = p
                            .map(|p| {
                                let v = *inst
                                    .params
                                    .get(&p.name.to_ascii_lowercase())
                                    .unwrap_or(&p.min);
                                if p.max > p.min {
                                    ((v - p.min) / (p.max - p.min)).clamp(0.0, 1.0)
                                } else {
                                    0.0
                                }
                            })
                            .unwrap_or(0.0);
                        let g: f32 = layer
                            .envelopes
                            .iter()
                            .filter(|e| {
                                e.effect.is_empty() && e.flags & envelope::VOLUME != 0 && !e.muted
                            })
                            .filter_map(|e| e.value(x))
                            .product();
                        (&layer.sounds[s.instance], g)
                    }
                };
                let tvol = fev
                    .sound_def(sound)
                    .and_then(|d| fev.template(d))
                    .map(|t| t.volume)
                    .unwrap_or(1.0);
                let g = ev.volume
                    * inst.options.volume
                    * tvol
                    * inst.category_gain
                    * duck_gain
                    * user
                    * env_gain
                    * gain3d
                    * inst.fade;
                for v in &s.voices {
                    let own = s.gains.get(v).copied().unwrap_or(1.0);
                    mixer.fade(*v, g * own, ramp, false);
                    mixer.set_pan(*v, pan);
                }
            }
        }
        // Retire events that finished or faded out.
        self.instances.retain(|inst| {
            let silent = inst.stopping && inst.fade <= 0.0;
            // An event that plays nothing but ducks (DA:O's `audio_categories/ducking/*`,
            // which the game starts and stops with its modes) lasts until it is stopped.
            // Provisional: FMOD's lifetime for an event with no sounds is not traced; the
            // exe stops these explicitly (0x5bd220), which says they last.
            let done = !(inst.slots.is_empty() && !inst.ducks.is_empty())
                && inst
                    .slots
                    .iter()
                    .all(|s| s.finished && s.voices.is_empty() && s.pending == 0);
            if silent {
                for s in &inst.slots {
                    for v in &s.voices {
                        mixer.stop(*v);
                    }
                }
            }
            !(silent || done)
        });
    }
}

/// A sound definition's playback-rate and gain factors for one play of it, from two uniform
/// draws in 0..=1 (`fmod_event.dll` 0x1002eb00 and 0x1002e9f0, run as a sound starts):
/// - pitch: `2^(4 * pitch + (2 * u1 - 1) * 4 * r)` for the template's raw pitch (+0x20) and
///   pitch randomization `r` (+0x2c, our `f2c`), when its pitch method bit is set;
/// - volume: `m + u2 * (1 - m)` for its minimum gain `m` (+0x1c, our `f1c`), when its volume
///   method bit is set.
///
/// Provisional: DA:O's templates all use those methods (`pitch_randomization_method` and
/// `volume_randomization_method` 1); the other method (a range, `f14`..`f18` and
/// `f24`..`f28`, in 1/10000 steps) is not modelled. The template's fixed volume is applied
/// with the event's.
fn template_variation(t: &SoundDefTemplate, (u1, u2): (f32, f32)) -> (f32, f32) {
    let mut octaves = 4.0 * t.pitch;
    if t.pitch_randomization_method == 1 && t.f2c != 0.0 {
        octaves += (2.0 * u1 - 1.0) * 4.0 * t.f2c;
    }
    let gain = if t.volume_randomization_method == 1 && t.f1c != 1.0 {
        t.f1c + u2 * (1.0 - t.f1c)
    } else {
        1.0
    };
    (2f32.powf(octaves), gain)
}

/// An event's playback-rate factor and gain factor for one play (its raw pitch `pitch`, pitch
/// randomization `r`, volume randomization `v` and flags), from two uniform draws in
/// 0..=1 (`rand() / 32767` in `fmod_event.dll`):
/// - pitch: the event's raw pitch plus a random offset `2 * u1 * r - r` for its pitch
///   randomization `r`, rounded to semitones (1/48) or tones (1/24) when the event's flags say
///   so (bits 30, 31; 0x1000f7c3), as a rate `2^(4 * raw)` (raw 1 is 4 octaves);
/// - volume: `1 - v + u2 * v` for its volume randomization `v` (0x10010184).
fn event_variation(pitch: f32, r: f32, v: f32, flags: u32, (u1, u2): (f32, f32)) -> (f32, f32) {
    let mut offset = 0.0;
    if r != 0.0 {
        let v = 2.0 * u1 * r;
        let v = match flags & 0xc000_0000 {
            0x4000_0000 => (v * 48.0 + 0.5).trunc() / 48.0,
            0x8000_0000 => (v * 24.0 + 0.5).trunc() / 24.0,
            _ => v,
        };
        offset = v - r;
    }
    let rate = 2f32.powf(4.0 * (pitch + offset));
    let gain = if v != 0.0 { 1.0 - v + u2 * v } else { 1.0 };
    (rate, gain)
}

/// Decibels to linear gain.
fn db(v: f32) -> f32 {
    10f32.powf(v / 20.0)
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

    /// A complex 2D event whose one layer plays sound definition `def`.
    fn event(
        w: &mut W,
        name: &str,
        volume: f32,
        fade_in: u32,
        fade_out: u32,
        def: u16,
        loop_mode: u32,
    ) {
        event_with(
            w,
            name,
            volume,
            (fade_in, fade_out),
            def,
            loop_mode,
            "param00",
        );
    }

    fn event_with(
        w: &mut W,
        name: &str,
        volume: f32,
        (fade_in, fade_out): (u32, u32),
        def: u16,
        loop_mode: u32,
        param: &str,
    ) {
        w.u32(8).s(name);
        w.0.extend_from_slice(&[0; 16]);
        w.f(volume)
            .f(0.0)
            .f(0.0)
            .f(0.0)
            .u32(0)
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
            .f(0.0)
            .f(0.0)
            .f(0.0)
            .f(0.0);
        w.u32(fade_in).u32(fade_out).f(1.0).f(0.0).f(1.0).u32(0);
        w.u32(1).u16(2).u16(0xffff).u16(0).u16(1).u16(0);
        w.u16(def)
            .f(0.0)
            .f(1.0)
            .u32(0)
            .u32(loop_mode)
            .u32(u32::MAX)
            .u32(0);
        w.f(0.0).f(0.0).f(0.0).f(1.0).f(-1.0).f(-1.0).u32(2).u32(2);
        w.u32(1)
            .s(param)
            .f(0.0)
            .f(0.0)
            .f(1.0)
            .u32(3)
            .f(0.0)
            .f(0.0)
            .u32(0);
        w.u32(0).u32(1).s("sfx");
    }

    fn project(dir: &std::path::Path) {
        let mut w = W(Vec::new());
        w.0.extend_from_slice(b"FEV1");
        w.u32(thedas_formats::fev::VERSION).u32(0).u32(0).s("test");
        w.u32(1).u32(0x200).u32(1).s("test");
        w.s("master").f(1.0).f(0.0).u32(0).u32(0).u32(1);
        w.s("sfx").f(0.5).f(0.0).u32(0).u32(0).u32(0);
        w.u32(1).s("grp").u32(0).u32(0).u32(4);
        event(&mut w, "loop", 1.0, 100, 100, 0, 0);
        event(&mut w, "once", 0.5, 0, 0, 0, 1);
        event(&mut w, "vo", 1.0, 0, 0, 1, 1);
        event_with(&mut w, "duck", 0.0, (0, 0), 0, 0, "#duck:sfx,100,0,200,-20");
        // Templates: sequential, programmer-selected.
        w.u32(2);
        for mode in [0u32, 5] {
            w.u32(mode)
                .u32(0)
                .u32(0)
                .u32(1)
                .f(1.0)
                .u32(1)
                .f(1.0)
                .f(1.0)
                .f(1.0);
            w.f(0.0).u32(1).f(0.0).f(0.0).f(0.0).f(0.0);
        }
        w.u32(2);
        w.s("/tone")
            .u32(0)
            .u32(1)
            .u32(0)
            .u32(100)
            .s("tone.wav")
            .s("test")
            .u32(0)
            .u32(100);
        w.s("/line").u32(1).u32(0);
        w.u32(0).u32(8);
        w.0.extend_from_slice(b"comp");
        std::fs::write(dir.join("test.fev"), &w.0).unwrap();

        // A bank with two 16-bit mono samples: "tone" (0.25) and "123_m" (-0.5), 4800 frames.
        let pcm = |v: i16| -> Vec<u8> { (0..4800).flat_map(|_| v.to_le_bytes()).collect() };
        let datas = [pcm(8192), pcm(-16384)];
        let mut headers = Vec::new();
        for (name, d) in ["tone.wav", "123_m.wav"].iter().zip(&datas) {
            let mut h = vec![0u8; 80];
            h[..2].copy_from_slice(&80u16.to_le_bytes());
            h[2..2 + name.len()].copy_from_slice(name.as_bytes());
            h[32..36].copy_from_slice(&4800u32.to_le_bytes());
            h[36..40].copy_from_slice(&(d.len() as u32).to_le_bytes());
            h[48..52].copy_from_slice(&0x30u32.to_le_bytes()); // 16-bit mono
            h[52..56].copy_from_slice(&48000u32.to_le_bytes());
            h[62..64].copy_from_slice(&1u16.to_le_bytes());
            headers.extend(h);
        }
        let mut f = b"FSB4".to_vec();
        f.extend_from_slice(&2u32.to_le_bytes());
        f.extend_from_slice(&(headers.len() as u32).to_le_bytes());
        f.extend_from_slice(&((datas[0].len() * 2) as u32).to_le_bytes());
        f.extend_from_slice(&0x40000u32.to_le_bytes());
        f.extend_from_slice(&0x20u32.to_le_bytes());
        f.extend_from_slice(&[0; 24]);
        f.extend(headers);
        for d in &datas {
            f.extend_from_slice(d);
        }
        std::fs::write(dir.join("test.fsb"), f).unwrap();
    }

    fn engine(tag: &str) -> (AudioEngine, Arc<Mutex<Mixer>>) {
        let dir = std::env::temp_dir().join(format!("thedas-audio-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        project(&dir);
        let mixer = Arc::new(Mutex::new(Mixer::new(48000)));
        let mut e = AudioEngine::new(Arc::new(Library::new(vec![dir])), mixer.clone());
        e.synchronous = true;
        (e, mixer)
    }

    /// Run `seconds`, returning the left channel's last sample.
    fn run(e: &mut AudioEngine, m: &Arc<Mutex<Mixer>>, seconds: f32) -> f32 {
        let mut buf = vec![0.0; 480 * 2];
        let mut last = 0.0;
        for _ in 0..(seconds * 100.0) as usize {
            e.update(0.01);
            m.lock().unwrap().render(&mut buf);
            last = buf[buf.len() - 2];
        }
        last
    }

    #[test]
    fn looping_event_fades_in_holds_and_fades_out() {
        let (mut e, m) = engine("loop");
        let h = e.play("test/grp/loop", PlayOptions::default()).unwrap();
        // Fading in after 50 ms: below the full level (0.25 x category 0.5).
        let early = run(&mut e, &m, 0.05);
        assert!(early > 0.0 && early < 0.125, "{early}");
        // Loops well past the sample's 0.1 s.
        let held = run(&mut e, &m, 1.0);
        assert!((held - 0.125).abs() < 1e-3, "{held}");
        assert!(e.is_playing(h));
        e.stop(h, false);
        run(&mut e, &m, 0.2);
        assert!(!e.is_playing(h));
        assert_eq!(m.lock().unwrap().active(), 0);
    }

    #[test]
    fn max_playbacks_steal_the_oldest() {
        let (mut e, m) = engine("maxpb");
        let a = e.play("test/grp/loop", PlayOptions::default()).unwrap();
        let b = e.play("test/grp/loop", PlayOptions::default()).unwrap();
        run(&mut e, &m, 0.3);
        assert!(!e.is_playing(a) && e.is_playing(b));
    }

    #[test]
    fn duck_parameters_turn_their_category_down_while_playing() {
        assert_eq!(
            Duck::parse("#duck:music,3000,0,5000,-60"),
            Some(Duck {
                category: "music".into(),
                fade_in_ms: 3000.0,
                fade_out_ms: 5000.0,
                db: -60.0
            })
        );
        assert!(Duck::parse("param00").is_none());
        let (mut e, m) = engine("duck");
        e.play("test/grp/loop", PlayOptions::default()).unwrap();
        let full = run(&mut e, &m, 0.5);
        let d = e.play("test/grp/duck", PlayOptions::default()).unwrap();
        let ducked = run(&mut e, &m, 0.5);
        assert!((ducked - full * 0.1).abs() < 1e-3, "{full} -> {ducked}");
        // An event that only ducks lasts until it is stopped.
        run(&mut e, &m, 2.0);
        assert!(e.is_playing(d));
        e.stop(d, true);
        let back = run(&mut e, &m, 0.5);
        assert!((back - full).abs() < 1e-3, "{back}");
    }

    #[test]
    fn oneshot_ends_and_programmer_sound_plays_the_named_sample() {
        let (mut e, m) = engine("once");
        let h = e.play("test/grp/once", PlayOptions::default()).unwrap();
        let v = run(&mut e, &m, 0.05);
        assert!((v - 0.0625).abs() < 1e-3, "{v}");
        run(&mut e, &m, 0.2);
        assert!(!e.is_playing(h));

        let vo = e
            .play(
                "test/grp/vo",
                PlayOptions {
                    programmer_sound: Some("123_m".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let v = run(&mut e, &m, 0.05);
        assert!((v + 0.25).abs() < 1e-3, "{v}");
        assert!(e.is_playing(vo));
        // Without the programmer's sound nothing plays.
        let none = e.play("test/grp/vo", PlayOptions::default()).unwrap();
        run(&mut e, &m, 0.01);
        assert!(!e.is_playing(none));
        assert!(e.play("test/grp/missing", PlayOptions::default()).is_none());
        assert!(e.play("nothere/grp/x", PlayOptions::default()).is_none());
    }

    #[test]
    fn events_vary_pitch_and_volume_as_fmod_event_does() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        // Raw pitch 0.25 is one octave up.
        assert!(close(event_variation(0.25, 0.0, 0.0, 0, (0.5, 0.5)).0, 2.0));
        // Pitch randomization 0.025 (0.1 octave) spans -0.1..+0.1 octave.
        let lo = event_variation(0.0, 0.025, 0.0, 0, (0.0, 0.0)).0;
        let hi = event_variation(0.0, 0.025, 0.0, 0, (1.0, 0.0)).0;
        assert!(close(lo, 2f32.powf(-0.1)) && close(hi, 2f32.powf(0.1)));
        // Semitone rounding (flag bit 30): the offset lands on 1/48 steps.
        let semi = event_variation(0.0, 0.03, 0.0, 0x4000_0000, (0.3, 0.0)).0;
        let steps = semi.log2() / 4.0 * 48.0 + 0.03 * 48.0;
        assert!((steps - steps.round()).abs() < 1e-3);
        // Volume randomization 0.5: gain from 0.5 to 1.
        assert!(close(event_variation(0.0, 0.0, 0.5, 0, (0.0, 0.0)).1, 0.5));
        assert!(close(event_variation(0.0, 0.0, 0.5, 0, (0.0, 1.0)).1, 1.0));
        assert!(close(event_variation(0.0, 0.0, 0.0, 0, (0.0, 0.3)).1, 1.0));
    }

    #[test]
    fn sound_definitions_vary_pitch_and_volume_per_play() {
        let t = SoundDefTemplate {
            play_mode: 0,
            spawn_time_min_ms: 0,
            spawn_time_max_ms: 0,
            max_spawned: 0,
            volume: 1.0,
            volume_randomization_method: 1,
            f14: 1.0,
            f18: 1.0,
            f1c: 0.5,
            pitch: -0.125,
            pitch_randomization_method: 1,
            f24: 0.0,
            f28: 0.0,
            f2c: 0.0625,
            position_randomization: 0.0,
        };
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        // Raw pitch -0.125 is half an octave down; randomization 0.0625 is +-0.25 octave.
        let (mid, _) = template_variation(&t, (0.5, 0.0));
        assert!(close(mid, 2f32.powf(-0.5)));
        let (lo, quiet) = template_variation(&t, (0.0, 0.0));
        let (hi, loud) = template_variation(&t, (1.0, 1.0));
        assert!(close(lo, 2f32.powf(-0.75)) && close(hi, 2f32.powf(-0.25)));
        // Minimum gain 0.5: from 0.5 to 1.
        assert!(close(quiet, 0.5) && close(loud, 1.0));
        // No randomization: fixed.
        let still = SoundDefTemplate {
            f1c: 1.0,
            f2c: 0.0,
            pitch: 0.0,
            ..t
        };
        assert_eq!(template_variation(&still, (0.9, 0.1)), (1.0, 1.0));
    }
}
