//! Sound occlusion: how much the geometry between the listener and a sound muffles it, as
//! DAOrigins.exe works it out (0x5b7220 -> 0xa2c700).
//!
//! The exe casts a ray from the listener to the sound through the physics scene's
//! placeables and static geometry (mask 0x1024; one hit per shape, nearest first) and adds
//! the hits up: starting at 0, each hit adds `(1 - occlusion) * a` until 1 is reached.
//!
//! - A placeable's `a` is its type's value (`placeable_types.gda` column 0x913e3094; see
//!   `thedas_sim::Sim::sound_occluders`).
//! - A static shape's `a` is how deep inside its outline the ray passes, as an angle in
//!   degrees over 45 ([`edge_angle`]), times its model's sound material's percentage
//!   (`sound_geometry_mat_types.gda` column 0xae2b65f5: stone and metal 100, wood 40,
//!   grass 10, unknown 0). A triangle mesh has no outline, so the angle is 0.
//! - A static hit within 0.5 m of an earlier static hit counts in full (0xa2c9b1..0xa2ca01).
//!
//! An occluded event is low-passed, not turned down (`thedas_audio`'s `occlusion_cutoff`):
//! FMOD's software occlusion, which the exe turns on.
//!
//! The event's occlusion moves towards the result at 1.2 per second (0x5b6eb0, 0xafebc0;
//! `thedas_audio::AudioEngine::occlude`). Every occludable 3D event is tested, not only placed
//! sounds: the exe's events are occludable unless their player says otherwise.
//!
//! Provisional: the box's angle (the exe picks one corner by a score not decoded and tests
//! its three edges; here the corner nearest the hit), a third shape group in the ray's
//! mask (0x1000, whose shapes are not identified), the distance the 0.5 m rule measures
//! (taken from the listener).

use thedas_sim::collision::{Collision, Outline};

type V = [f32; 3];

fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: V, s: f32) -> V {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V, b: V) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
/// A unit vector, or +X for a vanishing one (the exe's normalise, 0x451a30).
fn unit(a: V) -> V {
    let n = dot(a, a).sqrt();
    if n > 1e-6 {
        scale(a, 1.0 / n)
    } else {
        [1.0, 0.0, 0.0]
    }
}
/// The angle between two unit vectors, degrees (0x4d4bf0).
fn angle(a: V, b: V) -> f32 {
    dot(a, b).clamp(-1.0, 1.0).acos().to_degrees()
}

/// The nearest point to `c` on the line through `from` and `to` (not clamped to the
/// segment, as the exe's is not).
fn nearest_on_line(from: V, to: V, c: V) -> V {
    let d = sub(to, from);
    let len2 = dot(d, d);
    if len2 <= 0.0 {
        return from;
    }
    add(from, scale(d, dot(sub(c, from), d) / len2))
}

/// For a sphere: the angle at `from` between the ray and the sphere's edge on the side the
/// ray passes (0x8220d0): the edge point is the centre moved a radius towards the ray's
/// nearest point.
fn sphere_angle(from: V, to: V, c: V, r: f32) -> f32 {
    let p = nearest_on_line(from, to, c);
    let edge = add(c, scale(unit(sub(p, c)), r));
    angle(unit(sub(edge, from)), unit(sub(to, from)))
}

