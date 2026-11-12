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
/// lists, material parameters per node (a vector's index into the parameter's list of
/// four-float vectors; a scalar's index counts the list's components, four to a vector:
/// `g_vFacialHairWeights` 1 is the first vector's `y`, as the shipped morphs made from the
/// presets hold them), texture overrides, and the weights of the face's morph targets
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
        if let Some(s) = scalars
            .iter()
            .find(|s| !(0..MAX_PARAM_INDEX * 4).contains(&s.2))
        {
            return Err(TypedError::Invalid(format!("mop: scalar index {}", s.2)));
        }
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
    /// the preset's models and tints, its vector and scalar parameters (vectors first, then
    /// the scalars' single components; provisional order) and its textures. The preset's
    /// morph targets reshape the meshes separately ([`blend_targets`]), as they need the
    /// models' vertices.
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
        for (n, param, index, value) in &preset.scalars {
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
            let index = (*index).clamp(0, MAX_PARAM_INDEX * 4 - 1) as usize;
            if list.len() <= index / 4 {
                list.resize(index / 4 + 1, [0.0; 4]);
            }
            list[index / 4][index % 4] = *value;
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

/// One morph target's mesh for a face node (`UH_NOS_30a_0`'s `FaceM1`): its weight in the
/// preset and its vertex positions, triangle list and first texture coordinates.
#[derive(Clone, Copy, Debug)]
pub struct TargetMesh<'a> {
    pub weight: f32,
    pub positions: &'a [[f32; 4]],
    pub indices: &'a [u32],
    pub uvs: &'a [[f32; 4]],
}

/// A face mesh's vertices with a preset's morph targets blended in: `base + Σ wᵢ · tᵢ`.
///
/// A preset's weights for one mesh sum to zero: each family's neutral target
/// (`UH_UHM_BASa_0`, `UH_UEM_BASa_0`, `UH_ULM_BASa_0`) carries minus the sum of the others,
/// so the blend is the base plus each target's offset from the neutral, weighted. Read from
/// the shipped presets: blending them this way reproduces the shipped morphs made from them
/// (`hm_pcc_b02`–`b04`, `hf_pcc_b02`, `em_pcc_b02`: the faces within a millimetre at the
/// median, a centimetre at worst). Provisional: how DAOrigins.exe blends them is not traced.
///
/// The target meshes have their own vertex order but the base's triangles, so a vertex is
/// matched through the triangle corners it appears at (the corner whose texture coordinate
/// is nearest the base vertex's, where seams split it), and copies of a vertex at one
/// position move together. `None` when no target has the base's triangle list.
pub fn blend_targets(
    base: &[[f32; 4]],
    indices: &[u32],
    uvs: &[[f32; 4]],
    targets: &[TargetMesh],
) -> Option<Vec<[f32; 4]>> {
    let mut out = base.to_vec();
    let mut any = false;
    for t in targets {
        if t.indices.len() != indices.len() || t.weight == 0.0 {
            continue;
        }
        let uv_gap = |b: usize, v: usize| -> f32 {
            match (uvs.get(b), t.uvs.get(v)) {
                (Some(a), Some(c)) => (a[0] - c[0]).abs() + (a[1] - c[1]).abs(),
                _ => 0.0,
            }
        };
        let mut map: Vec<Option<usize>> = vec![None; base.len()];
        for (&b, &v) in indices.iter().zip(t.indices) {
            let (b, v) = (b as usize, v as usize);
            if b >= base.len() || v >= t.positions.len() {
                continue;
            }
            match map[b] {
                Some(old) if old == v || uv_gap(b, old) <= uv_gap(b, v) => {}
                _ => map[b] = Some(v),
            }
        }
        for (o, m) in out.iter_mut().zip(&map) {
            if let Some(v) = m {
                let q = t.positions[*v];
                for k in 0..3 {
                    o[k] += t.weight * q[k];
                }
            }
        }
        any = true;
    }
    if !any {
        return None;
    }
    // Copies of one vertex (split at a seam) move by their mean, so the seam stays shut.
    let key = |p: &[f32; 4]| {
        (
            (p[0] * 1e5).round() as i64,
            (p[1] * 1e5).round() as i64,
            (p[2] * 1e5).round() as i64,
        )
    };
    /// The base vertices at one position, and their offsets' sum.
    type Group = (Vec<usize>, [f32; 3]);
    let mut groups: std::collections::HashMap<(i64, i64, i64), Group> =
        std::collections::HashMap::new();
    for (i, p) in base.iter().enumerate() {
        let g = groups.entry(key(p)).or_default();
        g.0.push(i);
        for k in 0..3 {
            g.1[k] += out[i][k] - p[k];
        }
    }
    for (ids, sum) in groups.into_values() {
        let n = ids.len() as f32;
        for i in ids {
            for k in 0..3 {
                out[i][k] = base[i][k] + sum[k] / n;
            }
        }
    }
    Some(out)
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

/// One of `chargenmorphcfg.xml`'s lists (`hairs`, `hair_colors`, `tattoos`, ...), in order:
/// for a list kept per race and gender (`heads`, `hairs`, `beards`) the `race_gender` one
/// (`human_male`), else the list itself. Entries are resource names, lowercase, without
/// `.mop`; an empty name (the "none" choice, as `beards` starts with) is kept as "".
pub fn face_config_list(xml: &[u8], list: &str, race_gender: &str) -> TypedResult<Vec<String>> {
    let text = std::str::from_utf8(xml)
        .map_err(|e| TypedError::Invalid(format!("chargenmorphcfg: {e}")))?;
    let text = text.trim_start_matches('\u{feff}');
    let doc = roxmltree::Document::parse(text)
        .map_err(|e| TypedError::Invalid(format!("chargenmorphcfg: {e}")))?;
    let Some(node) = doc.root_element().children().find(|n| n.has_tag_name(list)) else {
        return Ok(Vec::new());
    };
    let node = node
        .children()
        .find(|n| n.has_tag_name(race_gender))
        .unwrap_or(node);
    Ok(node
        .children()
        .filter(|n| n.has_tag_name("resource"))
        .map(|n| {
            let n = n
                .attribute("name")
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            n.strip_suffix(".mop").map(str::to_string).unwrap_or(n)
        })
        .collect())
}

/// Choices made over a face (character creation's appearance sliders): model and tint
/// slots replaced. Carried in a morph's name after its base (`hm_cps_p01|m2=hm_har_ha1a_0|
/// t3=t3_har_bln`), so a creature's face stays one string from the game to the renderer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FaceOverlay {
    /// (model [`slot`], model or `None` for none).
    pub models: Vec<(usize, Option<String>)>,
    /// (tint slot, tint or `None` for none).
    pub tints: Vec<(usize, Option<String>)>,
}

