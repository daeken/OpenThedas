//! Translation of D3D9 shaders to naga IR.
//!
//! The translator emits WGSL that mirrors the bytecode instruction by instruction, then parses
//! it with naga's WGSL front end and runs naga's validator over the result, so the output is a
//! validated [`naga::Module`] (plus the WGSL text, which is far easier to debug than raw IR).
//!
//! # Conventions
//!
//! - Registers are `vec4<f32>` private variables (`r0`, `v0`, `o0`, `oc0`, ...). Constants
//!   declared with `def` are assigned at the start of the entry point rather than inlined, so
//!   naga never constant-folds expressions D3D evaluates at run time (`rcp` of a zero constant).
//! - Float, int and bool constants live in uniform arrays indexed by register number; the
//!   host fills them by name using the shader's constant table. Bindings: group 0, binding 0/1/2
//!   for vertex float/int/bool constants, 3/4/5 for pixel float/int/bool constants, 6 for the
//!   D3D9 fix-up vector. Samplers are group 1: texture at binding `2 * s`, sampler at
//!   `2 * s + 1` (vertex texture samplers start at binding 32).
//! - Inter-stage values use locations derived from their D3D semantic ([`location_of`]), so a
//!   vertex shader and a pixel shader translated separately agree on the interface.
//! - D3D9 rasterizes with pixel centres at integer coordinates; wgpu uses half-integers. The
//!   vertex shader shifts clip-space positions by half a pixel using the fix-up vector
//!   (`x = 1 / width`, `y = 1 / height`), and `vPos` subtracts 0.5. Clip-space depth is 0..w in
//!   both APIs and texture coordinates share the top-left origin, so neither needs changes.
//! - Alpha test is render state in D3D9; it is appended to the pixel shader when requested.
//!
//! Instruction semantics follow Microsoft's public D3D9 shader reference. Where the reference
//! leaves room (rounding in `mova`, `nrm` of a zero vector), the choice is marked provisional.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::bytecode::{
    Comparison, Decl, DefValue, Dst, Instruction, Opcode, RegType, Register, Shader, Src, SrcMod,
    Stage, TextureType, Usage,
};
use crate::ctab::ConstantTable;

#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum TranslateError {
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("malformed shader: {0}")]
    Malformed(String),
    #[error("naga rejected the generated WGSL: {0}")]
    Naga(String),
}

impl TranslateError {
    /// A short key for grouping failures in reports (the missing opcode or feature).
    pub fn key(&self) -> String {
        match self {
            TranslateError::Unsupported(s) => s.split(':').next().unwrap_or(s).to_string(),
            TranslateError::Malformed(_) => "malformed".into(),
            TranslateError::Naga(_) => "naga".into(),
        }
    }
}

type Result<T> = std::result::Result<T, TranslateError>;

fn unsupported(what: impl Into<String>) -> TranslateError {
    TranslateError::Unsupported(what.into())
}

/// A D3D semantic: usage and index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Semantic {
    pub usage: Usage,
    pub index: u8,
}

