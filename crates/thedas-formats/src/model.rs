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
}

pub const KIND_TRANSLATION: FourCC = FourCC::new(b"trsl");
pub const KIND_ROTATION: FourCC = FourCC::new(b"rota");
pub const KIND_MESH: FourCC = FourCC::new(b"mshh");

/// What a mesh node draws.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshRef {
    /// The chunk name inside the model's `.msh`.
    pub chunk: String,
    /// Material object stem: `<material>.mao`.
    pub material: String,
    /// Matched against [`crate::level::LightmapBinding::chunk`].
    pub lightmap_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub kind: FourCC,
    pub name: Option<String>,
    /// Local transform relative to the parent.
    pub transform: Transform,
    pub mesh: Option<MeshRef>,
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

impl Mmh {
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
            }),
            None => None,
        }
    } else {
        None
    };
    Ok(Some(Node {
        kind,
        name: d.opt_str(id, field::NAME)?,
        transform,
        mesh,
        children,
    }))
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
        assert_eq!(pieces[0].chain.len(), 2);
        assert_eq!(pieces[0].chain[1].position, [0.0, 0.0, 5.0]);
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