/// The material parameter tints fill.
const TINT_PARAMETER: &str = "mml_vTintMaskColours";

/// Slots past this are ignored (a morph holds a dozen tints).
const MAX_OVERLAY_SLOT: usize = 32;

impl FaceOverlay {
    pub fn is_empty(&self) -> bool {
        self.models.is_empty() && self.tints.is_empty()
    }

    /// `base` with the overlay after it (just `base` when empty).
    pub fn name(&self, base: &str) -> String {
        let mut s = base.to_string();
        for (k, list) in [('m', &self.models), ('t', &self.tints)] {
            for (slot, v) in list {
                s.push_str(&format!("|{k}{slot}={}", v.as_deref().unwrap_or("")));
            }
        }
        s
    }

    /// Splits a morph name into its base and overlay; parts that do not read are skipped.
    pub fn split(name: &str) -> (&str, FaceOverlay) {
        let mut parts = name.split('|');
        let base = parts.next().unwrap_or("");
        let mut o = FaceOverlay::default();
        for p in parts {
            let Some((key, value)) = p.split_once('=') else {
                continue;
            };
            let mut chars = key.chars();
            let kind = chars.next();
            let Ok(slot) = chars.as_str().parse::<usize>() else {
                continue;
            };
            if slot >= MAX_OVERLAY_SLOT {
                continue;
            }
            let v = Some(value.trim().to_ascii_lowercase()).filter(|v| !v.is_empty());
            match kind {
                Some('m') => o.models.push((slot, v)),
                Some('t') => o.tints.push((slot, v)),
                _ => {}
            }
        }
        (base, o)
    }
}

/// Where a face's tint slot puts its colours: on the nodes named with these prefixes, the
/// `mml_vTintMaskColours` entries (in the tint's shader order, [`crate::tint::FIELD_ORDER`])
/// it fills. Read from the exe's appearance setters (which slot each choice sets: lips 1,
/// eyes 2, hair 3, eye shadow 4, blush 5, skin 0) and from the face presets, whose baked
/// colours match their tints this way: the face takes its red pair from the lips, green from
/// the eye shadow, blue from the blush and skin from the skin tone; eyes and hair take their
/// three colour pairs from their own tint and skin from the skin tone. Provisional: the
/// exe's own composition step (its morph rebuild job) is not traced; the opacity entries (0,
/// 1), set by the intensity sliders, are left as they are.
pub fn tint_slot_targets(slot: usize) -> &'static [(&'static str, &'static [usize])] {
    const COLOURS: &[usize] = &[2, 3, 4, 5, 6, 7];
    const SKIN: &[usize] = &[8, 9];
    match slot {
        0 => &[
            ("FaceM", SKIN),
            ("EyesM", SKIN),
            ("HairM", SKIN),
            ("BeardM", SKIN),
        ],
        1 => &[("FaceM", &[2, 3])],
        2 => &[("EyesM", COLOURS)],
        3 => &[("HairM", COLOURS), ("BeardM", COLOURS)],
        4 => &[("FaceM", &[4, 5])],
        5 => &[("FaceM", &[6, 7])],
        _ => &[],
    }
}

