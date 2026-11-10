//! Environment layouts: the area layout (`.arl`, GFF4 `ARL `) and its room layouts (`.rml`,
//! GFF4 `RML `).
//!
//! An `.arl` lives loose in `packages/core/env/` and lists the level's rooms. Each room's
//! `.rml` (inside `env/<level>/<level>.rim`) places models in the room and binds their mesh
//! chunks to regions of the room's lightmap atlases.
//!
//! Provisional: every field label below was identified from the data (names, value ranges and
//! cross-checks such as the stored world-space bounding spheres), not from the original
//! executable. The room transform rule (`world = room * placement`) matches the stored sphere
//! centres (`3321`) on the levels checked.

use crate::gff4::{Gff4, StructId};
use crate::typed::{Doc, TypedResult};

pub mod field {
    //! GFF4 field labels used by the layout readers. All provisional.

    pub const ARL_NAME: u32 = 3012;
    pub const ARL_ROOMS: u32 = 3016;
    pub const ROOM_ID: u32 = 3031;
    pub const ROOM_NAME: u32 = 3033;
    pub const ROOM_BOUNDS_MIN: u32 = 3140;
    pub const ROOM_BOUNDS_MAX: u32 = 3141;

    pub const RML_MODELS: u32 = 3050;
    pub const POSITION: u32 = 4;
    pub const ROTATION: u32 = 5;
    pub const MDL_SCALE: u32 = 3059;
    pub const MDL_ID: u32 = 3061;
    pub const MDL_NAME: u32 = 3062;
    pub const MDL_RESOURCE: u32 = 3063;
    pub const MDL_LIGHTMAP_NAME: u32 = 3320;
    pub const MDL_SPHERE_CENTER: u32 = 3321;
    pub const MDL_SPHERE_RADIUS: u32 = 3322;
    pub const MDL_LIGHTMAPS: u32 = 3326;
    pub const LMAT_CHUNK: u32 = 3235;
    pub const LMAT_TEXTURE: u32 = 3323;
    pub const LMAT_OFFSET_SCALE: u32 = 3324;
}

const IDENTITY_QUAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// A rigid transform: translation, then a rotation quaternion `[x, y, z, w]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: IDENTITY_QUAT,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArlRoom {
    pub id: i64,
    /// The room's resource stem: `<name>.rml` in the level rim, `<name>.rim` and
    /// `<name>.gpu.rim` beside it.
    pub name: String,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
}

/// An area layout.
#[derive(Clone, Debug, PartialEq)]
pub struct Arl {
    pub name: Option<String>,
    pub rooms: Vec<ArlRoom>,
}

impl Arl {
    pub fn parse(data: &[u8]) -> TypedResult<Self> {
        Self::from_gff4(&Gff4::parse(data)?)
    }

    pub fn from_gff4(gff: &Gff4) -> TypedResult<Self> {
        let d = Doc::new(gff, "arl");
        d.expect_type("ARL ")?;
        let root = gff.root;
        let mut rooms = Vec::new();
        for r in d.structs(root, field::ARL_ROOMS)? {
            let d = Doc::new(gff, "arl room");
            rooms.push(ArlRoom {
                id: d.opt_i64(r, field::ROOM_ID)?.unwrap_or(-1),
                name: d.str(r, field::ROOM_NAME)?,
                bounds_min: d.opt_vec3(r, field::ROOM_BOUNDS_MIN)?.unwrap_or_default(),
                bounds_max: d.opt_vec3(r, field::ROOM_BOUNDS_MAX)?.unwrap_or_default(),
            });
        }
        Ok(Arl {
            name: d.opt_str(root, field::ARL_NAME)?,
            rooms,
        })
    }

    /// The union of all room bounds, if there are rooms.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut it = self.rooms.iter();
        let first = it.next()?;
        let (mut lo, mut hi) = (first.bounds_min, first.bounds_max);
        for r in it {
            for i in 0..3 {
                lo[i] = lo[i].min(r.bounds_min[i]);
                hi[i] = hi[i].max(r.bounds_max[i]);
            }
        }
        Some((lo, hi))
    }
}

