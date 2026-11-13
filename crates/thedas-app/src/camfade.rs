//! Creatures the game camera is inside fade out, as DAOrigins.exe's game camera fades them
//! (0x6c4130, called by the game camera's update 0x6c4c60 once it has placed the camera at
//! +0x70, before it eases back out):
//!
//! 1. Every creature fading is given a target opacity of 1 (0x6c3c80 with no object).
//! 2. Unless the game is in conversation, a cutscene or the fly camera (the game mode
//!    0x5d5e70 gives is 4, 5 or 6: `GM_DIALOG`, `GM_CUTSCENE`, `GM_FLYCAM`), the physics
//!    scene is asked for the shapes in the creatures group (mask 2, [`MASK_CREATURES`]) a
//!    sphere of [`FADE_RADIUS`] round the camera touches (0x648c00), and each creature they
//!    belong to gets a target of [`FADED`] (0x6c3c80).
//! 3. Each creature with a target moves its opacity towards it by [`FADE_RATE`] a second
//!    (0x6c4216-0x6c4311), set on the object (0x6c2520 -> 0x695060, the object's +0x190,
//!    which its update 0x695080 hands its model), and leaves the list once back at 1.
//!
//! The camera's own pull-in keeps it 0.25 m from the look point at the closest (see
//! [`crate::follow`]); it has no other near limit and does not raise the look point or the
//! pitch as it closes in, so in a tight spot the camera ends inside the leader, who fades.
//!
//! Presentation only: reads where the creatures stand and their models' collision shapes.
//!
//! The opacity reaches the renderer through [`crate::live::Live::opacity`], combined with any
//! other fade in `Live::combine_opacity`.
//!
//! The query meets only shapes whose groups mask has the creatures bit (0x648c00 ->
//! 0x82e980: `overlapSphereShapes` with groups mask {2, 0, 0, 0}): the creatures' models'
//! `.phy` shapes in that group (the shape loader 0x589db0 sets it from field 6066; armour
//! carries small per-bone capsules, heads a neck and a head capsule). A creature's movement
//! controller capsule is not among them (its groups mask is 0x10, 0x6b218f).
//!
//! Provisional: the shapes are placed at the creature's current placement by their bones'
//! rest pose, not the animated skeleton the physics scene follows.

use std::collections::BTreeMap;

use glam::{Mat4, Quat, Vec3};
use thedas_formats::level::Transform;
use thedas_formats::model::Mmh;
use thedas_formats::phy::{Geometry, Phy, mask};
use thedas_script::vm::ObjectId;

/// The radius of the sphere round the camera that finds creatures to fade (metres;
/// 0xaff3e8, pushed at 0x6c41ac).
pub const FADE_RADIUS: f32 = 0.25;
/// The opacity a creature the camera is inside fades to (0xc038e8, 0x6c41e5).
pub const FADED: f32 = 0.05;
/// How fast opacity changes (a second; 0xc038ec, 0x6c4216).
pub const FADE_RATE: f32 = 5.0;
/// The shapes the sphere looks for (the query's mask, 0x6c41b2; the shape loader's
/// creatures bit, 6066).
pub const MASK_CREATURES: u32 = mask::CREATURES;

/// A creature collision shape in its model's space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Sphere {
        centre: Vec3,
        radius: f32,
    },
    Capsule {
        a: Vec3,
        b: Vec3,
        radius: f32,
    },
    /// Model space to the box's (centred, axis-aligned there) and its half extents.
    Box {
        to_box: Mat4,
        half: Vec3,
    },
}

