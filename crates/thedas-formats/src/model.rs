//! Model hierarchies (`.mmh`, GFF4 `MMH `): a tree of nodes with transforms. Mesh nodes
//! (`mshh`) name a chunk of the model's `.msh`, the material object (`.mao`) to draw it with,
//! and the id that room layouts use to bind a lightmap region to it.
//!
//! Children of every node are in field `6999`, usually a generic list. `trsl` and `rota`
//! children set the parent's local transform; other children are nodes in their own right
//! (`node`, `mshh`, `snap`, `bbox`, `xprt`, lights, emitters and so on).
//!
//! Provisional: field labels and the transform rule (`parent * T * R`) come from the data,
//! not from the original executable.

use crate::gff4::{FourCC, Gff4, StructId};
use crate::level::Transform;
use crate::typed::{Doc, MAX_DEPTH, TypedError, TypedResult};

pub mod field {
    //! GFF4 field labels used by the model reader. All provisional.

    pub const NAME: u32 = 6000;
    pub const MESH_MATERIAL: u32 = 6001;
    pub const MESH_LIGHTMAP_ID: u32 = 6004;
    pub const MDLH_MESH_FILE: u32 = 6005;
    pub const MESH_CHUNK: u32 = 6006;
    pub const TRANSLATION: u32 = 6047;
    pub const ROTATION: u32 = 6048;
    pub const CHILDREN: u32 = 6999;
    pub const LIGHT_COLOR: u32 = 6007;
    pub const LIGHT_RADIUS: u32 = 6008;
    pub const AMBIENT_COLOR: u32 = 6010;
    pub const LIGHT_AFFECT_DOMAIN: u32 = 6296;
    /// A point light's `IsStatic` (DAOrigins.exe reads it into the blueprint at 0x59490b).
    pub const LIGHT_IS_STATIC: u32 = 6009;
    /// A point light's `Version` (0x5949ec), which picks its influence formula.
    pub const LIGHT_VERSION: u32 = 6345;
    /// A skeleton node's bone index.
    pub const BONE_INDEX: u32 = 6254;
    /// A skinned mesh's palette: the bone index each of its vertices' blend indices names.
    pub const MESH_BONES: u32 = 6255;
    /// Whether a mesh casts the engine's run-time (shadow partition) shadows; xoreos's
    /// `MMHMeshCastRuntimeShadow`.
    pub const MESH_CAST_RUNTIME_SHADOW: u32 = 6176;
    /// Whether a mesh is cut away when the game camera is high (`CutAway`, named by
    /// DAOrigins.exe's XML-to-GFF property table; read by the mesh blueprint at 0x595025).
    pub const MESH_CUT_AWAY: u32 = 6193;
    /// An irradiance node's probe file (`.mtx`).
    pub const IRRADIANCE_FILE: u32 = 6253;
}

pub const KIND_TRANSLATION: FourCC = FourCC::new(b"trsl");
pub const KIND_ROTATION: FourCC = FourCC::new(b"rota");
pub const KIND_MESH: FourCC = FourCC::new(b"mshh");
pub const KIND_POINT_LIGHT: FourCC = FourCC::new(b"nplt");
/// A light probe's irradiance node (under a `nlpr` light probe node).
pub const KIND_IRRADIANCE: FourCC = FourCC::new(b"nirr");
/// A particle emitter node.
pub const KIND_EMITTER: FourCC = FourCC::new(b"nemt");
pub const KIND_AMBIENT_LIGHT: FourCC = FourCC::new(b"nalt");
/// A crust node of a visual effect model: named after the body node it sits on
/// (`CrustHook1` …), with the effect's emitters for that place under it.
pub const KIND_CRUST: FourCC = FourCC::new(b"crst");

/// [`Light::Point`]'s affect domain for characters: the one value that puts a light in the
/// character lists (DAOrigins.exe 0x464ea8).
pub const DOMAIN_CHARACTERS: i64 = 1;
/// [`Light::Point`]'s affect domain the level light models give: any value other than
/// [`DOMAIN_CHARACTERS`] (0, 2, absent) puts a light in the level's lists.
pub const DOMAIN_LEVEL: i64 = 2;