/// The inter-stage location for a semantic. Texture coordinates 0..7 map to 0..7, colours to
/// 8 and 9, fog to 10, and a few other usages vertex shaders pass through to 11..15.
pub fn location_of(s: Semantic) -> Option<u32> {
    Some(match (s.usage, s.index) {
        (Usage::TexCoord, i) if i < 8 => i as u32,
        (Usage::Color, i) if i < 2 => 8 + i as u32,
        (Usage::Fog, 0) => 10,
        (Usage::Normal, 0) => 11,
        (Usage::Tangent, 0) => 12,
        (Usage::Binormal, 0) => 13,
        (Usage::Position, i) if i >= 1 => 14,
        (Usage::PSize, 0) => 15,
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct IoSlot {
    pub semantic: Semantic,
    /// Register number (`v#` for vertex inputs).
    pub register: u32,
    /// Shader location: the register for vertex inputs, [`location_of`] otherwise.
    pub location: u32,
}

/// What the host needs to know to run a translated shader.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Interface {
    pub inputs: Vec<IoSlot>,
    pub outputs: Vec<IoSlot>,
    pub samplers: Vec<(u32, TextureType)>,
    pub float_defs: Vec<(u32, [f32; 4])>,
    pub int_defs: Vec<(u32, [i32; 4])>,
    pub bool_defs: Vec<(u32, bool)>,
    /// Pixel shader colour outputs written (`oC#`).
    pub color_outputs: Vec<u32>,
    pub writes_depth: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Vertex shaders: semantics the paired pixel shader reads that this shader must output
    /// even if it never writes them (they read as zero).
    pub extra_outputs: Vec<Semantic>,
    /// Pixel shaders: how many colour targets the pipeline has. Writes to `oC#` beyond this are
    /// dropped. `None` keeps every output.
    pub color_targets: Option<u32>,
    /// Pixel shaders: D3D9 alpha test (`ALPHAFUNC`, `ALPHAREF / 255`).
    pub alpha_test: Option<(Comparison, f32)>,
    /// Pixel shaders: take the alpha test's reference (0–255) from the fix-up vector's `z`
    /// instead of the constant in [`Options::alpha_test`], for draws whose reference changes
    /// (D3D9's `ALPHAREF` render state set per draw).
    pub alpha_ref_uniform: bool,
}

pub struct Translation {
    pub wgsl: String,
    pub module: naga::Module,
    pub info: naga::valid::ModuleInfo,
    pub interface: Interface,
    pub stage: Stage,
    pub ctab: Option<ConstantTable>,
}

pub const ENTRY_POINT: &str = "main";
pub const FLOAT_CONSTANTS: u32 = 256;
pub const INT_CONSTANTS: u32 = 16;
pub const BOOL_CONSTANTS: u32 = 16;
pub const GROUP_CONSTANTS: u32 = 0;
pub const GROUP_SAMPLERS: u32 = 1;
pub const BINDING_FIXUP: u32 = 6;
pub const VERTEX_SAMPLER_BASE: u32 = 32;

pub fn binding_float(stage: Stage) -> u32 {
    match stage {
        Stage::Vertex => 0,
        Stage::Pixel => 3,
    }
}
pub fn binding_int(stage: Stage) -> u32 {
    binding_float(stage) + 1
}
pub fn binding_bool(stage: Stage) -> u32 {
    binding_float(stage) + 2
}
pub fn binding_texture(stage: Stage, sampler: u32) -> u32 {
    let base = if stage == Stage::Vertex {
        VERTEX_SAMPLER_BASE
    } else {
        0
    };
    base + 2 * sampler
}

const COMP: [char; 4] = ['x', 'y', 'z', 'w'];

fn flit(f: f32) -> String {
    if f.is_finite() {
        let s = format!("{f:?}");
        if s.contains('.') || s.contains('e') {
            s
        } else {
            format!("{s}.0")
        }
    } else {
        format!("bitcast<f32>({}u)", f.to_bits())
    }
}

fn vec4_lit(v: [f32; 4]) -> String {
    format!(
        "vec4<f32>({}, {}, {}, {})",
        flit(v[0]),
        flit(v[1]),
        flit(v[2]),
        flit(v[3])
    )
}

struct Gen<'a> {
    shader: &'a Shader,
    opts: &'a Options,
    stage: Stage,
    body: String,
    indent: usize,
    temps: BTreeSet<u32>,
    inputs: BTreeMap<u32, Semantic>,
    /// PS2 texture-coordinate inputs (`t#`).
    tex_inputs: BTreeSet<u32>,
    /// PS2 colour inputs without a dcl semantic.
    outputs: BTreeMap<String, Semantic>,
    samplers: BTreeMap<u32, TextureType>,
    float_defs: BTreeMap<u32, [f32; 4]>,
    int_defs: BTreeMap<u32, [i32; 4]>,
    bool_defs: BTreeMap<u32, bool>,
    color_outputs: BTreeSet<u32>,
    writes_depth: bool,
    uses_a0: bool,
    uses_al: bool,
    uses_pos: bool,
    uses_face: bool,
    uses_fog: bool,
    uses_psize: bool,
    loop_depth: usize,
    /// Centroid interpolation per input register.
    centroid: BTreeSet<u32>,
}

impl<'a> Gen<'a> {
    fn line(&mut self, s: impl AsRef<str>) {
        for _ in 0..self.indent {
            self.body.push_str("    ");
        }
        self.body.push_str(s.as_ref());
        self.body.push('\n');
    }

    fn is_ps2(&self) -> bool {
        self.stage == Stage::Pixel && self.shader.major < 3
    }

    /// The private variable that holds a register.
    fn reg_var(&mut self, r: Register) -> Result<String> {
        use RegType::*;
        Ok(match (r.ty, self.stage) {
            (Temp, _) | (TempFloat16, _) => {
                self.temps.insert(r.num);
                format!("r{}", r.num)
            }
            (Input, Stage::Vertex) => format!("v{}", r.num),
            (Input, Stage::Pixel) => {
                if self.is_ps2() && !self.inputs.contains_key(&r.num) {
                    // ps_2_x colour inputs v0/v1 are implicitly COLOR0/1.
                    self.inputs.insert(
                        r.num,
                        Semantic {
                            usage: Usage::Color,
                            index: r.num as u8,
                        },
                    );
                }
                format!("v{}", r.num)
            }
            (AddrOrTexture, Stage::Pixel) => {
                self.tex_inputs.insert(r.num);
                format!("t{}", r.num)
            }
            (AddrOrTexture, Stage::Vertex) => {
                self.uses_a0 = true;
                "vec4<f32>(a0)".into()
            }
            (Const, _) => {
                if self.float_defs.contains_key(&r.num) {
                    format!("cd{}", r.num)
                } else {
                    format!("cf.c[{}]", r.num)
                }
            }
            (ConstInt, _) => {
                if self.int_defs.contains_key(&r.num) {
                    format!("vec4<f32>(ci{})", r.num)
                } else {
                    format!("vec4<f32>(ci_u.c[{}])", r.num)
                }
            }
            (Loop, _) => {
                self.uses_al = true;
                "vec4<f32>(f32(aL))".into()
            }
            (MiscType, Stage::Pixel) => match r.num {
                0 => {
                    self.uses_pos = true;
                    "vpos".into()
                }
                1 => {
                    self.uses_face = true;
                    "vface".into()
                }
                n => return Err(unsupported(format!("misc register {n}"))),
            },
            (ty, st) => return Err(unsupported(format!("source register {ty:?} in {st:?}"))),
        })
    }

    fn rel_index(&mut self, rel: (Register, u8), base: u32) -> Result<String> {
        let c = COMP[rel.1 as usize];
        let idx = match rel.0.ty {
            RegType::AddrOrTexture if self.stage == Stage::Vertex => {
                self.uses_a0 = true;
                format!("a0.{c}")
            }
            RegType::Loop => {
                self.uses_al = true;
                "aL".into()
            }
            other => {
                return Err(unsupported(format!(
                    "relative addressing through {other:?}"
                )));
            }
        };
        Ok(format!(
            "clamp({base} + {idx}, 0, {})",
            FLOAT_CONSTANTS as i32 - 1
        ))
    }

    /// A source operand as a `vec4<f32>` expression, optionally offset by `extra` registers
    /// (for the matrix opcodes).
    fn src(&mut self, s: &Src, extra: u32) -> Result<String> {
        let reg = Register {
            ty: s.reg.ty,
            num: s.reg.num + extra,
        };
        let base = match s.relative {
            Some(rel) if reg.ty == RegType::Const => {
                format!("cf.c[{}]", self.rel_index(rel, reg.num)?)
            }
            Some(_) => return Err(unsupported("relative addressing of non-constant registers")),
            None => self.reg_var(reg)?,
        };
        let sw = if s.swizzle == [0, 1, 2, 3] {
            String::new()
        } else {
            let mut out = String::from(".");
            for c in s.swizzle {
                out.push(COMP[c as usize]);
            }
            out
        };
        let e = format!("{base}{sw}");
        Ok(match s.modifier {
            SrcMod::None => e,
            SrcMod::Neg => format!("(-{e})"),
            SrcMod::Abs => format!("abs({e})"),
            SrcMod::AbsNeg => format!("(-abs({e}))"),
            SrcMod::Bias => format!("({e} - vec4<f32>(0.5))"),
            SrcMod::BiasNeg => format!("(vec4<f32>(0.5) - {e})"),
            SrcMod::Sign => format!("({e} * 2.0 - vec4<f32>(1.0))"),
            SrcMod::SignNeg => format!("(vec4<f32>(1.0) - {e} * 2.0)"),
            SrcMod::Comp => format!("(vec4<f32>(1.0) - {e})"),
            SrcMod::X2 => format!("({e} * 2.0)"),
            SrcMod::X2Neg => format!("({e} * -2.0)"),
            m => return Err(unsupported(format!("source modifier {m:?}"))),
        })
    }

    /// The variable a destination writes to.
    fn dst_var(&mut self, d: &Dst) -> Result<String> {
        use RegType::*;
        if d.relative.is_some() {
            return Err(unsupported("relative destination"));
        }
        let n = d.reg.num;
        Ok(match (d.reg.ty, self.stage) {
            (Temp, _) | (TempFloat16, _) => {
                self.temps.insert(n);
                format!("r{n}")
            }
            (Output, Stage::Vertex) => {
                if self.shader.major < 3 {
                    self.outputs.entry(format!("o{n}")).or_insert(Semantic {
                        usage: Usage::TexCoord,
                        index: n as u8,
                    });
                }
                format!("o{n}")
            }
            (AttrOut, Stage::Vertex) => {
                self.outputs.entry(format!("od{n}")).or_insert(Semantic {
                    usage: Usage::Color,
                    index: n as u8,
                });
                format!("od{n}")
            }
            (RastOut, Stage::Vertex) => match n {
                0 => "opos".into(),
                1 => {
                    self.uses_fog = true;
                    "ofog".into()
                }
                2 => {
                    self.uses_psize = true;
                    "opts".into()
                }
                _ => return Err(unsupported(format!("rastout register {n}"))),
            },
            (ColorOut, Stage::Pixel) => {
                self.color_outputs.insert(n);
                format!("oc{n}")
            }
            (DepthOut, Stage::Pixel) => {
                self.writes_depth = true;
                "odepth".into()
            }
            (ty, st) => {
                return Err(unsupported(format!(
                    "destination register {ty:?} in {st:?}"
                )));
            }
        })
    }

    /// Writes a `vec4<f32>` expression to a destination under its mask and modifiers.
    fn write(&mut self, d: &Dst, value: &str) -> Result<()> {
        let var = self.dst_var(d)?;
        let mut v = value.to_string();
        if d.shift != 0 {
            v = format!("({v} * {})", flit(2f32.powi(d.shift as i32)));
        }
        if d.saturate {
            v = format!("saturate({v})");
        }
        if d.mask == 0xf {
            self.line(format!("{var} = {v};"));
        } else {
            self.line(format!("{{ let t = {v};"));
            for (i, c) in COMP.iter().enumerate() {
                if d.mask & (1 << i) != 0 {
                    self.line(format!("    {var}.{c} = t.{c};"));
                }
            }
            self.line("}");
        }
        Ok(())
    }

    fn dst(ins: &Instruction) -> Result<Dst> {
        ins.dst.ok_or_else(|| {
            TranslateError::Malformed(format!("{} without destination", ins.opcode.name()))
        })
    }

    fn srcs(&mut self, ins: &Instruction, n: usize) -> Result<Vec<String>> {
        if ins.src.len() < n {
            return Err(TranslateError::Malformed(format!(
                "{} needs {n} sources",
                ins.opcode.name()
            )));
        }
        (0..n).map(|i| self.src(&ins.src[i], 0)).collect()
    }

    fn scalar(e: &str) -> String {
        format!("vec4<f32>({e})")
    }

    fn cmp_expr(op: Comparison, a: &str, b: &str) -> String {
        let o = match op {
            Comparison::Gt => ">",
            Comparison::Eq => "==",
            Comparison::Ge => ">=",
            Comparison::Lt => "<",
            Comparison::Ne => "!=",
            Comparison::Le => "<=",
        };
        format!("({a} {o} {b})")
    }

    fn bool_expr(&mut self, s: &Src) -> Result<String> {
        match s.reg.ty {
            RegType::ConstBool => Ok(if self.bool_defs.contains_key(&s.reg.num) {
                format!("cb{}", s.reg.num)
            } else {
                format!("(cb_u.c[{}].x != 0u)", s.reg.num)
            }),
            RegType::Predicate => Err(unsupported("predicate registers")),
            other => Err(TranslateError::Malformed(format!("if on {other:?}"))),
        }
    }

    fn texture_coords(&self, s: u32, coord: &str) -> Result<String> {
        Ok(match self.samplers.get(&s) {
            Some(TextureType::Cube) | Some(TextureType::Volume) => format!("{coord}.xyz"),
            Some(TextureType::Tex2D) | Some(TextureType::Unknown) | None => format!("{coord}.xy"),
        })
    }

    fn sampler_names(&self, s: u32) -> (String, String) {
        let p = if self.stage == Stage::Vertex {
            "vt"
        } else {
            "t"
        };
        (format!("{p}ex{s}"), format!("{p}smp{s}"))
    }

    /// Applies the swizzle on a sampler operand (ps_3_0 allows one) to a sample result.
    fn sampler_swizzle(s: &Src, e: String) -> String {
        if s.swizzle == [0, 1, 2, 3] {
            e
        } else {
            let sw: String = s.swizzle.iter().map(|&c| COMP[c as usize]).collect();
            format!("({e}).{sw}")
        }
    }

    fn instruction(&mut self, ins: &Instruction) -> Result<()> {
        use Opcode::*;
        if ins.predicate.is_some() {
            return Err(unsupported("predication"));
        }
        let op = ins.opcode;
        match op {
            Nop | Dcl | Def | DefI | DefB => {}
            Mov => {
                let s = self.srcs(ins, 1)?;
                self.write(&Self::dst(ins)?, &s[0])?;
            }
            Add | Sub | Mul => {
                let s = self.srcs(ins, 2)?;
                let o = match op {
                    Add => "+",
                    Sub => "-",
                    _ => "*",
                };
                self.write(&Self::dst(ins)?, &format!("({} {o} {})", s[0], s[1]))?;
            }
            Mad => {
                let s = self.srcs(ins, 3)?;
                self.write(
                    &Self::dst(ins)?,
                    &format!("({} * {} + {})", s[0], s[1], s[2]),
                )?;
            }
            Rcp => {
                let s = self.srcs(ins, 1)?;
                self.write(
                    &Self::dst(ins)?,
                    &Self::scalar(&format!("1.0 / {}.x", s[0])),
                )?;
            }
            Rsq => {
                let s = self.srcs(ins, 1)?;
                self.write(
                    &Self::dst(ins)?,
                    &Self::scalar(&format!("inverseSqrt(abs({}.x))", s[0])),
                )?;
            }
            Dp3 => {
                let s = self.srcs(ins, 2)?;
                self.write(
                    &Self::dst(ins)?,
                    &Self::scalar(&format!("dot({}.xyz, {}.xyz)", s[0], s[1])),
                )?;
            }
            Dp4 => {
                let s = self.srcs(ins, 2)?;
                self.write(
                    &Self::dst(ins)?,
                    &Self::scalar(&format!("dot({}, {})", s[0], s[1])),
                )?;
            }
            Dp2Add => {
                let s = self.srcs(ins, 3)?;
                self.write(
                    &Self::dst(ins)?,
                    &Self::scalar(&format!("dot({}.xy, {}.xy) + {}.x", s[0], s[1], s[2])),
                )?;
            }
            Min | Max => {
                let s = self.srcs(ins, 2)?;
                let f = if op == Min { "min" } else { "max" };
                self.write(&Self::dst(ins)?, &format!("{f}({}, {})", s[0], s[1]))?;
            }
            Slt | Sge => {
                let s = self.srcs(ins, 2)?;
                let o = if op == Slt { "<" } else { ">=" };
                self.write(
                    &Self::dst(ins)?,
                    &format!(
                        "select(vec4<f32>(0.0), vec4<f32>(1.0), {} {o} {})",
                        s[0], s[1]
                    ),
                )?;
            }
            Exp | ExpP => {
                let s = self.srcs(ins, 1)?;
                self.write(
                    &Self::dst(ins)?,
                    &Self::scalar(&format!("exp2({}.x)", s[0])),
                )?;
            }
            Log | LogP => {
                let s = self.srcs(ins, 1)?;
                self.write(
                    &Self::dst(ins)?,
                    &Self::scalar(&format!("log2(abs({}.x))", s[0])),
                )?;
            }
            Pow => {
                let s = self.srcs(ins, 2)?;
                self.write(
                    &Self::dst(ins)?,
                    &Self::scalar(&format!("pow(abs({}.x), {}.x)", s[0], s[1])),
                )?;
            }
            Lit => {
                let s = self.srcs(ins, 1)?;
                let a = &s[0];
                self.write(
                    &Self::dst(ins)?,
                    &format!(
                        "vec4<f32>(1.0, max({a}.x, 0.0), select(0.0, pow(max({a}.y, 0.0), clamp({a}.w, -128.0, 128.0)), {a}.x > 0.0), 1.0)"
                    ),
                )?;
            }
            Dst => {
                let s = self.srcs(ins, 2)?;
                self.write(
                    &Self::dst(ins)?,
                    &format!("vec4<f32>(1.0, {0}.y * {1}.y, {0}.z, {1}.w)", s[0], s[1]),
                )?;
            }
            Lrp => {
                let s = self.srcs(ins, 3)?;
                self.write(
                    &Self::dst(ins)?,
                    &format!("mix({}, {}, {})", s[2], s[1], s[0]),
                )?;
            }
            Frc => {
                let s = self.srcs(ins, 1)?;
                self.write(&Self::dst(ins)?, &format!("fract({})", s[0]))?;
            }
            Abs => {
                let s = self.srcs(ins, 1)?;
                self.write(&Self::dst(ins)?, &format!("abs({})", s[0]))?;
            }
            Sgn => {
                let s = self.srcs(ins, 1)?;
                self.write(&Self::dst(ins)?, &format!("sign({})", s[0]))?;
            }
            Nrm => {
                // D3D scales all four components by 1/|xyz|. A zero vector gives inf/NaN in
                // D3D too (provisional: drivers differ).
                let s = self.srcs(ins, 1)?;
                self.write(
                    &Self::dst(ins)?,
                    &format!("({0} * inverseSqrt(dot({0}.xyz, {0}.xyz)))", s[0]),
                )?;
            }
            Crs => {
                let s = self.srcs(ins, 2)?;
                self.write(
                    &Self::dst(ins)?,
                    &format!("vec4<f32>(cross({}.xyz, {}.xyz), 0.0)", s[0], s[1]),
                )?;
            }
            SinCos => {
                let s = self.src(&ins.src[0], 0)?;
                self.write(
                    &Self::dst(ins)?,
                    &format!("vec4<f32>(cos({s}.x), sin({s}.x), 0.0, 0.0)"),
                )?;
            }
            Cmp => {
                let s = self.srcs(ins, 3)?;
                self.write(
                    &Self::dst(ins)?,
                    &format!("select({}, {}, {} >= vec4<f32>(0.0))", s[2], s[1], s[0]),
                )?;
            }
            Cnd => {
                let s = self.srcs(ins, 3)?;
                self.write(
                    &Self::dst(ins)?,
                    &format!("select({}, {}, {} > vec4<f32>(0.5))", s[2], s[1], s[0]),
                )?;
            }
            M4x4 | M4x3 | M3x4 | M3x3 | M3x2 => {
                let (rows, cols) = match op {
                    M4x4 => (4, 4),
                    M4x3 => (3, 4),
                    M3x4 => (4, 3),
                    M3x3 => (3, 3),
                    _ => (2, 3),
                };
                if ins.src.len() < 2 {
                    return Err(TranslateError::Malformed(
                        "matrix op needs 2 sources".into(),
                    ));
                }
                let a = self.src(&ins.src[0], 0)?;
                let mut parts = Vec::new();
                for i in 0..4 {
                    if i < rows {
                        let m = self.src(&ins.src[1], i)?;
                        parts.push(if cols == 4 {
                            format!("dot({a}, {m})")
                        } else {
                            format!("dot({a}.xyz, {m}.xyz)")
                        });
                    } else {
                        parts.push("0.0".into());
                    }
                }
                self.write(
                    &Self::dst(ins)?,
                    &format!("vec4<f32>({})", parts.join(", ")),
                )?;
            }
            Mova => {
                let d = Self::dst(ins)?;
                let s = self.srcs(ins, 1)?;
                self.uses_a0 = true;
                // Round to nearest, halves away from zero (provisional).
                self.line(format!("{{ let t = vec4<i32>(round_half_away({}));", s[0]));
                for (i, c) in COMP.iter().enumerate() {
                    if d.mask & (1 << i) != 0 {
                        self.line(format!("    a0.{c} = t.{c};"));
                    }
                }
                self.line("}");
            }
            Dsx | Dsy => {
                let s = self.srcs(ins, 1)?;
                let f = if op == Dsx { "dpdx" } else { "dpdy" };
                self.write(&Self::dst(ins)?, &format!("{f}({})", s[0]))?;
            }
            Tex | TexLdl | TexLdd => {
                let d = Self::dst(ins)?;
                if ins.src.len() < 2 || ins.src[1].reg.ty != RegType::Sampler {
                    return Err(TranslateError::Malformed(
                        "texture op without sampler".into(),
                    ));
                }
                let smp = ins.src[1];
                let s = smp.reg.num;
                self.samplers.entry(s).or_insert(TextureType::Tex2D);
                let coord = self.src(&ins.src[0], 0)?;
                let (tex, sampler) = self.sampler_names(s);
                let uv = self.texture_coords(s, &coord)?;
                let e = match (op, ins.control) {
                    (Tex, _) if self.stage == Stage::Vertex => {
                        return Err(unsupported("texld in a vertex shader"));
                    }
                    (Tex, 0) => format!("textureSample({tex}, {sampler}, {uv})"),
                    (Tex, 1) => {
                        let pc = format!("({coord} / {coord}.w)");
                        let uv = self.texture_coords(s, &pc)?;
                        format!("textureSample({tex}, {sampler}, {uv})")
                    }
                    (Tex, 2) => format!("textureSampleBias({tex}, {sampler}, {uv}, {coord}.w)"),
                    (Tex, c) => return Err(unsupported(format!("texld control {c}"))),
                    (TexLdl, _) => format!("textureSampleLevel({tex}, {sampler}, {uv}, {coord}.w)"),
                    _ => {
                        if ins.src.len() < 4 {
                            return Err(TranslateError::Malformed("texldd needs 4 sources".into()));
                        }
                        let dx = self.src(&ins.src[2], 0)?;
                        let dy = self.src(&ins.src[3], 0)?;
                        let (gx, gy) = (self.texture_coords(s, &dx)?, self.texture_coords(s, &dy)?);
                        format!("textureSampleGrad({tex}, {sampler}, {uv}, {gx}, {gy})")
                    }
                };
                let e = Self::sampler_swizzle(&smp, e);
                self.write(&d, &e)?;
            }
            TexKill => {
                let d = Self::dst(ins)?;
                let reg = d.reg;
                let v = self.reg_var(reg)?;
                let comps: Vec<String> = (0..4)
                    .filter(|i| d.mask & (1 << i) != 0)
                    .map(|i| format!("{v}.{} < 0.0", COMP[i]))
                    .collect();
                if !comps.is_empty() {
                    self.line(format!("if ({}) {{ discard; }}", comps.join(" || ")));
                }
            }
            If => {
                let b = self.bool_expr(&ins.src[0])?;
                self.line(format!("if ({b}) {{"));
                self.indent += 1;
            }
            IfC => {
                let s = self.srcs(ins, 2)?;
                let c = Comparison::from_control(ins.control).ok_or_else(|| {
                    TranslateError::Malformed(format!("ifc control {}", ins.control))
                })?;
                self.line(format!(
                    "if {} {{",
                    Self::cmp_expr(c, &format!("{}.x", s[0]), &format!("{}.x", s[1]))
                ));
                self.indent += 1;
            }
            Else => {
                self.indent = self.indent.saturating_sub(1);
                self.line("} else {");
                self.indent += 1;
            }
            EndIf | EndRep | EndLoop => {
                if op != EndIf {
                    self.loop_depth = self.loop_depth.saturating_sub(1);
                }
                self.indent = self.indent.saturating_sub(1);
                self.line("}");
            }
            Rep => {
                let n = self.int_x(&ins.src[0])?;
                let k = self.loop_depth;
                self.loop_depth += 1;
                self.line(format!(
                    "for (var rep{k}: i32 = 0; rep{k} < {n}; rep{k} = rep{k} + 1) {{"
                ));
                self.indent += 1;
            }
            Loop => {
                // loop aL, i#: count = i.x, start = i.y, step = i.z.
                let i = self.int_vec(&ins.src[1])?;
                self.uses_al = true;
                let k = self.loop_depth;
                self.loop_depth += 1;
                self.line(format!("aL = {i}.y;"));
                self.line(format!(
                    "for (var lp{k}: i32 = 0; lp{k} < {i}.x; lp{k} = lp{k} + 1) {{"
                ));
                self.indent += 1;
                // The step is applied at the top of the next iteration via a deferred add.
                self.line(format!("if (lp{k} > 0) {{ aL = aL + {i}.z; }}"));
            }
            Break => self.line("break;"),
            BreakC => {
                let s = self.srcs(ins, 2)?;
                let c = Comparison::from_control(ins.control).ok_or_else(|| {
                    TranslateError::Malformed(format!("breakc control {}", ins.control))
                })?;
                self.line(format!(
                    "if {} {{ break; }}",
                    Self::cmp_expr(c, &format!("{}.x", s[0]), &format!("{}.x", s[1]))
                ));
            }
            Ret => {
                if self.indent > 1 {
                    return Err(unsupported("ret inside control flow"));
                }
            }
            other => return Err(unsupported(other.name())),
        }
        Ok(())
    }

    fn int_vec(&mut self, s: &Src) -> Result<String> {
        if s.reg.ty != RegType::ConstInt {
            return Err(TranslateError::Malformed(
                "loop counter is not an int constant".into(),
            ));
        }
        Ok(if self.int_defs.contains_key(&s.reg.num) {
            format!("ci{}", s.reg.num)
        } else {
            format!("ci_u.c[{}]", s.reg.num)
        })
    }

    fn int_x(&mut self, s: &Src) -> Result<String> {
        Ok(format!("{}.x", self.int_vec(s)?))
    }
}

/// Translates a parsed shader to WGSL and validates it with naga.
pub fn translate(shader: &Shader, opts: &Options) -> Result<Translation> {
    let mut g = Gen {
        shader,
        opts,
        stage: shader.stage,
        body: String::new(),
        indent: 1,
        temps: BTreeSet::new(),
        inputs: BTreeMap::new(),
        tex_inputs: BTreeSet::new(),
        outputs: BTreeMap::new(),
        samplers: BTreeMap::new(),
        float_defs: BTreeMap::new(),
        int_defs: BTreeMap::new(),
        bool_defs: BTreeMap::new(),
        color_outputs: BTreeSet::new(),
        writes_depth: false,
        uses_a0: false,
        uses_al: false,
        uses_pos: false,
        uses_face: false,
        uses_fog: false,
        uses_psize: false,
        loop_depth: 0,
        centroid: BTreeSet::new(),
    };
    // Declarations first: they can appear anywhere before use but bind names globally.
    let mut vs3_outputs: BTreeMap<u32, Semantic> = BTreeMap::new();
    for ins in &shader.instructions {
        if let Some((reg, v)) = ins.def {
            match v {
                DefValue::Float(f) => {
                    g.float_defs.insert(reg.num, f);
                }
                DefValue::Int(i) => {
                    g.int_defs.insert(reg.num, i);
                }
                DefValue::Bool(b) => {
                    g.bool_defs.insert(reg.num, b);
                }
            }
        }
        match ins.decl {
            Some(Decl::Sampler { ty, dst }) => {
                g.samplers.insert(dst.reg.num, ty);
            }
            Some(Decl::Semantic { usage, index, dst }) => {
                let mut sem = Semantic { usage, index };
                // ps_2_x declares v#/t# without a meaningful usage: v# are COLOR#, t# are
                // TEXCOORD#.
                if shader.stage == Stage::Pixel && shader.major < 3 && dst.reg.ty == RegType::Input
                {
                    sem = Semantic {
                        usage: Usage::Color,
                        index: dst.reg.num as u8,
                    };
                }
                match (dst.reg.ty, shader.stage) {
                    (RegType::Input, _) => {
                        g.inputs.insert(dst.reg.num, sem);
                        if dst.centroid {
                            g.centroid.insert(dst.reg.num);
                        }
                    }
                    (RegType::AddrOrTexture, Stage::Pixel) => {
                        g.tex_inputs.insert(dst.reg.num);
                        if dst.centroid {
                            g.centroid.insert(1000 + dst.reg.num);
                        }
                    }
                    (RegType::Output, Stage::Vertex) => {
                        vs3_outputs.insert(dst.reg.num, sem);
                    }
                    (RegType::MiscType, Stage::Pixel) => {}
                    (ty, st) => {
                        return Err(unsupported(format!("dcl on {ty:?} in {st:?}")));
                    }
                }
            }
            None => {}
        }
    }
    for (reg, sem) in &vs3_outputs {
        g.outputs.insert(format!("o{reg}"), *sem);
    }
    for ins in &shader.instructions {
        g.instruction(ins)?;
    }
    if g.indent != 1 {
        return Err(TranslateError::Malformed("unbalanced control flow".into()));
    }
    let wgsl = assemble(&mut g)?;
    let interface = Interface {
        inputs: g.interface_inputs(),
        outputs: g.interface_outputs(),
        samplers: g.samplers.iter().map(|(k, v)| (*k, *v)).collect(),
        float_defs: g.float_defs.iter().map(|(k, v)| (*k, *v)).collect(),
        int_defs: g.int_defs.iter().map(|(k, v)| (*k, *v)).collect(),
        bool_defs: g.bool_defs.iter().map(|(k, v)| (*k, *v)).collect(),
        color_outputs: g.color_outputs.iter().copied().collect(),
        writes_depth: g.writes_depth,
    };
    let module = naga::front::wgsl::parse_str(&wgsl)
        .map_err(|e| TranslateError::Naga(e.emit_to_string(&wgsl)))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map_err(|e| TranslateError::Naga(format!("{:?}", e.into_inner())))?;
    Ok(Translation {
        wgsl,
        module,
        info,
        interface,
        stage: shader.stage,
        ctab: shader.ctab.clone(),
    })
}

impl Gen<'_> {
    fn interface_inputs(&self) -> Vec<IoSlot> {
        let mut out = Vec::new();
        match self.stage {
            Stage::Vertex => {
                for (r, s) in &self.inputs {
                    out.push(IoSlot {
                        semantic: *s,
                        register: *r,
                        location: *r,
                    });
                }
            }
            Stage::Pixel => {
                for (r, s) in &self.inputs {
                    if let Some(l) = location_of(*s) {
                        out.push(IoSlot {
                            semantic: *s,
                            register: *r,
                            location: l,
                        });
                    }
                }
                for t in &self.tex_inputs {
                    let s = Semantic {
                        usage: Usage::TexCoord,
                        index: *t as u8,
                    };
                    out.push(IoSlot {
                        semantic: s,
                        register: *t,
                        location: location_of(s).unwrap_or(0),
                    });
                }
            }
        }
        out
    }

    fn interface_outputs(&self) -> Vec<IoSlot> {
        self.outputs
            .iter()
            .filter_map(|(name, s)| {
                let reg = name
                    .trim_start_matches(|c: char| !c.is_ascii_digit())
                    .parse()
                    .ok()?;
                Some(IoSlot {
                    semantic: *s,
                    register: reg,
                    location: location_of(*s)?,
                })
            })
            .collect()
    }
}

