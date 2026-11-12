//! The world reverb: FMOD's reverb as DAOrigins.exe drives it.
//!
//! Traced in DAOrigins.exe:
//! - Presets come from `master_reverb.fev` by name (EventSystem `getReverbPreset`,
//!   0x5c26ae); the name `force_generic` is FMOD's generic preset and an empty name is no
//!   reverb (0x5c2440..0x5c266a).
//! - A preset change fades: every property moves linearly from where it was to the new
//!   preset over the fade (0.33 s by default, 0xaff1c8; 0 jumps), integer properties
//!   truncated (0x5c09a0, 0x5bfc00, 0x5bfc40).
//! - Events in the `gui`, `music` and `master` categories send nothing to it; every other
//!   event sends at full level (0x5bf217..0x5bf255, 0x5be6a0), times its own reverb wet
//!   level.
//!
//! Provisional: the reverb's sound. FMOD Ex's software reverb is an I3DL2 reverb taking the
//! properties listed in [`Params`] (DAOrigins.exe 0x5bfc70 maps them onto it); this is our
//! own I3DL2-style reverb (filtered input, early reflections, a feedback delay network whose
//! decay follows the decay time and HF ratio), not FMOD's algorithm, so its colour and
//! level only approximate the original's.

use thedas_formats::fev::ReverbProperties;

/// Millibels to linear gain.
fn mb(v: f32) -> f32 {
    if v <= -10000.0 {
        0.0
    } else {
        10f32.powf(v / 2000.0)
    }
}

/// A preset part way through a fade: each property `from + (to - from) * t`, integer ones
/// truncated, as DAOrigins.exe blends them.
pub fn blend(from: &ReverbProperties, to: &ReverbProperties, t: f32) -> ReverbProperties {
    let f = |a: f32, b: f32| a + (b - a) * t;
    let i = |a: i32, b: i32| f(a as f32, b as f32) as i32;
    ReverbProperties {
        room: i(from.room, to.room),
        room_hf: i(from.room_hf, to.room_hf),
        room_lf: i(from.room_lf, to.room_lf),
        decay_time: f(from.decay_time, to.decay_time),
        decay_hf_ratio: f(from.decay_hf_ratio, to.decay_hf_ratio),
        reflections: i(from.reflections, to.reflections),
        reflections_delay: f(from.reflections_delay, to.reflections_delay),
        reverb: i(from.reverb, to.reverb),
        reverb_delay: f(from.reverb_delay, to.reverb_delay),
        diffusion: f(from.diffusion, to.diffusion),
        density: f(from.density, to.density),
        hf_reference: f(from.hf_reference, to.hf_reference),
        lf_reference: f(from.lf_reference, to.lf_reference),
        room_rolloff_factor: f(from.room_rolloff_factor, to.room_rolloff_factor),
        air_absorption_hf: f(from.air_absorption_hf, to.air_absorption_hf),
        ..*to
    }
}

/// Coefficient `b` of a one-pole low-pass `y += (1 - b) * (x - y)` whose gain at angular
/// frequency `w` is `g` (0 for no filtering).
fn lowpass(g: f32, w: f32) -> f32 {
    let g = g.clamp(0.0001, 1.0);
    if g >= 0.9999 {
        return 0.0;
    }
    let g2 = g * g;
    let k = (1.0 - g2 * w.cos()) / (1.0 - g2);
    (k - (k * k - 1.0).max(0.0).sqrt()).clamp(0.0, 0.9999)
}

struct Delay {
    buf: Vec<f32>,
    at: usize,
}

impl Delay {
    fn new(len: usize) -> Delay {
        Delay {
            buf: vec![0.0; len.max(1)],
            at: 0,
        }
    }

    /// The sample written `n` samples ago (`n >= 1`).
    fn tap(&self, n: usize) -> f32 {
        let len = self.buf.len();
        self.buf[(self.at + len - n.clamp(1, len) % len) % len]
    }

    fn push(&mut self, x: f32) {
        self.buf[self.at] = x;
        self.at = (self.at + 1) % self.buf.len();
    }
}

/// Feedback delay lengths at 48 kHz (mutually prime, 22..36 ms).
const LINES: [usize; 8] = [1061, 1187, 1291, 1373, 1471, 1553, 1657, 1733];
/// Early reflection taps, as fractions after the reflections delay.
const TAPS: [(f32, f32); 4] = [(0.0, 1.0), (0.31, -0.8), (0.57, 0.7), (0.83, -0.6)];