/// A light node, as found in a level's `<room>_ll` and `<room>_lc` models.
///
/// `6007` is the colour and `6008` the radius of point lights (`nplt`), `6010` the colour of
/// ambient lights (`nalt`). The point light blueprint loader (DAOrigins.exe 0x594700-0x594a81)
/// also reads `6009` `IsStatic`, `6345` `Version` and `6296` `AffectsDomain`: 0, 1 or 2
/// (anything else reads as 0). The light manager (0x464e30) puts a light of domain 1 in the
/// characters' lists and any other in the level's (see `thedas_render::d3d::pick_lights`).
/// The rooms' level light models (`<room>_ll`) give 2 on every light; their character light
/// models (`<room>_lc`) give 1 on the `Char_sun`/`Char_bounce`/`Char_ambient` lights.
#[derive(Clone, Debug, PartialEq)]
pub enum Light {
    Point {
        color: [f32; 4],
        radius: f32,
        /// What the light affects (see [`Light`]): [`DOMAIN_CHARACTERS`] or the level;
        /// `None` when the node does not say.
        affect_domain: Option<i64>,
        /// `IsStatic` (6009); `None` when the node does not say.
        is_static: Option<bool>,
        /// `Version` (6345); `None` when the node does not say.
        version: Option<i64>,
    },
    Ambient {
        color: [f32; 4],
    },
}

/// A palette entry naming no bone.
pub const NO_BONE: u32 = u32::MAX;

/// What a mesh node draws.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshRef {
    /// The chunk name inside the model's `.msh`.
    pub chunk: String,
    /// Material object stem: `<material>.mao`.
    pub material: String,
    /// Matched against [`crate::level::LightmapBinding::chunk`].
    pub lightmap_id: Option<String>,
    /// For a skinned mesh, the bone index (a skeleton node's [`Node::bone`]) that each
    /// value of its vertices' blend indices stands for; empty for rigid meshes.
    pub bones: Vec<u32>,
    /// Whether the mesh casts run-time shadows (6176): static level geometry that the
    /// submission queue's shadow groups draw into the shadow partitions. `None` when the
    /// node does not say.
    pub cast_runtime_shadow: Option<bool>,
    /// Whether the mesh fades out and is cut away while the game camera is high above the
    /// party (6193, `CutAway`): ceilings and roofs.
    pub cut_away: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub kind: FourCC,
    pub name: Option<String>,
    /// Local transform relative to the parent.
    pub transform: Transform,
    pub mesh: Option<MeshRef>,
    pub light: Option<Light>,
    /// For a particle emitter node (`nemt`), its settings.
    pub emitter: Option<Box<crate::emitter::Emitter>>,
    /// For an irradiance node (`nirr`), the probe file it names.
    pub irradiance: Option<String>,
    /// The node's bone index, for skeleton nodes.
    pub bone: Option<u32>,
    pub children: Vec<Node>,
}

/// A mesh node with the chain of local transforms from the model root down to it.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshPiece<'a> {
    pub mesh: &'a MeshRef,
    pub node_name: Option<&'a str>,
    /// Outermost first. The piece's model-space transform is their product in this order.
    pub chain: Vec<Transform>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mmh {
    pub name: Option<String>,
    /// The `.msh` file holding this model's mesh chunks, if it has any.
    pub mesh_file: Option<String>,
    pub nodes: Vec<Node>,
}

/// An emitter node with the chain of local transforms from the model root down to it.
#[derive(Clone, Debug, PartialEq)]
pub struct EmitterPiece<'a> {
    pub emitter: &'a crate::emitter::Emitter,
    pub name: Option<&'a str>,
    pub chain: Vec<Transform>,
}

/// A light node with the chain of local transforms from the model root down to it.
#[derive(Clone, Debug, PartialEq)]
pub struct LightPiece<'a> {
    pub light: &'a Light,
    pub name: Option<&'a str>,
    pub chain: Vec<Transform>,
}

