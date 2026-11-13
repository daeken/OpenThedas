//! Particle emitters: simulation and billboard building, after DAOrigins.exe's sprite emitter
//! (`ObjectNodeEmitterSprite<SpriteParticle>`, vtable 0xaf87f4). Notes with every address
//! are kept outside the repository (`notes/vfx-emitters.md` in the test-data folder); the
//! functions mirrored here:
//!
//! - **Stepping** (0x552d70): a frame's time is cut into steps of at most 0.0666 s. Each step
//!   updates the living particles (0x5528e0), then births new ones (count 0x53ec80, each
//!   0x5525b0), at most 256 alive.
//! - **Count** (0x53ec80): a continuous emitter with no birthrate range accumulates time and
//!   births `trunc(acc · rate)`, dropping the accumulator to zero when that is positive; with
//!   a range it births `trunc(x)` of `x = (rate + u·range)·dt` when `x > 1`, else one with
//!   probability `x`. A burst emitter births `trunc(rate + u·range)` once.
//! - **Birth** (0x5525b0, 0x53f980): life `Life + u·LifeRange` (`Life` when that is not
//!   positive; a negative `Life` makes particles that never die), rotation speed `RotSpeed + u·RotSpeedRange`, size factor `1 + u·ScaleRange`,
//!   flipbook phase random in `[0, 1)` with `RandomStartFrame`. It moves along the emitter's
//!   +Z, turned about the emitter's X and Y axes by `u·spread` each, at
//!   `InitialSpeed + u·InitialSpeedRange`.
//! - **Motion** (0x57bf30): with `A = Acceleration·dt` and `G = WorldAxisAcceleration·dt`,
//!   `V' = V·(1 + A) + G`, `P += dt·(V' + dt/2·(A·V + G))`, and gravity
//!   `P.z += −9.8·GravityMultiplier·(age² − (age − dt)²)/2`.
//! - **Spin** (0x57c7b0): speed grows by `RotationalAcceleration·dt`; the angle by
//!   `speed·dt·2π`.
//! - **Age map** (0x5607c0): keys interpolated linearly by the fraction of life lived,
//!   clamped at both ends; colour times `AgeMapColorMultiplier`, sizes times the scale
//!   multipliers and the particle's size factor.
//! - **Quad** (0x550a9f-0x550c90): corners `P ∓ A ∓ B`, with `A` along the particle's first
//!   axis (the key's X size) and `B` along its second (its Y size); texture coordinates (0,0), (0,1),
//!   (1,1), (1,0) for `P−B−A`, `P−B+A`, `P+B+A`, `P+B−A`, and the flipbook phase in the
//!   third.
//!
//! `u` is uniform in [−1, 1], `f` in [0, 1], from the MSVC `rand` recurrence the exe runs on
//! two global seeds (0xc31a6c births, 0xc31994 directions).
//!
//! Provisional (not traced, or simplified):
//! - Each emitter keeps its own seeds (the exe shares two among all emitters), seeded from
//!   its placement, so a level's fires do not move in step.
//! - The quad's size factor: the exe multiplies the key's size twice by a value its owner
//!   gives (vfunc 0xe0, `+0x5c` of a `GraphicsStructures::Object`) and the bounds once;
//!   taken here as 1/√2 times the placement's scale, so a quad is as large as its key.
//! - Which camera vectors mode 0 reads (taken as down and right, so textures stand upright),
//!   and mode 3's (the first axis against the velocity). Modes 1 and 2 lie in the emitter's
//!   XY plane; 4 draws as 0 (camera-facing).
//! - Spawn volumes' sphere direction (0x4ee200's Euler order), and their normals as initial
//!   direction, are approximations.
//! - Not mirrored yet: targets and attraction, movement spread, particle inheritance,
//!   birth per metre, wind, collisions, physics and mesh particles.

use std::sync::Arc;

use glam::{Mat4, Quat, Vec3, Vec4};
use thedas_formats::emitter::{AgeKey, Emitter, SpawnShape};