/// Binds one mesh chunk of a placed model to a region of a lightmap atlas.
#[derive(Clone, Debug, PartialEq)]
pub struct LightmapBinding {
    /// Matches the lightmap id of a mesh piece in the model (`_7u` and similar).
    pub chunk: String,
    /// The atlas texture, e.g. `lgt101d_room000.dds`.
    pub texture: String,
    /// Atlas UV = mesh lightmap UV * scale + offset. The materials name this parameter
    /// `mml_vLightmapOffsetAndScale`, which fixes the component order.
    pub offset: [f32; 2],
    pub scale: [f32; 2],
}

/// One model placed in a room.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub id: Option<i64>,
    /// The base name, e.g. `tve_pil_05`.
    pub name: Option<String>,
    /// The model resource stem (`<model>.mmh`), e.g. `tve_pil_05_0`. Falls back to `name`
    /// when the resource field is missing.
    pub model: String,
    /// Relative to the room.
    pub transform: Transform,
    pub scale: f32,
    pub lightmap_name: Option<String>,
    /// World-space bounding sphere as stored by the toolset.
    pub sphere: Option<([f32; 3], f32)>,
    pub lightmaps: Vec<LightmapBinding>,
}

/// A room layout.
#[derive(Clone, Debug, PartialEq)]
pub struct Rml {
    /// Where the room sits in the level; placements are relative to it.
    pub transform: Transform,
    pub placements: Vec<Placement>,
}

impl Rml {
    pub fn parse(data: &[u8]) -> TypedResult<Self> {
        Self::from_gff4(&Gff4::parse(data)?)
    }

    pub fn from_gff4(gff: &Gff4) -> TypedResult<Self> {
        let d = Doc::new(gff, "rml");
        d.expect_type("RML ")?;
        let root = gff.root;
        let transform = read_transform(&d, root)?;
        let mut placements = Vec::new();
        for m in d.structs(root, field::RML_MODELS)? {
            placements.push(read_placement(gff, m)?);
        }
        Ok(Rml {
            transform,
            placements,
        })
    }
}

fn read_transform(d: &Doc, id: StructId) -> TypedResult<Transform> {
    Ok(Transform {
        position: d.opt_vec3(id, field::POSITION)?.unwrap_or_default(),
        rotation: d
            .opt_vec4(id, field::ROTATION, 1.0)?
            .unwrap_or(IDENTITY_QUAT),
    })
}

