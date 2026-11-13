//! `.phy` collision models (GFF4 `PHY `, version `V0.1`): the shapes PhysX collides with and
//! casts rays against.
//!
//! Structure (struct labels; field names as xoreos's MMH ones, which the same ids reuse):
//! `mdlh` → `node`/`mshh` (6000 name; children in 6999) → `cobj` (a collision object) → `shap`
//! (one shape). A shape's pose (6061 position, 6060 rotation) is relative to the `.mmh` node
//! named by its nearest `node`/`mshh` ancestor: the `.phy` itself carries no node transforms.
//! Its geometry is the struct in 6998: a sphere (`sphs`, 6072 radius), a box (`boxs`, 6071
//! **half** extents), a capsule (`caps`, 6072 radius, 6073 height between the hemisphere
//! centres; the axis is local +Z starting at the pose origin) or a triangle mesh (`mshs`, a
//! cooked PhysX 2.x stream in 6077, see [`cooked_mesh`]).
//!
//! DAOrigins.exe's shape loader (0x589dfe-0x58a2af) turns the shape's collision booleans into
//! the engine mask in [`mask`]. Creature perception casts its sight ray with
//! `Placeables | StaticGeometry` ([`mask::SIGHT`]; 0x67c530 -> 0x82deb0).
//!
//! Provisional: the capsule's axis and origin are read from the data (models' bounding boxes),
//! not traced in the executable.

use crate::gff4::{Gff4, StructId};
use crate::level::Transform;
use crate::typed::{Doc, TypedError, TypedResult};

pub mod field {
    pub const NAME: u32 = 6000;
    pub const CHILDREN: u32 = 6999;
    pub const SHAPE_DATA: u32 = 6998;
    pub const SHAPE_TYPE: u32 = 6058;
    pub const ROTATION: u32 = 6060;
    pub const POSITION: u32 = 6061;
    pub const MASK_ALL: u32 = 6063;
    pub const MASK_ITEMS: u32 = 6065;
    pub const MASK_CREATURES: u32 = 6066;
    pub const MASK_PLACEABLES: u32 = 6067;
    pub const MASK_TRIGGERS: u32 = 6068;
    pub const MASK_STATIC: u32 = 6069;
    pub const MASK_NON_WALKABLE: u32 = 6070;
    pub const MASK_WATER: u32 = 6277;
    pub const MASK_TERRAIN_WALL: u32 = 6295;
    pub const MASK_WALKABLE: u32 = 6305;
    pub const FADEABLE: u32 = 6252;
    /// A node's sound material (xoreos: MMHNodeSoundMaterial).
    pub const SOUND_MATERIAL: u32 = 6330;
    pub const BOX_HALF: u32 = 6071;
    pub const RADIUS: u32 = 6072;
    pub const HEIGHT: u32 = 6073;
    pub const COOKED: u32 = 6077;
}

/// The engine's shape mask bits (0x589dfe-0x58a2af).
pub mod mask {
    pub const ITEMS: u32 = 0x1;
    pub const CREATURES: u32 = 0x2;
    pub const PLACEABLES: u32 = 0x4;
    pub const TRIGGERS: u32 = 0x8;
    pub const STATIC: u32 = 0x20;
    pub const WATER: u32 = 0x100;
    /// Static geometry that does not fade (`Fadeable` 0).
    pub const STATIC_SOLID: u32 = 0x2000;
    pub const TERRAIN_WALL: u32 = 0x1_0000;
    pub const NON_WALKABLE: u32 = 0x100_0000;
    pub const WALKABLE: u32 = 0x200_0000;
    /// The sight ray's mask (perception, 0x67c530): placeables and static geometry.
    pub const SIGHT: u32 = PLACEABLES | STATIC;
}

