//! Mod mode's export to Blender: an object's models as one glTF 2.0 binary (`.glb`), with
//! its textures as PNG files beside it and a `manifest.json` naming every resource each
//! part came from, so a later import can map an edited file back onto the game's.
//!
//! What is written:
//! - A root node turning the game's Z-up into glTF's Y-up (Blender turns it back on
//!   import), and under it the object's placement (position, facing) as a node, which a
//!   later import reads back as the object's move.
//! - Per model (`.mmh`), a node with the model's node tree under it: names, local
//!   translations and rotations as the file holds them, each node's kind and bone index in
//!   `extras`.
//! - Per mesh node, one glTF mesh from its `.msh` chunk: positions, normals, tangents (the
//!   binormal's side as `w`), every texture coordinate set, colour, and for a skinned mesh
//!   joints and weights over a skin whose joints are the model's bone nodes. A rigid mesh
//!   hangs on its node; a skinned mesh hangs on the model's node (glTF ignores a skinned
//!   mesh node's own transform), with its node's rest transform folded into the inverse
//!   bind matrices. `extras.thedas` names the model, node, chunk and material.
//! - Per material (`.mao`), a glTF material: its diffuse map as the base colour; every
//!   texture it binds is written as a PNG (mip 0, as decoded) and listed in `extras`.
//!
//! Left out for now, and listed in the manifest: attached models (weapons and the like),
//! the head morph, equipment tints, lights, emitters and animations.
//!
//! Provisional: the winding (the renderer's front faces, counter-clockwise in its
//! right-handed space, are taken as glTF's), and the blend weights' implied last weight
//! (Direct3D 9's rule for fewer than four components).

use std::collections::BTreeMap;

use glam::{Mat4, Quat, Vec3};
use serde_json::{Value as J, json};
use thedas_formats::dds::Dds;
use thedas_formats::material::MaterialObject;
use thedas_formats::mesh::{DeclType, DeclUsage, MeshChunk, Msh};
use thedas_formats::model::{Mmh, Node};

/// The manifest's format version; an import refuses a newer one.
pub const MANIFEST_VERSION: u32 = 1;

/// A resource as it was read: the bytes and where they came from (the stack layer's label
/// and the archive entry, never an absolute path).
pub struct Read {
    pub data: Vec<u8>,
    pub layer: String,
    pub entry: Option<String>,
}

/// The object exported.
#[derive(Clone, Debug, PartialEq)]
pub struct Subject {
    pub id: u32,
    pub tag: String,
    pub area: Option<String>,
    pub template: String,
    pub position: [f32; 3],
    /// Degrees about Z, as the game keeps it.
    pub facing: f32,
}

/// What an export wrote, in memory.
pub struct Export {
    pub glb: Vec<u8>,
    /// PNG files to write beside the `.glb`: (file name, bytes).
    pub images: Vec<(String, Vec<u8>)>,
    pub manifest: J,
    /// What could not be read or was left out, one line each.
    pub warnings: Vec<String>,
}

/// The glTF document being built and its binary buffer.
#[derive(Default)]
struct Doc {
    nodes: Vec<J>,
    meshes: Vec<J>,
    accessors: Vec<J>,
    views: Vec<J>,
    materials: Vec<J>,
    textures: Vec<J>,
    images: Vec<J>,
    skins: Vec<J>,
    bin: Vec<u8>,
}

const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const FLOAT: u32 = 5126;
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_INT: u32 = 5125;

