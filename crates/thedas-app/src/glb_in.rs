//! Mod mode's mesh re-import: a `.glb` exported by [`crate::glb`] and saved again (from
//! Blender or anything else that writes glTF 2.0) turned back into the game's `.msh` files,
//! chunk by chunk, using the export's `manifest.json` to know which mesh is which.
//!
//! How a glTF mesh finds its chunk: by `extras.thedas` (`msh`, `chunk`) when the writer kept
//! it; else by its name, the chunk's name (a `.001`-style suffix dropped), when only one
//! exported chunk has it; else by an enclosing node named after the model.
//!
//! How its vertices come back into the chunk's space: each glTF vertex is placed where the
//! file draws it at rest (for a skinned mesh, through its first joint and inverse bind
//! matrix; else through its node), then taken into the chunk's own space as the export
//! placed it (the axes root, the object's placement and the mesh node's rest chain, from
//! the model). So moving or bending a mesh in Blender is kept in its vertices, however the
//! writer laid out its nodes. The chunk's vertex declaration is kept: every vertex is
//! written in the original layout (positions, normals, tangents and binormals, every UV set,
//! colour, blend indices and weights); what the file lacks (a UV set, tangents) is made
//! (tangents from the first UV set) or zero.
//!
//! Skinning: each joint is matched to a bone by node name (the model's node names), then to
//! the chunk's palette slot. An influence on a bone the chunk's palette lacks is dropped (the
//! palette lives in the `.mmh`, which this does not write yet); a vertex keeps at most as
//! many influences as the chunk's weights hold, the strongest. A chunk the file no longer has
//! is written empty, so the model still finds it.

use std::collections::BTreeMap;

use glam::{Mat3, Mat4, Quat, Vec2, Vec3, Vec4};
use serde_json::Value as J;
use thedas_formats::mesh::{ChunkData, DeclUsage, MeshChunk, Msh};
use thedas_formats::model::{Mmh, Node};

use crate::glb::Read;

/// A parsed `.glb`: its JSON and binary chunk.
pub struct Gltf {
    pub json: J,
    bin: Vec<u8>,
}

