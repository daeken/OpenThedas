//! Flickering lights: DAOrigins.exe's intensity controller for point lights.
//!
//! A point light whose blueprint gives `IntensityVariation` (GFF 6249) above zero gets a
//! controller when it is made (0x55fc08-0x55fc7b), built by 0x8155f0 with the variation,
//! `IntensityPeriod` (6250) and `IntensityPeriodDelta` (6251), and kept in its owner's
//! controller list. The controller drives the light's flicker factor (light +0x1bc), which
//! the light pass multiplies into the colour each selected light gives the shaders
//! (0x46b47e-0x46b4db, 0x46b603-0x46b660) and which does not enter the light's influence, so
//! it never changes which lights a draw gets. About one point light in five in the shipped
//! data has one: torches, braziers and fires, at 0.075 ± 0.025 seconds a move.
//!
//! The controller (0x8155f0 builds, 0x8156e0 steps) holds a base (the factor when it is
//! made: 1), two end values and a move's duration and elapsed time. Each draw from the C
//! runtime's random generator gives `u` in [-1, 1] (`rand() · 2/32767 − 1`):
//!
//! - Made: duration `period + delta · u₁`, from `base + base · variation · u₂`, to
//!   `base + base · variation · u₃`; elapsed 0.
//! - Stepped by `dt`: elapsed grows by `dt`. While it is at most the duration the factor is
//!   `from + (to − from) · s` with `s` the smoothstep `t²(3 − 2t)` of `t = elapsed /
//!   duration` clamped to [0, 1]. Past it, elapsed goes back to 0 (the overshoot is
//!   dropped), a new duration is drawn, `from` takes the old `to`, a new `to` is drawn, and
//!   the factor is the new `from`.
//!
//! Provisional: the exe draws from the process-wide generator every system shares, whose
//! state cannot be matched; each light here has its own generator of the same kind (the
//! Microsoft C runtime's), seeded from where the light stands so that every pass sees the
//! same flicker for the same light. Which clock steps the controllers (and whether a paused
//! game stops them) is not traced: the shaders' `ApplicationTime` steps them here.

use glam::Vec3;
pub use thedas_formats::model::Flicker;

use crate::scene::SceneLight;

/// One light's flicker controller (see the module docs).
#[derive(Clone, Debug, PartialEq)]
pub struct Controller {
    base: f32,
    variation: f32,
    period: f32,
    delta: f32,
    from: f32,
    to: f32,
    duration: f64,
    elapsed: f64,
    /// The light's factor now.
    value: f32,
    seed: u32,
    /// What it was made from: the flicker and the generator's first seed.
    made: (Flicker, u32),
}

impl Controller {
    /// A controller for `flicker`, or `None` when its variation is not above zero (the
    /// engine makes none then). The factor starts at 1 until it is first stepped.
    pub fn new(flicker: &Flicker, seed: u32) -> Option<Self> {
        if flicker.variation.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return None;
        }
        let mut c = Controller {
            base: 1.0,
            variation: flicker.variation,
            period: flicker.period,
            delta: flicker.period_delta,
            from: 1.0,
            to: 1.0,
            duration: 0.0,
            elapsed: 0.0,
            value: 1.0,
            seed,
            made: (*flicker, seed),
        };
        c.duration = c.draw_duration();
        c.from = c.draw_end();
        c.to = c.draw_end();
        Some(c)
    }

    /// The light's flicker factor now.
    pub fn value(&self) -> f32 {
        self.value
    }

    /// Steps the controller `dt` seconds on and returns the factor.
    pub fn step(&mut self, dt: f64) -> f32 {
        self.elapsed += dt;
        if self.elapsed > self.duration {
            self.elapsed = 0.0;
            self.duration = self.draw_duration();
            self.from = self.to;
            self.to = self.draw_end();
            self.value = self.from;
        } else {
            let t = if self.duration > 0.0 {
                ((self.elapsed / self.duration) as f32).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let s = t * t * (3.0 - 2.0 * t);
            self.value = (1.0 - s) * self.from + s * self.to;
        }
        self.value
    }

    /// The Microsoft C runtime's `rand()`, as a value in [-1, 1].
    fn draw(&mut self) -> f32 {
        self.seed = self.seed.wrapping_mul(0x343fd).wrapping_add(0x269ec3);
        let r = (self.seed >> 16) & 0x7fff;
        (f64::from(r) * (2.0 / 32767.0) - 1.0) as f32
    }

    fn draw_duration(&mut self) -> f64 {
        let u = self.draw();
        f64::from(self.period + self.delta * u)
    }

    fn draw_end(&mut self) -> f32 {
        let u = self.draw();
        self.base + self.base * self.variation * u
    }
}

