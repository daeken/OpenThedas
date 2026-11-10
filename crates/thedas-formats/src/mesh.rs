//! Mesh data (`.msh`, GFF4 `MESH`): one shared vertex blob and one shared index blob, cut
//! into named chunks. Each chunk declares its vertex layout with Direct3D 9 vertex elements
//! (`D3DVERTEXELEMENT9`: stream, offset, type, usage, usage index), so the bytes can go to the
//! GPU unchanged once the layout is translated.
//!
//! Provisional: field labels and the meaning of the chunk header fields come from the data.
//! In every mesh in the GOG 1.05 install, field `8006` is a byte offset into the vertex blob
//! (chunks are padded apart), field `8009` is the first index of the chunk in the index blob,
//! index format `0` is 16-bit and `1` is 32-bit, and primitive type `0` is a triangle list.

use crate::gff4::{Gff4, StructId};
use crate::typed::{Doc, TypedError, TypedResult};

pub mod field {
    //! GFF4 field labels used by the mesh reader. All provisional.

    pub const NAME: u32 = 2;
    pub const CHUNKS: u32 = 8021;
    pub const VERTEX_DATA: u32 = 8022;
    pub const INDEX_DATA: u32 = 8023;

    pub const VERTEX_SIZE: u32 = 8000;
    pub const VERTEX_COUNT: u32 = 8001;
    pub const INDEX_COUNT: u32 = 8002;
    pub const PRIMITIVE: u32 = 8003;
    pub const INDEX_FORMAT: u32 = 8004;
    pub const BASE_VERTEX: u32 = 8005;
    pub const VERTEX_OFFSET: u32 = 8006;
    pub const START_INDEX: u32 = 8009;
    pub const BOUNDS: u32 = 8020;
    pub const DECLARATION: u32 = 8025;

    pub const DECL_STREAM: u32 = 8026;
    pub const DECL_OFFSET: u32 = 8027;
    pub const DECL_TYPE: u32 = 8028;
    pub const DECL_USAGE: u32 = 8029;
    pub const DECL_USAGE_INDEX: u32 = 8030;

    pub const BOUNDS_MIN: u32 = 8017;
    pub const BOUNDS_MAX: u32 = 8018;
}

/// `D3DDECLTYPE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclType {
    Float1,
    Float2,
    Float3,
    Float4,
    /// `D3DCOLOR`: 4 bytes, BGRA order in memory.
    D3dColor,
    Ubyte4,
    Short2,
    Short4,
    Ubyte4N,
    Short2N,
    Short4N,
    Ushort2N,
    Ushort4N,
    Udec3,
    Dec3N,
    Float16x2,
    Float16x4,
}

impl DeclType {
    pub fn from_code(code: u32) -> Option<Self> {
        use DeclType::*;
        Some(match code {
            0 => Float1,
            1 => Float2,
            2 => Float3,
            3 => Float4,
            4 => D3dColor,
            5 => Ubyte4,
            6 => Short2,
            7 => Short4,
            8 => Ubyte4N,
            9 => Short2N,
            10 => Short4N,
            11 => Ushort2N,
            12 => Ushort4N,
            13 => Udec3,
            14 => Dec3N,
            15 => Float16x2,
            16 => Float16x4,
            _ => return None,
        })
    }

    /// Size in bytes.
    pub fn size(self) -> u32 {
        use DeclType::*;
        match self {
            Float1 | D3dColor | Ubyte4 | Short2 | Ubyte4N | Short2N | Ushort2N | Udec3 | Dec3N
            | Float16x2 => 4,
            Float2 | Short4 | Short4N | Ushort4N | Float16x4 => 8,
            Float3 => 12,
            Float4 => 16,
        }
    }

