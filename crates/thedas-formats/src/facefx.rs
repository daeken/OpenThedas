//! FaceFX: the facial animation behind lip sync.
//!
//! Dragon Age: Origins keeps one FaceFX animation per voiced line (`<strref>_m.fxe`, the voice
//! sample's own name, in `audio/vo/<language>/facialanimations/*.erf`) and one FaceFX actor
//! per face rig (`humanmale.fxa` in `face.erf`; a skeleton model names its rig in field
//! 6248, `hm_uhm_basa_0.mmh` → `humanmale.fxa`). The animation's curves (`jawOpen`,
//! `pucker`, `Blink`, `E_Happy_Interested`) drive the actor's face graph, whose bone pose
//! nodes move the head's face bones (`default_jawBone` is the skeleton's `jawBone`).
//!
//! Both files are FaceFX archives as BioWare's build of the FaceFX SDK writes them (magic
//! `BIOE` or `FACE`): a header (format version, licensee and project strings, SDK version,
//! the classes serialized with their versions), a table of names the rest refers to by
//! index, then the object. Read from the shipped files (every `.fxe` of the campaign and
//! every core `.fxa` parse); the field meanings marked provisional are inferred from the
//! data.
//!
//! Traced in DAOrigins.exe: the compiled face graph's evaluation (0xa19b60). Nodes are
//! evaluated in their order. A node's inputs are combined by its operation (0: sum,
//! starting from 0; 1: product, starting from 1; the table at 0xb57c5c) after each passes
//! through its link function (switch at 0xa19eb4, numbered as the SDK registers them:
//! 0 null, 1 linear `m·x + b`, 2 quadratic `a·x²`, 3 cubic `a·x³`, 4 square root, 5 negate,
//! 6 inverse, 7 one-clamp, 8 constant, 9 corrective, 10 clamped linear). A corrective link
//! does not add: it gathers `factor × |x|` (x as a fraction of its node's range) into a
//! correction, at most 1, that scales the node's result by `1 − correction` (0xa1a1c7,
//! 0xa1a26f). The animation's curve of the node's name is added to the inputs, and the
//! result is held to the node's range (0xa1a27d).
//!
//! Provisional: how the bone poses blend (the master bone list's blend is not traced):
//! each link gives its bone's offset at full weight, a translation added and a rotation
//! turned from the rig's reference, both scaled by the node's value; curves are Hermite
//! between keys with the stored slopes; the node fields after the range and the curve
//! fields after the name are not interpreted.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FaceFxError {
    #[error("facefx: not a FaceFX archive")]
    Magic,
    #[error("facefx: truncated at byte {0}")]
    Truncated(usize),
    #[error("facefx: {0}")]
    Malformed(String),
}

type Result<T> = std::result::Result<T, FaceFxError>;

/// No list in a shipped file comes near this; a larger count is taken as a bad file.
const MAX_COUNT: u32 = 1 << 20;

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&e| e <= self.data.len())
            .ok_or(FaceFxError::Truncated(self.at))?;
        let b = &self.data[self.at..end];
        self.at = end;
        Ok(b)
    }
    fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn floats<const N: usize>(&mut self) -> Result<[f32; N]> {
        let mut out = [0.0; N];
        for v in &mut out {
            *v = self.f32()?;
        }
        Ok(out)
    }
    fn count(&mut self) -> Result<usize> {
        let n = self.u32()?;
        if n > MAX_COUNT {
            return Err(FaceFxError::Malformed(format!(
                "a count of {n} at byte {}",
                self.at - 4
            )));
        }
        Ok(n as usize)
    }
    fn string(&mut self) -> Result<String> {
        let n = self.count()?;
        Ok(String::from_utf8_lossy(self.bytes(n)?).into_owned())
    }
}

/// The archive's header and name table; the reader is left at the object.
fn header(data: &[u8]) -> Result<(Reader<'_>, Vec<String>)> {
    if data.len() < 4 || (&data[..4] != b"BIOE" && &data[..4] != b"FACE") {
        return Err(FaceFxError::Magic);
    }
    let mut r = Reader { data, at: 4 };
    let _version = r.u32()?;
    let _licensee = r.string()?;
    let _project = r.string()?;
    let _sdk = r.u32()?;
    let _ = r.u16()?;
    for _ in 0..r.count()? {
        let _ = r.u32()?;
        let _ = r.u32()?;
        let _class = r.string()?;
        let _class_version = r.u16()?;
    }
    let n = r.count()?;
    let mut names = Vec::with_capacity(n.min(4096));
    for _ in 0..n {
        names.push(r.string()?);
    }
    Ok((r, names))
}