/// The longest step the exe advances emitters by (0xaf8a40).
pub const MAX_STEP: f32 = 0.0666;
/// The most particles an emitter keeps alive (0x553134, 0x5525be).
pub const MAX_PARTICLES: usize = 256;
const GRAVITY: f32 = -9.8;

/// A seed for the `index`th emitter of a scene placed at `world`: fixed for a placement,
/// different between neighbours.
pub fn seed(index: usize, world: Mat4) -> u32 {
    let p = world.w_axis;
    let mut h = 0x811c_9dc5u32 ^ index as u32;
    for x in [p.x, p.y, p.z] {
        h = (h ^ x.to_bits()).wrapping_mul(0x0100_0193);
    }
    h
}

/// The MSVC `rand` recurrence.
#[derive(Clone, Copy, Debug)]
pub struct Rand(u32);

impl Rand {
    pub fn new(seed: u32) -> Self {
        Rand(seed)
    }

    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(0x343fd).wrapping_add(0x269ec3);
        (self.0 >> 16) & 0x7fff
    }

    /// Uniform in [−1, 1] (`r·2/32767 − 1`).
    pub fn signed(&mut self) -> f32 {
        (self.next() as f64 * (2.0 / 32767.0) - 1.0) as f32
    }

    /// Uniform in [0, 1] (`r/32767`).
    pub fn unit(&mut self) -> f32 {
        (self.next() as f64 * (1.0 / 32767.0)) as f32
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    pub position: Vec3,
    pub velocity: Vec3,
    pub age: f32,
    pub inv_life: f32,
    /// The size factor `1 + u·ScaleRange`.
    pub scale: f32,
    /// Turns per second.
    pub spin: f32,
    /// Radians.
    pub angle: f32,
    /// The flipbook phase: whole sheets played.
    pub phase: f32,
}

/// What a particle looks like now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub color: Vec4,
    pub size: [f32; 2],
}

/// The age map at `t`, a fraction of life.
pub fn age_map(keys: &[AgeKey], t: f32) -> (Vec4, [f32; 2]) {
    let key = |k: &AgeKey| (Vec4::from_array(k.color), k.scale);
    let Some(first) = keys.first() else {
        return (Vec4::ONE, [1.0, 1.0]);
    };
    // The first key later than `t`.
    match keys.iter().position(|k| k.time > t) {
        Some(0) => key(first),
        None => key(keys.last().unwrap_or(first)),
        Some(i) => {
            let (a, b) = (&keys[i - 1], &keys[i]);
            let span = b.time - a.time;
            let w = if span > 0.0 { (t - a.time) / span } else { 1.0 };
            let (ca, sa) = key(a);
            let (cb, sb) = key(b);
            (
                ca * (1.0 - w) + cb * w,
                [sa[0] * (1.0 - w) + sb[0] * w, sa[1] * (1.0 - w) + sb[1] * w],
            )
        }
    }
}

/// One placed emitter and its particles.
#[derive(Clone, Debug)]
pub struct EmitterState {
    pub blueprint: Arc<Emitter>,
    /// The emitter node's placement, node to world.
    pub world: Mat4,
    pub particles: Vec<Particle>,
    births: Rand,
    directions: Rand,
    accumulator: f32,
    burst_done: bool,
    /// Multiplies the particles' alpha; at 0 none are drawn (a cut-away model's emitters,
    /// DAOrigins.exe 0x466f40 setting emitter +0x360). Provisional: what +0x360 scales is
    /// not traced.
    pub opacity: f32,
    /// Birthrate and its range set by an animation of the emitter's model (a visual
    /// effect's exported `<emitter>_birthrate` parameters), over the emitter's own.
    pub rate: Option<(f32, f32)>,
}

impl EmitterState {
    pub fn new(blueprint: Arc<Emitter>, world: Mat4, seed: u32) -> Self {
        EmitterState {
            blueprint,
            world,
            particles: Vec::new(),
            births: Rand::new(seed),
            directions: Rand::new(seed ^ 0x5bd1_e995),
            accumulator: 0.0,
            burst_done: false,
            opacity: 1.0,
            rate: None,
        }
    }

