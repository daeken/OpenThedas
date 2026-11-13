//! The wind animation (generation.md 7.11): a small simulation, separate from tree growth,
//! that gives the wind matrices branches and fronds bend by and the leaf rock and rustle
//! angles, step by step in time.
//!
//! [`Wind::dao`] sets it up the way the game drives it; the game runs five of them 0.2 s
//! apart and placements take them in turn. Provisional until checked against the executable.

use super::frame::{Frame, Vec3, cos, lerp, sin};

/// Wind matrices per simulation (7.1).
pub const MATRICES: usize = 6;
/// Leaf angles per simulation (7.1).
pub const LEAF_ANGLES: usize = 8;

/// One oscillator's settings: its angle (degrees) and speed at no wind and at full wind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oscillator {
    pub low_angle: f32,
    pub high_angle: f32,
    pub low_speed: f32,
    pub high_speed: f32,
}

impl Oscillator {
    const fn new(low_angle: f32, high_angle: f32, low_speed: f32, high_speed: f32) -> Self {
        Oscillator {
            low_angle,
            high_angle,
            low_speed,
            high_speed,
        }
    }
}

/// The simulation's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Wind strength, 0–1.
    pub strength: f32,
    /// Wind direction (unit, or zero).
    pub direction: Vec3,
    /// How fast the followers approach their wanted values, and the limit on how fast that
    /// approach may speed up (0: no limit).
    pub response: f32,
    pub response_limit: f32,
    /// Maximum bend in degrees.
    pub max_bend: f32,
    pub branch_exponent: f32,
    pub leaf_exponent: f32,
    /// Gusts: strength range, duration range (seconds) and frequency (per second).
    pub gust_strength: [f32; 2],
    pub gust_duration: [f32; 2],
    pub gust_frequency: f32,
    pub branch_horizontal: Oscillator,
    pub branch_vertical: Oscillator,
    pub leaf_rocking: Oscillator,
    pub leaf_rustling: Oscillator,
}

impl Settings {
    /// The values DA:O drives the wind with (7.11), for a level's strength and direction.
    /// No level has gusts.
    pub fn dao(strength: f32, direction: Vec3) -> Self {
        Settings {
            strength: strength.clamp(0.0, 1.0),
            direction: unit_or_zero(direction),
            response: 0.4,
            response_limit: 0.01,
            max_bend: 35.0,
            branch_exponent: 2.0,
            leaf_exponent: 2.0,
            gust_strength: [0.0, 0.0],
            gust_duration: [0.0, 0.0],
            gust_frequency: 0.0,
            // (low angle, high angle, low speed, high speed), as corrected in 7.11: the game's
            // configuration text runs low angle, low speed, high angle, high speed.
            branch_horizontal: Oscillator::new(0.3, 6.0, 3.0, 3.0),
            branch_vertical: Oscillator::new(0.1, 8.0, 4.0, 4.0),
            leaf_rocking: Oscillator::new(1.0, 2.0, 2.0, 10.0),
            leaf_rustling: Oscillator::new(3.0, 2.0, 1.0, 20.0),
        }
    }
}

/// `v` normalised, or left alone when it is zero (or not finite).
fn unit_or_zero(v: Vec3) -> Vec3 {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length > 0.0 && length.is_finite() {
        [v[0] / length, v[1] / length, v[2] / length]
    } else {
        [0.0; 3]
    }
}

/// A value that follows a wanted value with a limited response.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Follower {
    current: f32,
    wanted: f32,
    last_step: f32,
    last_dt: f32,
}

impl Follower {
    fn advance(&mut self, dt: f32, response: f32, limit: f32) {
        let h = dt.min(0.03);
        let mut step = response * h * (self.wanted - self.current);
        if limit != 0.0 && step.abs() > self.last_step.abs() && h > 0.0 && self.last_dt > 0.0 {
            let acceleration = (step / h - self.last_step / self.last_dt).clamp(-limit, limit);
            step = self.last_step + acceleration * h;
        }
        self.last_step = step;
        self.last_dt = h;
        self.current += step;
    }
}