#[derive(Clone, Debug, PartialEq)]
pub enum Geometry {
    Sphere {
        radius: f32,
    },
    /// Centred on the pose.
    Box {
        half: [f32; 3],
    },
    /// A segment from (0, 0, r) to (0, 0, r + height) in shape space, swept by `radius`.
    Capsule {
        radius: f32,
        height: f32,
    },
    Mesh {
        vertices: Vec<[f32; 3]>,
        triangles: Vec<[u32; 3]>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    /// The `.mmh` node the pose is relative to (the nearest `node`/`mshh` ancestor's name).
    pub parent: Option<String>,
    /// The engine mask ([`mask`]).
    pub mask: u32,
    pub pose: Transform,
    pub geometry: Geometry,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Phy {
    pub name: Option<String>,
    pub shapes: Vec<Shape>,
    /// The model's sound material: its root node's (6330), a `sound_geometry_mat_types.gda`
    /// row (0 Unknown, 1 Wood, 5 Stone, ...). DAOrigins.exe gives every part of a model its
    /// root's (0x543e70 walks the parents up and copies the root's byte), and sound
    /// occlusion reads it for the shapes a ray crosses (0x81c4f0 -> 0x5429c0).
    pub sound_material: u8,
}

const MAX_DEPTH: usize = 64;

/// A triangle mesh's vertices and its triangles' vertex indices.
pub type MeshData = (Vec<[f32; 3]>, Vec<[u32; 3]>);

impl Phy {
    pub fn parse(data: &[u8]) -> TypedResult<Self> {
        Self::from_gff4(&Gff4::parse(data)?)
    }

    pub fn from_gff4(gff: &Gff4) -> TypedResult<Self> {
        let d = Doc::new(gff, "phy");
        d.expect_type("PHY ")?;
        let root = gff.root;
        let mut shapes = Vec::new();
        walk(&d, root, None, &mut shapes, 0)?;
        // The root node: the first node under the model header.
        let mut sound_material = 0;
        for c in d.structs(root, field::CHILDREN)? {
            let label = d.label(c).0;
            if &label == b"node" || &label == b"mshh" {
                sound_material = d
                    .opt_i64(c, field::SOUND_MATERIAL)?
                    .unwrap_or(0)
                    .clamp(0, 255) as u8;
                break;
            }
        }
        Ok(Phy {
            name: d.opt_str(root, field::NAME)?,
            shapes,
            sound_material,
        })
    }
}

fn walk(
    d: &Doc,
    id: StructId,
    parent: Option<&str>,
    out: &mut Vec<Shape>,
    depth: usize,
) -> TypedResult<()> {
    if depth > MAX_DEPTH {
        return Err(TypedError::Invalid("phy: nodes nested too deep".into()));
    }
    let label = d.label(id).0;
    if &label == b"cobj" {
        for s in d.structs(id, field::CHILDREN)? {
            if &d.label(s).0 == b"shap" {
                if let Some(shape) = read_shape(d, s, parent)? {
                    out.push(shape);
                }
            }
        }
        return Ok(());
    }
    let name = if &label == b"node" || &label == b"mshh" {
        d.opt_str(id, field::NAME)?
    } else {
        parent.map(str::to_string)
    };
    for c in d.structs(id, field::CHILDREN)? {
        walk(d, c, name.as_deref(), out, depth + 1)?;
    }
    Ok(())
}

fn flag(d: &Doc, id: StructId, label: u32) -> TypedResult<bool> {
    Ok(d.opt_i64(id, label)?.unwrap_or(0) != 0)
}

/// The engine mask of a shape, as the loader builds it (0x589dfe-0x58a2af).
fn shape_mask(d: &Doc, s: StructId) -> TypedResult<u32> {
    let mut m = if flag(d, s, field::MASK_ALL)? {
        u32::MAX
    } else {
        0
    };
    for (label, bit) in [
        (field::MASK_ITEMS, mask::ITEMS),
        (field::MASK_CREATURES, mask::CREATURES),
        (field::MASK_PLACEABLES, mask::PLACEABLES),
        (field::MASK_TRIGGERS, mask::TRIGGERS),
        (field::MASK_STATIC, mask::STATIC),
        (field::MASK_WATER, mask::WATER),
        (field::MASK_TERRAIN_WALL, mask::TERRAIN_WALL),
        (field::MASK_NON_WALKABLE, mask::NON_WALKABLE),
        (field::MASK_WALKABLE, mask::WALKABLE),
    ] {
        if flag(d, s, label)? {
            m |= bit;
        }
    }
    if m & mask::TERRAIN_WALL != 0 {
        m |= 0x8000_0000;
    }
    if m & mask::STATIC != 0 && !flag(d, s, field::FADEABLE)? {
        m |= mask::STATIC_SOLID;
    }
    Ok(m)
}

fn read_shape(d: &Doc, s: StructId, parent: Option<&str>) -> TypedResult<Option<Shape>> {
    let kind = d.opt_i64(s, field::SHAPE_TYPE)?.unwrap_or(0);
    let Some(data) = d.opt_struct(s, field::SHAPE_DATA)? else {
        return Ok(None);
    };
    let positive = |v: f32| v.is_finite() && v >= 0.0;
    let geometry = match kind {
        1 => {
            let radius = d.opt_f32(data, field::RADIUS)?.unwrap_or(0.0);
            if !positive(radius) {
                return Ok(None);
            }
            Geometry::Sphere { radius }
        }
        2 => {
            let half = d.opt_vec3(data, field::BOX_HALF)?.unwrap_or([0.0; 3]);
            if !half.iter().all(|&h| positive(h)) {
                return Ok(None);
            }
            Geometry::Box { half }
        }
        3 => {
            let radius = d.opt_f32(data, field::RADIUS)?.unwrap_or(0.0);
            let height = d.opt_f32(data, field::HEIGHT)?.unwrap_or(0.0);
            if !positive(radius) || !positive(height) {
                return Ok(None);
            }
            Geometry::Capsule { radius, height }
        }
        5 => {
            let (vertices, triangles) = cooked_mesh(d.bytes(data, field::COOKED)?)?;
            Geometry::Mesh {
                vertices,
                triangles,
            }
        }
        // Convex shapes (4) do not occur in the shipped data.
        _ => return Ok(None),
    };
    let position = d.opt_vec3(s, field::POSITION)?.unwrap_or([0.0; 3]);
    let rotation = d
        .opt_vec4(s, field::ROTATION, 1.0)?
        .unwrap_or([0.0, 0.0, 0.0, 1.0]);
    if !position
        .iter()
        .chain(rotation.iter())
        .all(|v| v.is_finite())
    {
        return Ok(None);
    }
    Ok(Some(Shape {
        parent: parent.map(str::to_string),
        mask: shape_mask(d, s)?,
        pose: Transform { position, rotation },
        geometry,
    }))
}

/// The vertices and triangles of a cooked PhysX 2.x triangle mesh (`NXS\x01` `MESH`):
/// a 36-byte header (version, serial flags, convex edge threshold, height-field axis and
/// extent, vertex count, triangle count), the vertices as f32 triples, then the indices,
/// as bytes (flag 0x08), u16s (0x10) or u32s. What follows (remap table, acceleration
/// structures) is not needed and not read.
pub fn cooked_mesh(b: &[u8]) -> TypedResult<MeshData> {
    let bad = |m: &str| TypedError::Invalid(format!("phy cooked mesh: {m}"));
    if b.len() < 36 || &b[0..4] != b"NXS\x01" || &b[4..8] != b"MESH" {
        return Err(bad("not a cooked mesh"));
    }
    let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let flags = u32_at(12);
    let nv = u32_at(28) as usize;
    let nt = u32_at(32) as usize;
    let width = if flags & 0x08 != 0 {
        1
    } else if flags & 0x10 != 0 {
        2
    } else {
        4
    };
    let vert_end = nv
        .checked_mul(12)
        .and_then(|n| n.checked_add(36))
        .ok_or_else(|| bad("vertex count overflows"))?;
    let tri_end = nt
        .checked_mul(3 * width)
        .and_then(|n| n.checked_add(vert_end))
        .ok_or_else(|| bad("triangle count overflows"))?;
    if tri_end > b.len() {
        return Err(bad("truncated"));
    }
    let f = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let vertices: Vec<[f32; 3]> = (0..nv)
        .map(|i| {
            let o = 36 + 12 * i;
            [f(o), f(o + 4), f(o + 8)]
        })
        .collect();
    let index = |i: usize| -> u32 {
        let o = vert_end + i * width;
        match width {
            1 => b[o] as u32,
            2 => u16::from_le_bytes([b[o], b[o + 1]]) as u32,
            _ => u32_at(o),
        }
    };
    let mut triangles = Vec::with_capacity(nt);
    for t in 0..nt {
        let tri = [index(3 * t), index(3 * t + 1), index(3 * t + 2)];
        if tri.iter().any(|&i| i as usize >= nv) {
            return Err(bad("index out of range"));
        }
        triangles.push(tri);
    }
    Ok((vertices, triangles))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gff4::Value;
    use crate::typed::testutil::{Builder, generic, s};

    fn mesh_blob() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"NXS\x01MESH");
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&0x0Au32.to_le_bytes()); // u8 indices
        b.extend_from_slice(&0.001f32.to_le_bytes());
        b.extend_from_slice(&0xFFu32.to_le_bytes());
        b.extend_from_slice(&0f32.to_le_bytes());
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        for v in [[0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for c in v {
                b.extend_from_slice(&c.to_le_bytes());
            }
        }
        b.extend_from_slice(&[0, 1, 2]);
        b.extend_from_slice(&[9, 9, 9, 9]); // trailing data that is not read
        b
    }

