//! Direct3D 9 shader bytecode (shader models 2.0, 2.x and 3.0).
//!
//! What DA:O ships: every `<name>.vsh.fxo` / `<name>.psh.fxo` in `shaders.erf` is a bare D3D9
//! token stream as produced by `fxc` / `D3DXCompileShader`, not a D3DX effect container. The
//! stream starts with the version token (`0xFFFE0300` for vs_3_0, `0xFFFF0201` for ps_2_x and so
//! on), followed directly by a comment block holding the `CTAB` constant table, then the
//! instructions, then the end token `0x0000FFFF`. Of the 532 entries in the GOG 1.05 install,
//! 149 are ps_3_0, 132 ps_2_x, 48 ps_2_0, 93 vs_3_0 and 110 vs_2_0 (provisional: from a scan of
//! the install, not from the executable).
//!
//! The token layout below follows Microsoft's public D3D9 driver documentation
//! ("Direct3D Shader Codes" / `d3d9types.h`).

use std::fmt;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum BytecodeError {
    #[error("shader is shorter than its version token")]
    Empty,
    #[error("bad version token {0:#010x}")]
    BadVersion(u32),
    #[error("unsupported shader model {0}.{1}")]
    UnsupportedVersion(u8, u8),
    #[error("token stream ends early at token {0}")]
    Truncated(usize),
    #[error("instruction at token {at}: {msg}")]
    Malformed { at: usize, msg: String },
    #[error("constant table: {0}")]
    Ctab(String),
}

pub type Result<T> = std::result::Result<T, BytecodeError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    Vertex,
    Pixel,
}

/// D3D9 opcodes (`D3DSHADER_INSTRUCTION_OPCODE_TYPE`). Opcodes that only exist in shader
/// models 1.x are kept as [`Opcode::Other`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Opcode {
    Nop,
    Mov,
    Add,
    Sub,
    Mad,
    Mul,
    Rcp,
    Rsq,
    Dp3,
    Dp4,
    Min,
    Max,
    Slt,
    Sge,
    Exp,
    Log,
    Lit,
    Dst,
    Lrp,
    Frc,
    M4x4,
    M4x3,
    M3x4,
    M3x3,
    M3x2,
    Call,
    CallNz,
    Loop,
    Ret,
    EndLoop,
    Label,
    Dcl,
    Pow,
    Crs,
    Sgn,
    Abs,
    Nrm,
    SinCos,
    Rep,
    EndRep,
    If,
    IfC,
    Else,
    EndIf,
    Break,
    BreakC,
    Mova,
    DefB,
    DefI,
    TexKill,
    Tex,
    ExpP,
    LogP,
    Cnd,
    Def,
    Cmp,
    Dp2Add,
    Dsx,
    Dsy,
    TexLdd,
    SetP,
    TexLdl,
    BreakP,
    Other(u16),
}

impl Opcode {
    pub fn from_code(c: u16) -> Self {
        use Opcode::*;
        match c {
            0 => Nop,
            1 => Mov,
            2 => Add,
            3 => Sub,
            4 => Mad,
            5 => Mul,
            6 => Rcp,
            7 => Rsq,
            8 => Dp3,
            9 => Dp4,
            10 => Min,
            11 => Max,
            12 => Slt,
            13 => Sge,
            14 => Exp,
            15 => Log,
            16 => Lit,
            17 => Dst,
            18 => Lrp,
            19 => Frc,
            20 => M4x4,
            21 => M4x3,
            22 => M3x4,
            23 => M3x3,
            24 => M3x2,
            25 => Call,
            26 => CallNz,
            27 => Loop,
            28 => Ret,
            29 => EndLoop,
            30 => Label,
            31 => Dcl,
            32 => Pow,
            33 => Crs,
            34 => Sgn,
            35 => Abs,
            36 => Nrm,
            37 => SinCos,
            38 => Rep,
            39 => EndRep,
            40 => If,
            41 => IfC,
            42 => Else,
            43 => EndIf,
            44 => Break,
            45 => BreakC,
            46 => Mova,
            47 => DefB,
            48 => DefI,
            65 => TexKill,
            66 => Tex,
            78 => ExpP,
            79 => LogP,
            80 => Cnd,
            81 => Def,
            88 => Cmp,
            90 => Dp2Add,
            91 => Dsx,
            92 => Dsy,
            93 => TexLdd,
            94 => SetP,
            95 => TexLdl,
            96 => BreakP,
            other => Other(other),
        }
    }