/// How deep inside a shape's outline the ray from `from` to `to` passes, in degrees:
/// the angle at `from` between the ray and the outline's nearest edge. `None` for a mesh.
///
/// - Sphere: [`sphere_angle`].
/// - Capsule (0x828649): a sphere of its radius on its axis, at the ray's nearest point
///   projected on the axis and clamped to the axis's length either way from its middle
///   (the exe clamps to the full height, not half).
/// - Box (0x82894f, provisional): from the corner nearest `hit`, along each of its three
///   edges, the point nearest `hit` (edges it falls outside are skipped); the smallest
///   angle between the ray and such a point. With no such point the shape counts in full.
pub fn edge_angle(outline: &Outline, from: V, to: V, hit: V) -> Option<f32> {
    match outline {
        Outline::Mesh => None,
        Outline::Sphere { c, r } => Some(sphere_angle(from, to, *c, *r)),
        Outline::Capsule { a, b, r } => {
            let mid = scale(add(*a, *b), 0.5);
            let axis = sub(*b, *a);
            let height = dot(axis, axis).sqrt();
            let axis = unit(axis);
            let p = nearest_on_line(from, to, mid);
            let t = dot(sub(p, mid), axis).clamp(-height, height);
            Some(sphere_angle(from, to, add(mid, scale(axis, t)), *r))
        }
        Outline::Box { c, axes, half } => {
            // The corner nearest the hit, in the box's own axes.
            let local = sub(hit, *c);
            let sign: Vec<f32> = (0..3)
                .map(|i| if dot(local, axes[i]) < 0.0 { -1.0 } else { 1.0 })
                .collect();
            let corner = (0..3).fold(*c, |p, i| add(p, scale(axes[i], sign[i] * half[i])));
            let ray = unit(sub(to, from));
            let mut best: Option<f32> = None;
            for i in 0..3 {
                // The edge from the corner across the box along axis i.
                let edge = scale(axes[i], -sign[i] * 2.0 * half[i]);
                let len2 = dot(edge, edge);
                if len2 <= 0.0 {
                    continue;
                }
                let t = dot(sub(hit, corner), edge) / len2;
                if !(0.0..=1.0).contains(&t) {
                    continue;
                }
                let q = add(corner, scale(edge, t));
                let a = angle(unit(sub(q, from)), ray);
                best = Some(best.map_or(a, |b: f32| b.min(a)));
            }
            Some(best.unwrap_or(f32::MAX))
        }
    }
}

/// What a ray crossed, for [`occlusion`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Crossed {
    /// A placeable, with its type's value.
    Placeable(f32),
    /// Static geometry: metres from the listener, its edge angle (degrees; `None` for a
    /// mesh) and its sound material's percentage.
    Static {
        distance: f32,
        angle: Option<f32>,
        percent: f32,
    },
}

/// The occlusion of what a ray crossed, nearest first (0xa2c8ac..0xa2caeb).
pub fn occlusion(crossed: &[Crossed]) -> f32 {
    let mut occ = 0.0f32;
    let mut seen: Vec<f32> = Vec::new();
    for c in crossed {
        let a = match *c {
            Crossed::Placeable(v) => v,
            Crossed::Static {
                distance,
                angle,
                percent,
            } => {
                let again = seen.iter().any(|d| (distance - d).abs() < 0.5);
                seen.push(distance);
                if again {
                    1.0
                } else {
                    angle.unwrap_or(0.0) / 45.0 * percent * 0.01
                }
            }
        };
        occ += (1.0 - occ) * a;
        if occ >= 1.0 {
            break;
        }
    }
    occ.clamp(0.0, 1.0)
}

/// An area's sound occluders: its layout's collision and its placeables' with each one's
/// value, and each sound material's percentage.
#[derive(Default)]
pub struct Occluders {
    pub layout: Option<std::sync::Arc<Collision>>,
    pub placeables: Option<Collision>,
    /// Per placed model of `placeables` (its `ShapeInfo::object`), its value.
    pub values: Vec<f32>,
    /// `sound_geometry_mat_types.gda` column 0xae2b65f5 by row.
    pub percent: std::collections::HashMap<u8, f32>,
}