impl Mmh {
    /// Every light node, depth first in file order.
    pub fn lights(&self) -> Vec<LightPiece<'_>> {
        fn walk<'a>(n: &'a Node, chain: &mut Vec<Transform>, out: &mut Vec<LightPiece<'a>>) {
            chain.push(n.transform);
            if let Some(l) = &n.light {
                out.push(LightPiece {
                    light: l,
                    name: n.name.as_deref(),
                    chain: chain.clone(),
                });
            }
            for c in &n.children {
                walk(c, chain, out);
            }
            chain.pop();
        }
        let mut out = Vec::new();
        let mut chain = Vec::new();
        for n in &self.nodes {
            walk(n, &mut chain, &mut out);
        }
        out
    }

    /// Every particle emitter node, depth first in file order.
    pub fn emitters(&self) -> Vec<EmitterPiece<'_>> {
        fn walk<'a>(n: &'a Node, chain: &mut Vec<Transform>, out: &mut Vec<EmitterPiece<'a>>) {
            chain.push(n.transform);
            if let Some(e) = &n.emitter {
                out.push(EmitterPiece {
                    emitter: e,
                    name: n.name.as_deref(),
                    chain: chain.clone(),
                });
            }
            for c in &n.children {
                walk(c, chain, out);
            }
            chain.pop();
        }
        let mut out = Vec::new();
        let mut chain = Vec::new();
        for n in &self.nodes {
            walk(n, &mut chain, &mut out);
        }
        out
    }

    /// Every irradiance node (`nirr`) naming a probe file, with the chain of local transforms
    /// from the model root down to it, depth first in file order.
    pub fn irradiance_pieces(&self) -> Vec<(&str, Vec<Transform>)> {
        fn walk<'a>(
            n: &'a Node,
            chain: &mut Vec<Transform>,
            out: &mut Vec<(&'a str, Vec<Transform>)>,
        ) {
            chain.push(n.transform);
            if let Some(f) = &n.irradiance {
                out.push((f.as_str(), chain.clone()));
            }
            for c in &n.children {
                walk(c, chain, out);
            }
            chain.pop();
        }
        let mut out = Vec::new();
        let mut chain = Vec::new();
        for n in &self.nodes {
            walk(n, &mut chain, &mut out);
        }
        out
    }

    /// The irradiance probe file the first `nirr` node names (a light probe model, such as
    /// the paperdoll's `el_lp_outside_day`).
    pub fn irradiance_file(&self) -> Option<&str> {
        fn walk(n: &Node) -> Option<&str> {
            n.irradiance
                .as_deref()
                .or_else(|| n.children.iter().find_map(walk))
        }
        self.nodes.iter().find_map(walk)
    }

    pub fn parse(data: &[u8]) -> TypedResult<Self> {
        Self::from_gff4(&Gff4::parse(data)?)
    }

    pub fn from_gff4(gff: &Gff4) -> TypedResult<Self> {
        let d = Doc::new(gff, "mmh");
        d.expect_type("MMH ")?;
        let root = gff.root;
        let mut nodes = Vec::new();
        for c in d.structs(root, field::CHILDREN)? {
            if let Some(n) = read_node(gff, c, 1)? {
                nodes.push(n);
            }
        }
        Ok(Mmh {
            name: d.opt_str(root, field::NAME)?,
            mesh_file: d
                .opt_str(root, field::MDLH_MESH_FILE)?
                .filter(|s| !s.is_empty()),
            nodes,
        })
    }

    /// The transforms from the model root down to the first node named `name` (ignoring
    /// case), depth first in file order: a bone or attachment point's rest placement.
    pub fn node_chain(&self, name: &str) -> Option<Vec<Transform>> {
        fn walk(n: &Node, name: &str, chain: &mut Vec<Transform>) -> bool {
            chain.push(n.transform);
            if n.name
                .as_deref()
                .is_some_and(|x| x.eq_ignore_ascii_case(name))
            {
                return true;
            }
            for c in &n.children {
                if walk(c, name, chain) {
                    return true;
                }
            }
            chain.pop();
            false
        }
        let mut chain = Vec::new();
        self.nodes
            .iter()
            .any(|n| walk(n, name, &mut chain))
            .then_some(chain)
    }

    /// Every mesh node, depth first in file order.
    pub fn mesh_pieces(&self) -> Vec<MeshPiece<'_>> {
        fn walk<'a>(n: &'a Node, chain: &mut Vec<Transform>, out: &mut Vec<MeshPiece<'a>>) {
            chain.push(n.transform);
            if let Some(m) = &n.mesh {
                out.push(MeshPiece {
                    mesh: m,
                    node_name: n.name.as_deref(),
                    chain: chain.clone(),
                });
            }
            for c in &n.children {
                walk(c, chain, out);
            }
            chain.pop();
        }
        let mut out = Vec::new();
        let mut chain = Vec::new();
        for n in &self.nodes {
            walk(n, &mut chain, &mut out);
        }
        out
    }
}

/// A node being read: its own transform so far, its children read so far, and the child
/// structs still to read.
struct OpenNode {
    id: StructId,
    kind: FourCC,
    depth: usize,
    transform: Transform,
    children: Vec<Node>,
    pending: std::vec::IntoIter<StructId>,
}

/// Starts reading node `id`: folds its `trsl`/`rota` children into its transform and lists
/// the rest.
fn open_node(gff: &Gff4, id: StructId, kind: FourCC, depth: usize) -> TypedResult<OpenNode> {
    if depth > MAX_DEPTH {
        return Err(TypedError::Invalid("mmh: node tree too deep".into()));
    }
    let d = Doc::new(gff, "mmh node");
    let mut transform = Transform::default();
    let mut pending = Vec::new();
    for c in d.structs(id, field::CHILDREN)? {
        let ck = d.label(c);
        if ck == KIND_TRANSLATION {
            if let Some(t) = d.opt_vec3(c, field::TRANSLATION)? {
                transform.position = t;
            }
        } else if ck == KIND_ROTATION {
            if let Some(r) = d.opt_vec4(c, field::ROTATION, 1.0)? {
                transform.rotation = r;
            }
        } else {
            pending.push(c);
        }
    }
    Ok(OpenNode {
        id,
        kind,
        depth,
        transform,
        children: Vec::new(),
        pending: pending.into_iter(),
    })
}