/// The mixer's per-frame reverb.
pub struct Reverb {
    rate: f32,
    from: ReverbProperties,
    to: ReverbProperties,
    /// How far through the fade from `from` to `to` (1 = done).
    t: f32,
    fade: f32,
    now: ReverbProperties,
    input_lp: f32,
    input_b: f32,
    pre: Delay,
    diffusers: [Delay; 2],
    lines: Vec<Delay>,
    line_len: [usize; 8],
    feedback: [f32; 8],
    damp_b: [f32; 8],
    damp: [f32; 8],
    early_gain: f32,
    late_gain: f32,
    diffusion: f32,
    early_at: usize,
    late_at: usize,
}

impl Reverb {
    pub fn new(rate: u32) -> Reverb {
        let rate = rate.max(1) as f32;
        let scale = rate / 48000.0;
        let lines: Vec<Delay> = LINES
            .iter()
            .map(|&l| Delay::new(((l as f32 * scale) as usize).max(2) + 2))
            .collect();
        let mut r = Reverb {
            rate,
            from: ReverbProperties::OFF,
            to: ReverbProperties::OFF,
            t: 1.0,
            fade: 0.0,
            now: ReverbProperties::OFF,
            input_lp: 0.0,
            input_b: 0.0,
            // Reflections delay (to 0.3 s) plus reverb delay (to 0.1 s), with room.
            pre: Delay::new((rate * 0.45) as usize),
            diffusers: [
                Delay::new((rate * 0.0047) as usize),
                Delay::new((rate * 0.0036) as usize),
            ],
            line_len: [0; 8],
            lines,
            feedback: [0.0; 8],
            damp_b: [0.0; 8],
            damp: [0.0; 8],
            early_gain: 0.0,
            late_gain: 0.0,
            diffusion: 0.0,
            early_at: 1,
            late_at: 1,
        };
        for (i, l) in LINES.iter().enumerate() {
            r.line_len[i] = ((*l as f32 * scale) as usize).max(1);
        }
        r.configure();
        r
    }

    /// The properties now (mid-fade, the blend).
    pub fn properties(&self) -> ReverbProperties {
        self.now
    }

    /// Whether the reverb makes no sound now (nothing fading or ringing aside).
    pub fn off(&self) -> bool {
        self.now.room <= -10000 && self.t >= 1.0
    }

    /// Fade to `to` over `seconds` (0: at once).
    pub fn set(&mut self, to: ReverbProperties, seconds: f32) {
        if seconds <= 0.0 {
            self.from = to;
            self.to = to;
            self.t = 1.0;
        } else {
            self.from = self.now;
            self.to = to;
            self.t = 0.0;
            self.fade = seconds;
        }
        self.now = blend(&self.from, &self.to, self.t);
        self.configure();
    }

    fn configure(&mut self) {
        let p = self.now;
        let room = mb(p.room as f32);
        let w = |hz: f32| std::f32::consts::TAU * hz.min(self.rate * 0.45).max(1.0) / self.rate;
        self.input_b = lowpass(mb(p.room_hf as f32), w(p.hf_reference));
        self.early_gain = room * mb(p.reflections as f32) * 0.5;
        self.diffusion = (p.diffusion / 100.0).clamp(0.0, 1.0) * 0.6;
        self.early_at = (p.reflections_delay.clamp(0.0, 0.3) * self.rate) as usize + 1;
        self.late_at = ((p.reflections_delay.clamp(0.0, 0.3) + p.reverb_delay.clamp(0.0, 0.1))
            * self.rate) as usize
            + 1;
        // Denser rooms use the full line lengths; sparse ones shorter.
        let density = 0.5 + 0.5 * (p.density / 100.0).clamp(0.0, 1.0);
        let decay = p.decay_time.clamp(0.1, 20.0);
        let decay_hf = (decay * p.decay_hf_ratio.clamp(0.1, 2.0)).max(0.05);
        let mut energy = 0.0;
        for i in 0..8 {
            let len = ((self.line_len[i] as f32 * density) as usize).max(1);
            let secs = len as f32 / self.rate;
            let g = 10f32.powf(-3.0 * secs / decay);
            let g_hf = 10f32.powf(-3.0 * secs / decay_hf);
            self.feedback[i] = g;
            self.damp_b[i] = lowpass((g_hf / g).min(1.0), w(p.hf_reference));
            energy += 1.0 - g * g;
        }
        // Keep the tail's energy at the reverb level whatever the decay.
        self.late_gain = room * mb(p.reverb as f32) * (energy / 8.0).sqrt() * 0.7;
    }

    fn line_tap(&self, i: usize) -> usize {
        let density = 0.5 + 0.5 * (self.now.density / 100.0).clamp(0.0, 1.0);
        ((self.line_len[i] as f32 * density) as usize).clamp(1, self.lines[i].buf.len() - 1)
    }

