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
    // Morph presets (`.mop`).
    pub const PRESET_NAME: u32 = 23008;
    pub const PARAM_NODE: u32 = 23009;
    pub const PARAM_NAME: u32 = 23010;
    pub const PARAM_INDEX: u32 = 23011;
    pub const PARAM_SCALAR: u32 = 23012;
    pub const PARAM_VECTOR: u32 = 23013;
    pub const SCALARS: u32 = 23014;
    pub const PRESET_VECTORS: u32 = 23015;
    pub const TARGET_NAME: u32 = 23016;
    pub const TARGET_WEIGHT: u32 = 23017;
    pub const TARGETS: u32 = 23018;
    pub const TEXTURE_NODE: u32 = 23019;
    pub const TEXTURE_PARAM: u32 = 23020;
    pub const TEXTURE_NAME: u32 = 23021;
    pub const PRESET_TEXTURES: u32 = 23022;
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

/// A face preset (`.mop`, GFF4 file type `MOP `): the settings character creation's face
/// screen starts from (`chargenmorphcfg.xml` lists them per race and gender). Where a
/// [`Morph`] holds a finished face, a preset holds the choices: the same model and tint
/// lists, material parameters per node (a vector's index into the parameter's list, a
/// scalar's index), texture overrides, and the weights of the face's morph targets
/// (`UH_NOS_30a_0`), which the face screen blends into the head's vertices.
///
/// Field meanings are read from the shipped presets (provisional).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MorphPreset {
    pub name: Option<String>,
    /// Model resources by [`slot`], as in a [`Morph`].
    pub models: Vec<Option<String>>,
    pub tints: Vec<Option<String>>,
    /// (node, parameter, index, value).
    pub scalars: Vec<(String, String, i64, f32)>,
    /// (node, parameter, index into the parameter's vectors, value).
    pub vectors: Vec<(String, String, i64, [f32; 4])>,
    /// (node, parameter, texture resource).
    pub textures: Vec<(String, String, String)>,
    /// Morph target weights: (target, weight).
    pub targets: Vec<(String, f32)>,
}

/// Vector indices are kept below this (a head's parameters hold a handful).
const MAX_PARAM_INDEX: i64 = 64;

impl MorphPreset {
    pub fn parse(data: &[u8]) -> TypedResult<Self> {
        Self::from_gff4(&Gff4::parse(data)?)
    }

    pub fn from_gff4(gff: &Gff4) -> TypedResult<Self> {
        let d = Doc::new(gff, "mop");
        d.expect_type("MOP ")?;
        let root = gff.root;
        let text =
            |id, label| -> TypedResult<String> { Ok(d.opt_str(id, label)?.unwrap_or_default()) };
        let mut scalars = Vec::new();
        for p in d.structs(root, field::SCALARS)? {
            scalars.push((
                text(p, field::PARAM_NODE)?,
                text(p, field::PARAM_NAME)?,
                d.opt_i64(p, field::PARAM_INDEX)?.unwrap_or(0),
                d.opt_f32(p, field::PARAM_SCALAR)?.unwrap_or(0.0),
            ));
        }
        let mut vectors = Vec::new();
        for p in d.structs(root, field::PRESET_VECTORS)? {
            let index = d.opt_i64(p, field::PARAM_INDEX)?.unwrap_or(0);
            if !(0..MAX_PARAM_INDEX).contains(&index) {
                return Err(TypedError::Invalid(format!("mop: vector index {index}")));
            }
            let Some(value) = d.opt_vec4(p, field::PARAM_VECTOR, 1.0)? else {
                continue;
            };
            vectors.push((
                text(p, field::PARAM_NODE)?,
                text(p, field::PARAM_NAME)?,
                index,
                value,
            ));
        }
        let mut textures = Vec::new();
        for t in d.structs(root, field::PRESET_TEXTURES)? {
            textures.push((
                text(t, field::TEXTURE_NODE)?,
                text(t, field::TEXTURE_PARAM)?,
                text(t, field::TEXTURE_NAME)?,
            ));
        }
        let mut targets = Vec::new();
        for t in d.structs(root, field::TARGETS)? {
            targets.push((
                text(t, field::TARGET_NAME)?,
                d.opt_f32(t, field::TARGET_WEIGHT)?.unwrap_or(0.0),
            ));
        }
        Ok(MorphPreset {
            name: d.opt_str(root, field::PRESET_NAME)?,
            models: strings(&d, root, field::MODELS)?,
            tints: strings(&d, root, field::TINTS)?,
            scalars,
            vectors,
            textures,
            targets,
        })
    }
}