impl Gltf {
    pub fn parse(glb: &[u8]) -> Result<Gltf, String> {
        let u32_at = |i: usize| -> Result<usize, String> {
            glb.get(i..i + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
                .ok_or_else(|| "the file is cut short".to_string())
        };
        if glb.get(..4) != Some(b"glTF") || u32_at(4)? != 2 {
            return Err("not a glTF 2 binary (.glb)".into());
        }
        let mut at = 12;
        let (mut json, mut bin) = (None, Vec::new());
        while at + 8 <= glb.len() {
            let len = u32_at(at)?;
            let kind = glb.get(at + 4..at + 8).ok_or("the file is cut short")?;
            let body = glb
                .get(at + 8..at + 8 + len)
                .ok_or("a chunk runs past the end of the file")?;
            match kind {
                b"JSON" => json = Some(serde_json::from_slice(body).map_err(|e| e.to_string())?),
                b"BIN\0" if bin.is_empty() => bin = body.to_vec(),
                _ => {}
            }
            at += 8 + len;
        }
        Ok(Gltf {
            json: json.ok_or("no JSON chunk")?,
            bin,
        })
    }

    fn arr(&self, k: &str) -> &[J] {
        self.json[k].as_array().map_or(&[], |v| v.as_slice())
    }

    /// An accessor's elements as four floats each (missing components 0, 0, 0, 1;
    /// normalized integers scaled as glTF says).
    fn floats(&self, i: u64) -> Result<Vec<[f32; 4]>, String> {
        let a = self
            .arr("accessors")
            .get(i as usize)
            .ok_or("no such accessor")?;
        if a.get("sparse").is_some() {
            return Err("sparse accessors are not read".into());
        }
        let n = match a["type"].as_str() {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") => 4,
            other => return Err(format!("accessor type {other:?} is not read here")),
        };
        let ct = a["componentType"].as_u64().unwrap_or(0);
        let width = match ct {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            _ => return Err(format!("component type {ct}")),
        };
        let norm = a["normalized"].as_bool().unwrap_or(false);
        let count = a["count"].as_u64().unwrap_or(0) as usize;
        let view = a["bufferView"].as_u64().ok_or("an accessor without data")? as usize;
        let v = self
            .arr("bufferViews")
            .get(view)
            .ok_or("no such buffer view")?;
        if v["buffer"].as_u64().unwrap_or(0) != 0 {
            return Err("data outside the .glb's own buffer".into());
        }
        let base = v["byteOffset"].as_u64().unwrap_or(0) as usize
            + a["byteOffset"].as_u64().unwrap_or(0) as usize;
        let stride = v["byteStride"].as_u64().map_or(n * width, |s| s as usize);
        let mut out = Vec::with_capacity(count);
        for e in 0..count {
            let at = base + e * stride;
            let b = self
                .bin
                .get(at..at + n * width)
                .ok_or("an accessor runs past its buffer")?;
            let mut x = [0.0, 0.0, 0.0, 1.0];
            for (k, slot) in x.iter_mut().enumerate().take(n) {
                let c = &b[k * width..(k + 1) * width];
                *slot = match ct {
                    5120 => {
                        let v = c[0] as i8 as f32;
                        if norm { (v / 127.0).max(-1.0) } else { v }
                    }
                    5121 => {
                        let v = c[0] as f32;
                        if norm { v / 255.0 } else { v }
                    }
                    5122 => {
                        let v = i16::from_le_bytes([c[0], c[1]]) as f32;
                        if norm { (v / 32767.0).max(-1.0) } else { v }
                    }
                    5123 => {
                        let v = u16::from_le_bytes([c[0], c[1]]) as f32;
                        if norm { v / 65535.0 } else { v }
                    }
                    5125 => u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32,
                    _ => f32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                };
            }
            out.push(x);
        }
        Ok(out)
    }

    /// An index accessor's values.
    fn indices(&self, i: u64) -> Result<Vec<u32>, String> {
        let a = self
            .arr("accessors")
            .get(i as usize)
            .ok_or("no such accessor")?;
        let ct = a["componentType"].as_u64().unwrap_or(0);
        let count = a["count"].as_u64().unwrap_or(0) as usize;
        let view = a["bufferView"].as_u64().ok_or("indices without data")? as usize;
        let v = self
            .arr("bufferViews")
            .get(view)
            .ok_or("no such buffer view")?;
        let base = v["byteOffset"].as_u64().unwrap_or(0) as usize
            + a["byteOffset"].as_u64().unwrap_or(0) as usize;
        let width = match ct {
            5121 => 1,
            5123 => 2,
            5125 => 4,
            _ => return Err(format!("index component type {ct}")),
        };
        let b = self
            .bin
            .get(base..base + count * width)
            .ok_or("indices run past their buffer")?;
        Ok(b.chunks_exact(width)
            .map(|c| match width {
                1 => c[0] as u32,
                2 => u16::from_le_bytes([c[0], c[1]]) as u32,
                _ => u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
            })
            .collect())
    }

    /// A MAT4 accessor's matrices.
    fn matrices(&self, i: u64) -> Result<Vec<Mat4>, String> {
        let a = self
            .arr("accessors")
            .get(i as usize)
            .ok_or("no such accessor")?;
        if a["type"].as_str() != Some("MAT4") || a["componentType"].as_u64() != Some(5126) {
            return Err("inverse bind matrices that are not float MAT4".into());
        }
        let count = a["count"].as_u64().unwrap_or(0) as usize;
        let view = a["bufferView"].as_u64().ok_or("matrices without data")? as usize;
        let v = self
            .arr("bufferViews")
            .get(view)
            .ok_or("no such buffer view")?;
        let base = v["byteOffset"].as_u64().unwrap_or(0) as usize
            + a["byteOffset"].as_u64().unwrap_or(0) as usize;
        let stride = v["byteStride"].as_u64().map_or(64, |s| s as usize);
        (0..count)
            .map(|e| {
                let b = self
                    .bin
                    .get(base + e * stride..base + e * stride + 64)
                    .ok_or("matrices run past their buffer")?;
                let f: Vec<f32> = b
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                Ok(Mat4::from_cols_slice(&f))
            })
            .collect()
    }

    /// Every node's world matrix at rest, and its parent.
    fn worlds(&self) -> (Vec<Mat4>, Vec<Option<usize>>) {
        let nodes = self.arr("nodes");
        let mut parent = vec![None; nodes.len()];
        for (i, n) in nodes.iter().enumerate() {
            for c in n["children"].as_array().into_iter().flatten() {
                if let Some(c) = c.as_u64().map(|c| c as usize).filter(|&c| c < nodes.len()) {
                    parent[c] = Some(i);
                }
            }
        }
        let local = |n: &J| -> Mat4 {
            let f = |k: &str, d: &[f32]| -> Vec<f32> {
                n[k].as_array().map_or(d.to_vec(), |a| {
                    a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect()
                })
            };
            if n.get("matrix").is_some() {
                let m = f("matrix", &[]);
                if m.len() == 16 {
                    return Mat4::from_cols_slice(&m);
                }
            }
            let t = f("translation", &[0.0, 0.0, 0.0]);
            let r = f("rotation", &[0.0, 0.0, 0.0, 1.0]);
            let s = f("scale", &[1.0, 1.0, 1.0]);
            if t.len() != 3 || r.len() != 4 || s.len() != 3 {
                return Mat4::IDENTITY;
            }
            let q = Quat::from_xyzw(r[0], r[1], r[2], r[3]);
            let q = if q.length_squared() > 0.0 {
                q.normalize()
            } else {
                Quat::IDENTITY
            };
            Mat4::from_scale_rotation_translation(Vec3::from_slice(&s), q, Vec3::from_slice(&t))
        };
        let mut world: Vec<Option<Mat4>> = vec![None; nodes.len()];
        fn get(
            i: usize,
            nodes: &[J],
            parent: &[Option<usize>],
            world: &mut Vec<Option<Mat4>>,
            local: &dyn Fn(&J) -> Mat4,
            depth: usize,
        ) -> Mat4 {
            if let Some(m) = world[i] {
                return m;
            }
            let up = match parent[i] {
                Some(p) if depth < 512 => get(p, nodes, parent, world, local, depth + 1),
                _ => Mat4::IDENTITY,
            };
            let m = up * local(&nodes[i]);
            world[i] = Some(m);
            m
        }
        let out = (0..nodes.len())
            .map(|i| get(i, nodes, &parent, &mut world, &local, 0))
            .collect();
        (out, parent)
    }
}

/// A chunk the export wrote, as the import needs it.
struct Target {
    msh: String,
    chunk: String,
    model: String,
    /// From the chunk's own space to the file's world, as exported.
    to_world: Mat4,
    /// The chunk's palette: slot -> bone index.
    palette: Vec<u32>,
    /// The model's nodes with bone indices, by lowercase name.
    bones: BTreeMap<String, u32>,
    /// The export wrote it (a chunk it could not write is left as it is).
    exported: bool,
}

/// One `.msh` written back.
pub struct Imported {
    pub msh: String,
    pub data: Vec<u8>,
    /// The chunks rewritten from the file, and the ones written empty.
    pub chunks: Vec<String>,
    pub emptied: Vec<String>,
}

/// What an import made.
pub struct Import {
    pub meshes: Vec<Imported>,
    /// The object's new place, when the file moved or turned it (see [`import`]).
    pub moved: Option<Placed>,
    pub warnings: Vec<String>,
}

/// An object's place: where it stands and its facing (degrees about Z).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub position: [f32; 3],
    pub facing: f32,
}

/// The game's Z-up to glTF's Y-up, as the export's root turns it.
fn axes() -> Mat4 {
    let h = std::f32::consts::FRAC_1_SQRT_2;
    Mat4::from_quat(Quat::from_xyzw(-h, 0.0, 0.0, h))
}

/// A place as a matrix in the game's space.
fn placed(p: Placed) -> Mat4 {
    Mat4::from_rotation_translation(
        Quat::from_rotation_z(p.facing.to_radians()),
        Vec3::from_array(p.position),
    )
}

/// The export's place for the object.
fn exported_place(manifest: &J) -> Placed {
    let o = &manifest["object"];
    let p: Vec<f32> = o["position"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|x| x.as_f64().unwrap_or(0.0) as f32)
        .collect();
    Placed {
        position: if p.len() == 3 {
            [p[0], p[1], p[2]]
        } else {
            [0.0; 3]
        },
        facing: o["facing"].as_f64().unwrap_or(0.0) as f32,
    }
}