/// The C runtime generator gusts draw from (Microsoft's, as in 2.6).
#[derive(Clone, Copy, Debug)]
struct CRandom(u32);

impl CRandom {
    /// `rand() / RAND_MAX`.
    fn unit(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(214_013).wrapping_add(2_531_011);
        ((self.0 >> 16) & 0x7FFF) as f32 / 32767.0
    }
}

/// One wind simulation.
#[derive(Clone, Debug)]
pub struct Wind {
    pub settings: Settings,
    last_time: f32,
    strength: Follower,
    direction: [Follower; 3],
    gust: f32,
    gust_stop: f32,
    /// The oscillators' clocks: branch horizontal, branch vertical, leaf rocking, rustling.
    clocks: [f32; 4],
    random: CRandom,
    matrices: [[[f32; 4]; 4]; MATRICES],
    leaf_angles: [(f32, f32); LEAF_ANGLES],
}

impl Wind {
    /// A simulation at rest (every state value zero) with these settings.
    pub fn new(settings: Settings) -> Self {
        let mut wind = Wind {
            settings,
            last_time: 0.0,
            strength: Follower::default(),
            direction: [Follower::default(); 3],
            gust: 0.0,
            gust_stop: 0.0,
            clocks: [0.0; 4],
            random: CRandom(1),
            matrices: [IDENTITY; MATRICES],
            leaf_angles: [(0.0, 0.0); LEAF_ANGLES],
        };
        wind.set_direction(settings.direction);
        wind
    }

    /// The game's wind for a level's strength and direction (from its `.wnd` file).
    pub fn dao(strength: f32, direction: Vec3) -> Self {
        Wind::new(Settings::dao(strength, direction))
    }

    /// Sets the strength, clamped to 0–1; the simulation eases towards it.
    pub fn set_strength(&mut self, strength: f32) {
        self.settings.strength = strength.clamp(0.0, 1.0);
    }

    /// Sets the direction (normalised unless zero); the simulation eases towards it.
    pub fn set_direction(&mut self, direction: Vec3) {
        let d = unit_or_zero(direction);
        self.settings.direction = d;
        for (follower, wanted) in self.direction.iter_mut().zip(d) {
            follower.wanted = wanted;
        }
    }