impl Occluders {
    /// The occlusion of a sound at `to` heard from `from` (0 when they are within 0.05 m,
    /// 0x5b7362).
    pub fn between(&self, from: V, to: V) -> f32 {
        let d = sub(to, from);
        if dot(d, d).sqrt() <= 0.05 {
            return 0.0;
        }
        use thedas_formats::phy::mask;
        // (distance, what) from both collisions, nearest first.
        let mut crossed: Vec<(f32, Crossed)> = Vec::new();
        if let Some(c) = &self.placeables {
            for h in c.ray_hits(from, to) {
                let v = c
                    .shape(h.shape)
                    .and_then(|s| s.object)
                    .and_then(|o| self.values.get(o as usize).copied())
                    .unwrap_or(0.0);
                crossed.push((h.distance, Crossed::Placeable(v)));
            }
        }
        if let Some(c) = &self.layout {
            for h in c.ray_hits(from, to) {
                let Some(s) = c.shape(h.shape) else { continue };
                if s.mask & mask::STATIC == 0 {
                    continue;
                }
                crossed.push((
                    h.distance,
                    Crossed::Static {
                        distance: h.distance,
                        angle: edge_angle(&s.outline, from, to, h.point),
                        percent: self.percent.get(&s.material).copied().unwrap_or(0.0),
                    },
                ));
            }
        }
        crossed.sort_by(|a, b| a.0.total_cmp(&b.0));
        let crossed: Vec<Crossed> = crossed.into_iter().map(|c| c.1).collect();
        occlusion(&crossed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sphere_occludes_by_how_deep_the_ray_passes() {
        let s = Outline::Sphere {
            c: [10.0, 0.0, 0.0],
            r: 2.0,
        };
        let from = [0.0, 0.0, 0.0];
        // Near the middle (0.1 m off): the edge is 1.9 m off the ray at 10 m, about 10.8
        // degrees.
        let deep = edge_angle(&s, from, [20.0, 0.2, 0.0], [8.0, 0.08, 0.0]).unwrap();
        assert!(
            (deep - (1.9f32 / 10.0).atan().to_degrees()).abs() < 0.3,
            "{deep}"
        );
        // Dead through the centre there is no side to take: the exe's normalise gives +X.
        let dead = edge_angle(&s, from, [20.0, 0.0, 0.0], [8.0, 0.0, 0.0]).unwrap();
        assert!(dead.abs() < 1e-3, "{dead}");
        // Grazing: the ray passes 1.9 m from the centre, so the edge is close to it.
        let graze = edge_angle(&s, from, [20.0, 3.8, 0.0], [9.5, 1.8, 0.0]).unwrap();
        assert!(graze < 1.5 && graze < deep, "{graze}");
        assert_eq!(
            edge_angle(&Outline::Mesh, from, [1.0, 0.0, 0.0], from),
            None
        );
    }

    #[test]
    fn a_capsule_is_a_sphere_on_its_axis_and_a_box_uses_its_nearest_edges() {
        let from = [0.0, 0.0, 1.0];
        let to = [20.0, 0.4, 1.0];
        let c = Outline::Capsule {
            a: [10.0, 0.0, 0.0],
            b: [10.0, 0.0, 4.0],
            r: 1.0,
        };
        // The ray passes 0.2 m from the axis at its own height: the edge is 0.8 m off it.
        let a = edge_angle(&c, from, to, [9.0, 0.18, 1.0]).unwrap();
        assert!((a - (0.8f32 / 10.0).atan().to_degrees()).abs() < 0.3, "{a}");
        // A 2 m cube at x = 10; the ray enters its near face 0.5 m from one edge.
        let b = Outline::Box {
            c: [10.0, 0.0, 1.0],
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            half: [1.0, 1.0, 1.0],
        };
        let a = edge_angle(&b, from, [20.0, 1.0, 1.0], [9.0, 0.5, 1.0]).unwrap();
        assert!(a > 0.5 && a < 6.0, "{a}");
    }

    #[test]
    fn hits_add_up_as_the_exe_adds_them() {
        // A door at 0.5, then stone 22.5 degrees inside its edge (half): 0.5 + 0.5 * 0.5.
        let v = occlusion(&[
            Crossed::Placeable(0.5),
            Crossed::Static {
                distance: 6.0,
                angle: Some(22.5),
                percent: 100.0,
            },
        ]);
        assert!((v - 0.75).abs() < 1e-6, "{v}");
        // A mesh alone adds nothing; two static hits within 0.5 m block in full.
        let mesh = |distance| Crossed::Static {
            distance,
            angle: None,
            percent: 100.0,
        };
        assert_eq!(occlusion(&[mesh(3.0)]), 0.0);
        assert_eq!(occlusion(&[mesh(3.0), mesh(3.3)]), 1.0);
        assert_eq!(occlusion(&[mesh(3.0), mesh(4.0)]), 0.0);
        // Unknown material (0 %): nothing; deeper than 45 degrees: full.
        let stone = |angle, percent| Crossed::Static {
            distance: 2.0,
            angle: Some(angle),
            percent,
        };
        assert_eq!(occlusion(&[stone(30.0, 0.0)]), 0.0);
        assert_eq!(occlusion(&[stone(60.0, 100.0)]), 1.0);
        assert!((occlusion(&[stone(45.0, 40.0)]) - 0.4).abs() < 1e-6);
    }
}
