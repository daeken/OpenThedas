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
    /// A skeleton node's bone index.
    pub const BONE_INDEX: u32 = 6254;
    /// A skinned mesh's palette: the bone index each of its vertices' blend indices names.
    pub const MESH_BONES: u32 = 6255;
    /// Whether a mesh casts the engine's run-time (shadow partition) shadows; xoreos's
    /// `MMHMeshCastRuntimeShadow`.
    pub const MESH_CAST_RUNTIME_SHADOW: u32 = 6176;
}

pub const KIND_TRANSLATION: FourCC = FourCC::new(b"trsl");
pub const KIND_ROTATION: FourCC = FourCC::new(b"rota");
pub const KIND_MESH: FourCC = FourCC::new(b"mshh");
pub const KIND_POINT_LIGHT: FourCC = FourCC::new(b"nplt");
/// A particle emitter node.
pub const KIND_EMITTER: FourCC = FourCC::new(b"nemt");
pub const KIND_AMBIENT_LIGHT: FourCC = FourCC::new(b"nalt");

/// [`Light::Point`]'s affect domain bit for characters and objects.
pub const DOMAIN_CHARACTERS: i64 = 1;
/// [`Light::Point`]'s affect domain bit for level geometry.
pub const DOMAIN_LEVEL: i64 = 2;

/// A light node, as found in a level's `<room>_ll` and `<room>_lc` models.
///
/// Provisional: `6007` is the colour and `6008` the radius of point lights (`nplt`), `6010`
/// the colour of ambient lights (`nalt`), and `6296` what the light affects (xoreos names it
/// `LightAffectDomain`). The rooms' level light models (`<room>_ll`) give 2 on every light
/// (lamps, bounces, window light); their character light models (`<room>_lc`) give 1 on the
/// `Char_sun`/`Char_bounce`/`Char_ambient` lights, which reach 600 units. Read as a mask: 1
/// characters and objects, 2 level geometry.
#[derive(Clone, Debug, PartialEq)]
pub enum Light {
    Point {
        color: [f32; 4],
        radius: f32,
        /// What the light affects (see [`Light`]): [`DOMAIN_CHARACTERS`], [`DOMAIN_LEVEL`],
        /// or both; `None` when the node does not say.
        affect_domain: Option<i64>,
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

/// Reads one child struct. Returns `None` for `trsl`/`rota`, which are folded into their parent.
fn read_node(gff: &Gff4, id: StructId, depth: usize) -> TypedResult<Option<Node>> {
    if depth > MAX_DEPTH {
        return Err(TypedError::Invalid("mmh: node tree too deep".into()));
    }
    let d = Doc::new(gff, "mmh node");
    let kind = d.label(id);
    if kind == KIND_TRANSLATION || kind == KIND_ROTATION {
        return Ok(None);
    }
    let mut transform = Transform::default();
    let mut children = Vec::new();
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
        } else if let Some(n) = read_node(gff, c, depth + 1)? {
            children.push(n);
        }
    }
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
    Ok(Some(Node {
        kind,
        name: d.opt_str(id, field::NAME)?,
        transform,
        mesh,
        light,
        emitter,
        // Negative (-1) marks a node that is not a bone.
        bone: match d.opt_i64(id, field::BONE_INDEX)? {
            Some(b) if (0..=u16::MAX as i64).contains(&b) => Some(b as u32),
            Some(b) if b < 0 => None,
            Some(b) => return Err(TypedError::Invalid(format!("mmh: bone index {b}"))),
            None => None,
        },
        children,
    }))
}

/// Kept out of line: the emitter's fields would otherwise sit in every frame of the
/// recursive [`read_node`].
#[inline(never)]
fn read_emitter(gff: &Gff4, id: StructId) -> TypedResult<Box<crate::emitter::Emitter>> {
    crate::emitter::Emitter::read(gff, id).map(Box::new)
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
                affect_domain: Some(2)
            }
        );
        assert_eq!(lights[0].chain[1].position, [1.0, 2.0, 3.0]);
        assert!(matches!(lights[1].light, Light::Ambient { .. }));
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
        assert!(Mmh::from_gff4(&b.finish(root)).is_err());

        let mut b = Builder::new(b"MMH ");
        let root = b.add(b"mdlh", vec![(field::CHILDREN, Value::U32(3))]);
        assert!(Mmh::from_gff4(&b.finish(root)).is_err());
    }
}
