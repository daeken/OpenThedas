//! Compiled scripts (`.ncs`): NWScript bytecode as DA:O's compiler writes it.
//!
//! Layout: `"NCS V1.0"`, then `'B'` and the big-endian file size, then instructions. Each
//! instruction is an opcode byte, a type byte, and opcode-specific big-endian arguments. The
//! stack is made of 4-byte cells; stack offsets in arguments are in bytes and negative
//! (relative to the top); jumps are relative to the jumping instruction.
//!
//! The opcode set is BioWare's NWScript set (as publicly documented for Neverwinter Nights)
//! plus what DA:O added, identified from the 4,114 distinct scripts in the GOG 1.05 install
//! and the few `.nss` sources shipped next to their `.ncs` (provisional):
//!
//! - type `0x60` is `resource`, stored like a string (`CONST 0x60` has a u16 length);
//! - array types are `0x40` int[], `0x41` float[], `0x42` string[], `0x43` object[],
//!   `0x44` resource[] and `0x50 + n` arrays of engine structure `n`;
//! - `0x30` writes and `0x32` reads an array element (stack offset of the array and the
//!   element size as arguments, index on top of the stack);
//! - `0x2E` appears twice, right after the only two `STORE_STATE`s (in test scripts); its
//!   meaning is unknown.

use std::fmt;

use crate::ldf::{EngineTable, Type};

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum NcsError {
    #[error("not an NCS V1.0 file")]
    NotNcs,
    #[error("header says {header} bytes, file has {actual}")]
    SizeMismatch { header: u32, actual: usize },
    #[error("truncated instruction at {0:#x}")]
    Truncated(usize),
    #[error("unknown opcode {op:#04x} (type {ty:#04x}) at {at:#x}")]
    UnknownOpcode { op: u8, ty: u8, at: usize },
    #[error("unknown constant type {ty:#04x} at {at:#x}")]
    UnknownConstType { ty: u8, at: usize },
    #[error("jump at {at:#x} lands outside the code or between instructions ({target:#x})")]
    BadJump { at: usize, target: i64 },
}

pub const HEADER_SIZE: usize = 13;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Op {
    CpDownSp,
    RsAdd,
    CpTopSp,
    Const,
    Action,
    LogAnd,
    LogOr,
    IncOr,
    ExcOr,
    BoolAnd,
    Eq,
    Neq,
    Geq,
    Gt,
    Lt,
    Leq,
    ShLeft,
    ShRight,
    UShRight,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Neg,
    Comp,
    MovSp,
    StoreStateAll,
    Jmp,
    Jsr,
    Jz,
    Retn,
    Destruct,
    Not,
    DecISp,
    IncISp,
    Jnz,
    CpDownBp,
    CpTopBp,
    DecIBp,
    IncIBp,
    SaveBp,
    RestoreBp,
    StoreState,
    Nop,
    /// `0x2E`, meaning unknown.
    Unknown2E,
    WriteArray,
    ReadArray,
    /// `0x37` (Dragon Age II): pushes a reference to a stack variable.
    GetRef,
    /// `0x39` (Dragon Age II): pops an index and pushes a reference to that array element.
    GetRefArray,
}

impl Op {
    pub fn from_byte(b: u8) -> Option<Op> {
        use Op::*;
        Some(match b {
            0x01 => CpDownSp,
            0x02 => RsAdd,
            0x03 => CpTopSp,
            0x04 => Const,
            0x05 => Action,
            0x06 => LogAnd,
            0x07 => LogOr,
            0x08 => IncOr,
            0x09 => ExcOr,
            0x0A => BoolAnd,
            0x0B => Eq,
            0x0C => Neq,
            0x0D => Geq,
            0x0E => Gt,
            0x0F => Lt,
            0x10 => Leq,
            0x11 => ShLeft,
            0x12 => ShRight,
            0x13 => UShRight,
            0x14 => Add,
            0x15 => Sub,
            0x16 => Mul,
            0x17 => Div,
            0x18 => Mod,
            0x19 => Neg,
            0x1A => Comp,
            0x1B => MovSp,
            0x1C => StoreStateAll,
            0x1D => Jmp,
            0x1E => Jsr,
            0x1F => Jz,
            0x20 => Retn,
            0x21 => Destruct,
            0x22 => Not,
            0x23 => DecISp,
            0x24 => IncISp,
            0x25 => Jnz,
            0x26 => CpDownBp,
            0x27 => CpTopBp,
            0x28 => DecIBp,
            0x29 => IncIBp,
            0x2A => SaveBp,
            0x2B => RestoreBp,
            0x2C => StoreState,
            0x2D => Nop,
            0x2E => Unknown2E,
            0x30 => WriteArray,
            0x32 => ReadArray,
            0x37 => GetRef,
            0x39 => GetRefArray,
            _ => return None,
        })
    }