/// The object's place as the file has it: its node (by the export's extras, else by the
/// object's tag) in the game's space, taken apart into a position and a turn about Z.
/// Whatever else the node does (a tilt, a scale) is left for the meshes.
fn file_place(g: &Gltf, worlds: &[Mat4], manifest: &J) -> Option<Placed> {
    let o = &manifest["object"];
    let tag = o["tag"].as_str().unwrap_or("");
    let id = o["id"].as_u64();
    let nodes = g.arr("nodes");
    let i = nodes
        .iter()
        .position(|n| id.is_some() && n["extras"]["thedas"]["object"].as_u64() == id)
        .or_else(|| {
            nodes.iter().position(|n| {
                n["name"]
                    .as_str()
                    .is_some_and(|nm| !tag.is_empty() && base_name(nm).eq_ignore_ascii_case(tag))
            })
        })?;
    let m = axes().inverse() * worlds.get(i)?;
    let at = m.w_axis.truncate();
    let x = m.x_axis.truncate();
    let facing = x.y.atan2(x.x).to_degrees().rem_euclid(360.0);
    Some(Placed {
        position: at.to_array(),
        facing,
    })
}

/// Whether two places differ by more than a millimetre or a twentieth of a degree.
fn moved(a: Placed, b: Placed) -> bool {
    let d = Vec3::from_array(a.position).distance(Vec3::from_array(b.position));
    let turn = (a.facing - b.facing + 540.0).rem_euclid(360.0) - 180.0;
    d > 1e-3 || turn.abs() > 0.05
}