    /// Carries on from `old`, the same emitter in a scene since rebuilt: its particles and
    /// where its counting stood. The placement and the birthrate stay this one's.
    pub fn carry_on_from(&mut self, old: &EmitterState) {
        self.particles = old.particles.clone();
        self.births = old.births;
        self.directions = old.directions;
        self.accumulator = old.accumulator;
        self.burst_done = old.burst_done;
    }

    /// Advances by `seconds`, in steps of at most [`MAX_STEP`].
    pub fn advance(&mut self, seconds: f32) {
        if seconds.is_nan() || seconds <= 0.0 || !seconds.is_finite() {
            return;
        }
        let steps = (seconds / MAX_STEP).ceil().max(1.0) as u32;
        let dt = seconds / steps as f32;
        for _ in 0..steps.min(1000) {
            self.step(dt);
        }
    }

    pub fn step(&mut self, dt: f32) {
        self.update(dt);
        let n = self.count(dt);
        for _ in 0..n {
            if self.particles.len() >= MAX_PARTICLES {
                break;
            }
            self.spawn();
        }
    }

    fn update(&mut self, dt: f32) {
        let e = self.blueprint.clone();
        let frames = (e.flipbook_columns.max(1) * e.flipbook_rows.max(1)) as f32;
        let frame_step = if e.flipbook {
            e.frames_per_second * dt / frames
        } else {
            0.0
        };
        let accel = {
            let w = Vec3::from_array(e.world_axis_acceleration);
            if e.object_space_acceleration {
                self.world.transform_vector3(w)
            } else {
                w
            }
        };
        let mut i = 0;
        while i < self.particles.len() {
            let p = &mut self.particles[i];
            if p.age >= 0.0 {
                p.age += dt;
            }
            if p.age * p.inv_life >= 1.0 {
                // 0x552a04: the last particle takes its place.
                self.particles.swap_remove(i);
                continue;
            }
            let a = e.acceleration * dt;
            let g = accel * dt;
            let v0 = p.velocity;
            p.velocity = v0 * (1.0 + a) + g;
            p.position += dt * (p.velocity + 0.5 * dt * (a * v0 + g));
            let before = p.age - dt;
            p.position.z +=
                GRAVITY * e.gravity_multiplier * 0.5 * (p.age * p.age - before * before);
            p.spin += e.rotational_acceleration * dt;
            p.angle += p.spin * dt * std::f32::consts::TAU;
            p.phase += frame_step;
            i += 1;
        }
    }

    fn count(&mut self, dt: f32) -> u32 {
        let e = &self.blueprint;
        let dt = dt.max(0.0);
        let (birthrate, birthrate_range) = self.rate.unwrap_or((e.birthrate, e.birthrate_range));
        match e.emitter_type {
            0 => {
                if birthrate <= 0.0 {
                    return 0;
                }
                if birthrate_range == 0.0 {
                    self.accumulator += dt;
                    let n = (self.accumulator * birthrate) as i64;
                    if n > 0 {
                        self.accumulator = 0.0;
                        n as u32
                    } else {
                        0
                    }
                } else {
                    let x = (self.births.signed() * birthrate_range + birthrate) * dt;
                    if x > 1.0 {
                        x as u32
                    } else {
                        u32::from(self.births.unit() < x)
                    }
                }
            }
            // A burst waits for a birthrate (an effect's emitter before its timeline starts).
            1 if !self.burst_done && birthrate + birthrate_range > 0.0 => {
                self.burst_done = true;
                (birthrate + self.births.signed() * birthrate_range).max(0.0) as u32
            }
            _ => 0,
        }
    }