fn assemble(g: &mut Gen) -> Result<String> {
    let stage = g.stage;
    let mut w = String::new();
    let _ = writeln!(
        w,
        "// Translated from D3D9 {stage:?} shader model {}.{}.",
        g.shader.major, g.shader.minor
    );
    let _ = writeln!(w, "diagnostic(off, derivative_uniformity);\n");
    let _ = writeln!(
        w,
        "struct FloatConstants {{ c: array<vec4<f32>, {FLOAT_CONSTANTS}> }}\n\
         struct IntConstants {{ c: array<vec4<i32>, {INT_CONSTANTS}> }}\n\
         struct BoolConstants {{ c: array<vec4<u32>, {BOOL_CONSTANTS}> }}"
    );
    let _ = writeln!(
        w,
        "@group({GROUP_CONSTANTS}) @binding({}) var<uniform> cf: FloatConstants;\n\
         @group({GROUP_CONSTANTS}) @binding({}) var<uniform> ci_u: IntConstants;\n\
         @group({GROUP_CONSTANTS}) @binding({}) var<uniform> cb_u: BoolConstants;",
        binding_float(stage),
        binding_int(stage),
        binding_bool(stage)
    );
    if stage == Stage::Vertex || (g.opts.alpha_ref_uniform && g.opts.alpha_test.is_some()) {
        let _ = writeln!(
            w,
            "@group({GROUP_CONSTANTS}) @binding({BINDING_FIXUP}) var<uniform> d3d_fixup: vec4<f32>;"
        );
    }
    for (s, ty) in &g.samplers {
        let (tex, smp) = g.sampler_names(*s);
        let tty = match ty {
            TextureType::Cube => "texture_cube<f32>",
            TextureType::Volume => "texture_3d<f32>",
            _ => "texture_2d<f32>",
        };
        let b = binding_texture(stage, *s);
        let _ = writeln!(
            w,
            "@group({GROUP_SAMPLERS}) @binding({b}) var {tex}: {tty};\n@group({GROUP_SAMPLERS}) @binding({}) var {smp}: sampler;",
            b + 1
        );
    }
    w.push('\n');
    for r in &g.temps {
        let _ = writeln!(w, "var<private> r{r}: vec4<f32>;");
    }
    for r in g.float_defs.keys() {
        let _ = writeln!(w, "var<private> cd{r}: vec4<f32>;");
    }
    for r in g.int_defs.keys() {
        let _ = writeln!(w, "var<private> ci{r}: vec4<i32>;");
    }
    for r in g.bool_defs.keys() {
        let _ = writeln!(w, "var<private> cb{r}: bool;");
    }
    if g.uses_a0 {
        let _ = writeln!(w, "var<private> a0: vec4<i32>;");
        let _ = writeln!(
            w,
            "fn round_half_away(x: vec4<f32>) -> vec4<f32> {{ return sign(x) * floor(abs(x) + vec4<f32>(0.5)); }}"
        );
    }
    if g.uses_al {
        let _ = writeln!(w, "var<private> aL: i32;");
    }
    for r in g.inputs.keys() {
        let _ = writeln!(w, "var<private> v{r}: vec4<f32>;");
    }
    for t in &g.tex_inputs {
        let _ = writeln!(w, "var<private> t{t}: vec4<f32>;");
    }
    if g.uses_pos {
        let _ = writeln!(w, "var<private> vpos: vec4<f32>;");
    }
    if g.uses_face {
        let _ = writeln!(w, "var<private> vface: vec4<f32>;");
    }

    // Entry point interface.
    let (in_struct, out_struct, prologue, epilogue) = match stage {
        Stage::Vertex => vertex_interface(g)?,
        Stage::Pixel => pixel_interface(g)?,
    };
    w.push_str(&in_struct);
    w.push_str(&out_struct);
    let attr = if stage == Stage::Vertex {
        "@vertex"
    } else {
        "@fragment"
    };
    let _ = writeln!(w, "{attr}\nfn {ENTRY_POINT}(input: StageIn) -> StageOut {{");
    for (r, v) in g.float_defs.clone() {
        let _ = writeln!(w, "    cd{r} = {};", vec4_lit(v));
    }
    for (r, v) in g.int_defs.clone() {
        let _ = writeln!(
            w,
            "    ci{r} = vec4<i32>({}, {}, {}, {});",
            v[0], v[1], v[2], v[3]
        );
    }
    for (r, v) in g.bool_defs.clone() {
        let _ = writeln!(w, "    cb{r} = {v};");
    }
    w.push_str(&prologue);
    w.push_str(&g.body);
    w.push_str(&epilogue);
    w.push_str("}\n");
    Ok(w)
}

