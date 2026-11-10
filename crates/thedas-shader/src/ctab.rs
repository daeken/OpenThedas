//! The `CTAB` constant table that `fxc` embeds in a comment at the start of every shader: the
//! name, register set and register range of each uniform the HLSL source declared. The engine
//! fills constants by these names, so this is how parameters such as `aur_mWorldViewProj` or
//! `mml_vLightmapOffsetAndScale` find their registers.
//!
//! Layout per `D3DXSHADER_CONSTANTTABLE` / `D3DXSHADER_CONSTANTINFO` / `D3DXSHADER_TYPEINFO`
//! in Microsoft's public `d3dx9shader.h`. All offsets are relative to the start of the table
//! (just after the `CTAB` FourCC).

use crate::bytecode::BytecodeError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RegisterSet {
    Bool,
    Int4,
    Float4,
    Sampler,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamClass {
    Scalar,
    Vector,
    MatrixRows,
    MatrixColumns,
    Object,
    Struct,
    Other(u16),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Constant {
    pub name: String,
    pub set: RegisterSet,
    pub register: u16,
    pub count: u16,
    pub class: ParamClass,
    /// `D3DXPARAMETER_TYPE` (0 void, 1 bool, 2 int, 3 float, 10.. samplers).
    pub ty: u16,
    pub rows: u16,
    pub columns: u16,
    pub elements: u16,
    /// Default value, if the table carries one (`count` registers of four floats).
    pub default: Option<Vec<f32>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConstantTable {
    pub creator: String,
    pub target: String,
    pub constants: Vec<Constant>,
}

fn err(msg: impl Into<String>) -> BytecodeError {
    BytecodeError::Ctab(msg.into())
}

struct Table<'a> {
    d: &'a [u8],
}

impl Table<'_> {
    fn u16(&self, at: usize) -> Result<u16, BytecodeError> {
        self.d
            .get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .ok_or_else(|| err(format!("read past end at {at}")))
    }
    fn u32(&self, at: usize) -> Result<u32, BytecodeError> {
        self.d
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| err(format!("read past end at {at}")))
    }
    fn cstr(&self, at: usize) -> Result<String, BytecodeError> {
        let rest = self
            .d
            .get(at..)
            .ok_or_else(|| err(format!("string offset {at} past end")))?;
        let end = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| err("unterminated string"))?;
        Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
    }
}

impl ConstantTable {
    pub fn parse(d: &[u8]) -> Result<Self, BytecodeError> {
        let t = Table { d };
        let size = t.u32(0)?;
        if size < 28 {
            return Err(err(format!("header size {size}")));
        }
        let creator = t.cstr(t.u32(4)? as usize)?;
        let count = t.u32(12)? as usize;
        let info = t.u32(16)? as usize;
        let target = t.cstr(t.u32(24)? as usize)?;
        if count > 4096 {
            return Err(err(format!("{count} constants")));
        }
        let mut constants = Vec::with_capacity(count);
        for i in 0..count {
            let at = info + i * 20;
            let name = t.cstr(t.u32(at)? as usize)?;
            let set = match t.u16(at + 4)? {
                0 => RegisterSet::Bool,
                1 => RegisterSet::Int4,
                2 => RegisterSet::Float4,
                3 => RegisterSet::Sampler,
                s => return Err(err(format!("{name}: register set {s}"))),
            };
            let register = t.u16(at + 6)?;
            let reg_count = t.u16(at + 8)?;
            let ti = t.u32(at + 12)? as usize;
            let def = t.u32(at + 16)? as usize;
            let class = match t.u16(ti)? {
                0 => ParamClass::Scalar,
                1 => ParamClass::Vector,
                2 => ParamClass::MatrixRows,
                3 => ParamClass::MatrixColumns,
                4 => ParamClass::Object,
                5 => ParamClass::Struct,
                c => ParamClass::Other(c),
            };
            let default = if def != 0 && set == RegisterSet::Float4 {
                let mut v = Vec::with_capacity(reg_count as usize * 4);
                for k in 0..reg_count as usize * 4 {
                    v.push(f32::from_bits(t.u32(def + k * 4)?));
                }
                Some(v)
            } else {
                None
            };
            constants.push(Constant {
                name,
                set,
                register,
                count: reg_count,
                class,
                ty: t.u16(ti + 2)?,
                rows: t.u16(ti + 4)?,
                columns: t.u16(ti + 6)?,
                elements: t.u16(ti + 8)?,
                default,
            });
        }
        Ok(ConstantTable {
            creator,
            target,
            constants,
        })
    }

    pub fn get(&self, name: &str) -> Option<&Constant> {
        self.constants.iter().find(|c| c.name == name)
    }

    /// The constant that covers register `index` of `set`, if any.
    pub fn at(&self, set: RegisterSet, index: u32) -> Option<&Constant> {
        self.constants.iter().find(|c| {
            c.set == set
                && index >= c.register as u32
                && index < c.register as u32 + c.count.max(1) as u32
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_table() {
        // Header (28) + one constant info (20) + type info (16) + strings + default.
        let mut d = vec![0u8; 28 + 20 + 16];
        let put32 =
            |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        let put16 =
            |d: &mut Vec<u8>, at: usize, v: u16| d[at..at + 2].copy_from_slice(&v.to_le_bytes());
        let creator = d.len();
        d.extend(b"test\0");
        let target = d.len();
        d.extend(b"ps_3_0\0");
        let name = d.len();
        d.extend(b"g_vColor\0");
        while d.len() % 4 != 0 {
            d.push(0);
        }
        let def = d.len();
        for f in [1.0f32, 0.5, 0.25, 0.0] {
            d.extend(f.to_le_bytes());
        }
        put32(&mut d, 0, 28);
        put32(&mut d, 4, creator as u32);
        put32(&mut d, 8, 0xFFFF_0300);
        put32(&mut d, 12, 1);
        put32(&mut d, 16, 28);
        put32(&mut d, 24, target as u32);
        put32(&mut d, 28, name as u32);
        put16(&mut d, 32, 2); // float4
        put16(&mut d, 34, 5); // c5
        put16(&mut d, 36, 1);
        put32(&mut d, 40, 48); // type info
        put32(&mut d, 44, def as u32);
        put16(&mut d, 48, 1); // vector
        put16(&mut d, 50, 3); // float
        put16(&mut d, 52, 1);
        put16(&mut d, 54, 4);
        put16(&mut d, 56, 1);
        let t = ConstantTable::parse(&d).unwrap();
        assert_eq!(t.target, "ps_3_0");
        let c = t.get("g_vColor").unwrap();
        assert_eq!((c.set, c.register, c.count), (RegisterSet::Float4, 5, 1));
        assert_eq!(c.class, ParamClass::Vector);
        assert_eq!(c.default.as_deref(), Some(&[1.0, 0.5, 0.25, 0.0][..]));
        assert_eq!(t.at(RegisterSet::Float4, 5).unwrap().name, "g_vColor");
        assert!(t.at(RegisterSet::Float4, 6).is_none());
        // Truncation is an error, not a panic.
        assert!(ConstantTable::parse(&d[..40]).is_err());
    }
}
