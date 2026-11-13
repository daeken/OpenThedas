//! Dragon Age II's per-vertex baked lighting (`.vlm`).
//!
//! A room layout binds a placed model's mesh piece either to a region of a lightmap atlas or,
//! by a `VX...` name, to one of these files: lighting baked into the piece's vertices instead
//! of a texture. The file has no header. It is 12 bytes a vertex, in the piece's vertex
//! order: three colours of four bytes each.
//!
//! Provisional: the layout is read from the shipped files (every one of the 51,417 in the
//! base game's levels is a whole number of 12-byte records), not from the executable. The
//! three colours are presumably the same three lighting terms as the three textures of a
//! baked lightmap (`<name>x.dds`, `<name>y.dds`, `<name>z.dds`), and the fourth byte of each
//! is not a constant, so it carries something (a scale, perhaps); neither is worked out, and
//! the bytes are kept as stored.

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum VlmError {
    #[error("{0} bytes is not a whole number of 12-byte vertices")]
    BadLength(usize),
}

/// One vertex's baked lighting: three four-byte colours, as stored.
pub type VertexLight = [[u8; 4]; 3];

/// A mesh piece's per-vertex baked lighting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Vlm {
    /// One entry a vertex, in the mesh piece's vertex order.
    pub vertices: Vec<VertexLight>,
}

impl Vlm {
    /// The size of one vertex's record.
    pub const VERTEX_SIZE: usize = 12;

    pub fn parse(data: &[u8]) -> Result<Self, VlmError> {
        if data.len() % Self::VERTEX_SIZE != 0 {
            return Err(VlmError::BadLength(data.len()));
        }
        let vertices = data
            .chunks_exact(Self::VERTEX_SIZE)
            .map(|v| {
                [
                    [v[0], v[1], v[2], v[3]],
                    [v[4], v[5], v[6], v[7]],
                    [v[8], v[9], v[10], v[11]],
                ]
            })
            .collect();
        Ok(Vlm { vertices })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_three_colours_a_vertex() {
        let data: Vec<u8> = (0..24).collect();
        let vlm = Vlm::parse(&data).unwrap();
        assert_eq!(vlm.vertices.len(), 2);
        assert_eq!(
            vlm.vertices[1],
            [[12, 13, 14, 15], [16, 17, 18, 19], [20, 21, 22, 23]]
        );
        assert_eq!(Vlm::parse(&[]).unwrap().vertices.len(), 0);
        assert_eq!(Vlm::parse(&data[..23]), Err(VlmError::BadLength(23)));
    }
}