    pub fn name(self) -> String {
        format!("{self:?}").to_ascii_lowercase()
    }

    /// (has destination, number of sources) for shader model 2.0+. `None` for opcodes whose
    /// operands are read specially (`dcl`, `def*`) or that we do not know.
    pub fn operands(self, major: u8) -> Option<(bool, usize)> {
        use Opcode::*;
        Some(match self {
            Nop | Ret | EndLoop | EndRep | Else | EndIf | Break => (false, 0),
            Mov | Rcp | Rsq | Exp | Log | Lit | Frc | Abs | Nrm | Mova | ExpP | LogP | Dsx
            | Dsy => (true, 1),
            Add | Sub | Mul | Dp3 | Dp4 | Min | Max | Slt | Sge | Dst | M4x4 | M4x3 | M3x4
            | M3x3 | M3x2 | Pow | Crs | SetP => (true, 2),
            Mad | Lrp | Cnd | Cmp | Dp2Add => (true, 3),
            Sgn => (true, if major >= 3 { 1 } else { 3 }),
            SinCos => (true, if major >= 3 { 1 } else { 3 }),
            Tex => (true, 2),
            TexLdl => (true, 2),
            TexLdd => (true, 4),
            TexKill => (true, 0),
            Call | Label | Rep | If | BreakP => (false, 1),
            CallNz | Loop | IfC | BreakC => (false, 2),
            Dcl | Def | DefB | DefI => return None,
            Other(_) => return None,
        })
    }
}

/// D3D9 register types (`D3DSHADER_PARAM_REGISTER_TYPE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RegType {
    Temp,
    Input,
    Const,
    /// `a0` in vertex shaders, `t#` in pixel shaders.
    AddrOrTexture,
    RastOut,
    AttrOut,
    /// `oT#` in vs_2, `o#` in vs_3.
    Output,
    ConstInt,
    ColorOut,
    DepthOut,
    Sampler,
    Const2,
    Const3,
    Const4,
    ConstBool,
    Loop,
    TempFloat16,
    MiscType,
    Label,
    Predicate,
}