fn name(names: &[String], index: u32) -> Result<String> {
    names
        .get(index as usize)
        .cloned()
        .ok_or_else(|| FaceFxError::Malformed(format!("name {index} of {}", names.len())))
}

/// A key of a curve: seconds into the line, the value there and the slopes into and out of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceKey {
    pub time: f32,
    pub value: f32,
    pub slope_in: f32,
    pub slope_out: f32,
}

/// A curve driving the face graph node of the same name.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceCurve {
    pub name: String,
    /// In time order.
    pub keys: Vec<FaceKey>,
}

impl FaceCurve {
    /// Its value `t` seconds into the line: the first key's before it, the last key's after
    /// it, and between two keys the Hermite curve their values and slopes make.
    pub fn at(&self, t: f32) -> f32 {
        let Some(first) = self.keys.first() else {
            return 0.0;
        };
        if t <= first.time {
            return first.value;
        }
        let next = self.keys.partition_point(|k| k.time <= t);
        let Some(b) = self.keys.get(next) else {
            return self.keys.last().map_or(0.0, |k| k.value);
        };
        let a = &self.keys[next - 1];
        let span = b.time - a.time;
        if span <= 0.0 {
            return b.value;
        }
        let s = (t - a.time) / span;
        let (s2, s3) = (s * s, s * s * s);
        let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
        let h10 = s3 - 2.0 * s2 + s;
        let h01 = -2.0 * s3 + 3.0 * s2;
        let h11 = s3 - s2;
        h00 * a.value + h10 * span * a.slope_out + h01 * b.value + h11 * span * b.slope_in
    }
}

/// A line's facial animation (`.fxe`).
#[derive(Clone, Debug, PartialEq)]
pub struct FaceAnim {
    /// The animation's name (the voice sample's, `100004_m`).
    pub name: String,
    /// The actor it was made on (`humanmale`). Provisional: the name table's second entry.
    pub actor: Option<String>,
    pub curves: Vec<FaceCurve>,
    /// Seconds to blend in and out (provisional: the two floats after the curves).
    pub blend_in: f32,
    pub blend_out: f32,
}

impl FaceAnim {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let (mut r, names) = header(data)?;
        // The anim set, its group and the animation, by name (provisional).
        let mut set = [0u32; 6];
        for v in &mut set {
            *v = r.u32()?;
        }
        let n = r.count()?;
        let mut curve_names = Vec::with_capacity(n);
        for _ in 0..n {
            curve_names.push(name(&names, r.u32()?)?);
            let _interpolation = r.u32()?;
        }
        let total = r.count()?;
        let mut keys = Vec::with_capacity(total);
        for _ in 0..total {
            let [time, value, slope_in, slope_out] = r.floats::<4>()?;
            keys.push(FaceKey {
                time,
                value,
                slope_in,
                slope_out,
            });
        }
        if r.count()? != n {
            return Err(FaceFxError::Malformed(
                "key counts for another curve count".into(),
            ));
        }
        let mut curves = Vec::with_capacity(n);
        let mut from = 0usize;
        for curve_name in curve_names {
            let count = r.count()?;
            let to = from
                .checked_add(count)
                .filter(|&t| t <= keys.len())
                .ok_or_else(|| FaceFxError::Malformed("more keys counted than stored".into()))?;
            let mut k = keys[from..to].to_vec();
            k.sort_by(|a, b| a.time.total_cmp(&b.time));
            curves.push(FaceCurve {
                name: curve_name,
                keys: k,
            });
            from = to;
        }
        if from != keys.len() {
            return Err(FaceFxError::Malformed("keys left over".into()));
        }
        let [blend_in, blend_out] = r.floats::<2>()?;
        Ok(FaceAnim {
            name: name(&names, set[5])?,
            actor: names.get(1).cloned(),
            curves,
            blend_in,
            blend_out,
        })
    }

    /// When its last key falls (seconds into the line; 0 without keys).
    pub fn length(&self) -> f32 {
        self.curves
            .iter()
            .filter_map(|c| c.keys.last())
            .map(|k| k.time)
            .fold(0.0, f32::max)
    }

    /// The curve of a node, if the animation has one.
    pub fn curve(&self, node: &str) -> Option<&FaceCurve> {
        self.curves.iter().find(|c| c.name == node)
    }
}

/// How a node combines its inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputOp {
    Sum,
    Product,
}

