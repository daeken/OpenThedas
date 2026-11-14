//! Deferred vertex streams and skin palettes, separate from the faithful streams.

use crate::scene::{SceneMesh, Vertex};
use thedas_formats::mesh::DeclUsage;

// Origins' Char1 palette has 201 rows (67 three-row affine transforms).
pub(crate) const BONES: usize = 67;
pub(crate) const ROWS: usize = BONES * 3;
pub(crate) const SKIN_STRIDE: usize = (ROWS * 16).div_ceil(256) * 256;
pub(crate) const IDENTITY: [[f32; 4]; 3] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct DeferredVertex {
    base: Vertex,
    weights: [f32; 4],
    indices: [f32; 4],
}

pub(crate) fn vertices(mesh: &SceneMesh, skinned: bool) -> Option<Vec<DeferredVertex>> {
    if mesh.vertices.is_empty() {
        return None;
    }
    let attribute = |v: usize, usage| {
        let e = mesh.chunk.element(usage, 0)?;
        let at = v
            .checked_mul(mesh.chunk.vertex_size as usize)?
            .checked_add(e.offset as usize)?;
        Some(
            e.ty.decode(
                mesh.raw_vertices
                    .get(at..at.checked_add(e.ty.size() as usize)?)?,
            ),
        )
    };
    mesh.vertices
        .iter()
        .enumerate()
        .map(|(v, base)| {
            let mut base = *base;
            if let Some(p) = mesh.positions.as_ref().and_then(|p| p.get(v)) {
                base.position = [p[0], p[1], p[2]];
            }
            let (weights, indices) = if skinned {
                let weights = attribute(v, DeclUsage::BlendWeight)?;
                let mut indices = attribute(v, DeclUsage::BlendIndices)?;
                let total = weights.iter().sum::<f32>();
                if weights.iter().any(|w| !w.is_finite() || *w < 0.0)
                    || !total.is_finite()
                    || total <= 0.0
                {
                    return None;
                }
                for (index, weight) in indices.iter_mut().zip(weights) {
                    if weight == 0.0 {
                        *index = 0.0;
                        continue;
                    }
                    if !index.is_finite()
                        || *index < 0.0
                        || index.round() >= BONES as f32
                        || (*index - index.round()).abs() > 0.0001
                    {
                        return None;
                    }
                }
                (weights, indices)
            } else {
                ([1.0, 0.0, 0.0, 0.0], [0.0; 4])
            };
            Some(DeferredVertex {
                base,
                weights,
                indices,
            })
        })
        .collect()
}

pub(crate) fn write_palette(slot: &mut [u8], rows: &[[f32; 4]]) {
    for (i, dst) in slot[..ROWS * 16].chunks_exact_mut(16).enumerate() {
        dst.copy_from_slice(bytemuck::bytes_of(rows.get(i).unwrap_or(&IDENTITY[i % 3])));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use thedas_formats::mesh::{DeclType, IndexFormat, MeshChunk, VertexElement};

    #[test]
    fn vertex_stream_keeps_morphs_and_rejects_invalid_palettes() {
        let mut mesh = SceneMesh {
            key: "synthetic".into(),
            vertices: vec![Vertex {
                position: [0.0; 3],
                normal: [0.0, 0.0, 1.0],
                uv0: [0.0; 2],
                uv1: [0.0; 2],
            }],
            indices: vec![],
            chunk: MeshChunk {
                name: "synthetic".into(),
                vertex_size: 20,
                vertex_count: 1,
                index_count: 0,
                primitive: 0,
                index_format: IndexFormat::U16,
                base_vertex: 0,
                vertex_offset: 0,
                start_index: 0,
                bounds: None,
                elements: vec![
                    VertexElement {
                        stream: 0,
                        offset: 0,
                        ty: DeclType::Float4,
                        usage: DeclUsage::BlendWeight,
                        usage_index: 0,
                    },
                    VertexElement {
                        stream: 0,
                        offset: 16,
                        ty: DeclType::Ubyte4,
                        usage: DeclUsage::BlendIndices,
                        usage_index: 0,
                    },
                ],
            },
            raw_vertices: bytemuck::cast_slice(&[0.2f32, 0.3, 0.4, 0.1]).to_vec(),
            positions: Some(vec![[1.0, 2.0, 3.0, 1.0]]),
            vertex_light: None,
        };
        mesh.raw_vertices.extend([0, 1, 2, 3]);
        let v = vertices(&mesh, true).unwrap()[0];
        assert_eq!(v.base.position, [1.0, 2.0, 3.0]);
        assert_eq!(v.indices, [0.0, 1.0, 2.0, 3.0]);
        assert_eq!(v.weights, [0.2, 0.3, 0.4, 0.1]);
        mesh.raw_vertices[16] = 255;
        assert!(vertices(&mesh, true).is_none());
        assert_eq!(
            vertices(&mesh, false).unwrap()[0].weights,
            [1.0, 0.0, 0.0, 0.0]
        );
        mesh.raw_vertices.truncate(4);
        assert!(vertices(&mesh, true).is_none());
    }
}