/// Reads one child struct and its subtree. Returns `None` for `trsl`/`rota`, which are
/// folded into their parent. The tree is walked with a stack on the heap rather than by
/// recursion, so a deep tree needs no more thread stack than a shallow one (models are read
/// on worker threads with small stacks); [`MAX_DEPTH`] still bounds it, against cycles.
fn read_node(gff: &Gff4, id: StructId, depth: usize) -> TypedResult<Option<Node>> {
    let d = Doc::new(gff, "mmh node");
    let kind = d.label(id);
    if kind == KIND_TRANSLATION || kind == KIND_ROTATION {
        return Ok(None);
    }
    let mut stack = vec![open_node(gff, id, kind, depth)?];
    loop {
        let Some(top) = stack.last_mut() else {
            return Ok(None);
        };
        if let Some(c) = top.pending.next() {
            let depth = top.depth + 1;
            let open = open_node(gff, c, d.label(c), depth)?;
            stack.push(open);
            continue;
        }
        let Some(done) = stack.pop() else {
            return Ok(None);
        };
        let node = finish_node(gff, done.id, done.kind, done.transform, done.children)?;
        match stack.last_mut() {
            Some(parent) => parent.children.push(node),
            None => return Ok(Some(node)),
        }
    }
}

/// A node's own fields, once its children are read.
#[inline(never)]
fn finish_node(
    gff: &Gff4,
    id: StructId,
    kind: FourCC,
    transform: Transform,
    children: Vec<Node>,
) -> TypedResult<Node> {
    let d = Doc::new(gff, "mmh node");
    // A mesh node without a chunk name (such as the `GOB` root of many props) only groups
    // its children.
    let mesh = if kind == KIND_MESH {
        let d = Doc::new(gff, "mmh mesh");
        match d.opt_str(id, field::MESH_CHUNK)?.filter(|s| !s.is_empty()) {
            Some(chunk) => Some(MeshRef {
                chunk,
                material: d.opt_str(id, field::MESH_MATERIAL)?.unwrap_or_default(),
                lightmap_id: d
                    .opt_str(id, field::MESH_LIGHTMAP_ID)?
                    .filter(|s| !s.is_empty()),
                bones: d
                    .numbers(id, field::MESH_BONES)?
                    .into_iter()
                    .map(|b| {
                        if b.is_finite() && b >= 0.0 && b <= u16::MAX as f32 {
                            Ok(b as u32)
                        } else if b.is_finite() && b < 0.0 {
                            // No bone: never matches a skeleton node, so stays at rest.
                            Ok(NO_BONE)
                        } else {
                            Err(TypedError::Invalid(format!("mmh: bone index {b}")))
                        }
                    })
                    .collect::<TypedResult<_>>()?,
                cast_runtime_shadow: d
                    .opt_i64(id, field::MESH_CAST_RUNTIME_SHADOW)?
                    .map(|v| v != 0),
                cut_away: d.opt_i64(id, field::MESH_CUT_AWAY)?.is_some_and(|v| v != 0),
            }),
            None => None,
        }
    } else {
        None
    };
    let light = if kind == KIND_POINT_LIGHT {
        Some(Light::Point {
            color: d.opt_vec4(id, field::LIGHT_COLOR, 1.0)?.unwrap_or([1.0; 4]),
            radius: d.opt_f32(id, field::LIGHT_RADIUS)?.unwrap_or(0.0),
            affect_domain: d.opt_i64(id, field::LIGHT_AFFECT_DOMAIN)?,
            is_static: d.opt_i64(id, field::LIGHT_IS_STATIC)?.map(|v| v != 0),
            version: d.opt_i64(id, field::LIGHT_VERSION)?,
        })
    } else if kind == KIND_AMBIENT_LIGHT {
        Some(Light::Ambient {
            color: d
                .opt_vec4(id, field::AMBIENT_COLOR, 1.0)?
                .unwrap_or([0.0; 4]),
        })
    } else {
        None
    };
    let emitter = if kind == KIND_EMITTER {
        Some(read_emitter(gff, id)?)
    } else {
        None
    };
    Ok(Node {
        kind,
        name: d.opt_str(id, field::NAME)?,
        transform,
        mesh,
        light,
        emitter,
        irradiance: if kind == KIND_IRRADIANCE {
            d.opt_str(id, field::IRRADIANCE_FILE)?
                .filter(|s| !s.is_empty())
        } else {
            None
        },
        // Negative (-1) marks a node that is not a bone.
        bone: match d.opt_i64(id, field::BONE_INDEX)? {
            Some(b) if (0..=u16::MAX as i64).contains(&b) => Some(b as u32),
            Some(b) if b < 0 => None,
            Some(b) => return Err(TypedError::Invalid(format!("mmh: bone index {b}"))),
            None => None,
        },
        children,
    })
}

