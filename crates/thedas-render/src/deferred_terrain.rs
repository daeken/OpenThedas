//! Terrain1's palette inputs for the deferred material pass. The palette and its
//! masks are the original assets; lighting is evaluated from the G-buffer.

use crate::scene::SceneMaterial;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct TerrainData {
    pub dimensions: [f32; 4],
    pub parameters: [f32; 4],
    pub scales: [[f32; 4]; 2],
    pub specular: [[f32; 4]; 8],
    pub tint: [f32; 4],
}

/// Material-object arrays override the technique/definition's defaults, just as
/// the faithful constant binder does. Matrices hold the per-layer scale values.
fn array<const N: usize>(m: &SceneMaterial, name: &str, fallback: [f32; N]) -> [f32; N] {
    let given = m
        .object
        .vector_arrays
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name));
    let defaults = m
        .technique
        .iter()
        .flat_map(|t| &t.passes)
        .flat_map(|p| &p.parameters)
        .chain(
            m.definition
                .iter()
                .flat_map(|d| &d.parameter_sets)
                .flat_map(|s| &s.parameters),
        )
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .and_then(|p| p.default.as_deref());
    let values = given.map(|(_, v)| v.clone()).or_else(|| {
        defaults.map(|s| {
            s.split_whitespace()
                .filter_map(|v| v.parse().ok())
                .collect()
        })
    });
    let Some(values) = values else {
        return fallback;
    };
    let mut out = [0.0; N];
    for (dst, src) in out.iter_mut().zip(values) {
        *dst = src;
    }
    out
}

impl TerrainData {
    pub(crate) fn new(m: &SceneMaterial) -> Self {
        let vector = |name, default| crate::d3d::material_vector(m, name).unwrap_or(default);
        let scales = array(m, "mml_mUVScaleValues", [1.0; 8]);
        let specular = array(m, "mml_vSpecularColor", [0.0; 32]);
        Self {
            dimensions: vector("mml_vPalette_dimensions", [1.0, 0.25, 1.0, 4.0]),
            parameters: vector(
                "mml_vPalette_parameters",
                [0.009765625, 0.0024414063, 0.98046875, 0.24511719],
            ),
            scales: std::array::from_fn(|k| std::array::from_fn(|j| scales[k * 4 + j])),
            specular: std::array::from_fn(|k| std::array::from_fn(|j| specular[k * 4 + j])),
            tint: vector("mml_vTintAddValue", [0.0; 4]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrain_palette_keeps_all_eight_layers_and_atlas_padding() {
        let object = thedas_formats::material::MaterialObject::parse(br#"
            <MaterialObject Name="test"><Material Name="terrain.mat"/>
            <Matrix4x4f Name="mml_mUVScaleValues" value="10 15 10 10 9 6 10 8"/>
            <Vector4f Name="mml_vPalette_dimensions" value="0.5 0.25 2 4"/>
            <Vector4f Name="mml_vPalette_parameters" value="0.01 0.02 0.48 0.21"/>
            <Vector4fArray Name="mml_vSpecularColor" value="1 0 0 0.1 0 1 0 0.2 0 0 1 0.3 1 1 0 0.4 1 0 1 0.5 0 1 1 0.6 1 1 1 0.7 0.2 0.3 0.4 0.8"/>
            </MaterialObject>"#).unwrap();
        let m = SceneMaterial {
            mao: "test.mao".into(),
            object,
            definition: None,
            semantic: "HighLod".into(),
            render_class: None,
            technique: None,
        };
        let t = TerrainData::new(&m);
        assert_eq!(t.scales, [[10.0, 15.0, 10.0, 10.0], [9.0, 6.0, 10.0, 8.0]]);
        assert_eq!(t.specular[7], [0.2, 0.3, 0.4, 0.8]);
        assert_eq!(t.dimensions, [0.5, 0.25, 2.0, 4.0]);
        assert_eq!(t.parameters, [0.01, 0.02, 0.48, 0.21]);
    }
}