impl Shape {
    /// Whether a sphere at `centre` (in the shape's space after `placement`, model to
    /// world) with `radius` touches it.
    pub fn touches(&self, placement: &Mat4, centre: Vec3, radius: f32) -> bool {
        let to_model = placement.inverse();
        let p = to_model.transform_point3(centre);
        match *self {
            Shape::Sphere {
                centre: c,
                radius: r,
            } => p.distance(c) <= r + radius,
            Shape::Capsule { a, b, radius: r } => {
                let ab = b - a;
                let t = if ab.length_squared() > 1e-12 {
                    ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                p.distance(a + ab * t) <= r + radius
            }
            Shape::Box { to_box, half } => {
                let q = to_box.transform_point3(p);
                q.clamp(-half, half).distance(q) <= radius
            }
        }
    }
}

impl Shape {
    /// How far along a ray (world `origin`, unit `dir`) it meets the shape placed by
    /// `placement` (model to world), if it does. A capsule's distance is where the ray comes
    /// nearest its axis, which orders picks well enough.
    pub fn ray(&self, placement: &Mat4, origin: Vec3, dir: Vec3) -> Option<f32> {
        let to_model = placement.inverse();
        let o = to_model.transform_point3(origin);
        let d = to_model.transform_vector3(dir);
        let len = d.length();
        if len < 1e-9 {
            return None;
        }
        let (d, scale) = (d / len, 1.0 / len);
        let sphere = |c: Vec3, r: f32| -> Option<f32> {
            let oc = o - c;
            let b = oc.dot(d);
            let disc = b * b - (oc.length_squared() - r * r);
            (disc >= 0.0)
                .then(|| [-b - disc.sqrt(), -b + disc.sqrt()])
                .and_then(|ts| ts.into_iter().find(|&t| t >= 0.0))
        };
        let t = match *self {
            Shape::Sphere { centre, radius } => sphere(centre, radius),
            Shape::Capsule { a, b, radius } => {
                // The nearest points of the ray and the axis segment.
                let u = b - a;
                let w = o - a;
                let (uu, ud, uw, dw) = (u.dot(u), u.dot(d), u.dot(w), d.dot(w));
                let den = uu - ud * ud;
                let s = if uu < 1e-12 {
                    0.0
                } else if den.abs() < 1e-9 {
                    (uw / uu).clamp(0.0, 1.0)
                } else {
                    ((uw - ud * dw) / den).clamp(0.0, 1.0)
                };
                let p = a + u * s;
                sphere(p, radius).or_else(|| {
                    // Re-check the cap the clamp moved off.
                    let t = (p - o).dot(d).max(0.0);
                    ((o + d * t).distance(p) <= radius).then_some(t)
                })
            }
            Shape::Box { to_box, half } => {
                let o = to_box.transform_point3(o);
                let d = to_box.transform_vector3(d);
                let (mut near, mut far) = (0.0f32, f32::INFINITY);
                for k in 0..3 {
                    if d[k].abs() < 1e-9 {
                        if o[k].abs() > half[k] {
                            return None;
                        }
                        continue;
                    }
                    let (t0, t1) = ((-half[k] - o[k]) / d[k], (half[k] - o[k]) / d[k]);
                    near = near.max(t0.min(t1));
                    far = far.min(t0.max(t1));
                }
                (near <= far).then_some(near)
            }
        };
        t.map(|t| t * scale)
    }
}

fn matrix(t: &Transform) -> Mat4 {
    let [x, y, z, w] = t.rotation;
    let q = Quat::from_xyzw(x, y, z, w);
    let q = if q.length_squared() > 1e-12 {
        q.normalize()
    } else {
        Quat::IDENTITY
    };
    Mat4::from_rotation_translation(q, Vec3::from_array(t.position))
}

/// A model's creature shapes in its space: each `.phy` shape in the creatures group,
/// placed under the `.mmh` node it hangs from (the bone's rest placement) by its pose. A
/// capsule runs along its local +Z from (0, 0, r) to (0, 0, r + height) (see
/// [`thedas_formats::phy`]).
pub fn model_shapes(phy: &Phy, mmh: Option<&Mmh>) -> Vec<Shape> {
    let mut out = Vec::new();
    for s in &phy.shapes {
        if s.mask & MASK_CREATURES == 0 {
            continue;
        }
        let node = match (mmh, s.parent.as_deref()) {
            (Some(m), Some(parent)) => m
                .node_chain(parent)
                .map(|chain| chain.iter().fold(Mat4::IDENTITY, |acc, t| acc * matrix(t)))
                .unwrap_or(Mat4::IDENTITY),
            _ => Mat4::IDENTITY,
        };
        let xf = node * matrix(&s.pose);
        match &s.geometry {
            Geometry::Sphere { radius } => out.push(Shape::Sphere {
                centre: xf.transform_point3(Vec3::ZERO),
                radius: *radius,
            }),
            Geometry::Capsule { radius, height } => out.push(Shape::Capsule {
                a: xf.transform_point3(Vec3::new(0.0, 0.0, *radius)),
                b: xf.transform_point3(Vec3::new(0.0, 0.0, radius + height)),
                radius: *radius,
            }),
            Geometry::Box { half } => out.push(Shape::Box {
                to_box: xf.inverse(),
                half: Vec3::from_array(*half),
            }),
            // Triangle meshes are not creature shapes in the game's models.
            Geometry::Mesh { .. } => {}
        }
    }
    out
}

/// The creatures fading and their opacity and target (the game camera's +0xa8 list).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CameraFade {
    fading: BTreeMap<ObjectId, (f32, f32)>,
}