    fn spawn(&mut self) {
        let e = self.blueprint.clone();
        let r = &mut self.births;
        let mut life = r.signed() * e.life_range + e.life;
        if life <= 0.0 {
            life = e.life;
        }
        // 0x5525b0 stores 1/life as it is: a negative life (the Proving crowd's -1) gives a
        // negative fraction lived, so the particle never dies and keeps its first age key.
        let inv_life = 1.0 / life;
        let spin = r.signed() * e.rotation_speed_range + e.rotation_speed;
        let scale = 1.0 + r.signed() * e.scale_range;
        let phase = if e.random_start_frame { r.unit() } else { 0.0 };
        let angle = e.initial_rotation + r.signed() * e.initial_rotation_range;

        let (_, rotation, origin) = self.world.to_scale_rotation_translation();
        let d = &mut self.directions;
        let (offset, normal) = sample_volume(&e, d);
        let position = self.world.transform_point3(offset);
        let mut dir = if e.volume.normals_for_direction && normal != Vec3::ZERO {
            rotation * normal
        } else {
            rotation * Vec3::Z
        };
        for (k, axis) in [Vec3::X, Vec3::Y].into_iter().enumerate() {
            let spread = e.spread[k];
            if spread != 0.0 {
                let angle = d.signed() * spread.to_radians();
                dir = Quat::from_axis_angle(rotation * axis, angle) * dir;
            }
        }
        let speed = d.signed() * e.initial_speed_range + e.initial_speed;
        let _ = origin;
        self.particles.push(Particle {
            position,
            velocity: dir * speed,
            age: 0.0,
            inv_life,
            scale,
            spin,
            angle,
            phase,
        });
    }

    /// Particle `p`'s colour and size now.
    pub fn look(&self, p: &Particle) -> Look {
        let e = &self.blueprint;
        let t = (p.age * p.inv_life).clamp(0.0, 1.0);
        let (color, size) = age_map(&e.age_map, t);
        Look {
            color: color * Vec4::from_array(e.age_color_multiplier),
            size: [
                size[0] * e.age_scale_multiplier[0] * p.scale,
                size[1] * e.age_scale_multiplier[1] * p.scale,
            ],
        }
    }

    /// The particles as quads, four [`ParticleVertex`]es each, for a camera with the given
    /// world-space right, up and viewing directions.
    pub fn quads(&self, right: Vec3, up: Vec3, forward: Vec3, out: &mut Vec<ParticleVertex>) {
        if self.opacity <= 0.0 {
            return;
        }
        let e = &self.blueprint;
        let (scale, rotation, _) = self.world.to_scale_rotation_translation();
        let k2 = 0.5 * scale.max_element().powi(2);
        for p in &self.particles {
            let look = self.look(p);
            // The first axis takes the key's X size and the texture's v; the second its Y
            // size and u. Camera-facing (0x57c840): the first is the camera's down, the
            // second its right, so the texture stands upright.
            let (mut ax, mut ay, normal) = match e.orientation {
                1 | 2 => (rotation * Vec3::X, rotation * Vec3::Y, rotation * Vec3::Z),
                3 => {
                    // Along the velocity: the texture's top leads.
                    let along = p.velocity.normalize_or_zero();
                    if along == Vec3::ZERO {
                        (-up, right, forward)
                    } else {
                        let side = forward.cross(along).normalize_or(right);
                        (-along, side, forward)
                    }
                }
                _ => (-up, right, forward),
            };
            if p.angle != 0.0 && e.orientation != 3 {
                let q = Quat::from_axis_angle(normal, p.angle);
                ax = q * ax;
                ay = q * ay;
            }
            // Half sizes (0x550b0a): the key's size times a factor `k` read from the owner
            // (vfunc 0xe0), twice; the bounds take it once (0x550c93), as a quad's
            // half-diagonal, which makes `k` 1/√2 for a quad as large as the key.
            let a = ax * (look.size[0] * k2);
            let b = ay * (look.size[1] * k2);
            let mut color = look.color.clamp(Vec4::ZERO, Vec4::ONE).to_array();
            color[3] *= self.opacity.min(1.0);
            let c = p.position;
            for (corner, uv) in [
                (c - b - a, [0.0, 0.0]),
                (c - b + a, [0.0, 1.0]),
                (c + b + a, [1.0, 1.0]),
                (c + b - a, [1.0, 0.0]),
            ] {
                out.push(ParticleVertex {
                    position: corner.to_array(),
                    color,
                    texcoord: [uv[0], uv[1], p.phase, 0.0],
                });
            }
        }
    }
}