fn vertex_interface(g: &mut Gen) -> Result<(String, String, String, String)> {
    let mut ins = String::from("struct StageIn {\n");
    let mut pro = String::new();
    if g.inputs.is_empty() {
        ins.push_str("    @builtin(vertex_index) vertex_index: u32,\n");
    }
    for r in g.inputs.keys() {
        let _ = writeln!(ins, "    @location({r}) v{r}: vec4<f32>,");
        let _ = writeln!(pro, "    v{r} = input.v{r};");
    }
    ins.push_str("}\n");

    // Outputs: every written or required semantic.
    let mut outs: BTreeMap<u32, (String, Semantic)> = BTreeMap::new();
    let mut pos_var = "opos".to_string();
    let mut decls = String::new();
    let mut have_pos = false;
    for (name, sem) in g.outputs.clone() {
        if sem.usage == Usage::Position && sem.index == 0 {
            pos_var = name.clone();
            have_pos = true;
            let _ = writeln!(decls, "var<private> {name}: vec4<f32>;");
            continue;
        }
        let _ = writeln!(decls, "var<private> {name}: vec4<f32>;");
        if let Some(l) = location_of(sem) {
            outs.insert(l, (name, sem));
        }
    }
    if !have_pos {
        let _ = writeln!(decls, "var<private> opos: vec4<f32>;");
    }
    if g.uses_fog {
        let _ = writeln!(decls, "var<private> ofog: vec4<f32>;");
        outs.entry(10).or_insert((
            "ofog".into(),
            Semantic {
                usage: Usage::Fog,
                index: 0,
            },
        ));
    }
    if g.uses_psize {
        let _ = writeln!(decls, "var<private> opts: vec4<f32>;");
    }
    for s in g.opts.extra_outputs.clone() {
        if let Some(l) = location_of(s) {
            outs.entry(l).or_insert_with(|| {
                let name = format!("ox{l}");
                let _ = writeln!(decls, "var<private> {name}: vec4<f32>;");
                (name, s)
            });
        }
    }
    let mut out = decls;
    out.push_str("struct StageOut {\n    @builtin(position) position: vec4<f32>,\n");
    for (l, (name, _)) in &outs {
        let _ = writeln!(out, "    @location({l}) {name}: vec4<f32>,");
    }
    out.push_str("}\n");
    let mut epi = String::from("    var out: StageOut;\n");
    // Half-pixel shift from D3D9 to wgpu rasterization (see module docs).
    let _ = writeln!(
        epi,
        "    out.position = vec4<f32>({p}.x + d3d_fixup.x * {p}.w, {p}.y - d3d_fixup.y * {p}.w, {p}.z, {p}.w);",
        p = pos_var
    );
    for (name, _) in outs.values() {
        let _ = writeln!(epi, "    out.{name} = {name};");
    }
    epi.push_str("    return out;\n");
    g.outputs
        .retain(|_, s| !(s.usage == Usage::Position && s.index == 0));
    for (_, (name, s)) in outs {
        g.outputs.entry(name).or_insert(s);
    }
    Ok((ins, out, pro, epi))
}

