//! Head morphs (`.mor`, GFF4 file type `MORP`): the face a character was given in the
//! toolset or character creation. A morph names the head's models (head, eyes, hair, beard,
//! lashes), tint textures, and per node of those models: an offset for face bones, the
//! morphed vertex positions for the face meshes, and material overrides (textures and vector
//! parameters such as `mml_vTintMaskColours`).
//!
//! Field meanings are read from the shipped files (provisional where noted): the model list's
//! slots are named by what they hold in every file seen.

use crate::gff4::{Gff4, List, StructId, Value};
use crate::typed::{Doc, TypedError, TypedResult};

mod field {
    pub const NAME: u32 = 2;
    pub const OFFSET: u32 = 4;
    pub const VALUES: u32 = 17;
    pub const VERTEX_COUNT: u32 = 8013;
    pub const VERTICES: u32 = 8024;
    pub const MODELS: u32 = 23000;
    pub const TINTS: u32 = 23001;
    pub const NODES: u32 = 23002;
    pub const TEXTURE_VALUE: u32 = 23003;
    pub const TEXTURES: u32 = 23004;
    pub const VECTORS: u32 = 23005;
}

/// The model list's slots, in file order (provisional: named from the files' contents).
pub mod slot {
    pub const HEAD: usize = 0;
    pub const EYES: usize = 1;
    pub const HAIR: usize = 2;
    pub const BEARD: usize = 3;
    pub const LASHES: usize = 5;
}

/// One node's data: a face bone's offset, or a face mesh's vertices and material overrides.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MorphNode {
    /// The bone or mesh node's name (`cheek_left`, `FaceM1`).
    pub name: String,
    pub offset: [f32; 3],
    /// Morphed vertex positions in model space, one per vertex of the mesh's chunk; empty
    /// for nodes that keep the model's own.
    pub vertices: Vec<[f32; 4]>,
    /// Texture overrides: material parameter, texture resource (no extension).
    pub textures: Vec<(String, String)>,
    /// Vector parameter overrides: name, values.
    pub vectors: Vec<(String, Vec<[f32; 4]>)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Morph {
    pub name: Option<String>,
    /// Model resources (no extension) by [`slot`]; `None` for an empty slot.
    pub models: Vec<Option<String>>,
    /// Tint texture names; `None` for an empty slot.
    pub tints: Vec<Option<String>>,
    pub nodes: Vec<MorphNode>,
}

/// Vertices are kept to this many, far above any head mesh in the game.
const MAX_VERTICES: usize = 1 << 20;

impl Morph {
    pub fn parse(data: &[u8]) -> TypedResult<Self> {
        Self::from_gff4(&Gff4::parse(data)?)
    }

    pub fn from_gff4(gff: &Gff4) -> TypedResult<Self> {
        let d = Doc::new(gff, "mor");
        d.expect_type("MORP")?;
        let root = gff.root;
        let mut nodes = Vec::new();
        for n in d.structs(root, field::NODES)? {
            nodes.push(read_node(&d, n)?);
        }
        Ok(Morph {
            name: d.opt_str(root, field::NAME)?,
            models: strings(&d, root, field::MODELS)?,
            tints: strings(&d, root, field::TINTS)?,
            nodes,
        })
    }

    /// The model in a [`slot`], if any.
    pub fn model(&self, slot: usize) -> Option<&str> {
        self.models.get(slot)?.as_deref()
    }

    /// The node with this name, ignoring case.
    pub fn node(&self, name: &str) -> Option<&MorphNode> {
        self.nodes
            .iter()
            .find(|n| n.name.eq_ignore_ascii_case(name))
    }
}

/// A list of strings; empty strings read as `None`.
fn strings(d: &Doc, id: StructId, label: u32) -> TypedResult<Vec<Option<String>>> {
    match d.get(id, label) {
        None | Some(Value::List(None)) => Ok(Vec::new()),
        Some(Value::List(Some(List::String(v)))) => Ok(v
            .iter()
            .map(|s| {
                s.as_deref()
                    .map(crate::typed::clean)
                    .filter(|s| !s.is_empty())
            })
            .collect()),
        Some(v) => Err(TypedError::WrongType {
            context: d.context,
            label,
            found: crate::typed::value_kind(v),
        }),
    }
}

/// A list of four-float vectors, stored either as vectors or as flat floats.
fn vec4s(d: &Doc, id: StructId, label: u32) -> TypedResult<Vec<[f32; 4]>> {
    match d.get(id, label) {
        Some(Value::List(Some(List::Vec4(v) | List::Quat(v) | List::Color(v)))) => Ok(v.clone()),
        Some(Value::List(Some(List::Vec3(v)))) => {
            Ok(v.iter().map(|&[x, y, z]| [x, y, z, 1.0]).collect())
        }
        _ => {
            let flat = d.numbers(id, label)?;
            if flat.len() % 4 != 0 {
                return Err(TypedError::Invalid(format!(
                    "{}: field {label} holds {} floats, not a multiple of 4",
                    d.context,
                    flat.len()
                )));
            }
            Ok(flat
                .chunks_exact(4)
                .map(|c| [c[0], c[1], c[2], c[3]])
                .collect())
        }
    }
}