/// A node's input: another node's value through a link function.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeLink {
    /// The node it reads (always an earlier one).
    pub from: usize,
    /// The link function, numbered as the SDK registers them (see the module docs).
    pub function: u32,
    pub params: Vec<f32>,
}

pub mod link_fn {
    pub const NULL: u32 = 0;
    pub const LINEAR: u32 = 1;
    pub const QUADRATIC: u32 = 2;
    pub const CUBIC: u32 = 3;
    pub const SQRT: u32 = 4;
    pub const NEGATE: u32 = 5;
    pub const INVERSE: u32 = 6;
    pub const ONE_CLAMP: u32 = 7;
    pub const CONSTANT: u32 = 8;
    pub const CORRECTIVE: u32 = 9;
    pub const CLAMPED_LINEAR: u32 = 10;
}

/// A node of the face graph.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceNode {
    /// 0 a combiner, 4 a bone pose, 8 a material parameter (in the shipped rigs).
    pub kind: u32,
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub op: InputOp,
    pub inputs: Vec<NodeLink>,
}

/// A bone pose node's hold on a bone: the bone's place in that pose, in the rig's space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoneLink {
    pub node: usize,
    /// The offset from the reference position (centimetres, the rig's axes).
    pub translation: [f32; 3],
    /// The bone's rotation in the pose (w, x, y, z; the rig's axes).
    pub rotation: [f32; 4],
}

/// A bone the rig moves, its reference placement and the poses that move it.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceBone {
    /// As the rig names it (`default_jawBone`).
    pub name: String,
    /// Centimetres, the rig's axes.
    pub translation: [f32; 3],
    /// w, x, y, z.
    pub rotation: [f32; 4],
    pub links: Vec<BoneLink>,
}

impl FaceBone {
    /// The skeleton's name for the bone (lowercase, without the rig's `default_`).
    pub fn skeleton_name(&self) -> String {
        let n = self.name.to_ascii_lowercase();
        n.strip_prefix("default_").map(str::to_string).unwrap_or(n)
    }
}

/// A face rig (`.fxa`).
#[derive(Clone, Debug, PartialEq)]
pub struct FaceActor {
    pub name: String,
    pub bones: Vec<FaceBone>,
    pub nodes: Vec<FaceNode>,
}

/// A bone's offset from its rest in the skeleton's terms: a translation to add (metres)
/// and a rotation (x, y, z, w) to turn its local rotation by, after it.
#[derive(Clone, Debug, PartialEq)]
pub struct BoneOffset {
    /// The skeleton's bone (lowercase).
    pub bone: String,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
}

type Quat = [f32; 4];

/// Hamilton's product of two (w, x, y, z) quaternions.
fn qmul(a: Quat, b: Quat) -> Quat {
    [
        a[0] * b[0] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3],
        a[0] * b[1] + a[1] * b[0] + a[2] * b[3] - a[3] * b[2],
        a[0] * b[2] - a[1] * b[3] + a[2] * b[0] + a[3] * b[1],
        a[0] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[0],
    ]
}

fn qnormalize(q: Quat) -> Quat {
    let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if l > 0.0 && l.is_finite() {
        q.map(|v| v / l)
    } else {
        [1.0, 0.0, 0.0, 0.0]
    }
}

/// The identity turned toward `q` by `w` (0 to 1), the short way round.
fn qtoward(q: Quat, w: f32) -> Quat {
    let q = if q[0] < 0.0 { q.map(|v| -v) } else { q };
    qnormalize([1.0 + (q[0] - 1.0) * w, q[1] * w, q[2] * w, q[3] * w])
}