/// Where in the emitter's space a particle is born, and the volume's normal there.
fn sample_volume(e: &Emitter, r: &mut Rand) -> (Vec3, Vec3) {
    let v = &e.volume;
    let flip = |n: Vec3| if v.invert_normals { -n } else { n };
    match v.shape {
        SpawnShape::Point => (Vec3::ZERO, Vec3::ZERO),
        SpawnShape::Sphere { radius } => {
            let r_ = if v.within { r.unit() * radius } else { radius };
            let yaw = (r.unit() * 360.0).to_radians();
            let pitch = (r.unit() * 180.0).to_radians();
            let n = Quat::from_rotation_z(yaw) * Quat::from_rotation_x(pitch) * Vec3::Z;
            (n * r_, flip(n))
        }
        SpawnShape::Cylinder {
            radius,
            length,
            axis,
        } => {
            let axis = Vec3::from_array(axis).normalize_or(Vec3::Z);
            let h = r.signed() * length * 0.5;
            let r_ = if v.within { r.unit() * radius } else { radius };
            let angle = r.unit() * std::f32::consts::TAU;
            let perp = axis.any_orthonormal_vector();
            let n = Quat::from_axis_angle(axis, angle) * perp;
            (n * r_ + axis * h, flip(n))
        }
        SpawnShape::Box { min, max } => {
            let (lo, hi) = (Vec3::from_array(min), Vec3::from_array(max));
            let t = Vec3::new(r.unit(), r.unit(), r.unit());
            (lo + (hi - lo) * t, Vec3::ZERO)
        }
    }
}