/// The chunks the manifest's models hold, with where the export put them.
fn targets(
    manifest: &J,
    read: &dyn Fn(&str) -> Option<Read>,
    place: Mat4,
) -> Result<Vec<Target>, String> {
    let mut out = Vec::new();
    for m in manifest["models"].as_array().into_iter().flatten() {
        let (Some(mmh), Some(msh)) = (m["mmh"]["resource"].as_str(), m["msh"]["resource"].as_str())
        else {
            continue;
        };
        let r = read(mmh).ok_or_else(|| format!("{mmh}: not found"))?;
        let model = Mmh::parse(&r.data).map_err(|e| format!("{mmh}: {e}"))?;
        let mut bones = BTreeMap::new();
        let mut pieces = Vec::new();
        fn walk(
            n: &Node,
            above: Mat4,
            bones: &mut BTreeMap<String, u32>,
            pieces: &mut Vec<(String, Vec<u32>, Mat4)>,
        ) {
            let here = above * crate::glb::trs(&n.transform);
            if let (Some(b), Some(name)) = (n.bone, &n.name) {
                bones.entry(name.to_ascii_lowercase()).or_insert(b);
            }
            if let Some(m) = &n.mesh {
                pieces.push((m.chunk.clone(), m.bones.clone(), here));
            }
            for c in &n.children {
                walk(c, here, bones, pieces);
            }
        }
        for n in &model.nodes {
            walk(n, Mat4::IDENTITY, &mut bones, &mut pieces);
        }
        let name = m["model"].as_str().unwrap_or(mmh).to_string();
        let exported: Vec<&str> = m["meshes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x["chunk"].as_str())
            .collect();
        for (chunk, palette, at) in pieces {
            let was = exported.iter().any(|c| c.eq_ignore_ascii_case(&chunk));
            out.push(Target {
                msh: msh.to_ascii_lowercase(),
                chunk,
                model: name.clone(),
                to_world: place * at,
                exported: was,
                palette,
                bones: bones.clone(),
            });
        }
    }
    Ok(out)
}

/// A mesh's name with an editor's duplicate suffix (`.001`) and the export's ` (skinned)`
/// dropped.
fn base_name(s: &str) -> &str {
    let s = s.trim_end_matches(" (skinned)");
    match s.rsplit_once('.') {
        Some((head, tail)) if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => s,
    }
}

/// The vertices of one glTF mesh, gathered over its triangle primitives.
#[derive(Default)]
struct Gathered {
    pos: Vec<Vec3>,
    normal: Option<Vec<Vec3>>,
    tangent: Option<Vec<Vec4>>,
    /// The export's own `_BINORMAL`, when the file kept it.
    binormal: Option<Vec<Vec3>>,
    uv: BTreeMap<u32, Vec<Vec2>>,
    /// The export's `_TEXCOORD_n`: a set's every component, when it has more than two.
    uv_raw: BTreeMap<u32, Vec<Vec4>>,
    /// The export's `_COLOR_0`: the colour as stored, when it is not a colour.
    color_raw: Option<Vec<Vec4>>,
    color: Option<Vec<Vec4>>,
    joints: Option<Vec<[f32; 4]>>,
    weights: Option<Vec<[f32; 4]>>,
    indices: Vec<u32>,
}

fn gather(g: &Gltf, mesh: &J, warnings: &mut Vec<String>) -> Result<Gathered, String> {
    let mut out = Gathered::default();
    let prims = mesh["primitives"]
        .as_array()
        .ok_or("a mesh without primitives")?;
    let all = prims.len();
    for (pi, p) in prims.iter().enumerate() {
        if p["mode"].as_u64().unwrap_or(4) != 4 {
            warnings.push("a primitive that is not triangles was left out".into());
            continue;
        }
        let a = &p["attributes"];
        let pos = g.floats(
            a["POSITION"]
                .as_u64()
                .ok_or("a primitive without positions")?,
        )?;
        let n = pos.len();
        let base = out.pos.len() as u32;
        let first = out.pos.is_empty();
        let get = |k: &str| -> Result<Option<Vec<[f32; 4]>>, String> {
            match a[k].as_u64() {
                Some(i) => {
                    let v = g.floats(i)?;
                    if v.len() != n {
                        return Err(format!("{k} has {} values for {n} vertices", v.len()));
                    }
                    Ok(Some(v))
                }
                None => Ok(None),
            }
        };
        // An attribute only some primitives have is filled for the rest.
        fn merge<T: Clone>(
            slot: &mut Option<Vec<T>>,
            new: Option<Vec<T>>,
            before: usize,
            n: usize,
            fill: T,
        ) {
            match (slot.as_mut(), new) {
                (Some(v), Some(x)) => v.extend(x),
                (Some(v), None) => v.extend(std::iter::repeat_n(fill, n)),
                (None, Some(x)) => {
                    let mut v = vec![fill; before];
                    v.extend(x);
                    *slot = Some(v);
                }
                (None, None) => {}
            }
        }
        let before = out.pos.len();
        let v3 = |v: Option<Vec<[f32; 4]>>| {
            v.map(|v| v.into_iter().map(|x| Vec3::new(x[0], x[1], x[2])).collect())
        };
        let v4 =
            |v: Option<Vec<[f32; 4]>>| v.map(|v| v.into_iter().map(Vec4::from_array).collect());
        merge(&mut out.normal, v3(get("NORMAL")?), before, n, Vec3::Z);
        merge(
            &mut out.binormal,
            v3(get("_BINORMAL")?),
            before,
            n,
            Vec3::ZERO,
        );
        merge(
            &mut out.tangent,
            v4(get("TANGENT")?),
            before,
            n,
            Vec4::new(1.0, 0.0, 0.0, 1.0),
        );
        merge(&mut out.color, v4(get("COLOR_0")?), before, n, Vec4::ONE);
        merge(
            &mut out.color_raw,
            v4(get("_COLOR_0")?),
            before,
            n,
            Vec4::new(-1.0, -1.0, -1.0, -1.0),
        );
        for set in 0..8u32 {
            let raw = v4(get(&format!("_TEXCOORD_{set}"))?);
            let mut s = out.uv_raw.remove(&set);
            merge(&mut s, raw, before, n, Vec4::new(0.0, 0.0, 0.0, 1.0));
            if let Some(s) = s {
                out.uv_raw.insert(set, s);
            }
        }
        for v in out.uv_raw.values_mut() {
            if v.len() < before + n {
                v.resize(before + n, Vec4::new(0.0, 0.0, 0.0, 1.0));
            }
        }
        merge(&mut out.joints, get("JOINTS_0")?, before, n, [0.0; 4]);
        merge(
            &mut out.weights,
            get("WEIGHTS_0")?,
            before,
            n,
            [1.0, 0.0, 0.0, 0.0],
        );
        for set in 0..8u32 {
            let uv: Option<Vec<Vec2>> = get(&format!("TEXCOORD_{set}"))?
                .map(|v| v.into_iter().map(|x| Vec2::new(x[0], x[1])).collect());
            let slot = out.uv.remove(&set);
            let mut s = slot;
            merge(&mut s, uv, before, n, Vec2::ZERO);
            if let Some(s) = s {
                out.uv.insert(set, s);
            }
        }
        // Sets present earlier but absent here.
        for v in out.uv.values_mut() {
            if v.len() < before + n {
                v.resize(before + n, Vec2::ZERO);
            }
        }
        out.pos
            .extend(pos.into_iter().map(|x| Vec3::new(x[0], x[1], x[2])));
        let idx = match p["indices"].as_u64() {
            Some(i) => g.indices(i)?,
            None => (0..n as u32).collect(),
        };
        if idx.iter().any(|&i| i as usize >= n) {
            return Err(format!(
                "primitive {pi} of {all}: an index past its vertices"
            ));
        }
        out.indices
            .extend(idx.chunks_exact(3).flatten().map(|&i| i + base));
        let _ = first;
    }
    Ok(out)
}

/// Tangents from the first UV set, per vertex (`w`: the bitangent's side).
fn make_tangents(pos: &[Vec3], normal: &[Vec3], uv: &[Vec2], indices: &[u32]) -> Vec<Vec4> {
    let mut t = vec![Vec3::ZERO; pos.len()];
    let mut b = vec![Vec3::ZERO; pos.len()];
    for tri in indices.chunks_exact(3) {
        let [i0, i1, i2] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        let (e1, e2) = (pos[i1] - pos[i0], pos[i2] - pos[i0]);
        let (d1, d2) = (uv[i1] - uv[i0], uv[i2] - uv[i0]);
        let r = d1.x * d2.y - d2.x * d1.y;
        if r.abs() < 1e-12 {
            continue;
        }
        let s = (e1 * d2.y - e2 * d1.y) / r;
        let u = (e2 * d1.x - e1 * d2.x) / r;
        for i in [i0, i1, i2] {
            t[i] += s;
            b[i] += u;
        }
    }
    (0..pos.len())
        .map(|i| {
            let n = normal[i];
            let tt = (t[i] - n * n.dot(t[i]))
                .try_normalize()
                .unwrap_or_else(|| n.any_orthonormal_vector());
            let w = if n.cross(tt).dot(b[i]) < 0.0 {
                -1.0
            } else {
                1.0
            };
            tt.extend(w)
        })
        .collect()
}

/// Imports `glb` against its export's `manifest`; `read` reads the game's resources (the
/// models and the `.msh` files as they were exported from).
pub fn import(
    glb: &[u8],
    manifest: &J,
    read: &dyn Fn(&str) -> Option<Read>,
    keep_unchanged: bool,
) -> Result<Import, String> {
    if manifest["format"].as_str() != Some("openthedas-export") {
        return Err("manifest.json is not an OpenThedas export's".into());
    }
    if manifest["version"].as_u64().unwrap_or(0) > crate::glb::MANIFEST_VERSION as u64 {
        return Err("manifest.json is from a newer OpenThedas".into());
    }
    let g = Gltf::parse(glb)?;
    let (worlds, parents) = g.worlds();
    // The object moved or turned in the file: a move of the object, and the meshes taken
    // relative to its new place (so they do not move twice).
    let was = exported_place(manifest);
    let now = file_place(&g, &worlds, manifest).unwrap_or(was);
    let moved = moved(now, was).then_some(now);
    let targets = targets(manifest, read, axes() * placed(now))?;
    let nodes = g.arr("nodes");
    let meshes = g.arr("meshes");
    let skins = g.arr("skins");
    let mut warnings = Vec::new();
    // Chunk data by (msh, chunk).
    let mut made: BTreeMap<(String, String), ChunkData> = BTreeMap::new();
    let mut seen: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut msh_cache: BTreeMap<String, (Vec<u8>, Msh)> = BTreeMap::new();
    for (ni, node) in nodes.iter().enumerate() {
        let Some(mi) = node["mesh"].as_u64() else {
            continue;
        };
        let Some(mesh) = meshes.get(mi as usize) else {
            return Err(format!("node {ni} names a mesh the file lacks"));
        };
        let extras = &mesh["extras"]["thedas"];
        let name = mesh["name"].as_str().unwrap_or("");
        let found: Vec<&Target> = match (extras["msh"].as_str(), extras["chunk"].as_str()) {
            (Some(msh), Some(chunk)) => targets
                .iter()
                .filter(|t| t.msh.eq_ignore_ascii_case(msh) && t.chunk.eq_ignore_ascii_case(chunk))
                .collect(),
            _ => {
                let by_name: Vec<&Target> = targets
                    .iter()
                    .filter(|t| t.chunk.eq_ignore_ascii_case(base_name(name)))
                    .collect();
                if by_name.len() > 1 {
                    // An enclosing node named after a model settles it.
                    let mut up = parents[ni];
                    let mut hit = None;
                    while let Some(p) = up {
                        let pn = nodes[p]["name"].as_str().unwrap_or("");
                        if let Some(t) = by_name
                            .iter()
                            .find(|t| t.model.eq_ignore_ascii_case(base_name(pn)))
                        {
                            hit = Some(*t);
                            break;
                        }
                        up = parents[p];
                    }
                    hit.into_iter().collect()
                } else {
                    by_name
                }
            }
        };
        // A chunk drawn more than once (instances): the k-th mesh is the k-th instance.
        let found: Vec<&Target> = if found.len() > 1
            && found
                .iter()
                .all(|t| t.msh == found[0].msh && t.chunk == found[0].chunk)
        {
            let k = seen
                .entry((found[0].msh.clone(), found[0].chunk.clone()))
                .or_insert(0);
            *k += 1;
            found.get(*k - 1).copied().into_iter().collect()
        } else {
            found
        };
        let [t] = found.as_slice() else {
            warnings.push(format!(
                "mesh {name:?}: {} (left out)",
                if found.is_empty() {
                    "matches no exported chunk"
                } else {
                    "matches more than one chunk"
                }
            ));
            continue;
        };
        let key = (t.msh.clone(), t.chunk.clone());
        if made.contains_key(&key) {
            // Another instance of a chunk already taken: the first instance is what is
            // written (one chunk, one shape).
            continue;
        }
        if !msh_cache.contains_key(&t.msh) {
            let r = read(&t.msh).ok_or_else(|| format!("{}: not found", t.msh))?;
            let m = Msh::parse(&r.data).map_err(|e| format!("{}: {e}", t.msh))?;
            msh_cache.insert(t.msh.clone(), (r.data, m));
        }
        let (_, msh) = &msh_cache[&t.msh];
        let chunk = msh
            .chunk(&t.chunk)
            .ok_or_else(|| format!("{}: no chunk {}", t.msh, t.chunk))?;
        let got = gather(&g, mesh, &mut warnings).map_err(|e| format!("mesh {name:?}: {e}"))?;
        // Where the file draws it at rest.
        let skin = node["skin"].as_u64().and_then(|s| skins.get(s as usize));
        let (drawn, joint_bones) = match skin {
            Some(s) => {
                let joints: Vec<usize> = s["joints"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|j| j.as_u64().map(|j| j as usize))
                    .collect();
                let ibm = match s["inverseBindMatrices"].as_u64() {
                    Some(i) => g.matrices(i)?,
                    None => vec![Mat4::IDENTITY; joints.len()],
                };
                let first = *joints.first().ok_or("a skin without joints")?;
                let drawn = worlds.get(first).copied().unwrap_or(Mat4::IDENTITY)
                    * ibm.first().copied().unwrap_or(Mat4::IDENTITY);
                // Each joint's bone, by the node's name (or the export's extras).
                let palette: Vec<u32> = s["extras"]["thedas"]["palette"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|b| b.as_u64().map(|b| b as u32))
                    .collect();
                let palette = (palette.len() == joints.len()).then_some(palette);
                let bones: Vec<Option<u32>> = joints
                    .iter()
                    .enumerate()
                    .map(|(k, &j)| {
                        if let Some(p) = &palette {
                            return Some(p[k]);
                        }
                        let n = nodes.get(j)?;
                        n["extras"]["thedas"]["bone"]
                            .as_u64()
                            .map(|b| b as u32)
                            .or_else(|| {
                                let nm = n["name"].as_str()?;
                                t.bones.get(&base_name(nm).to_ascii_lowercase()).copied()
                            })
                    })
                    .collect();
                (drawn, Some(bones))
            }
            None => (worlds[ni], None),
        };
        let to_chunk = t.to_world.inverse() * drawn;
        let nm = Mat3::from_mat4(to_chunk).inverse().transpose();
        let tm = Mat3::from_mat4(to_chunk);
        let pos: Vec<Vec3> = got
            .pos
            .iter()
            .map(|&p| to_chunk.transform_point3(p))
            .collect();
        let normal: Vec<Vec3> = match &got.normal {
            Some(n) => n
                .iter()
                .map(|&x| (nm * x).try_normalize().unwrap_or(Vec3::Z))
                .collect(),
            None => {
                if chunk.element(DeclUsage::Normal, 0).is_some() {
                    warnings.push(format!("mesh {name:?}: no normals; they point up"));
                }
                vec![Vec3::Z; pos.len()]
            }
        };
        let uv0 = got
            .uv
            .get(&0)
            .cloned()
            .unwrap_or_else(|| vec![Vec2::ZERO; pos.len()]);
        // Directions keep a zero length (degenerate vertices stay so).
        let turn = |m: &Mat3, x: Vec3| (*m * x).try_normalize().unwrap_or(Vec3::ZERO);
        let binormal: Option<Vec<Vec3>> = got
            .binormal
            .as_ref()
            .map(|b| b.iter().map(|&x| turn(&tm, x)).collect());
        let tangent: Vec<Vec4> = match &got.tangent {
            Some(tv) => tv
                .iter()
                .zip(&normal)
                .map(|(x, n)| {
                    let v = (tm * x.truncate())
                        .try_normalize()
                        .unwrap_or_else(|| n.any_orthonormal_vector());
                    v.extend(if x.w < 0.0 { -1.0 } else { 1.0 })
                })
                .collect(),
            None => make_tangents(&pos, &normal, &uv0, &got.indices),
        };
        let influences = influences(
            &got,
            joint_bones.as_deref(),
            &t.palette,
            chunk,
            &mut warnings,
            name,
        );
        // The same vertices and triangles as the game's: what the file cannot say (weights
        // on a mesh it has no skin for, unknown elements) or says the same way comes from
        // the original bytes.
        let same = pos.len() == chunk.vertex_count as usize
            && msh.indices(chunk).is_ok_and(|ix| ix == got.indices);
        let orig = if same {
            msh.vertex_bytes(chunk).ok()
        } else {
            None
        };
        let vertices = encode(
            chunk,
            orig,
            t.palette.len(),
            &pos,
            &normal,
            &tangent,
            binormal.as_deref(),
            &got,
            &influences,
            &mut warnings,
            name,
        )?;
        let bounds = (!pos.is_empty()).then(|| {
            let lo = pos.iter().fold(Vec3::splat(f32::MAX), |a, &p| a.min(p));
            let hi = pos.iter().fold(Vec3::splat(f32::MIN), |a, &p| a.max(p));
            (lo.to_array(), hi.to_array())
        });
        made.insert(
            key,
            ChunkData {
                name: chunk.name.clone(),
                vertices,
                indices: got.indices,
                bounds,
            },
        );
    }
    let mut out = Vec::new();
    for (msh_name, (data, msh)) in &msh_cache {
        let mut chunks: Vec<ChunkData> = Vec::new();
        let (mut done, mut emptied) = (Vec::new(), Vec::new());
        let mut taken = std::collections::BTreeSet::new();
        for t in targets.iter().filter(|t| &t.msh == msh_name) {
            if !taken.insert(t.chunk.to_ascii_lowercase()) {
                continue;
            }
            match made.remove(&(t.msh.clone(), t.chunk.clone())) {
                Some(c) => {
                    done.push(t.chunk.clone());
                    chunks.push(c);
                }
                None if t.exported && msh.chunk(&t.chunk).is_some() => {
                    emptied.push(t.chunk.clone());
                    chunks.push(ChunkData {
                        name: t.chunk.clone(),
                        vertices: Vec::new(),
                        indices: Vec::new(),
                        bounds: None,
                    });
                }
                None => {}
            }
        }
        for e in &emptied {
            warnings.push(format!("{msh_name}: {e} is not in the file; written empty"));
        }
        let new =
            thedas_formats::mesh::rewrite(data, &chunks).map_err(|e| format!("{msh_name}: {e}"))?;
        // A mesh file that comes back saying the same is not written (moving the object
        // alone changes no mesh).
        if !keep_unchanged && emptied.is_empty() {
            let after = Msh::parse(&new).map_err(|e| format!("{msh_name}: {e}"))?;
            let same = done.iter().all(|c| match (msh.chunk(c), after.chunk(c)) {
                (Some(a), Some(b)) => chunk_difference(msh, a, &after, b).is_none(),
                _ => false,
            });
            if same {
                continue;
            }
        }
        out.push(Imported {
            msh: msh_name.clone(),
            data: new,
            chunks: done,
            emptied,
        });
    }
    Ok(Import {
        meshes: out,
        moved,
        warnings,
    })
}

/// How a rewritten chunk differs from the original beyond what its types keep (`None`: it
/// says the same): its triangles, or a vertex attribute (directions within about a
/// degree, weights after summing to one, blend indices not compared).
pub(crate) fn chunk_difference(
    orig: &Msh,
    a: &MeshChunk,
    new: &Msh,
    b: &MeshChunk,
) -> Option<String> {
    use thedas_formats::mesh::DeclType as T;
    if orig.indices(a).ok()? != new.indices(b).ok()? {
        return Some("indices differ".into());
    }
    let usages = [
        DeclUsage::Position,
        DeclUsage::Normal,
        DeclUsage::Tangent,
        DeclUsage::Binormal,
        DeclUsage::TexCoord,
        DeclUsage::Color,
        DeclUsage::BlendWeight,
    ];
    for e in a.elements.iter().filter(|e| usages.contains(&e.usage)) {
        let x = orig.attribute(a, e.usage, e.usage_index).ok()??;
        let Some(y) = new.attribute(b, e.usage, e.usage_index).ok()? else {
            return Some(format!("{:?}{} is gone", e.usage, e.usage_index));
        };
        if x.len() != y.len() {
            return Some("vertex counts differ".into());
        }
        let n = crate::glb::components(e.ty);
        let unit = matches!(
            e.usage,
            DeclUsage::Normal | DeclUsage::Tangent | DeclUsage::Binormal
        );
        // What the type keeps, relative to the values' size.
        let tol = |v: f32| match e.ty {
            T::Float16x2 | T::Float16x4 => 2e-3 * v.abs().max(1.0),
            T::Float1 | T::Float2 | T::Float3 | T::Float4 => 1e-3 * v.abs().max(1.0),
            _ => 2.5e-2,
        };
        let off = x.iter().zip(&y).position(|(p, q)| {
            if unit {
                // Directions: the same way (lengths are made 1; a zero one stays zero).
                let (a, b) = (Vec3::new(p[0], p[1], p[2]), Vec3::new(q[0], q[1], q[2]));
                match (a.try_normalize(), b.try_normalize()) {
                    (Some(a), Some(b)) => a.dot(b) < 0.9998,
                    (None, None) => false,
                    _ => a.length() > 1e-3,
                }
            } else {
                (0..n).any(|k| {
                    let t = if e.usage == DeclUsage::BlendWeight {
                        3e-2
                    } else {
                        tol(p[k])
                    };
                    (p[k] - q[k]).abs() > t
                })
            }
        });
        if let Some(i) = off {
            return Some(format!(
                "{:?}{} ({:?}): vertex {i}: {:?} became {:?}",
                e.usage, e.usage_index, e.ty, x[i], y[i]
            ));
        }
    }
    None
}

/// Each vertex's influences as (palette slot, weight), strongest first, at most as many as
/// the chunk's weights hold.
fn influences(
    got: &Gathered,
    joint_bones: Option<&[Option<u32>]>,
    palette: &[u32],
    chunk: &MeshChunk,
    warnings: &mut Vec<String>,
    name: &str,
) -> Vec<Vec<(u32, f32)>> {
    let n = got.pos.len();
    let keep = match chunk.element(DeclUsage::BlendWeight, 0) {
        Some(e) => crate::glb::components(e.ty) + 1,
        None => 1,
    }
    .min(4);
    let (Some(bones), Some(joints), Some(weights)) = (joint_bones, &got.joints, &got.weights)
    else {
        return vec![Vec::new(); n];
    };
    let mut dropped = 0usize;
    let out = (0..n)
        .map(|i| {
            // Each of the four in its place (a weight of 0 keeps its slot), so an unchanged
            // vertex comes back as it was.
            let mut v: Vec<(u32, f32)> = (0..4)
                .map(|k| {
                    let w = weights[i][k].max(0.0);
                    let slot = bones
                        .get(joints[i][k] as usize)
                        .copied()
                        .flatten()
                        .and_then(|b| palette.iter().position(|&p| p == b));
                    match slot {
                        Some(s) => (s as u32, w),
                        None if w > 0.0 => {
                            dropped += 1;
                            (0, 0.0)
                        }
                        None => (0, 0.0),
                    }
                })
                .collect();
            // More influences than the chunk holds: the strongest.
            if v.iter().skip(keep).any(|x| x.1 > 0.0) {
                v.sort_by(|a, b| b.1.total_cmp(&a.1));
            }
            v.truncate(keep);
            let sum: f32 = v.iter().map(|x| x.1).sum();
            if sum > 0.0 {
                v.iter_mut().for_each(|x| x.1 /= sum);
            } else {
                v = vec![(0, 1.0)];
            }
            v
        })
        .collect();
    if dropped > 0 {
        warnings.push(format!(
            "mesh {name:?}: {dropped} influences on bones its palette lacks were dropped"
        ));
    }
    out
}

/// Whether a vertex's original weights and indices (`old`, its bytes) say what
/// `influences` says: the same weights once summed to one, on the same slots wherever a
/// weight is not zero.
fn same_skin(
    old: &[u8],
    weights: Option<&thedas_formats::mesh::VertexElement>,
    indices: Option<&thedas_formats::mesh::VertexElement>,
    influences: &[(u32, f32)],
    palette: usize,
) -> bool {
    let (Some(we), Some(ie)) = (weights, indices) else {
        return false;
    };
    let read = |e: &thedas_formats::mesh::VertexElement| {
        old.get(e.offset as usize..(e.offset + e.ty.size()) as usize)
            .map(|b| e.ty.decode(b))
    };
    let (Some(w), Some(ix)) = (read(we), read(ie)) else {
        return false;
    };
    // As the export saw them: weights filled in, then those on slots past the palette
    // dropped.
    let mut w = crate::glb::weights(w, crate::glb::components(we.ty));
    for k in 0..4 {
        if ix[k] < 0.0 || ix[k] as usize >= palette {
            w[k] = 0.0;
        }
    }
    let w = crate::glb::weights(w, 4);
    (0..4).all(|k| {
        let (slot, nw) = influences.get(k).copied().unwrap_or((0, 0.0));
        (w[k] - nw).abs() < 2e-3 && (nw == 0.0 || ix[k] as u32 == slot)
    })
}

/// The vertices in the chunk's own layout.
#[allow(clippy::too_many_arguments)]
fn encode(
    chunk: &MeshChunk,
    orig: Option<&[u8]>,
    palette: usize,
    pos: &[Vec3],
    normal: &[Vec3],
    tangent: &[Vec4],
    binormal: Option<&[Vec3]>,
    got: &Gathered,
    influences: &[Vec<(u32, f32)>],
    warnings: &mut Vec<String>,
    name: &str,
) -> Result<Vec<u8>, String> {
    let stride = chunk.vertex_size as usize;
    if chunk.elements.iter().any(|e| e.stream != 0) {
        return Err(format!("{}: vertices in more than one stream", chunk.name));
    }
    if chunk
        .elements
        .iter()
        .any(|e| (e.offset + e.ty.size()) as usize > stride)
    {
        return Err(format!("{}: an element outside the vertex", chunk.name));
    }
    let mut out = vec![0u8; stride * pos.len()];
    let mut unknown = Vec::new();
    let weights_el = chunk.element(DeclUsage::BlendWeight, 0);
    let indices_el = chunk.element(DeclUsage::BlendIndices, 0);
    for (i, v) in out.chunks_exact_mut(stride).enumerate() {
        let old = orig.map(|o| &o[i * stride..(i + 1) * stride]);
        // The original skinning, where the file has none or the same.
        let keep_skin = old.is_some_and(|old| {
            influences[i].is_empty()
                || same_skin(old, weights_el, indices_el, &influences[i], palette)
        });
        for e in &chunk.elements {
            let at = e.offset as usize;
            let span = at..at + e.ty.size() as usize;
            let skin = matches!(e.usage, DeclUsage::BlendIndices | DeclUsage::BlendWeight);
            if let (Some(old), true) = (old, skin && keep_skin) {
                v[span.clone()].copy_from_slice(&old[span]);
                continue;
            }
            let x: Option<[f32; 4]> = match e.usage {
                DeclUsage::Position => Some(pos[i].extend(1.0).to_array()),
                // The files' four-component normals carry 1 in w.
                DeclUsage::Normal => Some(normal[i].extend(1.0).to_array()),
                // The side is in the binormal; the files' tangents carry 1 in w.
                DeclUsage::Tangent => Some(tangent[i].truncate().extend(1.0).to_array()),
                DeclUsage::Binormal => {
                    let b = match binormal {
                        Some(b) => b[i],
                        None => normal[i].cross(tangent[i].truncate()) * tangent[i].w,
                    };
                    Some(b.extend(1.0).to_array())
                }
                DeclUsage::TexCoord => {
                    let (z, w) = got
                        .uv_raw
                        .get(&e.usage_index)
                        .map_or((0.0, 1.0), |r| (r[i].z, r[i].w));
                    Some(
                        got.uv
                            .get(&e.usage_index)
                            .map_or([0.0, 0.0, z, w], |u| [u[i].x, u[i].y, z, w]),
                    )
                }
                DeclUsage::Color => {
                    let c = got.color.as_ref().map_or(Vec4::ONE, |c| c[i]);
                    // The stored values where the colour was not changed.
                    let raw = got.color_raw.as_ref().map(|r| r[i]).filter(|r| {
                        (r.clamp(Vec4::ZERO, Vec4::ONE) - c).abs().max_element() < 2e-3
                    });
                    Some(raw.unwrap_or(c).to_array())
                }
                DeclUsage::BlendIndices => {
                    let mut s = [0.0; 4];
                    for (k, (slot, _)) in influences[i].iter().enumerate().take(4) {
                        s[k] = *slot as f32;
                    }
                    Some(s)
                }
                DeclUsage::BlendWeight => {
                    let mut w = [0.0; 4];
                    for (k, (_, wt)) in influences[i].iter().enumerate().take(4) {
                        w[k] = *wt;
                    }
                    Some(w)
                }
                _ => None,
            };
            match x {
                Some(x) => {
                    let at = e.offset as usize;
                    e.ty.encode(x, &mut v[at..at + e.ty.size() as usize]);
                }
                None => match old {
                    Some(old) => v[span.clone()].copy_from_slice(&old[span]),
                    None if !unknown.contains(&e.usage) => unknown.push(e.usage),
                    None => {}
                },
            }
        }
    }
    if !unknown.is_empty() {
        warnings.push(format!("mesh {name:?}: {unknown:?} written as zero"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate: against a real install (`THEDAS_INSTALL`), every model exported and
    /// imported back unchanged gives the same chunks: the same indices, and every vertex
    /// attribute within what its stored type keeps (halves, normalized bytes).
    #[test]
    #[ignore]
    fn every_model_round_trips() {
        let root = std::env::var_os("THEDAS_INSTALL").expect("THEDAS_INSTALL");
        let install = thedas_vfs::DaoInstall::open(root).unwrap();
        let vfs = install
            .global_vfs(thedas_vfs::install::TextureQuality::High)
            .unwrap();
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
        let s = crate::glb::Subject {
            id: 0,
            tag: "t".into(),
            area: None,
            template: "t".into(),
            position: [12.5, -3.0, 1.25],
            facing: 37.0,
        };
        let models: Vec<String> = vfs
            .names()
            .into_iter()
            .filter_map(|n| n.strip_suffix(".mmh").map(str::to_string))
            .collect();
        let (mut chunks, mut bad) = (0usize, Vec::new());

        for m in &models {
            let e = crate::glb::export(&read, &s, std::slice::from_ref(m));
            if e.manifest["models"].as_array().is_none_or(|a| a.is_empty()) {
                continue;
            }
            let imp = match import(&e.glb, &e.manifest, &read, true) {
                Ok(i) => i,
                Err(err) => {
                    bad.push(format!("{m}: {err}"));
                    continue;
                }
            };
            for w in imp
                .warnings
                .iter()
                .filter(|w| !w.contains("written as zero"))
            {
                bad.push(format!("{m}: {w}"));
            }
            for im in &imp.meshes {
                let orig = Msh::parse(&read(&im.msh).unwrap().data).unwrap();
                let new = match Msh::parse(&im.data) {
                    Ok(x) => x,
                    Err(err) => {
                        bad.push(format!("{m}: {}: {err}", im.msh));
                        continue;
                    }
                };
                for name in &im.chunks {
                    chunks += 1;
                    let (a, b) = (orig.chunk(name).unwrap(), new.chunk(name).unwrap());
                    if let Some(d) = chunk_difference(&orig, a, &new, b) {
                        bad.push(format!("{m}: {name}: {d}"));
                    }
                }
            }
        }
        println!(
            "{} models, {chunks} chunks round-tripped, {} problems",
            models.len(),
            bad.len()
        );
        for b in bad.iter().take(200) {
            println!("  {b}");
        }
        assert!(bad.is_empty());
    }

    /// A `.glb` with its JSON changed.
    fn with_json(glb: &[u8], f: impl FnOnce(&mut J)) -> Vec<u8> {
        let g = Gltf::parse(glb).unwrap();
        let mut json = g.json.clone();
        f(&mut json);
        let mut text = serde_json::to_vec(&json).unwrap();
        while text.len() % 4 != 0 {
            text.push(b' ');
        }
        let total = 12 + 8 + text.len() + 8 + g.bin.len();
        let mut out = Vec::new();
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(text.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&text);
        out.extend_from_slice(&(g.bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&g.bin);
        out
    }

    /// Against a real install, with the export's extras stripped (as an editor may):
    /// moving and turning the object in the file is a move of the object and changes no
    /// mesh; stretching it reshapes every mesh and moves nothing; and every chunk is found
    /// by name.
    #[test]
    #[ignore]
    fn a_moved_object_is_a_move_and_a_stretch_reshapes() {
        let root = std::env::var_os("THEDAS_INSTALL").expect("THEDAS_INSTALL");
        let install = thedas_vfs::DaoInstall::open(root).unwrap();
        let vfs = install
            .global_vfs(thedas_vfs::install::TextureQuality::High)
            .unwrap();
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
        let s = crate::glb::Subject {
            id: 0,
            tag: "t".into(),
            area: None,
            template: "t".into(),
            position: [3.0, 4.0, 5.0],
            facing: 30.0,
        };
        let strip = |j: &mut J| {
            for m in j["meshes"].as_array_mut().unwrap() {
                m.as_object_mut().unwrap().remove("extras");
            }
            for sk in j["skins"].as_array_mut().into_iter().flatten() {
                sk.as_object_mut().unwrap().remove("extras");
            }
            for n in j["nodes"].as_array_mut().unwrap() {
                n.as_object_mut().unwrap().remove("extras");
            }
        };
        for model in ["c_doga_0", "blk_01_0", "c_berkbeara_0"] {
            let e = crate::glb::export(&read, &s, &[model.to_string()]);
            // Up 1 m and turned to 120 degrees.
            let q = Quat::from_rotation_z(120f32.to_radians()).to_array();
            let moved = with_json(&e.glb, |j| {
                j["nodes"][1]["translation"] = serde_json::json!([3.0, 4.0, 6.0]);
                j["nodes"][1]["rotation"] = serde_json::json!(q);
                strip(j);
            });
            let imp = import(&moved, &e.manifest, &read, false).unwrap();
            let p = imp.moved.expect(model);
            assert!((Vec3::from_array(p.position) - Vec3::new(3.0, 4.0, 6.0)).length() < 1e-4);
            assert!((p.facing - 120.0).abs() < 1e-3, "{model}: {}", p.facing);
            assert!(imp.meshes.is_empty(), "{model}: a move rewrote meshes");

            let stretched = with_json(&e.glb, |j| {
                j["nodes"][1]["scale"] = serde_json::json!([1.0, 1.0, 2.0]);
                strip(j);
            });
            let imp = import(&stretched, &e.manifest, &read, false).unwrap();
            assert!(imp.moved.is_none(), "{model}");
            assert!(!imp.meshes.is_empty(), "{model}");
            for im in &imp.meshes {
                assert!(im.emptied.is_empty(), "{model}: {:?}", im.emptied);
                assert!(!im.chunks.is_empty());
            }
            println!(
                "{model}: moved, then stretched; warnings {:?}",
                imp.warnings
            );
        }
    }

    #[test]
    fn duplicate_suffixes_drop() {
        assert_eq!(base_name("Mesh1.001"), "Mesh1");
        assert_eq!(base_name("Mesh1 (skinned)"), "Mesh1");
        assert_eq!(base_name("a.b"), "a.b");
        assert_eq!(base_name("Mesh1"), "Mesh1");
    }

    #[test]
    fn tangents_follow_the_first_uv_set() {
        let pos = [Vec3::ZERO, Vec3::X, Vec3::Y];
        let normal = [Vec3::Z; 3];
        let uv = [Vec2::ZERO, Vec2::X, Vec2::Y];
        let t = make_tangents(&pos, &normal, &uv, &[0, 1, 2]);
        assert!((t[0].truncate() - Vec3::X).length() < 1e-5);
        assert_eq!(t[0].w, 1.0);
        let flipped = [Vec2::ZERO, Vec2::X, -Vec2::Y];
        assert_eq!(
            make_tangents(&pos, &normal, &flipped, &[0, 1, 2])[0].w,
            -1.0
        );
    }

    #[test]
    fn a_glb_that_is_not_one_is_refused() {
        assert!(Gltf::parse(b"nope").is_err());
        let m = serde_json::json!({"format": "something else"});
        assert!(import(b"", &m, &|_| None, false).is_err());
    }
}