impl Doc {
    /// Appends bytes as a buffer view (4-byte aligned).
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while self.bin.len() % 4 != 0 {
            self.bin.push(0);
        }
        let mut v = json!({
            "buffer": 0,
            "byteOffset": self.bin.len(),
            "byteLength": bytes.len(),
        });
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        self.bin.extend_from_slice(bytes);
        self.views.push(v);
        self.views.len() - 1
    }

    /// A float accessor of `n`-component elements (`ty`: SCALAR, VEC2..4, MAT4).
    fn floats(
        &mut self,
        data: &[f32],
        n: usize,
        ty: &str,
        bounds: bool,
        target: Option<u32>,
    ) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
        let view = self.view(&bytes, target);
        let mut a = json!({
            "bufferView": view,
            "componentType": FLOAT,
            "count": data.len() / n,
            "type": ty,
        });
        if bounds && !data.is_empty() {
            let mut lo = vec![f32::MAX; n];
            let mut hi = vec![f32::MIN; n];
            for e in data.chunks_exact(n) {
                for i in 0..n {
                    lo[i] = lo[i].min(e[i]);
                    hi[i] = hi[i].max(e[i]);
                }
            }
            a["min"] = json!(lo);
            a["max"] = json!(hi);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn indices(&mut self, data: &[u32]) -> usize {
        let wide = data.iter().any(|&i| i > u16::MAX as u32);
        let bytes: Vec<u8> = if wide {
            data.iter().flat_map(|i| i.to_le_bytes()).collect()
        } else {
            data.iter()
                .flat_map(|&i| (i as u16).to_le_bytes())
                .collect()
        };
        let view = self.view(&bytes, Some(ELEMENT_ARRAY_BUFFER));
        self.accessors.push(json!({
            "bufferView": view,
            "componentType": if wide { UNSIGNED_INT } else { UNSIGNED_SHORT },
            "count": data.len(),
            "type": "SCALAR",
        }));
        self.accessors.len() - 1
    }

    fn joints(&mut self, data: &[[u16; 4]]) -> usize {
        let bytes: Vec<u8> = data
            .iter()
            .flatten()
            .flat_map(|j| j.to_le_bytes())
            .collect();
        let view = self.view(&bytes, Some(ARRAY_BUFFER));
        self.accessors.push(json!({
            "bufferView": view,
            "componentType": UNSIGNED_SHORT,
            "count": data.len(),
            "type": "VEC4",
        }));
        self.accessors.len() - 1
    }

    fn node(&mut self, n: J) -> usize {
        self.nodes.push(n);
        self.nodes.len() - 1
    }

    fn add_child(&mut self, parent: usize, child: usize) {
        let p = &mut self.nodes[parent];
        match p.get_mut("children").and_then(|c| c.as_array_mut()) {
            Some(c) => c.push(json!(child)),
            None => p["children"] = json!([child]),
        }
    }

    /// The `.glb`: header, JSON chunk, binary chunk.
    fn finish(mut self, extras: J) -> Vec<u8> {
        while self.bin.len() % 4 != 0 {
            self.bin.push(0);
        }
        let mut gltf = json!({
            "asset": {"version": "2.0", "generator": "OpenThedas mod mode"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": self.nodes,
            "buffers": [{"byteLength": self.bin.len()}],
            "bufferViews": self.views,
            "accessors": self.accessors,
            "extras": extras,
        });
        for (k, v) in [
            ("meshes", self.meshes),
            ("materials", self.materials),
            ("textures", self.textures),
            ("images", self.images),
            ("skins", self.skins),
        ] {
            if !v.is_empty() {
                gltf[k] = J::Array(v);
            }
        }
        if !self.bin.is_empty() {
            gltf["samplers"] = json!([{}]);
        }
        let mut text = serde_json::to_vec(&gltf).unwrap_or_default();
        while text.len() % 4 != 0 {
            text.push(b' ');
        }
        let total = 12 + 8 + text.len() + 8 + self.bin.len();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(text.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&text);
        out.extend_from_slice(&(self.bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&self.bin);
        out
    }
}

/// The source of a resource for the manifest.
fn source(name: &str, r: &Read) -> J {
    json!({"resource": name, "layer": r.layer, "entry": r.entry})
}

fn trs(t: &thedas_formats::level::Transform) -> Mat4 {
    let q = Quat::from_array(t.rotation);
    let q = if q.length_squared() > 0.0 {
        q.normalize()
    } else {
        Quat::IDENTITY
    };
    Mat4::from_rotation_translation(q, Vec3::from_array(t.position))
}

/// How many components a declaration type holds (Direct3D 9 fills the rest).
fn components(t: DeclType) -> usize {
    use DeclType::*;
    match t {
        Float1 => 1,
        Float2 | Short2 | Short2N | Ushort2N | Float16x2 => 2,
        Float3 | Udec3 | Dec3N => 3,
        _ => 4,
    }
}

/// Blend weights as glTF wants them: four, the implied last one filled in (Direct3D 9's
/// rule for fewer components), summing to one.
fn weights(w: [f32; 4], given: usize) -> [f32; 4] {
    let mut w = w;
    if given < 4 {
        let sum: f32 = w[..given].iter().sum();
        w[given] = (1.0 - sum).max(0.0);
        for x in w.iter_mut().skip(given + 1) {
            *x = 0.0;
        }
    }
    let sum: f32 = w.iter().map(|x| x.max(0.0)).sum();
    if sum > 0.0 {
        w.map(|x| x.max(0.0) / sum)
    } else {
        [1.0, 0.0, 0.0, 0.0]
    }
}

/// Exports `models` (resource names without `.mmh`) of `subject`. `read` reads a resource
/// from the game's stack.
pub fn export(read: &dyn Fn(&str) -> Option<Read>, subject: &Subject, models: &[String]) -> Export {
    let mut d = Doc::default();
    let mut warnings = Vec::new();
    // Z-up to Y-up: -90 degrees about X.
    let h = std::f32::consts::FRAC_1_SQRT_2;
    let root = d.node(json!({"name": "game axes (Z up)", "rotation": [-h, 0.0, 0.0, h]}));
    let placement = Quat::from_rotation_z(subject.facing.to_radians());
    let object = d.node(json!({
        "name": subject.tag,
        "translation": subject.position,
        "rotation": placement.to_array(),
        "extras": {"thedas": {"object": subject.id, "tag": subject.tag}},
    }));
    d.add_child(root, object);
    let mut materials: BTreeMap<String, Option<usize>> = BTreeMap::new();
    let mut material_list = Vec::new();
    let mut images: Vec<(String, Vec<u8>)> = Vec::new();
    let mut image_index: BTreeMap<String, usize> = BTreeMap::new();
    let mut model_list = Vec::new();
    for name in models {
        let file = format!("{}.mmh", name.to_ascii_lowercase());
        let Some(r) = read(&file) else {
            warnings.push(format!("{file}: not found"));
            continue;
        };
        let mmh = match Mmh::parse(&r.data) {
            Ok(m) => m,
            Err(e) => {
                warnings.push(format!("{file}: {e}"));
                continue;
            }
        };
        let msh = mmh.mesh_file.as_deref().and_then(|f| {
            let f = f.to_ascii_lowercase();
            match read(&f).map(|r| (Msh::parse(&r.data), r)) {
                Some((Ok(m), r)) => Some((m, f, r)),
                Some((Err(e), _)) => {
                    warnings.push(format!("{f}: {e}"));
                    None
                }
                None => {
                    warnings.push(format!("{f}: not found"));
                    None
                }
            }
        });
        let model_node = d.node(json!({
            "name": name,
            "extras": {"thedas": {"model": file}},
        }));
        d.add_child(object, model_node);
        // The node tree, with each node's model-space rest matrix.
        let mut rest: Vec<(usize, Mat4)> = Vec::new();
        let mut by_bone: BTreeMap<u32, usize> = BTreeMap::new();
        let mut pieces: Vec<(usize, &Node, Mat4)> = Vec::new();
        fn walk<'a>(
            d: &mut Doc,
            n: &'a Node,
            parent: usize,
            above: Mat4,
            rest: &mut Vec<(usize, Mat4)>,
            by_bone: &mut BTreeMap<u32, usize>,
            pieces: &mut Vec<(usize, &'a Node, Mat4)>,
        ) {
            let here = above * trs(&n.transform);
            let mut extras = json!({"kind": n.kind.to_string()});
            if let Some(b) = n.bone {
                extras["bone"] = json!(b);
            }
            let q = Quat::from_array(n.transform.rotation);
            let q = if q.length_squared() > 0.0 {
                q.normalize()
            } else {
                Quat::IDENTITY
            };
            let i = d.node(json!({
                "name": n.name.clone().unwrap_or_default(),
                "translation": n.transform.position,
                "rotation": q.to_array(),
                "extras": {"thedas": extras},
            }));
            d.add_child(parent, i);
            rest.push((i, here));
            if let Some(b) = n.bone {
                by_bone.entry(b).or_insert(i);
            }
            if n.mesh.is_some() {
                pieces.push((i, n, here));
            }
            for c in &n.children {
                walk(d, c, i, here, rest, by_bone, pieces);
            }
        }
        for n in &mmh.nodes {
            walk(
                &mut d,
                n,
                model_node,
                Mat4::IDENTITY,
                &mut rest,
                &mut by_bone,
                &mut pieces,
            );
        }
        let rest_of: BTreeMap<usize, Mat4> = rest.into_iter().collect();
        let mut mesh_list = Vec::new();
        for (node, n, at) in pieces {
            let Some(m) = &n.mesh else { continue };
            let Some((msh, msh_name, _)) = &msh else {
                continue;
            };
            let Some(chunk) = msh.chunk(&m.chunk) else {
                warnings.push(format!("{msh_name}: no chunk {}", m.chunk));
                continue;
            };
            let material = material(
                &mut d,
                read,
                &m.material,
                &mut materials,
                &mut material_list,
                &mut images,
                &mut image_index,
                &mut warnings,
            );
            let skin = if m.bones.is_empty() {
                None
            } else {
                let joints: Vec<usize> = m
                    .bones
                    .iter()
                    .map(|b| by_bone.get(b).copied().unwrap_or(model_node))
                    .collect();
                if m.bones.iter().any(|b| !by_bone.contains_key(b)) {
                    warnings.push(format!("{file}: {} names a bone it lacks", m.chunk));
                }
                let ibm: Vec<f32> = joints
                    .iter()
                    .flat_map(|j| {
                        let r = rest_of.get(j).copied().unwrap_or(Mat4::IDENTITY);
                        (r.inverse() * at).to_cols_array()
                    })
                    .collect();
                let acc = d.floats(&ibm, 16, "MAT4", false, None);
                d.skins.push(json!({
                    "joints": joints,
                    "inverseBindMatrices": acc,
                    "name": format!("{name}:{}", m.chunk),
                }));
                Some(d.skins.len() - 1)
            };
            let mesh = match mesh(&mut d, msh, chunk, material, skin.is_some(), m.bones.len()) {
                Ok(i) => i,
                Err(e) => {
                    warnings.push(format!("{msh_name}: {}: {e}", m.chunk));
                    continue;
                }
            };
            d.meshes[mesh]["extras"] = json!({"thedas": {
                "model": file, "node": n.name, "chunk": m.chunk, "material": m.material,
                "msh": msh_name,
            }});
            let at_node = match skin {
                Some(s) => {
                    let i = d.node(json!({
                        "name": format!("{} (skinned)", n.name.clone().unwrap_or_default()),
                        "mesh": mesh,
                        "skin": s,
                    }));
                    d.add_child(model_node, i);
                    i
                }
                None => {
                    d.nodes[node]["mesh"] = json!(mesh);
                    node
                }
            };
            mesh_list.push(json!({
                "node": n.name, "chunk": m.chunk, "material": m.material,
                "skinned": skin.is_some(), "gltf_mesh": mesh, "gltf_node": at_node,
            }));
        }
        model_list.push(json!({
            "model": name,
            "mmh": source(&file, &r),
            "msh": msh.as_ref().map(|(_, f, r)| source(f, r)),
            "meshes": mesh_list,
        }));
    }
    let manifest = json!({
        "format": "openthedas-export",
        "version": MANIFEST_VERSION,
        "object": {
            "id": subject.id, "tag": subject.tag, "area": subject.area,
            "template": subject.template, "position": subject.position,
            "facing": subject.facing,
        },
        "axes": "the glTF root node turns the game's Z-up into Y-up; its child is the object's placement",
        "models": model_list,
        "materials": material_list,
        "warnings": warnings,
    });
    let glb =
        d.finish(json!({"thedas": {"manifest": "manifest.json", "version": MANIFEST_VERSION}}));
    Export {
        glb,
        images,
        manifest,
        warnings,
    }
}

/// The glTF material for `.mao` `name`, made once; its textures written as PNGs.
#[allow(clippy::too_many_arguments)]
fn material(
    d: &mut Doc,
    read: &dyn Fn(&str) -> Option<Read>,
    name: &str,
    made: &mut BTreeMap<String, Option<usize>>,
    list: &mut Vec<J>,
    images: &mut Vec<(String, Vec<u8>)>,
    image_index: &mut BTreeMap<String, usize>,
    warnings: &mut Vec<String>,
) -> Option<usize> {
    let key = name.to_ascii_lowercase();
    if let Some(m) = made.get(&key) {
        return *m;
    }
    let file = format!("{key}.mao");
    let Some(r) = read(&file) else {
        warnings.push(format!("{file}: not found"));
        made.insert(key, None);
        return None;
    };
    let mao = match MaterialObject::parse(&r.data) {
        Ok(m) => m,
        Err(e) => {
            warnings.push(format!("{file}: {e}"));
            made.insert(key, None);
            return None;
        }
    };
    let mut bound = Vec::new();
    let mut base = None;
    for t in mao.textures.iter().filter(|t| !t.requires_id) {
        let res = t.resource().to_ascii_lowercase();
        let image = match image_index.get(&res) {
            Some(&i) => Some(i),
            None => match texture_png(read, &res) {
                Ok((png, src)) => {
                    let file = format!("{}.png", res.trim_end_matches(".dds"));
                    images.push((file.clone(), png));
                    d.images.push(json!({"uri": file, "name": res}));
                    d.textures
                        .push(json!({"source": d.images.len() - 1, "sampler": 0}));
                    let i = d.textures.len() - 1;
                    image_index.insert(res.clone(), i);
                    bound.push(json!({"parameter": t.parameter, "texture": src, "png": file}));
                    Some(i)
                }
                Err(e) => {
                    warnings.push(format!("{res}: {e}"));
                    None
                }
            },
        };
        if base.is_none() && t.parameter.to_ascii_lowercase().contains("diffuse") {
            base = image;
        }
    }
    let mut m = json!({
        "name": key,
        "pbrMetallicRoughness": {"metallicFactor": 0.0, "roughnessFactor": 1.0},
        "extras": {"thedas": {"mao": file, "material": mao.material,
            "textures": mao.textures.iter().map(|t| json!([t.parameter, t.res_name])).collect::<Vec<_>>()}},
    });
    if let Some(b) = base {
        m["pbrMetallicRoughness"]["baseColorTexture"] = json!({"index": b});
    }
    d.materials.push(m);
    let i = d.materials.len() - 1;
    list.push(json!({"mao": source(&file, &r), "material": mao.material, "textures": bound}));
    made.insert(key, Some(i));
    Some(i)
}

/// A texture's first mip as a PNG, and where it came from.
fn texture_png(read: &dyn Fn(&str) -> Option<Read>, res: &str) -> Result<(Vec<u8>, J), String> {
    let r = read(res).ok_or("not found")?;
    let dds = Dds::parse(&r.data).map_err(|e| e.to_string())?;
    let rgba = dds.decode_rgba8(0, 0).map_err(|e| e.to_string())?;
    let img = thedas_ui::Rgba {
        width: dds.width,
        height: dds.height,
        pixels: rgba,
    };
    let png = thedas_ui::stage::encode_png(&img).ok_or("could not be written as a PNG")?;
    Ok((png, source(res, &r)))
}

/// One chunk as a glTF mesh (a single triangle-list primitive).
fn mesh(
    d: &mut Doc,
    msh: &Msh,
    c: &MeshChunk,
    material: Option<usize>,
    skinned: bool,
    palette: usize,
) -> Result<usize, String> {
    if c.primitive != 0 {
        return Err(format!(
            "primitive type {} is not a triangle list",
            c.primitive
        ));
    }
    let attr = |u: DeclUsage, i: u32| msh.attribute(c, u, i).map_err(|e| e.to_string());
    let n = c.vertex_count as usize;
    let pos = attr(DeclUsage::Position, 0)?.ok_or("no positions")?;
    let mut indices = msh.indices(c).map_err(|e| e.to_string())?;
    indices.truncate(indices.len() / 3 * 3);
    if indices.iter().any(|&i| i as usize >= n) {
        return Err("an index past the chunk's vertices".into());
    }
    let mut attributes = serde_json::Map::new();
    let p: Vec<f32> = pos.iter().flat_map(|v| [v[0], v[1], v[2]]).collect();
    attributes.insert(
        "POSITION".into(),
        json!(d.floats(&p, 3, "VEC3", true, Some(ARRAY_BUFFER))),
    );
    let normals = attr(DeclUsage::Normal, 0)?;
    if let Some(nr) = &normals {
        let v: Vec<f32> = nr
            .iter()
            .flat_map(|v| {
                let x = Vec3::new(v[0], v[1], v[2])
                    .try_normalize()
                    .unwrap_or(Vec3::Z);
                x.to_array()
            })
            .collect();
        attributes.insert(
            "NORMAL".into(),
            json!(d.floats(&v, 3, "VEC3", false, Some(ARRAY_BUFFER))),
        );
    }
    if let (Some(t), Some(nr)) = (attr(DeclUsage::Tangent, 0)?, &normals) {
        let b = attr(DeclUsage::Binormal, 0)?;
        let v: Vec<f32> = t
            .iter()
            .enumerate()
            .flat_map(|(i, t)| {
                let tv = Vec3::new(t[0], t[1], t[2])
                    .try_normalize()
                    .unwrap_or(Vec3::X);
                let nv = Vec3::new(nr[i][0], nr[i][1], nr[i][2]);
                let w = match &b {
                    Some(b) if nv.cross(tv).dot(Vec3::new(b[i][0], b[i][1], b[i][2])) < 0.0 => -1.0,
                    _ => 1.0,
                };
                [tv.x, tv.y, tv.z, w]
            })
            .collect();
        attributes.insert(
            "TANGENT".into(),
            json!(d.floats(&v, 4, "VEC4", false, Some(ARRAY_BUFFER))),
        );
    }
    for set in 0..8 {
        if let Some(uv) = attr(DeclUsage::TexCoord, set)? {
            let v: Vec<f32> = uv.iter().flat_map(|v| [v[0], v[1]]).collect();
            attributes.insert(
                format!("TEXCOORD_{set}"),
                json!(d.floats(&v, 2, "VEC2", false, Some(ARRAY_BUFFER))),
            );
        }
    }
    if let Some(col) = attr(DeclUsage::Color, 0)? {
        let v: Vec<f32> = col.iter().flatten().map(|x| x.clamp(0.0, 1.0)).collect();
        attributes.insert(
            "COLOR_0".into(),
            json!(d.floats(&v, 4, "VEC4", false, Some(ARRAY_BUFFER))),
        );
    }
    if skinned {
        let idx = attr(DeclUsage::BlendIndices, 0)?.ok_or("skinned, but no blend indices")?;
        let given = c
            .element(DeclUsage::BlendWeight, 0)
            .map_or(0, |e| components(e.ty));
        let w = attr(DeclUsage::BlendWeight, 0)?;
        let mut joints = Vec::with_capacity(n);
        let mut ws = Vec::with_capacity(n * 4);
        for i in 0..n {
            let mut wt = match &w {
                Some(w) => weights(w[i], given),
                None => [1.0, 0.0, 0.0, 0.0],
            };
            let mut j = [0u16; 4];
            for k in 0..4 {
                let slot = idx[i][k];
                if slot >= 0.0 && (slot as usize) < palette {
                    j[k] = slot as u16;
                } else {
                    wt[k] = 0.0;
                }
            }
            let wt = weights(wt, 4);
            joints.push(j);
            ws.extend_from_slice(&wt);
        }
        attributes.insert("JOINTS_0".into(), json!(d.joints(&joints)));
        attributes.insert(
            "WEIGHTS_0".into(),
            json!(d.floats(&ws, 4, "VEC4", false, Some(ARRAY_BUFFER))),
        );
    }
    let ind = d.indices(&indices);
    let mut prim = json!({"attributes": attributes, "indices": ind, "mode": 4});
    if let Some(m) = material {
        prim["material"] = json!(m);
    }
    d.meshes.push(json!({"name": c.name, "primitives": [prim]}));
    Ok(d.meshes.len() - 1)
}

/// Checks a `.glb` this module wrote holds together: the chunks, every accessor inside its
/// view and every view inside the buffer, indices inside their vertices, joints inside
/// their skin. Returns the number of meshes.
pub fn check(glb: &[u8]) -> Result<usize, String> {
    let u32_at = |i: usize| -> Result<u32, String> {
        glb.get(i..i + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| "truncated".to_string())
    };
    if glb.get(..4) != Some(b"glTF") || u32_at(4)? != 2 || u32_at(8)? as usize != glb.len() {
        return Err("not a glTF 2 binary of its stated length".into());
    }
    let jlen = u32_at(12)? as usize;
    if glb.get(16..20) != Some(b"JSON") {
        return Err("no JSON chunk".into());
    }
    let json: J = serde_json::from_slice(glb.get(20..20 + jlen).ok_or("JSON chunk truncated")?)
        .map_err(|e| e.to_string())?;
    let bstart = 20 + jlen;
    let blen = u32_at(bstart)? as usize;
    if glb.get(bstart + 4..bstart + 8) != Some(b"BIN\0") {
        return Err("no BIN chunk".into());
    }
    let bin = glb
        .get(bstart + 8..bstart + 8 + blen)
        .ok_or("BIN chunk truncated")?;
    let arr = |k: &str| json[k].as_array().cloned().unwrap_or_default();
    let views = arr("bufferViews");
    let accessors = arr("accessors");
    for (i, v) in views.iter().enumerate() {
        let (o, l) = (
            v["byteOffset"].as_u64().unwrap_or(0),
            v["byteLength"].as_u64().unwrap_or(0),
        );
        if o + l > bin.len() as u64 {
            return Err(format!("view {i} outside the buffer"));
        }
    }
    let size = |a: &J| -> u64 {
        let comps = match a["type"].as_str() {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") => 4,
            Some("MAT4") => 16,
            _ => 0,
        };
        let width = match a["componentType"].as_u64() {
            Some(5126) | Some(5125) => 4,
            Some(5123) => 2,
            _ => 0,
        };
        comps * width
    };
    let data = |ai: u64| -> Result<(&[u8], &J), String> {
        let a = accessors.get(ai as usize).ok_or("no such accessor")?;
        let v = &views[a["bufferView"].as_u64().ok_or("accessor without a view")? as usize];
        let o = v["byteOffset"].as_u64().unwrap_or(0) as usize;
        let l = v["byteLength"].as_u64().unwrap_or(0) as usize;
        let need = a["count"].as_u64().unwrap_or(0) * size(a);
        if need > l as u64 || size(a) == 0 {
            return Err(format!("accessor {ai} does not fit its view"));
        }
        Ok((&bin[o..o + need as usize], a))
    };
    for i in 0..accessors.len() {
        data(i as u64)?;
    }
    let skins = arr("skins");
    let meshes = arr("meshes");
    let nodes = arr("nodes");
    for (ni, node) in nodes.iter().enumerate() {
        let Some(mi) = node["mesh"].as_u64() else {
            continue;
        };
        let mesh = meshes
            .get(mi as usize)
            .ok_or(format!("node {ni}: no mesh {mi}"))?;
        let joints = node["skin"]
            .as_u64()
            .and_then(|s| skins.get(s as usize))
            .map(|s| s["joints"].as_array().map_or(0, |j| j.len()));
        for p in mesh["primitives"].as_array().into_iter().flatten() {
            let (_, pos) = data(p["attributes"]["POSITION"].as_u64().ok_or("no positions")?)?;
            let n = pos["count"].as_u64().unwrap_or(0);
            let (bytes, ia) = data(p["indices"].as_u64().ok_or("no indices")?)?;
            let wide = ia["componentType"].as_u64() == Some(5125);
            let ix: Vec<u64> = if wide {
                bytes
                    .chunks_exact(4)
                    .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u64)
                    .collect()
            } else {
                bytes
                    .chunks_exact(2)
                    .map(|b| u16::from_le_bytes([b[0], b[1]]) as u64)
                    .collect()
            };
            if ix.iter().any(|&i| i >= n) {
                return Err(format!("mesh {mi}: an index past its vertices"));
            }
            if let Some(ja) = p["attributes"]["JOINTS_0"].as_u64() {
                let count = joints.ok_or(format!("mesh {mi}: joints but no skin"))?;
                let (bytes, _) = data(ja)?;
                if bytes
                    .chunks_exact(2)
                    .any(|b| u16::from_le_bytes([b[0], b[1]]) as usize >= count)
                {
                    return Err(format!("mesh {mi}: a joint past its skin"));
                }
            }
        }
    }
    Ok(meshes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_fill_the_implied_last_and_sum_to_one() {
        assert_eq!(weights([0.25, 0.5, 0.0, 1.0], 2), [0.25, 0.5, 0.25, 0.0]);
        assert_eq!(weights([0.0, 0.0, 0.0, 0.0], 4), [1.0, 0.0, 0.0, 0.0]);
        let w = weights([2.0, 2.0, 0.0, 0.0], 4);
        assert_eq!(w, [0.5, 0.5, 0.0, 0.0]);
    }

    #[test]
    fn a_model_without_its_files_still_makes_a_valid_glb() {
        let read = |_: &str| None;
        let s = Subject {
            id: 7,
            tag: "x".into(),
            area: None,
            template: "x".into(),
            position: [1.0, 2.0, 3.0],
            facing: 90.0,
        };
        let e = export(&read, &s, &["missing_0".into()]);
        assert_eq!(check(&e.glb), Ok(0));
        assert_eq!(e.warnings, vec!["missing_0.mmh: not found".to_string()]);
        assert_eq!(e.manifest["object"]["tag"], "x");
        assert_eq!(e.manifest["version"], MANIFEST_VERSION);
    }

    /// Against a real install (`THEDAS_INSTALL`): every model of the core stack exports
    /// to a `.glb` that holds together. Prints how many, and the first few failures.
    #[test]
    #[ignore]
    fn every_model_of_the_install_exports() {
        let root = std::env::var_os("THEDAS_INSTALL").expect("THEDAS_INSTALL");
        let install = thedas_vfs::DaoInstall::open(root).unwrap();
        let vfs = install
            .global_vfs(thedas_vfs::install::TextureQuality::High)
            .unwrap();
        // Textures are left out (decoding thousands takes long); the meshes are the point.
        let read = |name: &str| {
            if name.ends_with(".dds") {
                return None;
            }
            vfs.read(name).ok().map(|(data, p)| Read {
                data,
                layer: p.layer_label,
                entry: p.entry,
            })
        };
        let s = Subject {
            id: 0,
            tag: "t".into(),
            area: None,
            template: "t".into(),
            position: [0.0; 3],
            facing: 0.0,
        };
        let (mut ok, mut meshes, mut bad) = (0, 0, Vec::new());
        let models: Vec<String> = vfs
            .names()
            .into_iter()
            .filter_map(|n| n.strip_suffix(".mmh").map(str::to_string))
            .collect();
        for m in &models {
            let e = export(&read, &s, std::slice::from_ref(m));
            match check(&e.glb) {
                Ok(n) => {
                    ok += 1;
                    meshes += n;
                }
                Err(err) => bad.push(format!("{m}: {err}")),
            }
        }
        println!("{ok} of {} models exported, {meshes} meshes", models.len());
        for b in bad.iter().take(20) {
            println!("  {b}");
        }
        assert!(bad.is_empty(), "{} models failed", bad.len());
    }

    #[test]
    fn the_checker_catches_a_bad_index() {
        let mut d = Doc::default();
        let p = d.floats(&[0.0; 9], 3, "VEC3", true, Some(ARRAY_BUFFER));
        let i = d.indices(&[0, 1, 3]);
        d.meshes
            .push(json!({"primitives": [{"attributes": {"POSITION": p}, "indices": i}]}));
        d.node(json!({"mesh": 0}));
        let glb = d.finish(json!({}));
        assert!(check(&glb).unwrap_err().contains("past its vertices"));
    }
}