impl Morph {
    /// `self` (a finished face, the race and gender's base) with a preset's choices on it:
    /// the preset's models and tints, its vector parameters and its textures. Provisional:
    /// the preset's morph targets (the face's shape) and scalar parameters are not applied,
    /// so the face keeps the base's shape and hair weights.
    pub fn with_preset(&self, preset: &MorphPreset) -> Morph {
        let mut m = self.clone();
        if let Some(n) = &preset.name {
            m.name = Some(n.clone());
        }
        if !preset.models.is_empty() {
            m.models = preset.models.clone();
        }
        if !preset.tints.is_empty() {
            m.tints = preset.tints.clone();
        }
        let node = |m: &mut Morph, name: &str| -> usize {
            match m
                .nodes
                .iter()
                .position(|n| n.name.eq_ignore_ascii_case(name))
            {
                Some(i) => i,
                None => {
                    m.nodes.push(MorphNode {
                        name: name.to_string(),
                        ..Default::default()
                    });
                    m.nodes.len() - 1
                }
            }
        };
        for (n, param, index, value) in &preset.vectors {
            let i = node(&mut m, n);
            let vectors = &mut m.nodes[i].vectors;
            let j = match vectors
                .iter()
                .position(|(p, _)| p.eq_ignore_ascii_case(param))
            {
                Some(j) => j,
                None => {
                    vectors.push((param.clone(), Vec::new()));
                    vectors.len() - 1
                }
            };
            let list = &mut vectors[j].1;
            let index = (*index).clamp(0, MAX_PARAM_INDEX - 1) as usize;
            if list.len() <= index {
                list.resize(index + 1, [0.0; 4]);
            }
            list[index] = *value;
        }
        for (n, param, texture) in &preset.textures {
            let i = node(&mut m, n);
            let textures = &mut m.nodes[i].textures;
            match textures
                .iter_mut()
                .find(|(p, _)| p.eq_ignore_ascii_case(param))
            {
                Some(t) => t.1 = texture.clone(),
                None => textures.push((param.clone(), texture.clone())),
            }
        }
        m
    }
}