fn read_node(d: &Doc, n: StructId) -> TypedResult<MorphNode> {
    let vertices = vec4s(d, n, field::VERTICES)?;
    if vertices.len() > MAX_VERTICES {
        return Err(TypedError::Invalid(format!(
            "mor: {} vertices",
            vertices.len()
        )));
    }
    if let Some(count) = d.opt_i64(n, field::VERTEX_COUNT)? {
        if !vertices.is_empty() && count != vertices.len() as i64 {
            return Err(TypedError::Invalid(format!(
                "mor: node declares {count} vertices but holds {}",
                vertices.len()
            )));
        }
    }
    let mut textures = Vec::new();
    for t in d.structs(n, field::TEXTURES)? {
        if let (Some(name), Some(value)) = (
            d.opt_str(t, field::NAME)?,
            d.opt_str(t, field::TEXTURE_VALUE)?,
        ) {
            textures.push((name, value));
        }
    }
    let mut vectors = Vec::new();
    for v in d.structs(n, field::VECTORS)? {
        if let Some(name) = d.opt_str(v, field::NAME)? {
            vectors.push((name, vec4s(d, v, field::VALUES)?));
        }
    }
    Ok(MorphNode {
        name: d.opt_str(n, field::NAME)?.unwrap_or_default(),
        offset: d.opt_vec3(n, field::OFFSET)?.unwrap_or_default(),
        vertices,
        textures,
        vectors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gff4::Value;
    use crate::typed::testutil::{Builder, s, structs};

    fn strings_value(v: &[Option<&str>]) -> Value {
        Value::List(Some(List::String(
            v.iter().map(|s| s.map(|s| format!("{s}\0"))).collect(),
        )))
    }

    #[test]
    fn reads_models_vertices_and_overrides() {
        let mut b = Builder::new(b"MORP");
        let tex = b.add(
            b"tprm",
            vec![
                (field::NAME, s("mml_tDiffuse")),
                (field::TEXTURE_VALUE, s("uh_hed_masa_0d")),
            ],
        );
        let vec = b.add(
            b"vprm",
            vec![
                (field::NAME, s("mml_vTintMaskColours")),
                (
                    field::VALUES,
                    Value::List(Some(List::Vec4(vec![[1.0, 0.5, 0.25, 1.0]]))),
                ),
            ],
        );
        let face = b.add(
            b"node",
            vec![
                (field::NAME, s("FaceM1")),
                (field::OFFSET, Value::Vec3([0.0; 3])),
                (field::VERTEX_COUNT, Value::U32(2)),
                (
                    field::VERTICES,
                    Value::List(Some(List::F32(vec![
                        0.0, 0.1, 1.5, 1.0, 0.0, 0.2, 1.7, 1.0,
                    ]))),
                ),
                (field::TEXTURES, structs(&[tex])),
                (field::VECTORS, structs(&[vec])),
            ],
        );
        let bone = b.add(
            b"node",
            vec![
                (field::NAME, s("cheek_left")),
                (field::OFFSET, Value::Vec3([0.001, 0.0, -0.002])),
            ],
        );
        let root = b.add(
            b"morp",
            vec![
                (field::NAME, s("hm_test")),
                (
                    field::MODELS,
                    strings_value(&[Some("HM_UHM_BASa_0"), Some("hm_uem_basa_0"), None]),
                ),
                (field::TINTS, strings_value(&[Some("t1_skn_001")])),
                (field::NODES, structs(&[face, bone])),
            ],
        );
        let m = Morph::from_gff4(&b.finish(root)).unwrap();
        assert_eq!(m.name.as_deref(), Some("hm_test"));
        assert_eq!(m.model(slot::HEAD), Some("HM_UHM_BASa_0"));
        assert_eq!(m.model(slot::HAIR), None);
        assert_eq!(m.model(slot::LASHES), None);
        let face = m.node("facem1").unwrap();
        assert_eq!(
            face.vertices,
            vec![[0.0, 0.1, 1.5, 1.0], [0.0, 0.2, 1.7, 1.0]]
        );
        assert_eq!(face.textures[0].1, "uh_hed_masa_0d");
        assert_eq!(face.vectors[0].1, vec![[1.0, 0.5, 0.25, 1.0]]);
        assert_eq!(m.node("cheek_left").unwrap().offset, [0.001, 0.0, -0.002]);
    }

    #[test]
    fn rejects_bad_vertex_data() {
        let mut b = Builder::new(b"MORP");
        let bad = b.add(
            b"node",
            vec![
                (field::NAME, s("FaceM1")),
                (field::VERTEX_COUNT, Value::U32(5)),
                (field::VERTICES, Value::List(Some(List::F32(vec![0.0; 8])))),
            ],
        );
        let root = b.add(b"morp", vec![(field::NODES, structs(&[bad]))]);
        assert!(Morph::from_gff4(&b.finish(root)).is_err());

        let mut b = Builder::new(b"MORP");
        let ragged = b.add(
            b"node",
            vec![(field::VERTICES, Value::List(Some(List::F32(vec![0.0; 6]))))],
        );
        let root = b.add(b"morp", vec![(field::NODES, structs(&[ragged]))]);
        assert!(Morph::from_gff4(&b.finish(root)).is_err());

        let mut b = Builder::new(b"MMH ");
        let root = b.add(b"mdlh", vec![]);
        assert!(
            Morph::from_gff4(&b.finish(root)).is_err(),
            "wrong file type"
        );
        assert!(Morph::parse(b"not a gff").is_err());
    }
}