impl CameraFade {
    /// One step of the game camera's update `dt` seconds after the last: `touching` are
    /// the creatures whose shapes the sphere round the camera touches (none while the game
    /// mode keeps them from fading).
    pub fn update(&mut self, touching: impl IntoIterator<Item = ObjectId>, dt: f32) {
        for (_, target) in self.fading.values_mut() {
            *target = 1.0;
        }
        for id in touching {
            self.fading.entry(id).or_insert((1.0, 1.0)).1 = FADED;
        }
        let step = FADE_RATE * if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        for (alpha, target) in self.fading.values_mut() {
            *alpha = if *target < *alpha {
                (*alpha - step).max(*target)
            } else {
                (*alpha + step).min(*target)
            };
        }
        self.fading.retain(|_, (alpha, _)| *alpha != 1.0);
    }

    /// Creature `id`'s opacity.
    #[cfg(test)]
    pub fn alpha(&self, id: ObjectId) -> f32 {
        self.fading.get(&id).map_or(1.0, |(a, _)| *a)
    }

    /// The creatures not fully opaque, and their opacity.
    pub fn faded(&self) -> impl Iterator<Item = (ObjectId, f32)> + '_ {
        self.fading.iter().map(|(id, (a, _))| (*id, *a))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_meet_spheres_capsules_and_boxes_where_they_are_placed() {
        let at = Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0));
        let (o, x) = (Vec3::ZERO, Vec3::X);
        let sphere = Shape::Sphere {
            centre: Vec3::ZERO,
            radius: 1.0,
        };
        assert!((sphere.ray(&at, o, x).unwrap() - 9.0).abs() < 1e-4);
        assert_eq!(sphere.ray(&at, o, -x), None);
        assert_eq!(sphere.ray(&at, Vec3::new(0.0, 2.0, 0.0), x), None);
        // An upright capsule: met at its side, missed over its top.
        let capsule = Shape::Capsule {
            a: Vec3::ZERO,
            b: Vec3::new(0.0, 0.0, 1.5),
            radius: 0.3,
        };
        assert!((capsule.ray(&at, Vec3::new(0.0, 0.0, 1.0), x).unwrap() - 9.7).abs() < 1e-3);
        assert_eq!(capsule.ray(&at, Vec3::new(0.0, 0.0, 2.0), x), None);
        let cube = Shape::Box {
            to_box: Mat4::IDENTITY,
            half: Vec3::splat(0.5),
        };
        assert!((cube.ray(&at, o, x).unwrap() - 9.5).abs() < 1e-4);
        assert_eq!(cube.ray(&at, Vec3::new(0.0, 0.6, 0.0), x), None);
        // From inside, at once.
        assert_eq!(cube.ray(&at, Vec3::new(10.0, 0.0, 0.0), x), Some(0.0));
    }

    #[test]
    fn fades_out_and_back_at_the_executables_rate() {
        let mut f = CameraFade::default();
        f.update([7], 0.1);
        assert!((f.alpha(7) - 0.5).abs() < 1e-6);
        f.update([7], 0.1);
        assert!((f.alpha(7) - FADED).abs() < 1e-6, "{}", f.alpha(7));
        assert_eq!(f.faded().collect::<Vec<_>>(), [(7, FADED)]);
        // Out of the sphere: back to opaque, and off the list.
        f.update([], 0.1);
        assert!((f.alpha(7) - 0.55).abs() < 1e-5);
        f.update([], 1.0);
        assert_eq!(f.alpha(7), 1.0);
        assert!(f.fading.is_empty());
        // No time passing changes nothing.
        f.update([3], 0.0);
        assert_eq!(f.alpha(3), 1.0);
    }

    #[test]
    fn spheres_touch_capsules_spheres_and_boxes() {
        let at = Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0));
        let cap = Shape::Capsule {
            a: Vec3::new(0.0, 0.0, 1.0),
            b: Vec3::new(0.0, 0.0, 1.5),
            radius: 0.2,
        };
        assert!(cap.touches(&at, Vec3::new(10.4, 0.0, 1.2), 0.25));
        assert!(!cap.touches(&at, Vec3::new(10.5, 0.0, 1.2), 0.25));
        assert!(cap.touches(&at, Vec3::new(10.0, 0.0, 1.9), 0.25));
        let ball = Shape::Sphere {
            centre: Vec3::Z,
            radius: 0.5,
        };
        assert!(ball.touches(&at, Vec3::new(10.0, 0.7, 1.0), 0.25));
        assert!(!ball.touches(&at, Vec3::new(10.0, 0.8, 1.0), 0.25));
        let block = Shape::Box {
            to_box: Mat4::from_translation(-Vec3::Z),
            half: Vec3::splat(0.5),
        };
        assert!(block.touches(&at, Vec3::new(10.7, 0.0, 1.0), 0.25));
        assert!(!block.touches(&at, Vec3::new(10.8, 0.0, 1.0), 0.25));
    }
}