    #[test]
    fn cooked_meshes_read_their_vertices_and_triangles() {
        let (v, t) = cooked_mesh(&mesh_blob()).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[1], [1.0, 0.0, 0.0]);
        assert_eq!(t, vec![[0, 1, 2]]);
        let mut short = mesh_blob();
        short.truncate(40);
        assert!(cooked_mesh(&short).is_err());
        let mut out_of_range = mesh_blob();
        let at = 36 + 36;
        out_of_range[at] = 7;
        assert!(cooked_mesh(&out_of_range).is_err());
    }

    #[test]
    fn shapes_keep_their_node_mask_and_geometry() {
        let mut b = Builder::new(b"PHY ");
        let boxs = b.add(
            b"boxs",
            vec![(field::BOX_HALF, Value::Vec3([1.0, 2.0, 3.0]))],
        );
        let caps = b.add(
            b"caps",
            vec![
                (field::RADIUS, Value::F32(0.5)),
                (field::HEIGHT, Value::F32(1.0)),
            ],
        );
        let mshs = b.add(
            b"mshs",
            vec![(
                field::COOKED,
                Value::List(Some(crate::gff4::List::U8(mesh_blob()))),
            )],
        );
        let shap = |b: &mut Builder, data, kind: u32, static_: u8, creatures: u8| {
            b.add(
                b"shap",
                vec![
                    (field::SHAPE_TYPE, Value::U32(kind)),
                    (field::POSITION, Value::Vec4([0.0, 0.0, 1.0, 1.0])),
                    (field::ROTATION, Value::Quat([0.0, 0.0, 0.0, 1.0])),
                    (field::MASK_STATIC, Value::U8(static_)),
                    (field::MASK_CREATURES, Value::U8(creatures)),
                    (field::FADEABLE, Value::U8(0)),
                    (field::SHAPE_DATA, Value::Struct(Some(data))),
                ],
            )
        };
        let s1 = shap(&mut b, boxs, 2, 1, 0);
        let s2 = shap(&mut b, caps, 3, 0, 1);
        let s3 = shap(&mut b, mshs, 5, 1, 0);
        let cobj = b.add(b"cobj", vec![(field::CHILDREN, generic(&[s1, s2, s3]))]);
        let node = b.add(
            b"node",
            vec![
                (field::NAME, s("Root")),
                (field::SOUND_MATERIAL, Value::U8(5)),
                (field::CHILDREN, generic(&[cobj])),
            ],
        );
        let root = b.add(
            b"mdlh",
            vec![
                (field::NAME, s("plc_test_0")),
                (field::CHILDREN, generic(&[node])),
            ],
        );
        let phy = Phy::from_gff4(&b.finish(root)).unwrap();
        assert_eq!(phy.name.as_deref(), Some("plc_test_0"));
        assert_eq!(phy.shapes.len(), 3);
        assert_eq!(phy.sound_material, 5);
        assert_eq!(phy.shapes[0].parent.as_deref(), Some("Root"));
        assert_eq!(
            phy.shapes[0].geometry,
            Geometry::Box {
                half: [1.0, 2.0, 3.0]
            }
        );
        assert_eq!(phy.shapes[0].mask, mask::STATIC | mask::STATIC_SOLID);
        assert_ne!(phy.shapes[0].mask & mask::SIGHT, 0);
        assert_eq!(
            phy.shapes[1].mask & mask::SIGHT,
            0,
            "creature shapes do not block sight"
        );
        assert!(
            matches!(phy.shapes[2].geometry, Geometry::Mesh { ref triangles, .. } if triangles.len() == 1)
        );
        assert_eq!(phy.shapes[0].pose.position, [0.0, 0.0, 1.0]);
    }
}