/// A particle quad's corner as the VFX shaders read it: `POSITION` (world), `COLOR0`,
/// `TEXCOORD0` (u, v, flipbook phase, 0).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ParticleVertex {
    pub position: [f32; 3],
    pub color: [f32; 4],
    pub texcoord: [f32; 4],
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fire() -> Emitter {
        Emitter {
            material: "fx".into(),
            birthrate: 10.0,
            life: 1.0,
            initial_speed: 1.0,
            flipbook: true,
            frames_per_second: 16.0,
            flipbook_columns: 4,
            flipbook_rows: 4,
            age_color_multiplier: [1.0; 4],
            age_scale_multiplier: [1.0, 1.0],
            age_map: vec![
                AgeKey {
                    time: 0.0,
                    scale: [1.0, 2.0],
                    color: [1.0, 0.0, 0.0, 0.0],
                },
                AgeKey {
                    time: 0.5,
                    scale: [3.0, 2.0],
                    color: [1.0, 1.0, 0.0, 1.0],
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn rand_follows_msvc() {
        // MSVC's rand() from seed 1: 41, 18467, 6334.
        let mut r = Rand::new(1);
        assert_eq!([r.next(), r.next(), r.next()], [41, 18467, 6334]);
        let mut r = Rand::new(7);
        for _ in 0..100 {
            let (s, u) = (r.signed(), r.unit());
            assert!((-1.0..=1.0).contains(&s) && (0.0..=1.0).contains(&u));
        }
    }

    #[test]
    fn age_map_interpolates_and_clamps() {
        let e = fire();
        let (c, s) = age_map(&e.age_map, 0.25);
        assert_eq!(c, Vec4::new(1.0, 0.5, 0.0, 0.5));
        assert_eq!(s, [2.0, 2.0]);
        assert_eq!(age_map(&e.age_map, 0.9).1, [3.0, 2.0]);
        assert_eq!(age_map(&e.age_map, 0.0).1, [1.0, 2.0]);
        assert_eq!(age_map(&[], 0.5), (Vec4::ONE, [1.0, 1.0]));
    }

    #[test]
    fn continuous_births_drop_the_fraction() {
        // 10 per second in steps of 0.05 s: one every second step, never two.
        let mut s = EmitterState::new(Arc::new(fire()), Mat4::IDENTITY, 1);
        let mut born = Vec::new();
        for _ in 0..6 {
            born.push(s.count(0.05));
        }
        assert_eq!(born, vec![0, 1, 0, 1, 0, 1]);
    }

    #[test]
    fn particles_rise_age_and_die() {
        let world = Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0));
        let mut s = EmitterState::new(Arc::new(fire()), world, 3);
        s.advance(0.5);
        assert!(!s.particles.is_empty());
        for p in &s.particles {
            // Born at the origin moving up at 1 m/s, no spread or gravity.
            assert!((p.position.x - 5.0).abs() < 1e-5);
            assert!((p.position.z - p.age).abs() < 1e-4, "{p:?}");
            assert!(p.phase > 0.0 || p.age == 0.0);
        }
        s.advance(1.2);
        assert!(s.particles.iter().all(|p| p.age < 1.0));
    }

    #[test]
    fn a_negative_life_never_ends() {
        let mut e = fire();
        e.life = -1.0;
        e.life_range = 0.0;
        let mut s = EmitterState::new(Arc::new(e), Mat4::IDENTITY, 3);
        s.spawn();
        s.advance(30.0);
        assert!(!s.particles.is_empty());
        assert!(s.particles.iter().any(|p| p.age > 29.0));
    }

    #[test]
    fn gravity_matches_the_closed_form() {
        let mut e = fire();
        e.initial_speed = 0.0;
        e.gravity_multiplier = 1.0;
        e.life = 10.0;
        let mut s = EmitterState::new(Arc::new(e), Mat4::IDENTITY, 3);
        s.spawn();
        for _ in 0..10 {
            s.update(0.05);
        }
        let p = s.particles[0];
        assert!((p.position.z - (-4.9 * p.age * p.age)).abs() < 1e-4);
    }

    #[test]
    fn quads_face_the_camera_with_the_traced_corners() {
        let mut s = EmitterState::new(Arc::new(fire()), Mat4::IDENTITY, 3);
        s.particles.push(Particle {
            position: Vec3::new(1.0, 2.0, 3.0),
            velocity: Vec3::Z,
            age: 0.0,
            inv_life: 1.0,
            scale: 1.0,
            spin: 0.0,
            angle: 0.0,
            phase: 0.25,
        });
        let mut v = Vec::new();
        s.quads(Vec3::X, Vec3::Z, Vec3::Y, &mut v);
        assert_eq!(v.len(), 4);
        // Size (1, 2) at age 0: A = down·0.5, B = right·1; (0,0) at the top left.
        assert_eq!(v[0].position, [0.0, 2.0, 3.5]);
        assert_eq!(v[1].position, [0.0, 2.0, 2.5]);
        assert_eq!(v[2].position, [2.0, 2.0, 2.5]);
        assert_eq!(v[3].position, [2.0, 2.0, 3.5]);
        assert_eq!(v[1].texcoord, [0.0, 1.0, 0.25, 0.0]);
        assert_eq!(v[0].color, [1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_cut_away_emitter_draws_nothing() {
        let mut s = EmitterState::new(Arc::new(fire()), Mat4::IDENTITY, 3);
        s.particles.push(Particle {
            position: Vec3::ZERO,
            velocity: Vec3::Z,
            age: 0.0,
            inv_life: 1.0,
            scale: 1.0,
            spin: 0.0,
            angle: 0.0,
            phase: 0.0,
        });
        s.opacity = 0.0;
        let mut v = Vec::new();
        s.quads(Vec3::X, Vec3::Z, Vec3::Y, &mut v);
        assert!(v.is_empty());
    }

    #[test]
    fn spawn_volume_disc_stays_in_its_plane() {
        let mut e = fire();
        e.volume.shape = SpawnShape::Cylinder {
            radius: 2.0,
            length: 0.0,
            axis: [0.0, 0.0, 1.0],
        };
        let mut r = Rand::new(9);
        for _ in 0..20 {
            let (p, n) = sample_volume(&e, &mut r);
            assert!(p.z.abs() < 1e-6 && (p.length() - 2.0).abs() < 1e-4);
            assert!(n.z.abs() < 1e-6);
        }
    }
}