/// Kept out of line: the emitter's fields would otherwise sit in every frame of the
/// recursive [`read_node`].
#[inline(never)]
fn read_emitter(gff: &Gff4, id: StructId) -> TypedResult<Box<crate::emitter::Emitter>> {
    crate::emitter::Emitter::read(gff, id).map(Box::new)
}

/// A change to a model (see [`rewrite`]).
#[derive(Clone, Debug, PartialEq)]
pub enum ModelEdit {
    /// A node's local transform (the first node with this name, any case).
    Transform { node: String, transform: Transform },
    /// A mesh node's palette (the node drawing this chunk): the bone index each blend
    /// index of its vertices names.
    Palette { chunk: String, bones: Vec<u32> },
    /// A new skeleton node under `parent`, with bone index `bone`, at `transform` from its
    /// parent. Made like the model's other bone nodes (a copy of one, its animation
    /// exports left out: a new bone has no animation of its own and keeps its rest place).
    AddBone {
        parent: String,
        name: String,
        bone: u32,
        transform: Transform,
    },
}

/// A model with some changes, written as [`crate::gff4::Gff4::write`] writes it. Every
/// field the reader does not know is kept as it was. Provisional, as the reader is: the
/// model's own counts of its bones and exports (root fields 6256 and 6275) are left as
/// they were.
pub fn rewrite(data: &[u8], edits: &[ModelEdit]) -> TypedResult<Vec<u8>> {
    let mut gff = Gff4::parse(data)?;
    let invalid = |m: String| TypedError::Invalid(m);
    for e in edits {
        match e {
            ModelEdit::Transform { node, transform } => {
                let id = find_node(&gff, |g, s| {
                    name_of(g, s).is_some_and(|n| n.eq_ignore_ascii_case(node))
                })
                .ok_or_else(|| invalid(format!("mmh: no node {node}")))?;
                set_transform(&mut gff, id, transform)?;
            }
            ModelEdit::Palette { chunk, bones } => {
                let id = find_node(&gff, |g, s| {
                    Doc::new(g, "mmh mesh")
                        .opt_str(s, field::MESH_CHUNK)
                        .ok()
                        .flatten()
                        .is_some_and(|c| c.eq_ignore_ascii_case(chunk))
                })
                .ok_or_else(|| invalid(format!("mmh: no mesh node draws {chunk}")))?;
                let v = value_mut(&mut gff, id, field::MESH_BONES)
                    .ok_or_else(|| invalid(format!("mmh: {chunk} has no palette")))?;
                set_numbers(v, bones).ok_or_else(|| {
                    invalid(format!("mmh: {chunk}'s palette is not a number list"))
                })?;
            }
            ModelEdit::AddBone {
                parent,
                name,
                bone,
                transform,
            } => add_bone(&mut gff, parent, name, *bone, transform)?,
        }
    }
    Ok(gff.write()?)
}

/// A node's name (without the stored trailing NUL).
fn name_of(gff: &Gff4, id: StructId) -> Option<String> {
    Doc::new(gff, "mmh node")
        .opt_str(id, field::NAME)
        .ok()
        .flatten()
}

/// The first struct (depth first from the root) that `want` picks.
fn find_node(gff: &Gff4, want: impl Fn(&Gff4, StructId) -> bool) -> Option<StructId> {
    let d = Doc::new(gff, "mmh");
    let mut stack = vec![gff.root];
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) || seen.len() > 1 << 20 {
            continue;
        }
        if id != gff.root && want(gff, id) {
            return Some(id);
        }
        let kids = d.structs(id, field::CHILDREN).unwrap_or_default();
        stack.extend(kids.into_iter().rev());
    }
    None
}

/// A struct's field value to change (inside a generic slot if it is one).
fn value_mut(gff: &mut Gff4, id: StructId, label: u32) -> Option<&mut crate::gff4::Value> {
    use crate::gff4::Value;
    let t = gff.structs.get(id.0)?.template as usize;
    let i = gff
        .templates
        .get(t)?
        .fields
        .iter()
        .position(|f| f.label == label)?;
    match gff.structs.get_mut(id.0)?.values.get_mut(i)? {
        Value::Absent => None,
        Value::Generic(slot) => slot.value.as_mut(),
        v => Some(v),
    }
}