/// The faces character creation's face screen offers one race and gender (`human_male`,
/// `elf_female`; `chargenmorphcfg.xml`'s `heads`), in order, as preset names without
/// `.mop` (`hm_cps_p01`).
pub fn face_screen_heads(xml: &[u8], race_gender: &str) -> TypedResult<Vec<String>> {
    let text = std::str::from_utf8(xml)
        .map_err(|e| TypedError::Invalid(format!("chargenmorphcfg: {e}")))?;
    let text = text.trim_start_matches('\u{feff}');
    let doc = roxmltree::Document::parse(text)
        .map_err(|e| TypedError::Invalid(format!("chargenmorphcfg: {e}")))?;
    let Some(heads) = doc
        .root_element()
        .children()
        .find(|n| n.has_tag_name("heads"))
    else {
        return Ok(Vec::new());
    };
    let Some(list) = heads.children().find(|n| n.has_tag_name(race_gender)) else {
        return Ok(Vec::new());
    };
    Ok(list
        .children()
        .filter(|n| n.has_tag_name("resource"))
        .filter_map(|n| n.attribute("name"))
        .map(|n| {
            let n = n.trim().to_ascii_lowercase();
            n.strip_suffix(".mop").map(str::to_string).unwrap_or(n)
        })
        .filter(|n| !n.is_empty())
        .collect())
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

    #[test]
    fn a_preset_puts_its_hair_colours_and_textures_on_the_base_face() {
        let mut b = Builder::new(b"MOP ");
        let colour = b.add(
            b"MAT4",
            vec![
                (field::PARAM_NODE, s("HairM1")),
                (field::PARAM_NAME, s("mml_vTintMaskColours")),
                (field::PARAM_INDEX, Value::I32(2)),
                (field::PARAM_VECTOR, Value::Vec4([0.4, 0.2, 0.1, 1.0])),
            ],
        );
        let tex = b.add(
            b"TEX ",
            vec![
                (field::TEXTURE_NODE, s("FaceM1")),
                (field::TEXTURE_PARAM, s("mml_tDiffuse")),
                (field::TEXTURE_NAME, s("uh_hed_test_0d")),
            ],
        );
        let target = b.add(
            b"MOD ",
            vec![
                (field::TARGET_NAME, s("UH_NOS_30a_0")),
                (field::TARGET_WEIGHT, Value::F32(0.5)),
            ],
        );
        let root = b.add(
            b"MOP ",
            vec![
                (
                    field::MODELS,
                    strings_value(&[Some("HM_UHM_BASa_0"), None, Some("hm_har_hb4a_0")]),
                ),
                (field::TINTS, strings_value(&[Some("t1_skn_001")])),
                (field::PRESET_NAME, s("hm_cps_p01")),
                (field::PRESET_VECTORS, structs(&[colour])),
                (field::PRESET_TEXTURES, structs(&[tex])),
                (field::TARGETS, structs(&[target])),
            ],
        );
        let p = MorphPreset::from_gff4(&b.finish(root)).unwrap();
        assert_eq!(p.name.as_deref(), Some("hm_cps_p01"));
        assert_eq!(p.targets, vec![("UH_NOS_30a_0".to_string(), 0.5)]);
        // On a bald base face: the preset's hair, colour and texture.
        let base = Morph {
            name: Some("hm_pcc_b01".into()),
            models: vec![
                Some("HM_UHM_BASa_0".into()),
                None,
                Some("hm_har_blda_0".into()),
            ],
            tints: vec![],
            nodes: vec![MorphNode {
                name: "FaceM1".into(),
                vertices: vec![[0.0, 0.0, 1.0, 1.0]],
                ..Default::default()
            }],
        };
        let m = base.with_preset(&p);
        assert_eq!(m.model(slot::HAIR), Some("hm_har_hb4a_0"));
        assert_eq!(m.name.as_deref(), Some("hm_cps_p01"));
        let hair = m.node("HairM1").unwrap();
        assert_eq!(hair.vectors[0].1[2], [0.4, 0.2, 0.1, 1.0]);
        let face = m.node("facem1").unwrap();
        assert_eq!(face.vertices.len(), 1, "the base face's shape stays");
        assert_eq!(face.textures[0].1, "uh_hed_test_0d");
    }

    #[test]
    fn the_face_screen_lists_its_heads_per_race_and_gender() {
        let xml = "\u{feff}<?xml version=\"1.0\"?><morph_config><heads><human_male>\
            <resource name=\"HM_CPS_P01.mop\"/><resource name=\"HM_PCC_B01.mop\"/>\
            </human_male></heads><hairs><human_male><resource name=\"hm_har_blda_0\"/>\
            </human_male></hairs></morph_config>";
        assert_eq!(
            face_screen_heads(xml.as_bytes(), "human_male").unwrap(),
            vec!["hm_cps_p01", "hm_pcc_b01"]
        );
        assert!(
            face_screen_heads(xml.as_bytes(), "elf_female")
                .unwrap()
                .is_empty()
        );
        assert!(face_screen_heads(b"<not xml", "human_male").is_err());
    }
}
