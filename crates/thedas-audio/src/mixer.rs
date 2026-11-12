//! The software mixer: playing voices, each a decoded [`Pcm`] with gain, pan, pitch, an
//! optional loop and a bus (FMOD event category), mixed to interleaved stereo `f32`.
//!
//! Resampling is linear interpolation, which is FMOD Ex's default software resampler.
//! Provisional: the pan law (linear balance of a mono source, stereo sources balanced per
//! side) is not checked against `fmodex.dll`.

use std::sync::Arc;

use crate::decode::Pcm;

/// The volume groups the options screen controls. DA:O's `audio_categories.fev` nests finer
/// categories under these; until it is read, a voice names the group directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bus {
    Music,
    Sfx,
    Dialog,
    Ambient,
    Ui,
}

impl Bus {
    pub const ALL: [Bus; 5] = [Bus::Music, Bus::Sfx, Bus::Dialog, Bus::Ambient, Bus::Ui];

    fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VoiceId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceParams {
    /// Linear gain.
    pub gain: f32,
    /// -1 (left) ..= 1 (right).
    pub pan: f32,
    /// Playback-rate multiplier on top of the sample's own rate.
    pub pitch: f32,
    /// Loop between these frames (end exclusive) instead of stopping at the end.
    pub looping: Option<(usize, usize)>,
    pub bus: Bus,
}

impl Default for VoiceParams {
    fn default() -> Self {
        VoiceParams {
            gain: 1.0,
            pan: 0.0,
            pitch: 1.0,
            looping: None,
            bus: Bus::Sfx,
        }
    }
}

struct Fade {
    target: f32,
    /// Gain change per output frame.
    step: f32,
    stop_at_end: bool,
}

struct Voice {
    id: VoiceId,
    pcm: Arc<Pcm>,
    params: VoiceParams,
    /// Position in source frames.
    pos: f64,
    fade: Option<Fade>,
    paused: bool,
}

pub struct Mixer {
    rate: u32,
    voices: Vec<Voice>,
    next_id: u64,
    bus_gain: [f32; 5],
    pub master_gain: f32,
}

impl Mixer {
    /// A mixer producing stereo at `rate` Hz.
    pub fn new(rate: u32) -> Self {
        Mixer {
            rate: rate.max(1),
            voices: Vec::new(),
            next_id: 1,
            bus_gain: [1.0; 5],
            master_gain: 1.0,
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn play(&mut self, pcm: Arc<Pcm>, params: VoiceParams) -> VoiceId {
        let id = VoiceId(self.next_id);
        self.next_id += 1;
        self.voices.push(Voice {
            id,
            pcm,
            params,
            pos: 0.0,
            fade: None,
            paused: false,
        });
        id
    }

    fn voice(&mut self, id: VoiceId) -> Option<&mut Voice> {
        self.voices.iter_mut().find(|v| v.id == id)
    }

    pub fn is_playing(&self, id: VoiceId) -> bool {
        self.voices.iter().any(|v| v.id == id)
    }

    pub fn active(&self) -> usize {
        self.voices.len()
    }

    pub fn stop(&mut self, id: VoiceId) {
        self.voices.retain(|v| v.id != id);
    }

    pub fn stop_bus(&mut self, bus: Bus) {
        self.voices.retain(|v| v.params.bus != bus);
    }

    pub fn set_paused(&mut self, id: VoiceId, paused: bool) {
        if let Some(v) = self.voice(id) {
            v.paused = paused;
        }
    }

    pub fn set_gain(&mut self, id: VoiceId, gain: f32) {
        if let Some(v) = self.voice(id) {
            v.params.gain = gain;
            v.fade = None;
        }
    }

    pub fn set_pan(&mut self, id: VoiceId, pan: f32) {
        if let Some(v) = self.voice(id) {
            v.params.pan = pan.clamp(-1.0, 1.0);
        }
    }

    pub fn set_pitch(&mut self, id: VoiceId, pitch: f32) {
        if let Some(v) = self.voice(id) {
            v.params.pitch = pitch.max(0.0);
        }
    }

    /// Ramp the voice's gain to `target` over `seconds`, then stop it if `stop_at_end`.
    pub fn fade(&mut self, id: VoiceId, target: f32, seconds: f32, stop_at_end: bool) {
        let frames = (seconds * self.rate as f32).max(1.0);
        if let Some(v) = self.voice(id) {
            v.fade = Some(Fade {
                target,
                step: (target - v.params.gain) / frames,
                stop_at_end,
            });
        }
    }

    pub fn bus_gain(&self, bus: Bus) -> f32 {
        self.bus_gain[bus.index()]
    }

    pub fn set_bus_gain(&mut self, bus: Bus, gain: f32) {
        self.bus_gain[bus.index()] = gain.max(0.0);
    }

    /// Mix the next `out.len() / 2` stereo frames into `out` (overwriting it).
    pub fn render(&mut self, out: &mut [f32]) {
        out.iter_mut().for_each(|s| *s = 0.0);
        let rate = self.rate as f64;
        let master = self.master_gain;
        let buses = self.bus_gain;
        self.voices.retain_mut(|v| {
            if v.paused {
                return true;
            }
            let pcm = &*v.pcm;
            let frames = pcm.frames();
            let ch = pcm.channels as usize;
            if frames == 0 || ch == 0 {
                return false;
            }
            let step = pcm.rate as f64 / rate * v.params.pitch as f64;
            let bus = buses[v.params.bus.index()] * master;
            let (lp, rp) = pan_gains(v.params.pan);
            for o in out.chunks_exact_mut(2) {
                if let Some((start, end)) = v.params.looping {
                    let end = end.min(frames);
                    if end > start && v.pos >= end as f64 {
                        v.pos = start as f64 + (v.pos - end as f64) % (end - start) as f64;
                    }
                }
                let i = v.pos as usize;
                if i >= frames {
                    return false;
                }
                let t = (v.pos - i as f64) as f32;
                let next = match v.params.looping {
                    Some((start, end)) if i + 1 >= end.min(frames) => start,
                    _ => (i + 1).min(frames - 1),
                };
                let at = |f: usize, c: usize| pcm.samples[f * ch + c.min(ch - 1)];
                let l = at(i, 0) + (at(next, 0) - at(i, 0)) * t;
                let r = at(i, 1) + (at(next, 1) - at(i, 1)) * t;
                let g = v.params.gain * bus;
                o[0] += l * g * lp;
                o[1] += r * g * rp;
                v.pos += step;
                if let Some(f) = &v.fade {
                    v.params.gain += f.step;
                    let done = if f.step >= 0.0 {
                        v.params.gain >= f.target
                    } else {
                        v.params.gain <= f.target
                    };
                    if done {
                        v.params.gain = f.target;
                        let stop = f.stop_at_end;
                        v.fade = None;
                        if stop {
                            return false;
                        }
                    }
                }
            }
            true
        });
    }
}

/// Left and right gains for a pan position: the far side falls off linearly, the near side
/// stays at full gain.
fn pan_gains(pan: f32) -> (f32, f32) {
    let p = pan.clamp(-1.0, 1.0);
    ((1.0 - p).min(1.0), (1.0 + p).min(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(rate: u32, ch: u16, samples: Vec<f32>) -> Arc<Pcm> {
        Arc::new(Pcm {
            rate,
            channels: ch,
            samples,
        })
    }

    #[test]
    fn mono_voice_plays_to_both_sides_then_ends() {
        let mut m = Mixer::new(100);
        let id = m.play(pcm(100, 1, vec![0.5; 10]), VoiceParams::default());
        let mut out = vec![0.0; 2 * 8];
        m.render(&mut out);
        assert!(out.iter().all(|&s| s == 0.5));
        assert!(m.is_playing(id));
        m.render(&mut out);
        assert_eq!(&out[..4], &[0.5; 4]);
        assert_eq!(&out[4..], &[0.0; 12]);
        assert!(!m.is_playing(id));
    }

    #[test]
    fn resamples_pans_and_applies_bus_gain() {
        let mut m = Mixer::new(200);
        m.set_bus_gain(Bus::Music, 0.5);
        let ramp: Vec<f32> = (0..10).map(|i| i as f32 / 10.0).collect();
        m.play(
            pcm(100, 1, ramp),
            VoiceParams {
                pan: 1.0,
                bus: Bus::Music,
                ..Default::default()
            },
        );
        let mut out = vec![0.0; 2 * 4];
        m.render(&mut out);
        // Half-rate source: every other output frame interpolates.
        let right: Vec<f32> = out.chunks(2).map(|f| f[1]).collect();
        let left: Vec<f32> = out.chunks(2).map(|f| f[0]).collect();
        assert!(left.iter().all(|&l| l == 0.0));
        for (got, want) in right.iter().zip([0.0, 0.025, 0.05, 0.075]) {
            assert!((got - want).abs() < 1e-6, "{got} vs {want}");
        }
    }

    #[test]
    fn loops_and_fades_out() {
        let mut m = Mixer::new(10);
        let id = m.play(
            pcm(10, 2, vec![1.0, -1.0, 0.5, -0.5]),
            VoiceParams {
                looping: Some((0, 2)),
                ..Default::default()
            },
        );
        let mut out = vec![0.0; 2 * 6];
        m.render(&mut out);
        assert_eq!(
            out,
            vec![
                1.0, -1.0, 0.5, -0.5, 1.0, -1.0, 0.5, -0.5, 1.0, -1.0, 0.5, -0.5
            ]
        );
        m.fade(id, 0.0, 0.5, true);
        m.render(&mut out);
        assert!(!m.is_playing(id));
        assert!(out[8].abs() < out[0].abs());
    }
}