/// A number list set to `n`, keeping its element type; `None` if it is not one, or a
/// value does not fit.
fn set_numbers(v: &mut crate::gff4::Value, n: &[u32]) -> Option<()> {
    use crate::gff4::{List, Value};
    let Value::List(Some(l)) = v else {
        return None;
    };
    // NO_BONE is stored as -1.
    let signed = |b: u32| if b == NO_BONE { -1i64 } else { b as i64 };
    *l = match l {
        List::U8(_) => List::U8(
            n.iter()
                .map(|&b| u8::try_from(b).ok())
                .collect::<Option<_>>()?,
        ),
        List::U16(_) => List::U16(
            n.iter()
                .map(|&b| u16::try_from(b).ok())
                .collect::<Option<_>>()?,
        ),
        List::U32(_) => List::U32(n.to_vec()),
        List::I16(_) => List::I16(
            n.iter()
                .map(|&b| i16::try_from(signed(b)).ok())
                .collect::<Option<_>>()?,
        ),
        List::I32(_) => List::I32(
            n.iter()
                .map(|&b| i32::try_from(signed(b)).ok())
                .collect::<Option<_>>()?,
        ),
        List::I64(_) => List::I64(n.iter().map(|&b| signed(b)).collect()),
        List::F32(_) => List::F32(n.iter().map(|&b| signed(b) as f32).collect()),
        _ => return None,
    };
    Some(())
}

/// A node's `trsl` and `rota` children set to `t`.
fn set_transform(gff: &mut Gff4, id: StructId, t: &Transform) -> TypedResult<()> {
    use crate::gff4::Value;
    let d = Doc::new(gff, "mmh node");
    let kids = d.structs(id, field::CHILDREN)?;
    let (mut trsl, mut rota) = (None, None);
    for c in kids {
        let k = d.label(c);
        if k == KIND_TRANSLATION && trsl.is_none() {
            trsl = Some(c);
        } else if k == KIND_ROTATION && rota.is_none() {
            rota = Some(c);
        }
    }
    let (Some(trsl), Some(rota)) = (trsl, rota) else {
        return Err(TypedError::Invalid(
            "mmh: the node keeps no translation and rotation to set".into(),
        ));
    };
    let [x, y, z] = t.position;
    match value_mut(gff, trsl, field::TRANSLATION) {
        Some(Value::Vec4(v)) => *v = [x, y, z, v[3]],
        Some(Value::Vec3(v)) => *v = [x, y, z],
        _ => {
            return Err(TypedError::Invalid(
                "mmh: a translation of another type".into(),
            ));
        }
    }
    match value_mut(gff, rota, field::ROTATION) {
        Some(Value::Vec4(v) | Value::Quat(v)) => *v = t.rotation,
        _ => {
            return Err(TypedError::Invalid(
                "mmh: a rotation of another type".into(),
            ));
        }
    }
    Ok(())
}

/// [`ModelEdit::AddBone`].
fn add_bone(gff: &mut Gff4, parent: &str, name: &str, bone: u32, t: &Transform) -> TypedResult<()> {
    use crate::gff4::{GenericSlot, List, Value};
    let invalid = |m: String| TypedError::Invalid(m);
    let parent_id = find_node(gff, |g, s| {
        name_of(g, s).is_some_and(|n| n.eq_ignore_ascii_case(parent))
    })
    .ok_or_else(|| invalid(format!("mmh: no node {parent}")))?;
    if find_node(gff, |g, s| {
        name_of(g, s).is_some_and(|n| n.eq_ignore_ascii_case(name))
    })
    .is_some()
    {
        return Err(invalid(format!("mmh: there is already a node {name}")));
    }
    // A bone node to copy: one with a bone index, its translation and rotation.
    let template = find_node(gff, |g, s| {
        Doc::new(g, "mmh node")
            .opt_i64(s, field::BONE_INDEX)
            .ok()
            .flatten()
            .is_some_and(|b| b >= 0)
    })
    .ok_or_else(|| invalid("mmh: no bone node to make a new one like".into()))?;
    let children_at = |g: &Gff4, s: StructId| {
        let tl = g.structs[s.0].template as usize;
        g.templates[tl]
            .fields
            .iter()
            .position(|f| f.label == field::CHILDREN)
    };
    let ci =
        children_at(gff, template).ok_or_else(|| invalid("mmh: a bone without children".into()))?;
    let Value::List(Some(List::Generic(slots))) = gff.structs[template.0].values[ci].clone() else {
        return Err(invalid(
            "mmh: a bone's children are not a generic list".into(),
        ));
    };
    // Its translation and rotation, copied; the rest (animation exports, children) left.
    let mut kids = Vec::new();
    let mut node_slot = None;
    for slot in &slots {
        let Some(Value::Struct(Some(c))) = &slot.value else {
            continue;
        };
        let k = Doc::new(gff, "mmh node").label(*c);
        if k == KIND_TRANSLATION || k == KIND_ROTATION {
            let copy = gff.structs[c.0].clone();
            gff.structs.push(copy);
            kids.push(GenericSlot {
                type_code: slot.type_code,
                flags: slot.flags,
                value: Some(Value::Struct(Some(StructId(gff.structs.len() - 1)))),
            });
        }
    }
    // The slot type a node takes in its parent's list: the template's own, from its parent.
    let pi = children_at(gff, parent_id)
        .ok_or_else(|| invalid(format!("mmh: {parent} keeps no children")))?;
    let holds = |g: &Gff4, s: StructId| {
        Doc::new(g, "mmh node")
            .structs(s, field::CHILDREN)
            .is_ok_and(|k| k.contains(&template))
    };
    let owner = if holds(gff, gff.root) {
        Some(gff.root)
    } else {
        find_node(gff, holds)
    };
    if let Some(owner) = owner {
        let oi = children_at(gff, owner).unwrap_or(0);
        if let Value::List(Some(List::Generic(os))) = &gff.structs[owner.0].values[oi] {
            node_slot = os
                .iter()
                .find(|x| matches!(x.value, Some(Value::Struct(Some(c))) if c == template))
                .map(|x| (x.type_code, x.flags));
        }
    }
    let (type_code, flags) =
        node_slot.ok_or_else(|| invalid("mmh: the bone's place is not found".into()))?;
    let mut node = gff.structs[template.0].clone();
    node.values[ci] = Value::List(Some(List::Generic(kids)));
    gff.structs.push(node);
    let id = StructId(gff.structs.len() - 1);
    let named = format!("{name}\0");
    match value_mut(gff, id, field::NAME) {
        Some(Value::String(s)) => *s = Some(named),
        _ => return Err(invalid("mmh: a bone without a name field".into())),
    }
    match value_mut(gff, id, field::BONE_INDEX) {
        Some(v) => {
            set_int(v, bone as i64).ok_or_else(|| invalid(format!("mmh: bone index {bone}")))?
        }
        None => return Err(invalid("mmh: a bone without an index field".into())),
    }
    set_transform(gff, id, t)?;
    match &mut gff.structs[parent_id.0].values[pi] {
        Value::List(Some(List::Generic(ps))) => ps.push(GenericSlot {
            type_code,
            flags,
            value: Some(Value::Struct(Some(id))),
        }),
        _ => {
            return Err(invalid(format!(
                "mmh: {parent}'s children are not a generic list"
            )));
        }
    }
    Ok(())
}