    /// Add the reverb of the stereo `send` (interleaved) into `out`.
    pub fn process(&mut self, send: &[f32], out: &mut [f32]) {
        let frames = send.len().min(out.len()) / 2;
        if frames == 0 {
            return;
        }
        if self.t < 1.0 {
            self.t = (self.t + frames as f32 / self.rate / self.fade.max(1e-3)).min(1.0);
            self.now = blend(&self.from, &self.to, self.t);
            self.configure();
        }
        let taps: [usize; 8] = std::array::from_fn(|i| self.line_tap(i));
        let span = (self.late_at - self.early_at).max(1) as f32;
        for f in 0..frames {
            let x = (send[2 * f] + send[2 * f + 1]) * 0.5;
            self.input_lp += (1.0 - self.input_b) * (x - self.input_lp);
            self.pre.push(self.input_lp);
            // Early reflections: a few taps between the reflections and reverb delays.
            let (mut el, mut er) = (0.0, 0.0);
            for (k, (at, g)) in TAPS.iter().enumerate() {
                let n = self.early_at + (at * span) as usize;
                let v = self.pre.tap(n) * g * self.early_gain;
                if k % 2 == 0 {
                    el += v;
                } else {
                    er += v;
                }
            }
            // Late reverb: two all-pass diffusers into an 8-line feedback delay network
            // mixed by a Hadamard matrix.
            let mut v = self.pre.tap(self.late_at);
            for d in &mut self.diffusers {
                let len = d.buf.len();
                let delayed = d.tap(len);
                let w = v + self.diffusion * delayed;
                d.push(w);
                v = delayed - self.diffusion * w;
            }
            let outs: [f32; 8] = std::array::from_fn(|i| self.lines[i].tap(taps[i]));
            let mut h = outs;
            let mut step = 1;
            while step < 8 {
                for i in (0..8).step_by(step * 2) {
                    for j in i..i + step {
                        let (a, b) = (h[j], h[j + step]);
                        h[j] = a + b;
                        h[j + step] = a - b;
                    }
                }
                step *= 2;
            }
            let norm = 1.0 / 8f32.sqrt();
            let (mut ll, mut lr) = (0.0, 0.0);
            for i in 0..8 {
                let fb = h[i] * norm * self.feedback[i];
                self.damp[i] += (1.0 - self.damp_b[i]) * (fb - self.damp[i]);
                self.lines[i].push(v + self.damp[i]);
                if i % 2 == 0 {
                    ll += outs[i];
                } else {
                    lr += outs[i];
                }
            }
            out[2 * f] += el + ll * self.late_gain;
            out[2 * f + 1] += er + lr * self.late_gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn energy(s: &[f32]) -> f32 {
        s.iter().map(|v| v * v).sum()
    }

    fn impulse(r: &mut Reverb, seconds: f32) -> Vec<f32> {
        let n = (48000.0 * seconds) as usize;
        let mut send = vec![0.0; 2 * n];
        send[0] = 1.0;
        send[1] = 1.0;
        let mut out = vec![0.0; 2 * n];
        r.process(&send, &mut out);
        out
    }

    #[test]
    fn off_is_silent_and_a_room_rings_for_its_decay_time() {
        let mut r = Reverb::new(48000);
        assert!(r.off());
        assert_eq!(energy(&impulse(&mut r, 0.5)), 0.0);

        let mut r = Reverb::new(48000);
        r.set(ReverbProperties::GENERIC, 0.0);
        let out = impulse(&mut r, 3.0);
        let window = |from: f32| {
            let a = (from * 48000.0) as usize * 2;
            energy(&out[a..a + 9600])
        };
        // Sound arrives after the reflections delay, then decays about 60 dB over the
        // decay time (1.49 s): the tail half a decay later is about 30 dB down.
        assert_eq!(energy(&out[..2 * 300]), 0.0);
        let (early, later) = (window(0.2), window(0.2 + 1.49 / 2.0));
        let drop = 10.0 * (early / later).log10();
        assert!((20.0..40.0).contains(&drop), "{drop} dB");
    }

    #[test]
    fn presets_fade_linearly_with_integers_truncated() {
        let mut a = ReverbProperties::GENERIC;
        a.room = -1000;
        let mut b = a;
        b.room = -2001;
        b.decay_time = 3.49;
        let m = blend(&a, &b, 0.5);
        assert_eq!(m.room, -1500);
        assert!((m.decay_time - 2.49).abs() < 1e-5);

        let mut r = Reverb::new(48000);
        r.set(a, 0.0);
        r.set(b, 0.5);
        let mut out = vec![0.0; 2 * 12000];
        r.process(&vec![0.0; 2 * 12000], &mut out);
        assert_eq!(r.properties().room, -1500);
        r.process(&vec![0.0; 2 * 12000], &mut out);
        assert_eq!(r.properties(), b);
    }
}