    /// Decodes one element into four floats the way Direct3D 9 expands it for a shader input
    /// (missing components default to 0, 0, 0, 1). `bytes` must be at least [`Self::size`] long.
    pub fn decode(self, b: &[u8]) -> [f32; 4] {
        use DeclType::*;
        let f32_at = |i: usize| f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        let i16_at = |i: usize| i16::from_le_bytes([b[i], b[i + 1]]);
        let h = |i: usize| f16_to_f32(u16_at(i));
        let sn = |v: i16| (v as f32 / 32767.0).max(-1.0);
        let un = |v: u16| v as f32 / 65535.0;
        let u32_le = || u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        match self {
            Float1 => [f32_at(0), 0.0, 0.0, 1.0],
            Float2 => [f32_at(0), f32_at(4), 0.0, 1.0],
            Float3 => [f32_at(0), f32_at(4), f32_at(8), 1.0],
            Float4 => [f32_at(0), f32_at(4), f32_at(8), f32_at(12)],
            D3dColor => [
                b[2] as f32 / 255.0,
                b[1] as f32 / 255.0,
                b[0] as f32 / 255.0,
                b[3] as f32 / 255.0,
            ],
            Ubyte4 => [b[0] as f32, b[1] as f32, b[2] as f32, b[3] as f32],
            Ubyte4N => [
                b[0] as f32 / 255.0,
                b[1] as f32 / 255.0,
                b[2] as f32 / 255.0,
                b[3] as f32 / 255.0,
            ],
            Short2 => [i16_at(0) as f32, i16_at(2) as f32, 0.0, 1.0],
            Short4 => [
                i16_at(0) as f32,
                i16_at(2) as f32,
                i16_at(4) as f32,
                i16_at(6) as f32,
            ],
            Short2N => [sn(i16_at(0)), sn(i16_at(2)), 0.0, 1.0],
            Short4N => [sn(i16_at(0)), sn(i16_at(2)), sn(i16_at(4)), sn(i16_at(6))],
            Ushort2N => [un(u16_at(0)), un(u16_at(2)), 0.0, 1.0],
            Ushort4N => [un(u16_at(0)), un(u16_at(2)), un(u16_at(4)), un(u16_at(6))],
            Udec3 => {
                let v = u32_le();
                [
                    (v & 0x3ff) as f32,
                    ((v >> 10) & 0x3ff) as f32,
                    ((v >> 20) & 0x3ff) as f32,
                    1.0,
                ]
            }
            Dec3N => {
                let v = u32_le();
                let s = |x: u32| {
                    let x = ((x << 22) as i32) >> 22;
                    (x as f32 / 511.0).max(-1.0)
                };
                [s(v), s(v >> 10), s(v >> 20), 1.0]
            }
            Float16x2 => [h(0), h(2), 0.0, 1.0],
            Float16x4 => [h(0), h(2), h(4), h(6)],
        }
    }
}

/// `D3DDECLUSAGE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeclUsage {
    Position,
    BlendWeight,
    BlendIndices,
    Normal,
    PointSize,
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