    pub fn mnemonic(self) -> &'static str {
        use Op::*;
        match self {
            CpDownSp => "CPDOWNSP",
            RsAdd => "RSADD",
            CpTopSp => "CPTOPSP",
            Const => "CONST",
            Action => "ACTION",
            LogAnd => "LOGAND",
            LogOr => "LOGOR",
            IncOr => "INCOR",
            ExcOr => "EXCOR",
            BoolAnd => "BOOLAND",
            Eq => "EQUAL",
            Neq => "NEQUAL",
            Geq => "GEQ",
            Gt => "GT",
            Lt => "LT",
            Leq => "LEQ",
            ShLeft => "SHLEFT",
            ShRight => "SHRIGHT",
            UShRight => "USHRIGHT",
            Add => "ADD",
            Sub => "SUB",
            Mul => "MUL",
            Div => "DIV",
            Mod => "MOD",
            Neg => "NEG",
            Comp => "COMP",
            MovSp => "MOVSP",
            StoreStateAll => "STORESTATEALL",
            Jmp => "JMP",
            Jsr => "JSR",
            Jz => "JZ",
            Retn => "RETN",
            Destruct => "DESTRUCT",
            Not => "NOT",
            DecISp => "DECISP",
            IncISp => "INCISP",
            Jnz => "JNZ",
            CpDownBp => "CPDOWNBP",
            CpTopBp => "CPTOPBP",
            DecIBp => "DECIBP",
            IncIBp => "INCIBP",
            SaveBp => "SAVEBP",
            RestoreBp => "RESTOREBP",
            StoreState => "STORESTATE",
            Nop => "NOP",
            Unknown2E => "OP_2E",
            WriteArray => "WRITEARRAY",
            ReadArray => "READARRAY",
            GetRef => "GETREF",
            GetRefArray => "GETREFARRAY",
        }
    }
}

/// Instruction type codes (the byte after the opcode). Provisional for the DA:O additions.
pub mod tc {
    pub const INT: u8 = 0x03;
    pub const FLOAT: u8 = 0x04;
    pub const STRING: u8 = 0x05;
    pub const OBJECT: u8 = 0x06;
    pub const ENGINE0: u8 = 0x10;
    pub const INT_INT: u8 = 0x20;
    pub const FLOAT_FLOAT: u8 = 0x21;
    pub const OBJECT_OBJECT: u8 = 0x22;
    pub const STRING_STRING: u8 = 0x23;
    pub const STRUCT_STRUCT: u8 = 0x24;
    pub const INT_FLOAT: u8 = 0x25;
    pub const FLOAT_INT: u8 = 0x26;
    pub const ENGINE_ENGINE0: u8 = 0x30;
    pub const VECTOR_VECTOR: u8 = 0x3A;
    pub const VECTOR_FLOAT: u8 = 0x3B;
    pub const FLOAT_VECTOR: u8 = 0x3C;
    pub const ARRAY0: u8 = 0x40;
    pub const ENGINE_ARRAY0: u8 = 0x50;
    pub const RESOURCE: u8 = 0x60;
}