impl FaceActor {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let (mut r, names) = header(data)?;
        let actor = name(&names, r.u32()?)?;
        let _ = r.u32()?;
        let mut bones = Vec::new();
        for _ in 0..r.count()? {
            let bone_name = name(&names, r.u32()?)?;
            let translation = r.floats::<3>()?;
            let rotation = r.floats::<4>()?;
            let _scale = r.floats::<3>()?;
            let _inverse_rotation = r.floats::<4>()?;
            let _ = r.u32()?;
            let _weight = r.f32()?;
            let mut links = Vec::new();
            for _ in 0..r.count()? {
                let node = r.u32()? as usize;
                let _bone = r.u32()?;
                let t = r.floats::<3>()?;
                let q = r.floats::<4>()?;
                let _scale = r.floats::<3>()?;
                links.push(BoneLink {
                    node,
                    translation: t,
                    rotation: q,
                });
            }
            bones.push(FaceBone {
                name: bone_name,
                translation,
                rotation,
                links,
            });
        }
        let n = r.count()?;
        let mut nodes = Vec::with_capacity(n);
        for i in 0..n {
            let kind = r.u32()?;
            let node_name = name(&names, r.u32()?)?;
            let [min, _inv_min, max, _inv_max] = r.floats::<4>()?;
            let op = match r.u32()? {
                1 => InputOp::Product,
                _ => InputOp::Sum,
            };
            let mut inputs = Vec::new();
            for _ in 0..r.count()? {
                let from = r.u32()? as usize;
                if from >= i {
                    return Err(FaceFxError::Malformed(format!(
                        "node {i} reads node {from}, not an earlier one"
                    )));
                }
                let function = r.u32()?;
                let mut params = Vec::new();
                for _ in 0..r.count()? {
                    params.push(r.f32()?);
                }
                inputs.push(NodeLink {
                    from,
                    function,
                    params,
                });
            }
            // User properties (a material parameter node's mesh and parameter).
            for _ in 0..r.count()? {
                let _name = r.u32()?;
                let _kind = r.u32()?;
                let _int = r.u32()?;
                let _float = r.f32()?;
                for _ in 0..r.count()? {
                    let _ = r.string()?;
                }
            }
            nodes.push(FaceNode {
                kind,
                name: node_name,
                min,
                max,
                op,
                inputs,
            });
        }
        for b in &bones {
            if let Some(l) = b.links.iter().find(|l| l.node >= nodes.len()) {
                return Err(FaceFxError::Malformed(format!(
                    "{} is moved by node {} of {}",
                    b.name,
                    l.node,
                    nodes.len()
                )));
            }
        }
        Ok(FaceActor {
            name: actor,
            bones,
            nodes,
        })
    }

    /// Every node's value with the curves `curve` gives by node name (see the module docs).
    pub fn evaluate(&self, curve: impl Fn(&str) -> Option<f32>) -> Vec<f32> {
        let mut values: Vec<f32> = Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {
            let mut acc = match node.op {
                InputOp::Sum => 0.0,
                InputOp::Product => 1.0,
            };
            let mut correction = 0.0f32;
            for link in &node.inputs {
                let src = &self.nodes[link.from];
                let x = values[link.from];
                let p = |i: usize| link.params.get(i).copied();
                if link.function == link_fn::CORRECTIVE {
                    let up = if src.max != 0.0 { x / src.max } else { x };
                    let down = if src.min != 0.0 { -(x / src.min) } else { -x };
                    let fraction = up.max(down);
                    correction = (correction + p(0).unwrap_or(0.0) * fraction).min(1.0);
                    continue;
                }
                let y = match link.function {
                    link_fn::LINEAR => match (p(0), p(1)) {
                        (Some(m), Some(b)) => m * x + b,
                        (Some(m), None) => m * x,
                        _ => x,
                    },
                    link_fn::QUADRATIC => p(0).unwrap_or(1.0) * x * x,
                    link_fn::CUBIC => p(0).unwrap_or(1.0) * x * x * x,
                    link_fn::SQRT => {
                        if x > 0.0 {
                            p(0).unwrap_or(1.0) * x.sqrt()
                        } else {
                            0.0
                        }
                    }
                    link_fn::NEGATE => -x,
                    link_fn::INVERSE => {
                        if x.abs() > f32::EPSILON {
                            1.0 / x
                        } else {
                            x
                        }
                    }
                    link_fn::ONE_CLAMP => {
                        if x <= 1.0 {
                            1.0
                        } else {
                            1.0 / x
                        }
                    }
                    link_fn::CONSTANT => p(0).unwrap_or(x),
                    link_fn::CLAMPED_LINEAR => match link.params[..] {
                        [x1, y1, x2, y2] => {
                            if x <= x1 {
                                y1
                            } else if x >= x2 {
                                y2
                            } else {
                                y1 + (x - x1) * (y2 - y1) / (x2 - x1)
                            }
                        }
                        _ => x,
                    },
                    _ => x,
                };
                match node.op {
                    InputOp::Sum => acc += y,
                    InputOp::Product => acc *= y,
                }
            }
            if node.inputs.is_empty() {
                acc = 0.0;
            }
            acc += curve(&node.name).unwrap_or(0.0);
            acc *= 1.0 - correction;
            let v = if acc > node.max {
                node.max
            } else if acc < node.min {
                node.min
            } else {
                acc
            };
            values.push(if v.is_finite() { v } else { 0.0 });
        }
        values
    }

    /// Each moved bone's offset from its rest for node values `values` (from
    /// [`FaceActor::evaluate`]), in the skeleton's terms: the rig's centimetres are metres
    /// over 100, and its axes are the skeleton's with y and z swapped (a mirror), so a
    /// rig quaternion (w, x, y, z) is the skeleton's (−x, −z, −y, w).
    pub fn bone_offsets(&self, values: &[f32]) -> Vec<BoneOffset> {
        let mut out = Vec::new();
        for b in &self.bones {
            let inverse = [
                b.rotation[0],
                -b.rotation[1],
                -b.rotation[2],
                -b.rotation[3],
            ];
            let mut t = [0.0f32; 3];
            let mut q: Quat = [1.0, 0.0, 0.0, 0.0];
            let mut moved = false;
            for l in &b.links {
                let w = values.get(l.node).copied().unwrap_or(0.0);
                if w == 0.0 {
                    continue;
                }
                moved = true;
                for (a, d) in t.iter_mut().zip(l.translation) {
                    *a += d * w;
                }
                let turn = qmul(inverse, qnormalize(l.rotation));
                q = qmul(q, qtoward(turn, w.clamp(-1.0, 1.0).abs()));
            }
            if !moved {
                continue;
            }
            let q = qnormalize(q);
            out.push(BoneOffset {
                bone: b.skeleton_name(),
                translation: [t[0] / 100.0, t[2] / 100.0, t[1] / 100.0],
                rotation: [-q[1], -q[3], -q[2], q[0]],
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &mut Vec<u8>, text: &str) {
        v.extend((text.len() as u32).to_le_bytes());
        v.extend(text.as_bytes());
    }
    fn u(v: &mut Vec<u8>, x: u32) {
        v.extend(x.to_le_bytes());
    }
    fn f(v: &mut Vec<u8>, x: f32) {
        v.extend(x.to_le_bytes());
    }
    fn head(names: &[&str]) -> Vec<u8> {
        let mut v = b"BIOE".to_vec();
        u(&mut v, 1710);
        s(&mut v, "Licensee");
        s(&mut v, "Project");
        u(&mut v, 1000);
        v.extend(1u16.to_le_bytes());
        u(&mut v, 1);
        u(&mut v, 26);
        u(&mut v, 1);
        s(&mut v, "FxObject");
        v.extend(0u16.to_le_bytes());
        u(&mut v, names.len() as u32);
        for n in names {
            s(&mut v, n);
        }
        v
    }

    fn anim() -> Vec<u8> {
        let mut v = head(&["1_m", "rig", "jawOpen", "Blink"]);
        for x in [0, 0, 1, 0, 1, 0] {
            u(&mut v, x);
        }
        u(&mut v, 2);
        for n in [2, 3] {
            u(&mut v, n);
            u(&mut v, 0);
        }
        u(&mut v, 3);
        // jawOpen: 0 at 0 s, 1 at 1 s; Blink: 0.5 throughout.
        for k in [
            [0.0, 0.0, 0.0, 0.0],
            [1.0, 1.0, 0.0, 0.0],
            [0.0, 0.5, 0.0, 0.0],
        ] {
            for x in k {
                f(&mut v, x);
            }
        }
        u(&mut v, 2);
        u(&mut v, 2);
        u(&mut v, 1);
        f(&mut v, 0.16);
        f(&mut v, 0.22);
        v
    }

    #[test]
    fn reads_a_lines_curves() {
        let a = FaceAnim::parse(&anim()).unwrap();
        assert_eq!(a.name, "1_m");
        assert_eq!(a.actor.as_deref(), Some("rig"));
        assert_eq!(a.curves.len(), 2);
        let jaw = a.curve("jawOpen").unwrap();
        assert_eq!(jaw.at(-1.0), 0.0);
        assert_eq!(jaw.at(2.0), 1.0);
        // Flat slopes: the Hermite curve eases through the middle at half.
        assert!((jaw.at(0.5) - 0.5).abs() < 1e-6);
        assert!(jaw.at(0.25) < 0.25);
        assert_eq!(a.curve("Blink").unwrap().at(3.0), 0.5);
        assert_eq!(a.blend_in, 0.16);
    }

    #[test]
    fn bad_files_are_errors_not_panics() {
        let good = anim();
        for n in 0..good.len() {
            assert!(FaceAnim::parse(&good[..n]).is_err(), "truncated at {n}");
        }
        assert!(matches!(
            FaceAnim::parse(b"RIFF...."),
            Err(FaceFxError::Magic)
        ));
        let rig = actor();
        for n in 0..rig.len() {
            assert!(FaceActor::parse(&rig[..n]).is_err(), "truncated at {n}");
        }
        let mut huge = head(&["x"]);
        u(&mut huge, u32::MAX);
        assert!(FaceAnim::parse(&huge).is_err());
    }

    /// A rig with nodes `jawOpen` (a bone pose), `lipCurl` (a combiner, -1 to 1),
    /// `smile` (a bone pose: twice `lipCurl`, corrected by `jawOpen` at 0.5), and one bone,
    /// `default_jawBone`, which `jawOpen` turns a quarter turn about the rig's x and moves
    /// 2 cm down the rig's z.
    fn actor() -> Vec<u8> {
        let mut v = head(&["rig", "default_jawBone", "jawOpen", "lipCurl", "smile"]);
        u(&mut v, 0);
        u(&mut v, 0);
        u(&mut v, 1);
        u(&mut v, 1);
        for x in [
            0.0, -1.0, -2.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0,
        ] {
            f(&mut v, x);
        }
        u(&mut v, 18);
        f(&mut v, 1.0);
        u(&mut v, 1);
        u(&mut v, 0);
        u(&mut v, 1);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        for x in [0.0, 0.0, -2.0, h, h, 0.0, 0.0, 0.0, 0.0, 0.0] {
            f(&mut v, x);
        }
        u(&mut v, 3);
        // jawOpen.
        for x in [4, 2] {
            u(&mut v, x);
        }
        for x in [0.0, 1.0, 1.0, 1.0] {
            f(&mut v, x);
        }
        u(&mut v, 0);
        u(&mut v, 0);
        u(&mut v, 0);
        // lipCurl.
        for x in [0, 3] {
            u(&mut v, x);
        }
        for x in [-1.0, -1.0, 1.0, 1.0] {
            f(&mut v, x);
        }
        u(&mut v, 0);
        u(&mut v, 0);
        u(&mut v, 0);
        // smile.
        for x in [4, 4] {
            u(&mut v, x);
        }
        for x in [0.0, 1.0, 1.0, 1.0] {
            f(&mut v, x);
        }
        u(&mut v, 0);
        u(&mut v, 2);
        u(&mut v, 1);
        u(&mut v, link_fn::LINEAR);
        u(&mut v, 2);
        f(&mut v, 2.0);
        f(&mut v, 0.0);
        u(&mut v, 0);
        u(&mut v, link_fn::CORRECTIVE);
        u(&mut v, 1);
        f(&mut v, 0.5);
        // A user property, as a material parameter node has.
        u(&mut v, 1);
        u(&mut v, 3);
        u(&mut v, 3);
        u(&mut v, 0);
        f(&mut v, 0.0);
        u(&mut v, 1);
        s(&mut v, "FaceM1");
        v
    }

    #[test]
    fn the_graph_and_the_bones() {
        let a = FaceActor::parse(&actor()).unwrap();
        assert_eq!(a.name, "rig");
        assert_eq!(a.bones[0].skeleton_name(), "jawbone");
        let curves = |jaw: f32, curl: f32| {
            move |n: &str| match n {
                "jawOpen" => Some(jaw),
                "lipCurl" => Some(curl),
                _ => None,
            }
        };
        // Held to range: 2 × 0.3 = 0.6, corrected by half the jaw's opening.
        let v = a.evaluate(curves(0.4, 0.3));
        assert_eq!(v[0], 0.4);
        assert!((v[2] - 0.6 * 0.8).abs() < 1e-6);
        assert_eq!(a.evaluate(curves(3.0, -2.0))[..2], [1.0, -1.0]);

        // Fully open: the jaw turns a quarter turn about the rig's x (the skeleton's −x)
        // and moves 2 cm down the rig's z (the skeleton's y).
        let o = a.bone_offsets(&a.evaluate(curves(1.0, 0.0)));
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].bone, "jawbone");
        assert!((o[0].translation[1] + 0.02).abs() < 1e-6);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let r = o[0].rotation;
        assert!((r[0] + h).abs() < 1e-5 && (r[3] - h).abs() < 1e-5, "{r:?}");
        // Closed: nothing moves.
        assert!(a.bone_offsets(&a.evaluate(curves(0.0, 0.0))).is_empty());
    }
}