impl DeclUsage {
    pub fn from_code(code: u32) -> Option<Self> {
        use DeclUsage::*;
        Some(match code {
            0 => Position,
            1 => BlendWeight,
            2 => BlendIndices,
            3 => Normal,
            4 => PointSize,
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
pub struct VertexElement {
    pub stream: u32,
    pub offset: u32,
    pub ty: DeclType,
    pub usage: DeclUsage,
    pub usage_index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexFormat {
    U16,
    U32,
}

impl IndexFormat {
    pub fn size(self) -> usize {
        match self {
            IndexFormat::U16 => 2,
            IndexFormat::U32 => 4,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MeshChunk {
    pub name: String,
    /// Vertex stride in bytes.
    pub vertex_size: u32,
    pub vertex_count: u32,
    pub index_count: u32,
    /// Raw primitive type; `0` is a triangle list (provisional).
    pub primitive: u32,
    pub index_format: IndexFormat,
    pub base_vertex: u32,
    /// Byte offset of the chunk's first vertex in [`Msh::vertex_data`].
    pub vertex_offset: u32,
    /// Index of the chunk's first index in [`Msh::index_data`].
    pub start_index: u32,
    pub elements: Vec<VertexElement>,
    pub bounds: Option<([f32; 3], [f32; 3])>,
}

impl MeshChunk {
    pub fn element(&self, usage: DeclUsage, usage_index: u32) -> Option<&VertexElement> {
        self.elements
            .iter()
            .find(|e| e.usage == usage && e.usage_index == usage_index && e.stream == 0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Msh {
    pub name: Option<String>,
    pub chunks: Vec<MeshChunk>,
    pub vertex_data: Vec<u8>,
    pub index_data: Vec<u8>,
}

impl Msh {
    pub fn parse(data: &[u8]) -> TypedResult<Self> {
        Self::from_gff4(&Gff4::parse(data)?)
    }

    /// Reads and validates a mesh: every chunk's vertex and index ranges must lie inside the
    /// blobs, every element must fit in the vertex stride, and every index must address a
    /// vertex of its chunk.
    pub fn from_gff4(gff: &Gff4) -> TypedResult<Self> {
        let d = Doc::new(gff, "msh");
        d.expect_type("MESH")?;
        let root = gff.root;
        let mut chunks = Vec::new();
        for c in d.structs(root, field::CHUNKS)? {
            chunks.push(read_chunk(gff, c)?);
        }
        let msh = Msh {
            name: d.opt_str(root, field::NAME)?,
            chunks,
            vertex_data: d.bytes(root, field::VERTEX_DATA)?.to_vec(),
            index_data: d.bytes(root, field::INDEX_DATA)?.to_vec(),
        };
        for c in &msh.chunks {
            msh.validate(c)?;
        }
        Ok(msh)
    }

    pub fn chunk(&self, name: &str) -> Option<&MeshChunk> {
        self.chunks
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }

    fn invalid(chunk: &MeshChunk, msg: impl std::fmt::Display) -> TypedError {
        TypedError::Invalid(format!("msh chunk {}: {msg}", chunk.name))
    }

    fn validate(&self, c: &MeshChunk) -> TypedResult<()> {
        for e in &c.elements {
            if e.offset as u64 + e.ty.size() as u64 > c.vertex_size as u64 {
                return Err(Self::invalid(
                    c,
                    format!("element {e:?} exceeds the vertex stride"),
                ));
            }
        }
        self.vertex_bytes(c)?;
        for i in self.indices(c)? {
            if i as u64 + c.base_vertex as u64 >= c.vertex_count as u64 {
                return Err(Self::invalid(
                    c,
                    format!("index {i} out of range for {} vertices", c.vertex_count),
                ));
            }
        }
        Ok(())
    }

    /// The chunk's vertex bytes: `vertex_count * vertex_size` bytes from its offset.
    pub fn vertex_bytes(&self, c: &MeshChunk) -> TypedResult<&[u8]> {
        let len = c.vertex_count as u64 * c.vertex_size as u64;
        let start = c.vertex_offset as u64;
        match start.checked_add(len) {
            Some(end) if end <= self.vertex_data.len() as u64 => {
                Ok(&self.vertex_data[start as usize..end as usize])
            }
            _ => Err(Self::invalid(
                c,
                format!(
                    "vertices {start}+{len} exceed the {}-byte vertex blob",
                    self.vertex_data.len()
                ),
            )),
        }
    }

    /// The chunk's indices, widened to `u32`, relative to its first vertex.
    pub fn indices(&self, c: &MeshChunk) -> TypedResult<Vec<u32>> {
        let size = c.index_format.size() as u64;
        let start = c.start_index as u64 * size;
        let len = c.index_count as u64 * size;
        let bytes = match start.checked_add(len) {
            Some(end) if end <= self.index_data.len() as u64 => {
                &self.index_data[start as usize..end as usize]
            }
            _ => {
                return Err(Self::invalid(
                    c,
                    format!(
                        "indices {start}+{len} exceed the {}-byte index blob",
                        self.index_data.len()
                    ),
                ));
            }
        };
        Ok(match c.index_format {
            IndexFormat::U16 => bytes
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]) as u32)
                .collect(),
            IndexFormat::U32 => bytes
                .chunks_exact(4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect(),
        })
    }

    /// Decodes one attribute for every vertex of a chunk, or `None` if the chunk lacks it.
    pub fn attribute(
        &self,
        c: &MeshChunk,
        usage: DeclUsage,
        usage_index: u32,
    ) -> TypedResult<Option<Vec<[f32; 4]>>> {
        let Some(e) = c.element(usage, usage_index) else {
            return Ok(None);
        };
        let bytes = self.vertex_bytes(c)?;
        let stride = c.vertex_size as usize;
        let (off, size) = (e.offset as usize, e.ty.size() as usize);
        if stride == 0 || off + size > stride {
            return Err(Self::invalid(c, "element outside the vertex stride"));
        }
        Ok(Some(
            bytes
                .chunks_exact(stride)
                .map(|v| e.ty.decode(&v[off..off + size]))
                .collect(),
        ))
    }
}

fn read_chunk(gff: &Gff4, c: StructId) -> TypedResult<MeshChunk> {
    let d = Doc::new(gff, "msh chunk");
    let name = d.opt_str(c, field::NAME)?.unwrap_or_default();
    let index_format = match d.opt_i64(c, field::INDEX_FORMAT)?.unwrap_or(0) {
        0 => IndexFormat::U16,
        1 => IndexFormat::U32,
        other => {
            return Err(TypedError::Invalid(format!(
                "msh chunk {name}: unknown index format {other}"
            )));
        }
    };
    let mut elements = Vec::new();
    for e in d.structs(c, field::DECLARATION)? {
        let d = Doc::new(gff, "msh vertex element");
        let stream = d.i64(e, field::DECL_STREAM)?;
        let ty = d.i64(e, field::DECL_TYPE)?;
        // D3DDECL_END(): stream 0xFF (stored as -1 here) and type UNUSED.
        if stream < 0 || stream == 0xFF || ty == 17 || ty == 0xFFFF_FFFF {
            continue;
        }
        let ty_code = u32::try_from(ty).ok();
        let usage = d.i64(e, field::DECL_USAGE)?;
        elements.push(VertexElement {
            stream: stream as u32,
            offset: d.u32(e, field::DECL_OFFSET)?,
            ty: ty_code.and_then(DeclType::from_code).ok_or_else(|| {
                TypedError::Invalid(format!("msh chunk {name}: unknown vertex type {ty}"))
            })?,
            usage: u32::try_from(usage)
                .ok()
                .and_then(DeclUsage::from_code)
                .ok_or_else(|| {
                    TypedError::Invalid(format!("msh chunk {name}: unknown vertex usage {usage}"))
                })?,
            usage_index: d.u32(e, field::DECL_USAGE_INDEX)?,
        });
    }
    let bounds = match d.structs(c, field::BOUNDS)?.first() {
        Some(&b) => {
            let d = Doc::new(gff, "msh bounds");
            match (
                d.opt_vec3(b, field::BOUNDS_MIN)?,
                d.opt_vec3(b, field::BOUNDS_MAX)?,
            ) {
                (Some(lo), Some(hi)) => Some((lo, hi)),
                _ => None,
            }
        }
        None => None,
    };
    Ok(MeshChunk {
        vertex_size: d.u32(c, field::VERTEX_SIZE)?,
        vertex_count: d.u32(c, field::VERTEX_COUNT)?,
        index_count: d.u32(c, field::INDEX_COUNT)?,
        primitive: d.opt_i64(c, field::PRIMITIVE)?.unwrap_or(0) as u32,
        index_format,
        base_vertex: d.opt_i64(c, field::BASE_VERTEX)?.unwrap_or(0).max(0) as u32,
        vertex_offset: d.opt_i64(c, field::VERTEX_OFFSET)?.unwrap_or(0).max(0) as u32,
        start_index: d.opt_i64(c, field::START_INDEX)?.unwrap_or(0).max(0) as u32,
        name,
        elements,
        bounds,
    })
}

/// IEEE 754 half precision to single precision.
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) as u32) << 31;
    let exp = ((h >> 10) & 0x1f) as u32;
    let mant = (h & 0x3ff) as u32;
    let bits = match (exp, mant) {
        (0, 0) => sign,
        (0, m) => {
            // Subnormal: normalize.
            let mut e = 127 - 15 + 1;
            let mut m = m;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | ((e as u32) << 23) | ((m & 0x3ff) << 13)
        }
        (0x1f, m) => sign | 0x7f80_0000 | (m << 13),
        (e, m) => sign | ((e + 127 - 15) << 23) | (m << 13),
    };
    f32::from_bits(bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gff4::{List, Value};
    use crate::typed::testutil::{Builder, s, structs};

    fn decl(
        b: &mut Builder,
        stream: i32,
        offset: u32,
        ty: u32,
        usage: u32,
        index: u32,
    ) -> StructId {
        b.add(
            b"decl",
            vec![
                (field::DECL_STREAM, Value::I32(stream)),
                (field::DECL_OFFSET, Value::U32(offset)),
                (field::DECL_TYPE, Value::U32(ty)),
                (field::DECL_USAGE, Value::U32(usage)),
                (field::DECL_USAGE_INDEX, Value::U32(index)),
            ],
        )
    }

    /// Two chunks: a padded first chunk and a second one starting at index 3.
    fn sample(indices: &[u16]) -> Gff4 {
        let mut b = Builder::new(b"MESH");
        let p = decl(&mut b, 0, 0, 2, 0, 0); // FLOAT3 position
        let t = decl(&mut b, 0, 12, 15, 5, 1); // FLOAT16_2 texcoord1
        let end = decl(&mut b, -1, 0, 0xFFFF_FFFF, 0xFFFF_FFFF, 0);
        let chunk = |b: &mut Builder, name: &str, voff: u32, start: u32| {
            b.add(
                b"chnk",
                vec![
                    (field::NAME, s(name)),
                    (field::VERTEX_SIZE, Value::U32(16)),
                    (field::VERTEX_COUNT, Value::U32(3)),
                    (field::INDEX_COUNT, Value::U32(3)),
                    (field::PRIMITIVE, Value::U32(0)),
                    (field::INDEX_FORMAT, Value::U32(0)),
                    (field::VERTEX_OFFSET, Value::U32(voff)),
                    (field::START_INDEX, Value::U32(start)),
                    (field::DECLARATION, structs(&[p, t, end])),
                ],
            )
        };
        let c0 = chunk(&mut b, "A", 0, 0);
        let c1 = chunk(&mut b, "B", 64, 3);
        let mut verts = Vec::new();
        for i in 0..7u32 {
            let f = i as f32;
            for v in [f, f + 0.5, -f] {
                verts.extend_from_slice(&v.to_le_bytes());
            }
            verts.extend_from_slice(&0x3c00u16.to_le_bytes()); // 1.0
            verts.extend_from_slice(&0x3800u16.to_le_bytes()); // 0.5
        }
        let idx: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let root = b.add(
            b"mesh",
            vec![
                (field::NAME, s("test.msh")),
                (field::CHUNKS, structs(&[c0, c1])),
                (field::VERTEX_DATA, Value::List(Some(List::U8(verts)))),
                (field::INDEX_DATA, Value::List(Some(List::U8(idx)))),
            ],
        );
        b.finish(root)
    }

    #[test]
    fn reads_chunks_and_attributes() {
        let msh = Msh::from_gff4(&sample(&[0, 1, 2, 2, 1, 0])).unwrap();
        assert_eq!(msh.chunks.len(), 2);
        let b = msh.chunk("b").unwrap();
        assert_eq!(b.elements.len(), 2, "end marker skipped");
        assert_eq!(msh.indices(b).unwrap(), vec![2, 1, 0]);
        let pos = msh.attribute(b, DeclUsage::Position, 0).unwrap().unwrap();
        // Chunk B starts at byte 64 = vertex 4 of the blob.
        assert_eq!(pos[0], [4.0, 4.5, -4.0, 1.0]);
        let uv = msh.attribute(b, DeclUsage::TexCoord, 1).unwrap().unwrap();
        assert_eq!(uv[2], [1.0, 0.5, 0.0, 1.0]);
        assert!(msh.attribute(b, DeclUsage::Normal, 0).unwrap().is_none());
    }

    #[test]
    fn rejects_out_of_range_data() {
        // Index 3 is outside a 3-vertex chunk.
        assert!(Msh::from_gff4(&sample(&[0, 1, 3, 0, 1, 2])).is_err());
        // Index blob too short for the second chunk.
        assert!(Msh::from_gff4(&sample(&[0, 1, 2])).is_err());
    }

    #[test]
    fn half_floats() {
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
        assert!(f16_to_f32(0x7c00).is_infinite());
        assert!(f16_to_f32(0x7e00).is_nan());
        assert_eq!(
            DeclType::D3dColor.decode(&[0, 0, 255, 255]),
            [1.0, 0.0, 0.0, 1.0]
        );
    }
}
