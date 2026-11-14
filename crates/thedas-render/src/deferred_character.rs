//! Authored face composition and eye controls for the optional renderer.

use crate::scene::SceneMaterial;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct CharacterData {
    pub age: [[f32; 4]; 6],
    pub emotion: [[f32; 4]; 2],
    pub hair_weights: [[f32; 4]; 2],
    pub hair_colour: [f32; 4],
    pub tattoo: [[f32; 4]; 5],
    pub specular: [[f32; 4]; 3],
    pub ambient: [f32; 4],
    pub eye: [f32; 4],
}

pub(crate) fn vectors<const N: usize>(
    m: &SceneMaterial,
    name: &str,
    fallback: [[f32; 4]; N],
) -> [[f32; 4]; N] {
    let values = m
        .object
        .vector_arrays
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| {
            v.chunks(4)
                .map(|v| {
                    let mut row = [0.0; 4];
                    row[..v.len()].copy_from_slice(v);
                    row
                })
                .collect::<Vec<_>>()
        })
        .or_else(|| crate::d3d::default_vectors(m, name));
    std::array::from_fn(|i| {
        values
            .as_ref()
            .and_then(|v| v.get(i).copied())
            .unwrap_or(fallback[i])
    })
}

impl CharacterData {
    pub(crate) fn new(m: &SceneMaterial) -> Self {
        Self {
            age: vectors(m, "mml_fAgeBlendWeight", [[0.0; 4]; 6]),
            emotion: vectors(m, "mml_fEmotionsMapWeight", [[0.0; 4]; 2]),
            hair_weights: vectors(m, "g_vFacialHairWeights", [[0.0; 4]; 2]),
            hair_colour: crate::d3d::material_vector(m, "g_vFacialHairColour").unwrap_or([0.0; 4]),
            tattoo: vectors(m, "mml_vTattooParameters", [[0.0; 4]; 5]),
            specular: vectors(
                m,
                "g_vSpecularExponents",
                [
                    [0.17, 0.17, 0.15, 0.15],
                    [0.10, 0.10, 0.20, 0.10],
                    [0.10, 0.15, 0.0, 0.0],
                ],
            ),
            ambient: crate::d3d::material_vector(m, "g_vAmbientMult")
                .unwrap_or([0.5, 0.5, 0.5, 1.0]),
            eye: crate::d3d::material_vector(m, "mml_vEyeSpecularParameters")
                .unwrap_or([30.0, 0.75, 75.0, 0.75]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_face_arrays_override_defaults_and_keep_later_rows() {
        let object = thedas_formats::material::MaterialObject::parse(br#"
            <MaterialObject Name="test"><Material Name="face.mat"/>
            <Vector4fArray Name="MML_FAGEBLENDWEIGHT" value="1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24"/>
            <Vector4fArray Name="g_vFacialHairWeights" value="0.1 0.2 0.3 0.4 0.5 0.6 0.7 0.8"/>
            <Vector4f Name="mml_vEyeSpecularParameters" value="25 0.3 80 0.7"/>
            </MaterialObject>"#).unwrap();
        let m = SceneMaterial {
            mao: "test.mao".into(),
            object,
            definition: None,
            semantic: "Default".into(),
            render_class: None,
            technique: None,
        };
        let data = CharacterData::new(&m);
        assert_eq!(data.age[5], [21.0, 22.0, 23.0, 24.0]);
        assert_eq!(data.hair_weights[1], [0.5, 0.6, 0.7, 0.8]);
        assert_eq!(data.eye, [25.0, 0.3, 80.0, 0.7]);
        assert_eq!(data.tattoo, [[0.0; 4]; 5]);
        assert_eq!(data.ambient, [0.5, 0.5, 0.5, 1.0]);
    }
}