/// Sets an integer value of whatever width it has; `None` if `n` does not fit it.
fn set_int(v: &mut crate::gff4::Value, n: i64) -> Option<()> {
    use crate::gff4::Value;
    match v {
        Value::U8(x) => *x = u8::try_from(n).ok()?,
        Value::I8(x) => *x = i8::try_from(n).ok()?,
        Value::U16(x) => *x = u16::try_from(n).ok()?,
        Value::I16(x) => *x = i16::try_from(n).ok()?,
        Value::U32(x) => *x = u32::try_from(n).ok()?,
        Value::I32(x) => *x = i32::try_from(n).ok()?,
        Value::U64(x) => *x = u64::try_from(n).ok()?,
        Value::I64(x) => *x = n,
        _ => return None,
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gff4::Value;
    use crate::typed::testutil::{Builder, generic, s};

    #[test]
    fn reads_tree_and_mesh_pieces() {
        let mut b = Builder::new(b"MMH ");
        let t1 = b.add(
            b"trsl",
            vec![(field::TRANSLATION, Value::Vec4([1.0, 2.0, 3.0, 1.0]))],
        );
        let r1 = b.add(
            b"rota",
            vec![(field::ROTATION, Value::Quat([0.0, 0.0, 1.0, 0.0]))],
        );
        let t2 = b.add(
            b"trsl",
            vec![(field::TRANSLATION, Value::Vec4([0.0, 0.0, 5.0, 1.0]))],
        );
        let mesh = b.add(
            b"mshh",
            vec![
                (field::NAME, s("MshMain_wall")),
                (field::MESH_MATERIAL, s("wall_master")),
                (field::MESH_LIGHTMAP_ID, s("_7u")),
                (field::MESH_CHUNK, s("MshMain_wall")),
                (
                    field::MESH_BONES,
                    Value::List(Some(crate::gff4::List::U32(vec![5, 52, 0]))),
                ),
                (field::CHILDREN, generic(&[t2])),
            ],
        );
        let bbox = b.add(b"bbox", vec![]);
        // A chunk-less mesh node is a plain group.
        let group = b.add(b"mshh", vec![(field::NAME, s("GOB"))]);
        let gob = b.add(
            b"node",
            vec![
                (field::NAME, s("GOB")),
                (field::BONE_INDEX, Value::I32(7)),
                (field::CHILDREN, generic(&[t1, r1, bbox, mesh, group])),
            ],
        );
        let root = b.add(
            b"mdlh",
            vec![
                (field::NAME, s("wall.mmh")),
                (field::MDLH_MESH_FILE, s("wall.msh")),
                (field::CHILDREN, generic(&[gob])),
            ],
        );
        let mmh = Mmh::from_gff4(&b.finish(root)).unwrap();
        assert_eq!(mmh.mesh_file.as_deref(), Some("wall.msh"));
        assert_eq!(mmh.nodes.len(), 1);
        let gob = &mmh.nodes[0];
        assert_eq!(gob.transform.position, [1.0, 2.0, 3.0]);
        assert_eq!(gob.transform.rotation, [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(gob.children.len(), 3, "trsl/rota fold into the parent");
        let pieces = mmh.mesh_pieces();
        assert_eq!(pieces.len(), 1);
        assert_eq!(pieces[0].mesh.material, "wall_master");
        assert_eq!(pieces[0].mesh.lightmap_id.as_deref(), Some("_7u"));
        assert_eq!(pieces[0].mesh.bones, vec![5, 52, 0]);
        assert_eq!(gob.bone, Some(7));
        assert_eq!(pieces[0].chain.len(), 2);
        assert_eq!(pieces[0].chain[1].position, [0.0, 0.0, 5.0]);
        assert!(mmh.lights().is_empty());
        let chain = mmh.node_chain("mshmain_WALL").unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].position, [1.0, 2.0, 3.0]);
        assert_eq!(mmh.node_chain("gob").unwrap().len(), 1);
        assert!(mmh.node_chain("Prop01").is_none());
    }

    #[test]
    fn reads_lights() {
        let mut b = Builder::new(b"MMH ");
        let t = b.add(
            b"trsl",
            vec![(field::TRANSLATION, Value::Vec4([1.0, 2.0, 3.0, 1.0]))],
        );
        let p = b.add(
            b"nplt",
            vec![
                (field::NAME, s("Candle")),
                (field::LIGHT_COLOR, Value::Color([0.5, 0.25, 0.0, 1.0])),
                (field::LIGHT_RADIUS, Value::F32(12.0)),
                (field::LIGHT_AFFECT_DOMAIN, Value::I32(2)),
                (field::LIGHT_IS_STATIC, Value::U8(1)),
                (field::CHILDREN, generic(&[t])),
            ],
        );
        let a = b.add(
            b"nalt",
            vec![(field::AMBIENT_COLOR, Value::Color([0.1, 0.2, 0.3, 1.0]))],
        );
        let gob = b.add(b"node", vec![(field::CHILDREN, generic(&[p, a]))]);
        let root = b.add(b"mdlh", vec![(field::CHILDREN, generic(&[gob]))]);
        let mmh = Mmh::from_gff4(&b.finish(root)).unwrap();
        let lights = mmh.lights();
        assert_eq!(lights.len(), 2);
        assert_eq!(
            lights[0].light,
            &Light::Point {
                color: [0.5, 0.25, 0.0, 1.0],
                radius: 12.0,
                affect_domain: Some(2),
                is_static: Some(true),
                version: None,
            }
        );
        assert_eq!(lights[0].chain[1].position, [1.0, 2.0, 3.0]);
        assert!(matches!(lights[1].light, Light::Ambient { .. }));
        assert_eq!(mmh.irradiance_file(), None);
    }

    #[test]
    fn reads_a_light_probes_file() {
        let mut b = Builder::new(b"MMH ");
        let irr = b.add(b"nirr", vec![(field::IRRADIANCE_FILE, s("probe_1.mtx"))]);
        let probe = b.add(b"nlpr", vec![(field::CHILDREN, generic(&[irr]))]);
        let root = b.add(b"mdlh", vec![(field::CHILDREN, generic(&[probe]))]);
        let mmh = Mmh::from_gff4(&b.finish(root)).unwrap();
        assert_eq!(mmh.irradiance_file(), Some("probe_1.mtx"));
    }

    #[test]
    fn rejects_cycles_and_bad_types() {
        // A node that lists itself as its own child.
        let mut b = Builder::new(b"MMH ");
        let n = b.add(b"node", vec![]);
        b.doc.structs[n.0].values = vec![generic(&[n])];
        b.doc.templates[0].fields.push(crate::gff4::FieldDef {
            label: field::CHILDREN,
            type_code: 0,
            flags: 0,
            offset: 0,
        });
        let root = b.add(b"mdlh", vec![(field::CHILDREN, generic(&[n]))]);
        let doc = b.finish(root);
        assert!(Mmh::from_gff4(&doc).is_err());
        // Walking it to the depth bound needs little thread stack: worker threads that read
        // models may have small ones.
        let small = std::thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(move || Mmh::from_gff4(&doc).is_err())
            .unwrap();
        assert!(small.join().unwrap());

        let mut b = Builder::new(b"MMH ");
        let root = b.add(b"mdlh", vec![(field::CHILDREN, Value::U32(3))]);
        assert!(Mmh::from_gff4(&b.finish(root)).is_err());
    }
}