fn pixel_interface(g: &mut Gen) -> Result<(String, String, String, String)> {
    let mut ins = String::from("struct StageIn {\n    @builtin(position) frag_coord: vec4<f32>,\n");
    let mut pro = String::new();
    if g.uses_face {
        ins.push_str("    @builtin(front_facing) front_facing: bool,\n");
        pro.push_str("    vface = vec4<f32>(select(-1.0, 1.0, input.front_facing));\n");
    }
    if g.uses_pos {
        pro.push_str("    vpos = vec4<f32>(input.frag_coord.xy - vec2<f32>(0.5), 0.0, 0.0);\n");
    }
    let mut used = BTreeSet::new();
    for (r, s) in g.inputs.clone() {
        let Some(l) = location_of(s) else {
            return Err(unsupported(format!("pixel input semantic {s:?}")));
        };
        if !used.insert(l) {
            return Err(unsupported(format!("two pixel inputs share location {l}")));
        }
        let interp = if g.centroid.contains(&r) {
            " @interpolate(perspective, centroid)"
        } else {
            ""
        };
        let _ = writeln!(ins, "    @location({l}){interp} v{r}: vec4<f32>,");
        let _ = writeln!(pro, "    v{r} = input.v{r};");
    }
    for t in g.tex_inputs.clone() {
        let s = Semantic {
            usage: Usage::TexCoord,
            index: t as u8,
        };
        let l = location_of(s).ok_or_else(|| unsupported(format!("texture input t{t}")))?;
        if !used.insert(l) {
            return Err(unsupported(format!("two pixel inputs share location {l}")));
        }
        let interp = if g.centroid.contains(&(1000 + t)) {
            " @interpolate(perspective, centroid)"
        } else {
            ""
        };
        let _ = writeln!(ins, "    @location({l}){interp} t{t}: vec4<f32>,");
        let _ = writeln!(pro, "    t{t} = input.t{t};");
    }
    ins.push_str("}\n");

    let keep = |n: u32| g.opts.color_targets.is_none_or(|k| n < k);
    let mut out = String::new();
    for c in &g.color_outputs {
        let _ = writeln!(out, "var<private> oc{c}: vec4<f32>;");
    }
    if g.writes_depth {
        out.push_str("var<private> odepth: vec4<f32>;\n");
    }
    out.push_str("struct StageOut {\n");
    let kept: Vec<u32> = g
        .color_outputs
        .iter()
        .copied()
        .filter(|&c| keep(c))
        .collect();
    // A pipeline with colour targets expects location 0 even if the shader never writes it.
    let mut locations: BTreeSet<u32> = kept.iter().copied().collect();
    if g.opts.color_targets.is_some_and(|k| k > 0) {
        locations.insert(0);
    }
    for c in &locations {
        let _ = writeln!(out, "    @location({c}) c{c}: vec4<f32>,");
    }
    if g.writes_depth {
        out.push_str("    @builtin(frag_depth) depth: f32,\n");
    }
    if locations.is_empty() && !g.writes_depth {
        // WGSL needs at least one member; a never-written colour 0 is harmless.
        out.push_str("    @location(0) c0: vec4<f32>,\n");
        locations.insert(0);
    }
    out.push_str("}\n");
    let mut epi = String::new();
    if let Some((cmp, reference)) = g.opts.alpha_test {
        let o = match cmp {
            Comparison::Gt => ">",
            Comparison::Eq => "==",
            Comparison::Ge => ">=",
            Comparison::Lt => "<",
            Comparison::Ne => "!=",
            Comparison::Le => "<=",
        };
        // D3D9 tests the pixel's alpha as an 8-bit value against the 8-bit reference
        // (ALPHAREF), so an alpha a hair under 1.0 still passes `>= 255`.
        let a = if g.color_outputs.contains(&0) {
            "round(saturate(oc0.w) * 255.0)"
        } else {
            "0.0"
        };
        let reference = if g.opts.alpha_ref_uniform {
            "d3d_fixup.z".to_string()
        } else {
            flit((reference * 255.0).round())
        };
        let _ = writeln!(epi, "    if (!({a} {o} {reference})) {{ discard; }}");
    }
    epi.push_str("    var out: StageOut;\n");
    for c in &locations {
        if g.color_outputs.contains(c) {
            let _ = writeln!(epi, "    out.c{c} = oc{c};");
        }
    }
    if g.writes_depth {
        epi.push_str("    out.depth = odepth.x;\n");
    }
    epi.push_str("    return out;\n");
    Ok((ins, out, pro, epi))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::tests::{XYZW, dst, ins, src};

    fn assemble_tokens(t: &[u32]) -> Shader {
        let bytes: Vec<u8> = t.iter().flat_map(|x| x.to_le_bytes()).collect();
        Shader::parse(&bytes).unwrap()
    }

    #[test]
    fn translates_a_textured_pixel_shader() {
        let mut t = vec![0xFFFF_0300];
        t.extend([ins(31, 2), 0x8000_0005, dst(1, 0, 0b0011)]); // dcl_texcoord0 v0.xy
        t.extend([ins(31, 2), 0x8000_0000 | (2 << 27), dst(10, 0, 0xf)]); // dcl_2d s0
        t.extend([ins(81, 5), dst(2, 1, 0xf), 0, 0, 0, 0]); // def c1, 0, 0, 0, 0
        t.extend([
            ins(66, 3),
            dst(0, 0, 0xf),
            src(1, 0, XYZW, 0),
            src(10, 0, XYZW, 0),
        ]);
        // rcp r1, c1.x: must not constant-fold to a division by zero.
        t.extend([ins(6, 2), dst(0, 1, 0xf), src(2, 1, 0, 0)]);
        t.extend([
            ins(5, 3),
            dst(8, 0, 0xf),
            src(0, 0, XYZW, 0),
            src(2, 0, XYZW, 1),
        ]);
        t.push(0xFFFF);
        let s = assemble_tokens(&t);
        let tr = translate(
            &s,
            &Options {
                alpha_test: Some((Comparison::Gt, 0.5)),
                color_targets: Some(1),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            tr.wgsl.contains("textureSample(tex0, tsmp0, v0.xy)"),
            "{}",
            tr.wgsl
        );
        assert!(tr.wgsl.contains("discard"));
        assert!(
            tr.wgsl.contains("round(saturate(oc0.w) * 255.0) > 128"),
            "alpha tests compare 8-bit alpha: {}",
            tr.wgsl
        );
        assert_eq!(tr.interface.samplers, vec![(0, TextureType::Tex2D)]);
        assert_eq!(tr.interface.inputs[0].location, 0);
        assert_eq!(tr.interface.color_outputs, vec![0]);
    }

    #[test]
    fn translates_a_vertex_shader_with_relative_constants() {
        let mut t = vec![0xFFFE_0300];
        t.extend([ins(31, 2), 0x8000_0000, dst(1, 0, 0xf)]); // dcl_position v0
        t.extend([ins(31, 2), 0x8000_0000 | 1 << 16, dst(1, 1, 0xf)]); // dcl_position1 v1 -> idx 1
        t.extend([ins(31, 2), 0x8000_0000, dst(6, 0, 0xf)]); // dcl_position o0
        t.extend([ins(31, 2), 0x8000_0005 | (3 << 16), dst(6, 1, 0xf)]); // dcl_texcoord3 o1
        // mova a0.x, v1.x
        t.extend([ins(46, 2), dst(3, 0, 0b0001), src(1, 1, 0, 0)]);
        // mov o1, c[a0.x + 4]
        t.extend([
            ins(1, 3),
            dst(6, 1, 0xf),
            src(2, 4, XYZW, 0) | (1 << 13),
            src(3, 0, 0, 0),
        ]);
        // m4x4 o0, v0, c0
        t.extend([
            ins(20, 3),
            dst(6, 0, 0xf),
            src(1, 0, XYZW, 0),
            src(2, 0, XYZW, 0),
        ]);
        t.push(0xFFFF);
        let s = assemble_tokens(&t);
        let tr = translate(&s, &Options::default()).unwrap_or_else(|e| panic!("{e}"));
        assert!(tr.wgsl.contains("cf.c[clamp(4 + a0.x"), "{}", tr.wgsl);
        assert!(tr.wgsl.contains("d3d_fixup"));
        assert_eq!(tr.interface.outputs.len(), 1);
        assert_eq!(tr.interface.outputs[0].location, 3);
    }

    #[test]
    fn reports_unsupported_opcodes() {
        let mut t = vec![0xFFFF_0300];
        // call l0 (unsupported)
        t.extend([ins(25, 1), src(18, 0, XYZW, 0)]);
        t.push(0xFFFF);
        let s = assemble_tokens(&t);
        let e = translate(&s, &Options::default()).err().unwrap();
        assert_eq!(e.key(), "call");
    }
}