/// The nodes a tint reaches when the face has none of them yet (a hair model the face's
/// preset did not have): the head models' usual mesh nodes.
const TINT_NODES: &[&str] = &[
    "FaceM1", "EyesM1", "HairM1", "HairM2", "HairM3", "BeardM1", "BeardM2", "BeardM3",
];

impl Morph {
    /// `self` with tint slot `slot`'s colours put on its nodes (see [`tint_slot_targets`]).
    pub fn with_tint(&self, slot: usize, tint: &crate::tint::Tint) -> Morph {
        let mut m = self.clone();
        for (prefix, entries) in tint_slot_targets(slot) {
            let starts =
                |n: &str| n.len() >= prefix.len() && n[..prefix.len()].eq_ignore_ascii_case(prefix);
            for name in TINT_NODES.iter().filter(|n| starts(n)) {
                if !m.nodes.iter().any(|n| n.name.eq_ignore_ascii_case(name)) {
                    m.nodes.push(MorphNode {
                        name: name.to_string(),
                        ..Default::default()
                    });
                }
            }
            for node in m.nodes.iter_mut().filter(|n| starts(&n.name)) {
                let j = match node
                    .vectors
                    .iter()
                    .position(|(p, _)| p.eq_ignore_ascii_case(TINT_PARAMETER))
                {
                    Some(j) => j,
                    None => {
                        // Untouched entries keep the tint's own (as an item tint fills all).
                        node.vectors
                            .push((TINT_PARAMETER.to_string(), tint.colours.to_vec()));
                        node.vectors.len() - 1
                    }
                };
                let list = &mut node.vectors[j].1;
                if list.len() < tint.colours.len() {
                    list.resize(tint.colours.len(), [1.0; 4]);
                }
                for &e in *entries {
                    list[e] = tint.colours[e];
                }
            }
        }
        m
    }