impl RegType {
    fn from_code(c: u32) -> Option<Self> {
        use RegType::*;
        Some(match c {
            0 => Temp,
            1 => Input,
            2 => Const,
            3 => AddrOrTexture,
            4 => RastOut,
            5 => AttrOut,
            6 => Output,
            7 => ConstInt,
            8 => ColorOut,
            9 => DepthOut,
            10 => Sampler,
            11 => Const2,
            12 => Const3,
            13 => Const4,
            14 => ConstBool,
            15 => Loop,
            16 => TempFloat16,
            17 => MiscType,
            18 => Label,
            19 => Predicate,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SrcMod {
    None,
    Neg,
    Bias,
    BiasNeg,
    Sign,
    SignNeg,
    Comp,
    X2,
    X2Neg,
    Dz,
    Dw,
    Abs,
    AbsNeg,
    Not,
}

impl SrcMod {
    fn from_code(c: u32) -> Option<Self> {
        use SrcMod::*;
        Some(match c {
            0 => None,
            1 => Neg,
            2 => Bias,
            3 => BiasNeg,
            4 => Sign,
            5 => SignNeg,
            6 => Comp,
            7 => X2,
            8 => X2Neg,
            9 => Dz,
            10 => Dw,
            11 => Abs,
            12 => AbsNeg,
            13 => Not,
            _ => return Option::None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Register {
    pub ty: RegType,
    pub num: u32,
}

/// A source operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Src {
    pub reg: Register,
    /// Component index (0..3) for each of x, y, z, w.
    pub swizzle: [u8; 4],
    pub modifier: SrcMod,
    /// Relative addressing: `c[a0.x + n]` or `c[aL + n]`.
    pub relative: Option<(Register, u8)>,
}

/// A destination operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dst {
    pub reg: Register,
    /// Bit 0 = x ... bit 3 = w.
    pub mask: u8,
    pub saturate: bool,
    pub partial_precision: bool,
    pub centroid: bool,
    /// Shift scale (ps 1.x only); signed power of two.
    pub shift: i8,
    pub relative: Option<(Register, u8)>,
}

/// `D3DDECLUSAGE`, as used by `dcl` on inputs and outputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Usage {
    Position,
    BlendWeight,
    BlendIndices,
    Normal,
    PSize,
    TexCoord,
    Tangent,
    Binormal,
    TessFactor,
    PositionT,
    Color,
    Fog,
    Depth,
    Sample,
}

impl Usage {
    fn from_code(c: u32) -> Option<Self> {
        use Usage::*;
        Some(match c {
            0 => Position,
            1 => BlendWeight,
            2 => BlendIndices,
            3 => Normal,
            4 => PSize,
            5 => TexCoord,
            6 => Tangent,
            7 => Binormal,
            8 => TessFactor,
            9 => PositionT,
            10 => Color,
            11 => Fog,
            12 => Depth,
            13 => Sample,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureType {
    Unknown,
    Tex2D,
    Cube,
    Volume,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decl {
    /// An input or output register with a semantic.
    Semantic {
        usage: Usage,
        index: u8,
        dst: Dst,
    },
    Sampler {
        ty: TextureType,
        dst: Dst,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DefValue {
    Float([f32; 4]),
    Int([i32; 4]),
    Bool(bool),
}

/// Comparison codes used by `ifc`, `breakc` and `setp`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparison {
    Gt,
    Eq,
    Ge,
    Lt,
    Ne,
    Le,
}

impl Comparison {
    pub fn from_control(c: u8) -> Option<Self> {
        Some(match c {
            1 => Comparison::Gt,
            2 => Comparison::Eq,
            3 => Comparison::Ge,
            4 => Comparison::Lt,
            5 => Comparison::Ne,
            6 => Comparison::Le,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Instruction {
    pub opcode: Opcode,
    /// Opcode-specific control bits (comparison, texld variants).
    pub control: u8,
    pub dst: Option<Dst>,
    pub src: Vec<Src>,
    /// For predicated instructions: the predicate register operand.
    pub predicate: Option<Src>,
    pub decl: Option<Decl>,
    /// For `def`, `defi`, `defb`: the register and value.
    pub def: Option<(Register, DefValue)>,
    /// Token index of the opcode, for diagnostics.
    pub offset: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shader {
    pub stage: Stage,
    pub major: u8,
    pub minor: u8,
    pub instructions: Vec<Instruction>,
    pub ctab: Option<crate::ctab::ConstantTable>,
}

struct Reader<'a> {
    t: &'a [u32],
}

impl Reader<'_> {
    fn get(&self, i: usize) -> Result<u32> {
        self.t.get(i).copied().ok_or(BytecodeError::Truncated(i))
    }
}

fn malformed(at: usize, msg: impl Into<String>) -> BytecodeError {
    BytecodeError::Malformed {
        at,
        msg: msg.into(),
    }
}

fn reg_of(tok: u32, at: usize) -> Result<Register> {
    let ty = ((tok >> 28) & 7) | ((tok >> 8) & 0x18);
    Ok(Register {
        ty: RegType::from_code(ty).ok_or_else(|| malformed(at, format!("register type {ty}")))?,
        num: tok & 0x7ff,
    })
}

impl Shader {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 4 {
            return Err(BytecodeError::Empty);
        }
        let tokens: Vec<u32> = bytes
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        let r = Reader { t: &tokens };
        let version = r.get(0)?;
        let stage = match version >> 16 {
            0xFFFE => Stage::Vertex,
            0xFFFF => Stage::Pixel,
            _ => return Err(BytecodeError::BadVersion(version)),
        };
        let (major, minor) = (((version >> 8) & 0xff) as u8, (version & 0xff) as u8);
        if !(2..=3).contains(&major) {
            return Err(BytecodeError::UnsupportedVersion(major, minor));
        }
        let mut shader = Shader {
            stage,
            major,
            minor,
            instructions: Vec::new(),
            ctab: None,
        };
        let mut i = 1;
        loop {
            let tok = r.get(i)?;
            if tok == 0x0000_FFFF {
                break;
            }
            let code = (tok & 0xffff) as u16;
            if code == 0xFFFE {
                // Comment: length in DWORDs in bits 16..30.
                let len = ((tok >> 16) & 0x7fff) as usize;
                let end = i + 1 + len;
                if end > tokens.len() {
                    return Err(BytecodeError::Truncated(end));
                }
                let body = &tokens[i + 1..end];
                if body.first() == Some(&u32::from_le_bytes(*b"CTAB")) && shader.ctab.is_none() {
                    let raw: Vec<u8> = body[1..].iter().flat_map(|t| t.to_le_bytes()).collect();
                    shader.ctab = Some(crate::ctab::ConstantTable::parse(&raw)?);
                }
                i = end;
                continue;
            }
            if code == 0xFFFD {
                return Err(malformed(i, "phase instruction (ps 1.4 only)"));
            }
            let len = ((tok >> 24) & 0xf) as usize;
            let end = i + 1 + len;
            if end > tokens.len() {
                return Err(BytecodeError::Truncated(end));
            }
            let ins = parse_instruction(&tokens[i + 1..end], tok, i, stage, major)?;
            shader.instructions.push(ins);
            i = end;
        }
        Ok(shader)
    }
}

fn parse_dst(tok: u32, at: usize) -> Result<Dst> {
    let mods = (tok >> 20) & 0xf;
    let shift = ((tok >> 24) & 0xf) as i8;
    Ok(Dst {
        reg: reg_of(tok, at)?,
        mask: ((tok >> 16) & 0xf) as u8,
        saturate: mods & 1 != 0,
        partial_precision: mods & 2 != 0,
        centroid: mods & 4 != 0,
        shift: if shift > 7 { shift - 16 } else { shift },
        relative: None,
    })
}

fn parse_src(tok: u32, at: usize) -> Result<Src> {
    let sw = (tok >> 16) & 0xff;
    let modifier = SrcMod::from_code((tok >> 24) & 0xf)
        .ok_or_else(|| malformed(at, format!("source modifier {}", (tok >> 24) & 0xf)))?;
    Ok(Src {
        reg: reg_of(tok, at)?,
        swizzle: [
            (sw & 3) as u8,
            ((sw >> 2) & 3) as u8,
            ((sw >> 4) & 3) as u8,
            ((sw >> 6) & 3) as u8,
        ],
        modifier,
        relative: None,
    })
}

/// Reads operands from the instruction body, following relative-address tokens.
struct Operands<'a> {
    body: &'a [u32],
    pos: usize,
    at: usize,
    major: u8,
}

impl Operands<'_> {
    fn next(&mut self) -> Result<u32> {
        let t = self
            .body
            .get(self.pos)
            .copied()
            .ok_or_else(|| malformed(self.at, "too few operand tokens"))?;
        self.pos += 1;
        Ok(t)
    }

    fn relative(&mut self, tok: u32, stage: Stage) -> Result<Option<(Register, u8)>> {
        if tok & (1 << 13) == 0 {
            return Ok(None);
        }
        // vs_2_0 and later: an extra token names the address register and its component.
        // (vs_1_1 implied a0.x; we do not support 1.x.)
        if self.major >= 2 || stage == Stage::Pixel {
            let r = self.next()?;
            let reg = reg_of(r, self.at)?;
            let comp = ((r >> 16) & 3) as u8;
            return Ok(Some((reg, comp)));
        }
        Ok(None)
    }
}

fn parse_instruction(
    body: &[u32],
    tok: u32,
    at: usize,
    stage: Stage,
    major: u8,
) -> Result<Instruction> {
    let code = (tok & 0xffff) as u16;
    let opcode = Opcode::from_code(code);
    let control = ((tok >> 16) & 0xff) as u8;
    let predicated = tok & (1 << 28) != 0;
    let mut ins = Instruction {
        opcode,
        control,
        dst: None,
        src: Vec::new(),
        predicate: None,
        decl: None,
        def: None,
        offset: at,
    };
    let mut ops = Operands {
        body,
        pos: 0,
        at,
        major,
    };
    match opcode {
        Opcode::Dcl => {
            let info = ops.next()?;
            let dtok = ops.next()?;
            let dst = parse_dst(dtok, at)?;
            ins.decl = Some(if dst.reg.ty == RegType::Sampler {
                let ty = match (info >> 27) & 0xf {
                    2 => TextureType::Tex2D,
                    3 => TextureType::Cube,
                    4 => TextureType::Volume,
                    _ => TextureType::Unknown,
                };
                Decl::Sampler { ty, dst }
            } else {
                let usage = Usage::from_code(info & 0x1f).unwrap_or(Usage::TexCoord);
                Decl::Semantic {
                    usage,
                    index: ((info >> 16) & 0xf) as u8,
                    dst,
                }
            });
            ins.dst = Some(dst);
            return Ok(ins);
        }
        Opcode::Def | Opcode::DefI | Opcode::DefB => {
            let reg = reg_of(ops.next()?, at)?;
            let value = match opcode {
                Opcode::Def => {
                    let mut v = [0f32; 4];
                    for x in &mut v {
                        *x = f32::from_bits(ops.next()?);
                    }
                    DefValue::Float(v)
                }
                Opcode::DefI => {
                    let mut v = [0i32; 4];
                    for x in &mut v {
                        *x = ops.next()? as i32;
                    }
                    DefValue::Int(v)
                }
                _ => DefValue::Bool(ops.next()? != 0),
            };
            ins.def = Some((reg, value));
            return Ok(ins);
        }
        _ => {}
    }
    let (has_dst, nsrc) = opcode
        .operands(major)
        .ok_or_else(|| malformed(at, format!("unsupported opcode {code}")))?;
    if has_dst {
        let t = ops.next()?;
        let mut d = parse_dst(t, at)?;
        d.relative = ops.relative(t, stage)?;
        ins.dst = Some(d);
    }
    if predicated {
        let t = ops.next()?;
        ins.predicate = Some(parse_src(t, at)?);
    }
    for _ in 0..nsrc {
        let t = ops.next()?;
        let mut s = parse_src(t, at)?;
        s.relative = ops.relative(t, stage)?;
        ins.src.push(s);
    }
    if ops.pos != body.len() {
        return Err(malformed(
            at,
            format!(
                "{} used {} of {} operand tokens",
                opcode.name(),
                ops.pos,
                body.len()
            ),
        ));
    }
    Ok(ins)
}

// ---------------------------------------------------------------------------------------------
// Disassembly, for diagnostics and tests.

const COMP: [char; 4] = ['x', 'y', 'z', 'w'];

impl fmt::Display for Register {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use RegType::*;
        let p = match self.ty {
            Temp => "r",
            Input => "v",
            Const => "c",
            AddrOrTexture => "t",
            RastOut => "oRast",
            AttrOut => "oD",
            Output => "o",
            ConstInt => "i",
            ColorOut => "oC",
            DepthOut => "oDepth",
            Sampler => "s",
            Const2 | Const3 | Const4 => "c",
            ConstBool => "b",
            Loop => "aL",
            TempFloat16 => "h",
            MiscType => "vMisc",
            Label => "l",
            Predicate => "p",
        };
        write!(f, "{p}{}", self.num)
    }
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some((reg, v)) = &self.def {
            return write!(f, "{} {reg}, {v:?}", self.opcode.name());
        }
        if let Some(p) = &self.predicate {
            write!(f, "({}) ", p.reg)?;
        }
        write!(f, "{}", self.opcode.name())?;
        if let Some(Decl::Semantic { usage, index, .. }) = &self.decl {
            write!(f, "_{usage:?}{index}")?;
        }
        if let Some(Decl::Sampler { ty, .. }) = &self.decl {
            write!(f, "_{ty:?}")?;
        }
        let mut first = true;
        let mut sep = |f: &mut fmt::Formatter<'_>| {
            let s = if first { " " } else { ", " };
            first = false;
            write!(f, "{s}")
        };
        if let Some(d) = &self.dst {
            sep(f)?;
            write!(f, "{}", d.reg)?;
            if d.mask != 0xf {
                write!(f, ".")?;
                for (i, c) in COMP.iter().enumerate() {
                    if d.mask & (1 << i) != 0 {
                        write!(f, "{c}")?;
                    }
                }
            }
            if d.saturate {
                write!(f, "_sat")?;
            }
        }
        for s in &self.src {
            sep(f)?;
            if s.modifier != SrcMod::None {
                write!(f, "{:?}(", s.modifier)?;
            }
            write!(f, "{}", s.reg)?;
            if let Some((r, c)) = s.relative {
                write!(f, "[{r}.{}]", COMP[c as usize])?;
            }
            if s.swizzle != [0, 1, 2, 3] {
                write!(f, ".")?;
                for c in s.swizzle {
                    write!(f, "{}", COMP[c as usize])?;
                }
            }
            if s.modifier != SrcMod::None {
                write!(f, ")")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Assembles tokens for tests.
    pub fn dst(ty: u32, num: u32, mask: u32) -> u32 {
        0x8000_0000 | ((ty & 7) << 28) | ((ty & 0x18) << 8) | (mask << 16) | num
    }
    pub fn src(ty: u32, num: u32, swizzle: u32, modifier: u32) -> u32 {
        0x8000_0000
            | ((ty & 7) << 28)
            | ((ty & 0x18) << 8)
            | (modifier << 24)
            | (swizzle << 16)
            | num
    }
    pub const XYZW: u32 = 0b11_10_01_00;
    pub fn ins(op: u32, len: u32) -> u32 {
        op | (len << 24)
    }

    #[test]
    fn parses_a_small_pixel_shader() {
        let mut t = vec![0xFFFF_0300];
        // dcl_texcoord0 v0.xy
        t.extend([ins(31, 2), 0x8000_0005, dst(1, 0, 0b0011)]);
        // dcl_2d s0
        t.extend([ins(31, 2), 0x8000_0000 | (2 << 27), dst(10, 0, 0xf)]);
        // def c0, 1, 0.5, 0, 0
        t.extend([
            ins(81, 5),
            dst(2, 0, 0xf),
            1f32.to_bits(),
            0.5f32.to_bits(),
            0,
            0,
        ]);
        // texld r0, v0, s0
        t.extend([
            ins(66, 3),
            dst(0, 0, 0xf),
            src(1, 0, XYZW, 0),
            src(10, 0, XYZW, 0),
        ]);
        // mul_sat oC0, r0, -c0.x
        t.extend([
            ins(5, 3),
            dst(8, 0, 0xf) | (1 << 20),
            src(0, 0, XYZW, 0),
            src(2, 0, 0, 1),
        ]);
        t.push(0x0000_FFFF);
        let bytes: Vec<u8> = t.iter().flat_map(|x| x.to_le_bytes()).collect();
        let s = Shader::parse(&bytes).unwrap();
        assert_eq!((s.stage, s.major), (Stage::Pixel, 3));
        assert_eq!(s.instructions.len(), 5);
        assert!(matches!(
            s.instructions[0].decl,
            Some(Decl::Semantic {
                usage: Usage::TexCoord,
                index: 0,
                ..
            })
        ));
        assert!(matches!(
            s.instructions[1].decl,
            Some(Decl::Sampler {
                ty: TextureType::Tex2D,
                ..
            })
        ));
        assert_eq!(
            s.instructions[2].def,
            Some((
                Register {
                    ty: RegType::Const,
                    num: 0
                },
                DefValue::Float([1.0, 0.5, 0.0, 0.0])
            ))
        );
        let mul = &s.instructions[4];
        assert!(mul.dst.unwrap().saturate);
        assert_eq!(mul.src[1].modifier, SrcMod::Neg);
        assert_eq!(mul.src[1].swizzle, [0, 0, 0, 0]);
        assert_eq!(mul.to_string(), "mul oC0_sat, r0, Neg(c0.xxxx)");
    }

    #[test]
    fn rejects_malformed_streams() {
        let b = |t: &[u32]| -> Vec<u8> { t.iter().flat_map(|x| x.to_le_bytes()).collect() };
        assert_eq!(Shader::parse(&[]), Err(BytecodeError::Empty));
        assert!(matches!(
            Shader::parse(&b(&[0x1234_0300])),
            Err(BytecodeError::BadVersion(_))
        ));
        assert!(matches!(
            Shader::parse(&b(&[0xFFFF_0101, 0xFFFF])),
            Err(BytecodeError::UnsupportedVersion(1, 1))
        ));
        // No end token.
        assert!(Shader::parse(&b(&[0xFFFF_0300])).is_err());
        // Instruction claims more tokens than exist.
        assert!(Shader::parse(&b(&[0xFFFF_0300, ins(1, 5), 0])).is_err());
        // Wrong operand count for mov (3 tokens instead of 2).
        assert!(
            Shader::parse(&b(&[
                0xFFFF_0300,
                ins(1, 3),
                dst(0, 0, 0xf),
                src(0, 1, XYZW, 0),
                src(0, 2, XYZW, 0),
                0xFFFF
            ]))
            .is_err()
        );
        // Comment running past the end.
        assert!(Shader::parse(&b(&[0xFFFF_0300, 0x0010_FFFE, 0xFFFF])).is_err());
    }
}