/// A seed for a light's generator from where it stands and how far it reaches, the same for
/// the same light in every pass.
pub fn seed(position: Vec3, radius: f32) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for v in [position.x, position.y, position.z, radius] {
        for b in v.to_bits().to_le_bytes() {
            h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
        }
    }
    h
}

/// How long the first step plays (see [`Flickers::advance`]), in seconds.
const FIRST_RUN: f64 = 1.0;
/// The frame the first step plays in.
const FRAME: f64 = 1.0 / 60.0;

/// The flicker of a list of scene lights: one factor per light, 1 for a light that does not
/// flicker.
#[derive(Clone, Debug, Default)]
pub struct Flickers {
    controllers: Vec<Option<Controller>>,
    values: Vec<f32>,
    /// The clock the controllers were last stepped to.
    last: Option<f32>,
}

impl Flickers {
    pub fn new(lights: &[SceneLight]) -> Self {
        let mut f = Flickers::default();
        f.set_lights(lights);
        f
    }

    /// Follows a changed light list: a light that stays (the same light at the same index)
    /// keeps its controller; the others get new ones.
    pub fn set_lights(&mut self, lights: &[SceneLight]) {
        self.controllers.truncate(lights.len());
        self.values.truncate(lights.len());
        for (i, l) in lights.iter().enumerate() {
            let made = match l {
                SceneLight::Point {
                    position,
                    radius,
                    flicker: Some(f),
                    ..
                } => Controller::new(f, seed(*position, *radius)),
                _ => None,
            };
            if i < self.controllers.len() {
                let same = match (&self.controllers[i], &made) {
                    (Some(a), Some(b)) => a.made == b.made,
                    (None, None) => true,
                    _ => false,
                };
                if !same {
                    self.values[i] = made.as_ref().map_or(1.0, Controller::value);
                    self.controllers[i] = made;
                }
            } else {
                self.values
                    .push(made.as_ref().map_or(1.0, Controller::value));
                self.controllers.push(made);
            }
        }
    }

    /// Steps every controller to the clock `time` (seconds); a clock that stands or runs
    /// back leaves them as they are.
    pub fn advance(&mut self, time: f32) {
        let Some(last) = self.last.replace(time) else {
            // The first step plays from a whole second before the last whole second (or
            // from 0) at 60 frames a second, so that even a single frame (a capture) shows
            // the lights partway through a move, where their flicker has taken them by
            // `time`.
            let time = f64::from(time);
            let mut t = (time.floor() - FIRST_RUN).max(0.0);
            while t < time {
                let dt = FRAME.min(time - t);
                self.step(dt);
                t += dt;
            }
            return;
        };
        let dt = f64::from(time) - f64::from(last);
        if dt > 0.0 {
            self.step(dt);
        }
    }

    fn step(&mut self, dt: f64) {
        for (c, v) in self.controllers.iter_mut().zip(&mut self.values) {
            if let Some(c) = c {
                *v = c.step(dt);
            }
        }
    }