    /// Steps the simulation to the absolute time `time_seconds` (7.11). Times are expected
    /// not to go backwards.
    pub fn advance(&mut self, time_seconds: f32) {
        let s = self.settings;
        let t = time_seconds;
        // Step 1.
        let dt = t - self.last_time;
        self.last_time = t;
        // Step 2: gusts.
        if t > self.gust_stop {
            self.gust = 0.0;
            let r = self.random.unit();
            if s.gust_frequency * dt > r {
                let [lo, hi] = s.gust_strength;
                self.gust = lo + self.random.unit() * (hi - lo);
                let [lo, hi] = s.gust_duration;
                self.gust_stop = t + lo + self.random.unit() * (hi - lo);
            }
        }
        // Step 3: followers.
        self.strength.wanted = (s.strength + self.gust).clamp(0.0, 1.0);
        self.strength.advance(dt, s.response, s.response_limit);
        self.strength.current = self.strength.current.clamp(0.0, 1.0);
        for follower in &mut self.direction {
            follower.advance(dt, s.response, s.response_limit);
        }
        // Steps 4–5.
        let strength = self.strength.current;
        let branch = strength.powf(s.branch_exponent);
        let leaf = strength.powf(s.leaf_exponent);
        let d = self.direction.map(|f| f.current);
        let bend = branch * s.max_bend * (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        // Step 6: oscillators.
        let oscillators = [
            (s.branch_horizontal, branch),
            (s.branch_vertical, branch),
            (s.leaf_rocking, leaf),
            (s.leaf_rustling, leaf),
        ];
        let mut outputs = [[0.0f32; LEAF_ANGLES]; 4];
        for (k, (oscillator, strength)) in oscillators.into_iter().enumerate() {
            let angle = lerp(oscillator.low_angle, oscillator.high_angle, strength);
            let speed = lerp(oscillator.low_speed, oscillator.high_speed, strength);
            self.clocks[k] += dt * speed;
            for (i, out) in outputs[k].iter_mut().enumerate() {
                *out = angle * sin(self.clocks[k] + i as f32);
            }
        }
        // Step 7: wind matrices.
        let w = -d[0].atan2(d[1]);
        let (x, y) = (-cos(w), -sin(w));
        for (i, matrix) in self.matrices.iter_mut().enumerate() {
            let vertical = Frame::about_axis([x, y, 0.0], bend + outputs[1][i]);
            let horizontal = Frame::about_axis([-y, x, 0.0], outputs[0][i]);
            let m = vertical.then(&horizontal).0;
            *matrix = IDENTITY;
            for (row, source) in matrix.iter_mut().zip(m) {
                row[..3].copy_from_slice(&source);
            }
        }
        // Step 8: leaf angles.
        for (i, angles) in self.leaf_angles.iter_mut().enumerate() {
            *angles = (outputs[2][i], outputs[3][i]);
        }
    }

    /// The six wind matrices of the last step, each as four rows (a point is a row vector
    /// times the matrix; copy the sixteen numbers in this order into `g_amWindMatrices`).
    pub fn matrices(&self) -> [[[f32; 4]; 4]; MATRICES] {
        self.matrices
    }

    /// The eight (rock, rustle) leaf angles of the last step, in degrees (the game passes
    /// them to `g_vLeafAngles` in radians).
    pub fn leaf_angles(&self) -> [(f32, f32); LEAF_ANGLES] {
        self.leaf_angles
    }

    /// The eased wind strength of the last step, 0–1.
    pub fn strength(&self) -> f32 {
        self.strength.current
    }
}

const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Steps `wind` in 1/60 s frames up to `seconds`.
    fn run(wind: &mut Wind, seconds: f32) {
        let frames = (seconds * 60.0) as usize;
        for f in 0..=frames {
            wind.advance(f as f32 / 60.0);
        }
    }

    #[test]
    fn still_air_leaves_the_branches_nearly_still_and_the_leaves_rocking() {
        let mut wind = Wind::dao(0.0, [0.0, 1.0, 0.0]);
        wind.advance(0.0);
        // At time 0 the clocks are 0: output i = low angle · sin(i).
        let leaves = wind.leaf_angles();
        assert_eq!(leaves[0], (0.0, 0.0));
        assert!((leaves[1].0 - 1.0f32.sin()).abs() < 1e-6);
        assert!((leaves[1].1 - 3.0 * 1.0f32.sin()).abs() < 1e-6);
        // Matrix 0 is the identity; the others turn by the small low-wind angles only.
        assert_eq!(wind.matrices()[0], IDENTITY);
        for m in wind.matrices() {
            assert!((m[0][0] - 1.0).abs() < 1e-3 && (m[2][2] - 1.0).abs() < 1e-3);
            assert_eq!(m[3], [0.0, 0.0, 0.0, 1.0]);
        }
    }

    #[test]
    fn the_strength_eases_in_with_the_response() {
        let mut wind = Wind::dao(1.0, [0.0, 1.0, 0.0]);
        wind.advance(0.0);
        assert_eq!(wind.strength(), 0.0);
        // One 0.03 s step: 0.4 · 0.03 · (1 − 0).
        wind.advance(0.03);
        assert!((wind.strength() - 0.012).abs() < 1e-7);
        // A long frame is capped at 0.03 s of approach.
        let mut slow = Wind::dao(1.0, [0.0, 1.0, 0.0]);
        slow.advance(0.0);
        slow.advance(5.0);
        assert!((slow.strength() - 0.012).abs() < 1e-7);
        // It converges on the set strength and never passes 1.
        run(&mut wind, 60.0);
        assert!(wind.strength() > 0.99 && wind.strength() <= 1.0);
    }