    /// `self` with an overlay's model and tint slots put in.
    pub fn with_overlay(&self, o: &FaceOverlay) -> Morph {
        let mut m = self.clone();
        for (list, slots) in [(&mut m.models, &o.models), (&mut m.tints, &o.tints)] {
            for (slot, v) in slots {
                if list.len() <= *slot {
                    list.resize(slot + 1, None);
                }
                list[*slot] = v.clone();
            }
        }
        m
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

    #[test]
    fn a_face_overlay_rides_in_the_morph_name() {
        let o = FaceOverlay {
            models: vec![(2, Some("hm_har_ha1a_0".into())), (3, None)],
            tints: vec![(3, Some("t3_har_bln".into()))],
        };
        let name = o.name("hm_cps_p01");
        assert_eq!(name, "hm_cps_p01|m2=hm_har_ha1a_0|m3=|t3=t3_har_bln");
        let (base, back) = FaceOverlay::split(&name);
        assert_eq!((base, &back), ("hm_cps_p01", &o));
        assert_eq!(
            FaceOverlay::split("hm_cps_p01"),
            ("hm_cps_p01", FaceOverlay::default())
        );
        // Bad parts are skipped.
        let (_, bad) = FaceOverlay::split("x|q1=a|m=b|m99=c|t1");
        assert!(bad.is_empty());
        let m = Morph {
            models: vec![
                Some("head".into()),
                None,
                Some("hair".into()),
                Some("beard".into()),
            ],
            tints: vec![Some("skin".into())],
            ..Default::default()
        };
        let m = m.with_overlay(&o);
        assert_eq!(m.model(slot::HAIR), Some("hm_har_ha1a_0"));
        assert_eq!(m.model(slot::BEARD), None);
        assert_eq!(m.tints[3].as_deref(), Some("t3_har_bln"));
        assert_eq!(m.tints[0].as_deref(), Some("skin"));
    }

    #[test]
    fn a_tint_slot_colours_its_nodes_entries() {
        let mut t = crate::tint::Tint {
            colours: [[0.0; 4]; crate::tint::COLOURS],
        };
        for (i, c) in t.colours.iter_mut().enumerate() {
            *c = [i as f32; 4];
        }
        let m = Morph {
            nodes: vec![MorphNode {
                name: "HairM1".into(),
                vectors: vec![("mml_vTintMaskColours".into(), vec![[9.0; 4]; 10])],
                ..Default::default()
            }],
            ..Default::default()
        };
        // Hair: its colour pairs, not its opacity or skin entries.
        let h = m.with_tint(3, &t);
        let v = &h.node("HairM1").unwrap().vectors[0].1;
        assert_eq!(v[0], [9.0; 4]);
        assert_eq!(v[2], [2.0; 4]);
        assert_eq!(v[7], [7.0; 4]);
        assert_eq!(v[8], [9.0; 4]);
        // A hair node the face lacked gets the whole tint.
        assert_eq!(h.node("HairM2").unwrap().vectors[0].1[0], [0.0; 4]);
        // Skin: the skin pair of every head mesh.
        let s = m.with_tint(0, &t);
        assert_eq!(s.node("HairM1").unwrap().vectors[0].1[8], [8.0; 4]);
        assert_eq!(s.node("HairM1").unwrap().vectors[0].1[2], [9.0; 4]);
        // Lips: the face's red pair only.
        let l = m.with_tint(1, &t);
        assert_eq!(l.node("FaceM1").unwrap().vectors[0].1[2], [2.0; 4]);
        assert!(tint_slot_targets(11).is_empty());
    }

    #[test]
    fn the_face_config_lists_read_per_race_and_gender_or_whole() {
        let xml = br#"<?xml version="1.0"?><morph_config>
            <beards><human_male><resource name=""/><resource name="HM_BRD_B1A_0"/></human_male></beards>
            <hair_colors><resource name ="t3_har_wht"/><resource name="t3_har_bln"/></hair_colors>
            <heads><elf_female><resource name="EF_CPS_P01.mop"/></elf_female></heads>
        </morph_config>"#;
        assert_eq!(
            face_config_list(xml, "beards", "human_male").unwrap(),
            ["", "hm_brd_b1a_0"]
        );
        assert_eq!(
            face_config_list(xml, "hair_colors", "human_male").unwrap(),
            ["t3_har_wht", "t3_har_bln"]
        );
        assert_eq!(
            face_config_list(xml, "heads", "elf_female").unwrap(),
            ["ef_cps_p01"]
        );
        assert!(
            face_config_list(xml, "tattoos", "elf_female")
                .unwrap()
                .is_empty()
        );
        assert!(face_config_list(b"not xml", "heads", "x").is_err());
    }
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
        let weight = b.add(
            b"MAT ",
            vec![
                (field::PARAM_NODE, s("FaceM1")),
                (field::PARAM_NAME, s("g_vFacialHairWeights")),
                (field::PARAM_INDEX, Value::I32(5)),
                (field::PARAM_SCALAR, Value::F32(0.72)),
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
                (field::SCALARS, structs(&[weight])),
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
        // Scalar 5: the second vector's `y`.
        let weights = &face
            .vectors
            .iter()
            .find(|v| v.0 == "g_vFacialHairWeights")
            .unwrap()
            .1;
        assert_eq!(weights, &vec![[0.0; 4], [0.0, 0.72, 0.0, 0.0]]);
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

    #[test]
    fn targets_blend_as_offsets_from_their_neutral_through_shared_triangles() {
        // A base triangle and two targets in another vertex order: the neutral (as the base)
        // and one with the first corner raised.
        let base = [
            [0.0, 0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
        ];
        let uvs = [
            [0.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
        ];
        let indices = [0, 1, 2];
        let neutral = [
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let nuv = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 0.0],
        ];
        let raised = [
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
            [0.0, 0.0, 0.5, 1.0],
        ];
        let nidx = [2, 0, 1];
        let t = |weight, positions: &'static [[f32; 4]]| TargetMesh {
            weight,
            positions,
            indices: &nidx,
            uvs: &nuv,
        };
        let neutral: &'static [[f32; 4]] = Box::leak(Box::new(neutral));
        let raised: &'static [[f32; 4]] = Box::leak(Box::new(raised));
        let out =
            blend_targets(&base, &indices, &uvs, &[t(-0.4, neutral), t(0.4, raised)]).unwrap();
        assert!((out[0][2] - 0.2).abs() < 1e-6, "{out:?}");
        assert_eq!(out[1], base[1]);
        assert_eq!(out[2], base[2]);
        // A target with another triangle list does not apply.
        let other = TargetMesh {
            weight: 1.0,
            positions: raised,
            indices: &[0, 1, 2, 0, 1, 2],
            uvs: &nuv,
        };
        assert!(blend_targets(&base, &indices, &uvs, &[other]).is_none());
    }
}
