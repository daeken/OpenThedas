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
//! The properties take their documented I3DL2 meanings (FMOD Ex's software reverb is an
//! I3DL2 reverb; DAOrigins.exe 0x5bfc70 lists the ones it takes: room, room HF and LF,
//! decay time and HF ratio, reflections and reverb levels and delays, diffusion, density,
//! and the HF and LF references):
//! - `room` scales everything the reverb adds; `room_hf` and `room_lf` attenuate it at
//!   `hf_reference` (a low-pass) and `lf_reference` (a low shelf).
//! - `reflections` is the early reflections' level relative to `room`, `reverb` the late
//!   reverberation's: each is an energy, so a unit impulse sent at `room` 0 and level 0 mB
//!   comes back with unit energy in each (per output channel).
//! - `decay_time` is the late reverberation's 60 dB decay at middle frequencies,
//!   `decay_hf_ratio` scales it at `hf_reference`; `reflections_delay` is the first
//!   reflection's delay and `reverb_delay` the late reverberation's after it.
//!
//! Provisional: the algorithm. This is our own reverb (filtered input, four early taps, two
//! diffusers, an 8-line feedback delay network), not FMOD's, so its colour (and how
//! `diffusion` and `density` shape it) only approximates the original's. `room_rolloff_factor`
//! (0 in every DA:O preset), the decay LF ratio and the modulation, echo and environment
//! properties, which FMOD's software reverb does not take, are unused.

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
/// The taps' combined energy (their gains' squares), to normalise them by.
const TAPS_ENERGY: f32 = 1.0 + 0.64 + 0.49 + 0.36;

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
    /// The room LF shelf: a low-pass state, its coefficient and the shelf's gain.
    lf_lp: f32,
    lf_b: f32,
    lf_gain: f32,
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
            lf_lp: 0.0,
            lf_b: 0.0,
            lf_gain: 1.0,
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
        self.lf_b = (-w(p.lf_reference)).exp();
        self.lf_gain = mb(p.room_lf as f32);
        self.early_gain = room * mb(p.reflections as f32) / TAPS_ENERGY.sqrt();
        self.diffusion = (p.diffusion / 100.0).clamp(0.0, 1.0) * 0.6;
        self.early_at = (p.reflections_delay.clamp(0.0, 0.3) * self.rate) as usize + 1;
        self.late_at = ((p.reflections_delay.clamp(0.0, 0.3) + p.reverb_delay.clamp(0.0, 0.1))
            * self.rate) as usize
            + 1;
        // Denser rooms use the full line lengths; sparse ones shorter.
        let density = 0.5 + 0.5 * (p.density / 100.0).clamp(0.0, 1.0);
        let decay = p.decay_time.clamp(0.1, 20.0);
        let decay_hf = (decay * p.decay_hf_ratio.clamp(0.1, 2.0)).max(0.05);
        let mut kept = 0.0;
        for i in 0..8 {
            let len = ((self.line_len[i] as f32 * density) as usize).max(1);
            let secs = len as f32 / self.rate;
            let g = 10f32.powf(-3.0 * secs / decay);
            let g_hf = 10f32.powf(-3.0 * secs / decay_hf);
            self.feedback[i] = g;
            self.damp_b[i] = lowpass((g_hf / g).min(1.0), w(p.hf_reference));
            kept += g * g / 8.0;
        }
        // The tail's energy at the reverb level whatever the decay: an impulse puts unit
        // energy into each of the 8 lines; each pass through the (energy-preserving) mix
        // keeps `g^2` of it, so the lines carry `8 / (1 - g^2)` in all, half of it to each
        // output channel (4 lines each). Paths of equal length through the network add
        // coherently, which measures about 1.7 dB over that (0.3..8 s decays), so take it
        // off.
        self.late_gain = room * mb(p.reverb as f32) * ((1.0 - kept) / 4.0).sqrt() * 0.82;
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
            self.lf_lp += (1.0 - self.lf_b) * (self.input_lp - self.lf_lp);
            self.pre
                .push(self.input_lp + (self.lf_gain - 1.0) * self.lf_lp);
            // Early reflections: four taps between the reflections and reverb delays, to both
            // sides (alternate ones inverted on the right, to decorrelate them).
            let (mut el, mut er) = (0.0, 0.0);
            for (k, (at, g)) in TAPS.iter().enumerate() {
                let n = self.early_at + (at * span) as usize;
                let v = self.pre.tap(n) * g * self.early_gain;
                el += v;
                er += if k % 2 == 0 { v } else { -v };
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

    fn flat(mut p: ReverbProperties) -> ReverbProperties {
        p.room = 0;
        p.room_hf = 0;
        p.room_lf = 0;
        p.decay_time = 1.0;
        p.decay_hf_ratio = 1.0;
        p.diffusion = 100.0;
        p.density = 100.0;
        p
    }

    #[test]
    fn levels_are_energies_relative_to_room() {
        let db = |e: f32| 10.0 * e.log10();
        // Early only: exactly unit energy per channel.
        let mut p = flat(ReverbProperties::GENERIC);
        p.reflections = 0;
        p.reverb = -10000;
        let mut r = Reverb::new(48000);
        r.set(p, 0.0);
        let out = impulse(&mut r, 1.0);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        // (The impulse is in both input channels at 1, so the send is 1.)
        assert!(db(energy(&left)).abs() < 0.1, "{} dB", db(energy(&left)));
        // Late only: unit energy per channel, within the network's spread.
        p.reflections = -10000;
        p.reverb = 0;
        let mut r = Reverb::new(48000);
        r.set(p, 0.0);
        let out = impulse(&mut r, 4.0);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert!(db(energy(&left)).abs() < 1.5, "{} dB", db(energy(&left)));
        // Room scales both: -600 mB is -6 dB.
        p.room = -600;
        let mut r = Reverb::new(48000);
        r.set(p, 0.0);
        let out = impulse(&mut r, 4.0);
        let quieter: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert!((db(energy(&left)) - db(energy(&quieter)) - 6.0).abs() < 0.1);
    }

    #[test]
    fn room_lf_attenuates_low_frequencies() {
        let mut p = flat(ReverbProperties::GENERIC);
        p.reverb = -10000;
        p.reflections = 0;
        let tone = |r: &mut Reverb, hz: f32| {
            let n = 48000;
            let send: Vec<f32> = (0..2 * n)
                .map(|i| (std::f32::consts::TAU * hz * (i / 2) as f32 / 48000.0).sin())
                .collect();
            let mut out = vec![0.0; 2 * n];
            r.process(&send, &mut out);
            energy(&out[n..])
        };
        let mut flat_r = Reverb::new(48000);
        flat_r.set(p, 0.0);
        p.room_lf = -1200;
        let mut cut = Reverb::new(48000);
        cut.set(p, 0.0);
        let low = 10.0 * (tone(&mut flat_r, 40.0) / tone(&mut cut, 40.0)).log10();
        let high = 10.0 * (tone(&mut flat_r, 4000.0) / tone(&mut cut, 4000.0)).log10();
        assert!(low > 8.0, "{low} dB at 40 Hz");
        assert!(high < 1.0, "{high} dB at 4 kHz");
    }

    #[test]
    fn late_level_holds_across_decay_times() {
        for decay in [0.3f32, 1.0, 4.0] {
            let mut p = flat(ReverbProperties::GENERIC);
            p.reflections = -10000;
            p.reverb = 0;
            p.decay_time = decay;
            let mut r = Reverb::new(48000);
            r.set(p, 0.0);
            let out = impulse(&mut r, decay * 2.5 + 0.5);
            for side in 0..2 {
                let e: f32 = out.iter().skip(side).step_by(2).map(|v| v * v).sum();
                let db = 10.0 * e.log10();
                assert!(db.abs() < 1.5, "decay {decay} side {side}: {db} dB");
            }
        }
    }
}