fn read_placement(gff: &Gff4, m: StructId) -> TypedResult<Placement> {
    let d = Doc::new(gff, "rml model");
    let name = d.opt_str(m, field::MDL_NAME)?.filter(|s| !s.is_empty());
    let resource = d.opt_str(m, field::MDL_RESOURCE)?.filter(|s| !s.is_empty());
    let model = resource.or_else(|| name.clone()).ok_or_else(|| {
        crate::typed::TypedError::Invalid("rml model: no model name (3062/3063)".into())
    })?;
    let sphere = match (
        d.opt_vec3(m, field::MDL_SPHERE_CENTER)?,
        d.opt_f32(m, field::MDL_SPHERE_RADIUS)?,
    ) {
        (Some(c), Some(r)) => Some((c, r)),
        _ => None,
    };
    let mut lightmaps = Vec::new();
    for l in d.structs(m, field::MDL_LIGHTMAPS)? {
        let d = Doc::new(gff, "rml lightmap");
        let [ox, oy, sx, sy] = d
            .opt_vec4(l, field::LMAT_OFFSET_SCALE, 0.0)?
            .unwrap_or([0.0, 0.0, 1.0, 1.0]);
        lightmaps.push(LightmapBinding {
            chunk: d.str(l, field::LMAT_CHUNK)?,
            texture: d.str(l, field::LMAT_TEXTURE)?,
            offset: [ox, oy],
            scale: [sx, sy],
        });
    }
    Ok(Placement {
        id: d.opt_i64(m, field::MDL_ID)?,
        name,
        model,
        transform: read_transform(&d, m)?,
        scale: d.opt_f32(m, field::MDL_SCALE)?.unwrap_or(1.0),
        lightmap_name: d.opt_str(m, field::MDL_LIGHTMAP_NAME)?,
        sphere,
        lightmaps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gff4::Value;
    use crate::typed::testutil::{Builder, generic, s, structs};

    #[test]
    fn reads_area_layout() {
        let mut b = Builder::new(b"ARL ");
        let r0 = b.add(
            b"ROOM",
            vec![
                (field::ROOM_ID, Value::I32(0)),
                (field::ROOM_NAME, s("lvl_0")),
                (field::ROOM_BOUNDS_MIN, Value::Vec3([-1.0, -2.0, -3.0])),
                (field::ROOM_BOUNDS_MAX, Value::Vec3([1.0, 2.0, 3.0])),
            ],
        );
        let r1 = b.add(
            b"ROOM",
            vec![
                (field::ROOM_ID, Value::I32(7)),
                (field::ROOM_NAME, s("lvl_7")),
                (field::ROOM_BOUNDS_MIN, Value::Vec3([0.0, 0.0, 0.0])),
                (field::ROOM_BOUNDS_MAX, Value::Vec3([5.0, 1.0, 1.0])),
            ],
        );
        let root = b.add(
            b"AREA",
            vec![
                (field::ARL_NAME, s("Test Area")),
                (field::ARL_ROOMS, structs(&[r0, r1])),
            ],
        );
        let gff = b.finish(root);
        let arl = Arl::from_gff4(&gff).unwrap();
        assert_eq!(arl.name.as_deref(), Some("Test Area"));
        assert_eq!(arl.rooms.len(), 2);
        assert_eq!(arl.rooms[1].name, "lvl_7");
        assert_eq!(arl.bounds(), Some(([-1.0, -2.0, -3.0], [5.0, 2.0, 3.0])));
    }

    #[test]
    fn reads_room_layout() {
        let mut b = Builder::new(b"RML ");
        let lm = b.add(
            b"LMAT",
            vec![
                (field::LMAT_CHUNK, s("_7u")),
                (field::LMAT_TEXTURE, s("lvl_room000.dds")),
                (
                    field::LMAT_OFFSET_SCALE,
                    Value::Vec4([0.5, 0.25, 0.125, 0.0625]),
                ),
            ],
        );
        let m = b.add(
            b"MDL ",
            vec![
                (field::MDL_NAME, s("wall_01")),
                (field::MDL_RESOURCE, s("wall_01_0")),
                (field::MDL_SCALE, Value::F32(2.0)),
                (field::POSITION, Value::Vec3([1.0, 2.0, 3.0])),
                (field::ROTATION, Value::Quat([0.0, 0.0, 1.0, 0.0])),
                (field::MDL_LIGHTMAPS, generic(&[lm])),
            ],
        );
        // A model with only the base name and no lightmaps.
        let m2 = b.add(b"MDL ", vec![(field::MDL_NAME, s("prop"))]);
        let root = b.add(
            b"ROOM",
            vec![
                (field::POSITION, Value::Vec3([10.0, 0.0, 0.0])),
                (field::RML_MODELS, structs(&[m, m2])),
            ],
        );
        let rml = Rml::from_gff4(&b.finish(root)).unwrap();
        assert_eq!(rml.transform.position, [10.0, 0.0, 0.0]);
        assert_eq!(rml.transform.rotation, IDENTITY_QUAT);
        let p = &rml.placements[0];
        assert_eq!(p.model, "wall_01_0");
        assert_eq!(p.name.as_deref(), Some("wall_01"));
        assert_eq!(p.scale, 2.0);
        assert_eq!(p.transform.rotation, [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(
            p.lightmaps,
            vec![LightmapBinding {
                chunk: "_7u".into(),
                texture: "lvl_room000.dds".into(),
                offset: [0.5, 0.25],
                scale: [0.125, 0.0625],
            }]
        );
        assert_eq!(rml.placements[1].model, "prop");
        assert_eq!(rml.placements[1].scale, 1.0);
    }

    #[test]
    fn errors_instead_of_panicking() {
        // Wrong file type.
        let mut b = Builder::new(b"MESH");
        let root = b.add(b"ROOM", vec![]);
        assert!(Rml::from_gff4(&b.finish(root)).is_err());
        // Wrong field type.
        let mut b = Builder::new(b"ARL ");
        let root = b.add(b"AREA", vec![(field::ARL_ROOMS, Value::F32(1.0))]);
        assert!(Arl::from_gff4(&b.finish(root)).is_err());
        // Model with no name at all.
        let mut b = Builder::new(b"RML ");
        let m = b.add(b"MDL ", vec![]);
        let root = b.add(b"ROOM", vec![(field::RML_MODELS, structs(&[m]))]);
        assert!(Rml::from_gff4(&b.finish(root)).is_err());
        // Garbage bytes.
        assert!(Arl::parse(b"not a gff").is_err());
    }
}