    /// Each light's factor, by its index in the light list.
    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// Whether any light flickers.
    pub fn any(&self) -> bool {
        self.controllers.iter().any(Option::is_some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TORCH: Flicker = Flicker {
        variation: 0.5,
        period: 0.075,
        period_delta: 0.025,
    };

    #[test]
    fn no_variation_makes_no_controller() {
        let none = Flicker {
            variation: 0.0,
            ..TORCH
        };
        assert!(Controller::new(&none, 1).is_none());
        let nan = Flicker {
            variation: f32::NAN,
            ..TORCH
        };
        assert!(Controller::new(&nan, 1).is_none());
    }

    #[test]
    fn a_controller_draws_its_move_and_ends_as_the_exe_does() {
        let c = Controller::new(&TORCH, 0).unwrap();
        // The C runtime's first three draws from seed 0: 38, 7719, 21238.
        let u = |r: f64| (r * (2.0 / 32767.0) - 1.0) as f32;
        assert!((c.duration - f64::from(0.075 + 0.025 * u(38.0))).abs() < 1e-9);
        assert_eq!(c.from, 1.0 + 0.5 * u(7719.0));
        assert_eq!(c.to, 1.0 + 0.5 * u(21238.0));
        assert_eq!(c.value(), 1.0);
    }

    #[test]
    fn a_move_eases_from_end_to_end_then_the_next_starts_where_it_ended() {
        let mut c = Controller::new(&TORCH, 7).unwrap();
        let (from, to, duration) = (c.from, c.to, c.duration);
        assert!((c.step(duration / 2.0) - (from + to) / 2.0).abs() < 1e-5);
        // Smoothstep: a quarter of the way in is 15.625% of the move.
        let mut d = Controller::new(&TORCH, 7).unwrap();
        let v = d.step(duration / 4.0);
        assert!((v - (from + (to - from) * 0.15625)).abs() < 1e-5);
        // Past the end: the old `to` is the new `from`, and the factor sits on it.
        let v = c.step(duration);
        assert_eq!(v, to);
        assert_eq!(c.from, to);
        assert_eq!(c.elapsed, 0.0);
        // Every value stays within 1 ± variation.
        for _ in 0..1000 {
            let v = c.step(0.016);
            assert!((0.5..=1.5).contains(&v), "{v}");
        }
    }

    fn torch(x: f32, flicker: Option<Flicker>) -> SceneLight {
        SceneLight::Point {
            position: Vec3::new(x, 0.0, 0.0),
            color: Vec3::ONE,
            radius: 5.0,
            name: String::new(),
            domain: None,
            is_static: true,
            version: 0,
            intensity: 1.0,
            flicker,
            room: None,
        }
    }

    #[test]
    fn the_same_light_flickers_alike_in_every_list_and_others_hold_at_one() {
        let lights = [torch(0.0, None), torch(1.0, Some(TORCH))];
        let mut a = Flickers::new(&lights);
        let mut b = Flickers::new(&lights[1..]);
        assert!(a.any());
        for k in 0..50 {
            let t = k as f32 * 0.016;
            a.advance(t);
            b.advance(t);
            assert_eq!(a.values()[0], 1.0);
            assert_eq!(a.values()[1], b.values()[0]);
        }
        assert_ne!(a.values()[1], 1.0);
        // A stopped or reversed clock holds them.
        let held = a.values()[1];
        a.advance(0.0);
        assert_eq!(a.values()[1], held);
        // A first frame starts the lights mid-move, by where in its second the clock is.
        let first = |t: f32| {
            let mut f = Flickers::new(&lights);
            f.advance(t);
            f.values()[1]
        };
        assert_eq!(first(10.25), first(10.25));
        assert_ne!(first(10.25), first(10.5));
        // A list that grows keeps its lights' controllers.
        let more = [torch(0.0, None), torch(1.0, Some(TORCH)), torch(2.0, None)];
        a.set_lights(&more);
        assert_eq!(a.values(), &[1.0, held, 1.0]);
    }
}