    #[test]
    fn full_wind_bends_about_the_axis_across_the_direction() {
        // Wind along +Y: w = 0, so the bend axis is (−1, 0, 0) and matrices keep X in place
        // apart from the small horizontal sway.
        let mut wind = Wind::dao(1.0, [0.0, 2.0, 0.0]);
        run(&mut wind, 60.0);
        for m in wind.matrices() {
            // Rows stay unit length and perpendicular: a rotation.
            for row in &m[..3] {
                let n: f32 = row[..3].iter().map(|x| x * x).sum();
                assert!((n - 1.0).abs() < 1e-4);
            }
            let dot: f32 = (0..3).map(|c| m[0][c] * m[1][c]).sum();
            assert!(dot.abs() < 1e-4);
            // The bend is about 35° ± the 8° vertical sway: cos between 27° and 43°.
            assert!(m[2][2] < 27f32.to_radians().cos() + 0.01);
            assert!(m[2][2] > 43f32.to_radians().cos() - 0.02);
            // The up axis leans along the wind's line (Y), not across it.
            // (the horizontal sway of up to 6° tips it sideways a little).
            assert!(m[2][1].abs() > 0.4 && m[2][0].abs() < 0.12, "{m:?}");
        }
        // Leaf rocking and rustling are within their high-wind angles of 2°.
        for (rock, rustle) in wind.leaf_angles() {
            assert!(rock.abs() <= 2.0 + 1e-3 && rustle.abs() <= 2.0 + 0.05);
        }
    }

    #[test]
    fn direction_changes_ease_in_too() {
        let mut wind = Wind::dao(1.0, [1.0, 0.0, 0.0]);
        run(&mut wind, 60.0);
        let before = wind.matrices()[0];
        wind.set_direction([0.0, 1.0, 0.0]);
        wind.advance(60.0 + 1.0 / 60.0);
        let after = wind.matrices()[0];
        // One frame later the matrix has barely moved.
        for r in 0..3 {
            for c in 0..3 {
                assert!((before[r][c] - after[r][c]).abs() < 0.05);
            }
        }
    }

    #[test]
    fn no_gust_ever_starts_at_frequency_zero() {
        let mut wind = Wind::dao(0.25, [0.0, 1.0, 0.0]);
        run(&mut wind, 30.0);
        assert!(wind.strength() <= 0.25 + 1e-6);
        assert_eq!(wind.gust, 0.0);
    }

    #[test]
    fn settings_clamp_and_normalise() {
        let s = Settings::dao(7.0, [0.0, 0.0, 5.0]);
        assert_eq!(s.strength, 1.0);
        assert_eq!(s.direction, [0.0, 0.0, 1.0]);
        assert_eq!(Settings::dao(-1.0, [0.0; 3]).direction, [0.0; 3]);
        assert_eq!(
            Settings::dao(f32::NAN.max(0.0), [f32::NAN, 0.0, 0.0]).direction,
            [0.0; 3]
        );
    }

    #[test]
    fn dao_oscillators_match_the_spec() {
        let s = Settings::dao(1.0, [0.0, 1.0, 0.0]);
        assert_eq!(s.branch_horizontal, Oscillator::new(0.3, 6.0, 3.0, 3.0));
        assert_eq!(s.branch_vertical, Oscillator::new(0.1, 8.0, 4.0, 4.0));
        assert_eq!(s.leaf_rocking, Oscillator::new(1.0, 2.0, 2.0, 10.0));
        assert_eq!(s.leaf_rustling, Oscillator::new(3.0, 2.0, 1.0, 20.0));
        // At full wind the branches sway by the high angles: horizontal up to 6°, vertical
        // up to 8° either side of the 35° bend, at 3 and 4 radians per second.
        let mut wind = Wind::new(s);
        run(&mut wind, 120.0);
        assert!(wind.strength() > 0.999);
        let (mut lowest, mut highest) = (f32::MAX, f32::MIN);
        for f in 0..600 {
            wind.advance(120.0 + (f + 1) as f32 / 60.0);
            // m[2][2] ≈ cos(bend + vertical) · cos(horizontal).
            let lean = wind.matrices()[0][2][2].acos().to_degrees();
            lowest = lowest.min(lean);
            highest = highest.max(lean);
        }
        assert!((lowest - 27.0).abs() < 1.0, "{lowest}");
        assert!((highest - 43.0).abs() < 1.0, "{highest}");
    }
}