/// The script type a single-cell type code denotes (for `RSADD` and `CONST`).
pub fn cell_type(code: u8) -> Option<Type> {
    Some(match code {
        tc::INT => Type::Int,
        tc::FLOAT => Type::Float,
        tc::STRING => Type::String,
        tc::OBJECT => Type::Object,
        tc::RESOURCE => Type::Resource,
        0x10..=0x1F => Type::Engine(code - 0x10),
        0x40 => Type::Array(Box::new(Type::Int)),
        0x41 => Type::Array(Box::new(Type::Float)),
        0x42 => Type::Array(Box::new(Type::String)),
        0x43 => Type::Array(Box::new(Type::Object)),
        0x44 => Type::Array(Box::new(Type::Resource)),
        0x50..=0x5F => Type::Array(Box::new(Type::Engine(code - 0x50))),
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConstArg {
    Int(i32),
    Float(f32),
    String(String),
    Object(u32),
    Resource(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    None,
    /// Stack offset (bytes, usually negative) and size (bytes).
    Copy {
        offset: i32,
        size: u16,
    },
    Const(ConstArg),
    Action {
        id: u16,
        argc: u8,
    },
    /// Struct comparison size in bytes (`EQUAL`/`NEQUAL` with type `0x24`).
    StructSize(u16),
    /// `MOVSP`, `DECISP`, ... offsets.
    Offset(i32),
    /// Relative jump.
    Jump(i32),
    Destruct {
        size: u16,
        offset: i16,
        keep: u16,
    },
    StoreState {
        bp: u32,
        sp: u32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Instruction {
    /// Byte offset in the file.
    pub offset: usize,
    pub op: Op,
    pub ty: u8,
    pub arg: Arg,
    /// Encoded length in bytes.
    pub len: usize,
}

impl Instruction {
    /// Absolute jump target for jumps and calls.
    pub fn target(&self) -> Option<usize> {
        match self.arg {
            Arg::Jump(rel) => usize::try_from(self.offset as i64 + rel as i64).ok(),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ncs {
    pub instructions: Vec<Instruction>,
    /// Instruction index by byte offset.
    index: std::collections::HashMap<usize, usize>,
}

struct Rd<'a> {
    b: &'a [u8],
    at: usize,
    start: usize,
}

impl Rd<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], NcsError> {
        let s = self
            .b
            .get(self.at..self.at + n)
            .ok_or(NcsError::Truncated(self.start))?;
        self.at += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, NcsError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, NcsError> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Result<u32, NcsError> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn i32(&mut self) -> Result<i32, NcsError> {
        Ok(self.u32()? as i32)
    }
}

impl Ncs {
    pub fn parse(b: &[u8]) -> Result<Ncs, NcsError> {
        if b.len() < HEADER_SIZE || &b[..8] != b"NCS V1.0" || b[8] != b'B' {
            return Err(NcsError::NotNcs);
        }
        let size = u32::from_be_bytes([b[9], b[10], b[11], b[12]]);
        if size as usize != b.len() {
            return Err(NcsError::SizeMismatch {
                header: size,
                actual: b.len(),
            });
        }
        let mut instructions = Vec::new();
        let mut index = std::collections::HashMap::new();
        let mut at = HEADER_SIZE;
        while at < b.len() {
            let mut r = Rd { b, at, start: at };
            let opb = r.u8()?;
            let ty = r.u8()?;
            let op = Op::from_byte(opb).ok_or(NcsError::UnknownOpcode { op: opb, ty, at })?;
            use Op::*;
            let arg = match op {
                CpDownSp | CpTopSp | CpDownBp | CpTopBp | WriteArray | ReadArray | GetRef
                | GetRefArray => Arg::Copy {
                    offset: r.i32()?,
                    size: r.u16()?,
                },
                Const => Arg::Const(match ty {
                    tc::INT => ConstArg::Int(r.i32()?),
                    tc::FLOAT => ConstArg::Float(f32::from_bits(r.u32()?)),
                    tc::OBJECT => ConstArg::Object(r.u32()?),
                    tc::STRING | tc::RESOURCE => {
                        let n = r.u16()? as usize;
                        let s: String = r.take(n)?.iter().map(|&c| c as char).collect();
                        if ty == tc::STRING {
                            ConstArg::String(s)
                        } else {
                            ConstArg::Resource(s)
                        }
                    }
                    _ => return Err(NcsError::UnknownConstType { ty, at }),
                }),
                Action => Arg::Action {
                    id: r.u16()?,
                    argc: r.u8()?,
                },
                Eq | Neq if ty == tc::STRUCT_STRUCT => Arg::StructSize(r.u16()?),
                MovSp | DecISp | IncISp | DecIBp | IncIBp => Arg::Offset(r.i32()?),
                Jmp | Jsr | Jz | Jnz => Arg::Jump(r.i32()?),
                Destruct => Arg::Destruct {
                    size: r.u16()?,
                    offset: r.u16()? as i16,
                    keep: r.u16()?,
                },
                StoreState => Arg::StoreState {
                    bp: r.u32()?,
                    sp: r.u32()?,
                },
                _ => Arg::None,
            };
            index.insert(at, instructions.len());
            instructions.push(Instruction {
                offset: at,
                op,
                ty,
                arg,
                len: r.at - at,
            });
            at = r.at;
        }
        let ncs = Ncs {
            instructions,
            index,
        };
        for i in &ncs.instructions {
            if matches!(i.arg, Arg::Jump(_)) {
                let ok = i.target().is_some_and(|t| ncs.index.contains_key(&t));
                if !ok {
                    let Arg::Jump(rel) = i.arg else {
                        unreachable!()
                    };
                    return Err(NcsError::BadJump {
                        at: i.offset,
                        target: i.offset as i64 + rel as i64,
                    });
                }
            }
        }
        Ok(ncs)
    }

    /// The instruction index at a byte offset, if an instruction starts there.
    pub fn at(&self, offset: usize) -> Option<usize> {
        self.index.get(&offset).copied()
    }

    /// Every `ACTION` id with its argument count, in code order.
    pub fn actions(&self) -> impl Iterator<Item = (u16, u8)> + '_ {
        self.instructions.iter().filter_map(|i| match i.arg {
            Arg::Action { id, argc } => Some((id, argc)),
            _ => None,
        })
    }

    /// A listing with jump targets labelled and action names resolved when a table is given.
    pub fn disassemble(&self, table: Option<&EngineTable>) -> String {
        use std::fmt::Write;
        let targets: std::collections::BTreeSet<usize> = self
            .instructions
            .iter()
            .filter_map(|i| i.target())
            .collect();
        let mut out = String::new();
        for i in &self.instructions {
            if targets.contains(&i.offset) {
                let _ = writeln!(out, "loc_{:08x}:", i.offset);
            }
            let _ = writeln!(out, "  {:08x}  {}", i.offset, Display { i, table });
        }
        out
    }
}

struct Display<'a> {
    i: &'a Instruction,
    table: Option<&'a EngineTable>,
}

fn type_suffix(ty: u8) -> String {
    match ty {
        0 | 1 => String::new(),
        tc::INT => "I".into(),
        tc::FLOAT => "F".into(),
        tc::STRING => "S".into(),
        tc::OBJECT => "O".into(),
        tc::RESOURCE => "R".into(),
        tc::INT_INT => "II".into(),
        tc::FLOAT_FLOAT => "FF".into(),
        tc::OBJECT_OBJECT => "OO".into(),
        tc::STRING_STRING => "SS".into(),
        tc::STRUCT_STRUCT => "TT".into(),
        tc::INT_FLOAT => "IF".into(),
        tc::FLOAT_INT => "FI".into(),
        tc::VECTOR_VECTOR => "VV".into(),
        tc::VECTOR_FLOAT => "VF".into(),
        tc::FLOAT_VECTOR => "FV".into(),
        0x10..=0x1F => format!("E{}", ty - 0x10),
        0x30..=0x39 => format!("E{}E{}", ty - 0x30, ty - 0x30),
        0x40..=0x44 => format!("A{}", ["I", "F", "S", "O", "R"][(ty - 0x40) as usize]),
        0x50..=0x5F => format!("AE{}", ty - 0x50),
        other => format!("{other:#04x}"),
    }
}

impl fmt::Display for Display<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let i = self.i;
        let m = format!("{}{}", i.op.mnemonic(), type_suffix(i.ty));
        match &i.arg {
            Arg::None => write!(f, "{m}"),
            Arg::Copy { offset, size } => write!(f, "{m:<14} {offset}, {size}"),
            Arg::Const(c) => match c {
                ConstArg::Int(v) => write!(f, "{m:<14} {v}"),
                ConstArg::Float(v) => write!(f, "{m:<14} {v:?}"),
                ConstArg::String(s) => write!(f, "{m:<14} {s:?}"),
                ConstArg::Object(o) => write!(f, "{m:<14} object {o:#x}"),
                ConstArg::Resource(s) => write!(f, "{m:<14} resource {s:?}"),
            },
            Arg::Action { id, argc } => {
                let name = self
                    .table
                    .and_then(|t| t.function(*id))
                    .map_or_else(|| format!("#{id}"), |fun| fun.name.clone());
                write!(f, "{m:<14} {name}({argc})")
            }
            Arg::StructSize(n) => write!(f, "{m:<14} {n}"),
            Arg::Offset(o) => write!(f, "{m:<14} {o}"),
            Arg::Jump(_) => write!(f, "{m:<14} loc_{:08x}", i.target().unwrap_or(0)),
            Arg::Destruct { size, offset, keep } => write!(f, "{m:<14} {size}, {offset}, {keep}"),
            Arg::StoreState { bp, sp } => write!(f, "{m:<14} bp {bp}, sp {sp}"),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Assembles instructions after an NCS header with the right size.
    pub fn assemble(body: &[u8]) -> Vec<u8> {
        let mut v = b"NCS V1.0B".to_vec();
        v.extend(((HEADER_SIZE + body.len()) as u32).to_be_bytes());
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn parses_and_disassembles() {
        // From the shipped setplayerimmortal.ncs (pattern, rebuilt by hand):
        // JSR +8; RETN; RSADD O; CONST O 0; CPDOWNSP -8,4; MOVSP -4; CONST I 1;
        // CPTOPSP -8,4; ACTION 220(2); MOVSP -4; RETN
        let mut b = vec![
            0x1E, 0x00, 0, 0, 0, 8, 0x20, 0x00, 0x02, 0x06, 0x04, 0x06, 0, 0, 0, 0,
        ];
        b.extend([0x01, 0x01, 0xff, 0xff, 0xff, 0xf8, 0x00, 0x04]);
        b.extend([0x1b, 0x00, 0xff, 0xff, 0xff, 0xfc]);
        b.extend([0x04, 0x03, 0, 0, 0, 1]);
        b.extend([0x03, 0x01, 0xff, 0xff, 0xff, 0xf8, 0x00, 0x04]);
        b.extend([0x05, 0x00, 0x00, 0xdc, 0x02]);
        b.extend([0x1b, 0x00, 0xff, 0xff, 0xff, 0xfc, 0x20, 0x00]);
        let ncs = Ncs::parse(&assemble(&b)).unwrap();
        assert_eq!(ncs.instructions.len(), 11);
        assert_eq!(ncs.instructions[0].target(), Some(HEADER_SIZE + 8));
        assert_eq!(ncs.actions().collect::<Vec<_>>(), vec![(220, 2)]);
        let text = ncs.disassemble(None);
        assert!(text.contains("ACTION         #220(2)"), "{text}");
        assert!(text.contains("loc_00000015:"), "{text}");
        assert!(text.contains("CPDOWNSP"), "{text}");
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(Ncs::parse(b"NCS V2.0B\0\0\0\x0d"), Err(NcsError::NotNcs));
        let mut v = assemble(&[0x20, 0x00]);
        v.push(0);
        assert!(matches!(Ncs::parse(&v), Err(NcsError::SizeMismatch { .. })));
        assert!(matches!(
            Ncs::parse(&assemble(&[0x04, 0x03, 0, 0])),
            Err(NcsError::Truncated(_))
        ));
        assert!(matches!(
            Ncs::parse(&assemble(&[0x99, 0x00])),
            Err(NcsError::UnknownOpcode { .. })
        ));
        // Jump into the middle of an instruction.
        assert!(matches!(
            Ncs::parse(&assemble(&[0x1D, 0x00, 0, 0, 0, 3, 0x20, 0x00])),
            Err(NcsError::BadJump { .. })
        ));
        // Resource constants are length-prefixed like strings.
        let ncs = Ncs::parse(&assemble(&[0x04, 0x60, 0, 2, b'a', b'b'])).unwrap();
        assert_eq!(
            ncs.instructions[0].arg,
            Arg::Const(ConstArg::Resource("ab".into()))
        );
    }
}
