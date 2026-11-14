// One affine palette, shared by the material and point-shadow vertex paths.
struct Skin {
    rows: array<vec4<f32>, 201>,
};
@group(0) @binding(2) var<uniform> skin: Skin;

fn skin_vector(p: vec4<f32>, weights: vec4<f32>, indices: vec4<f32>) -> vec3<f32> {
    var result = vec3<f32>(0.0);
    let total = dot(weights, vec4<f32>(1.0));
    for (var k = 0u; k < 4u; k += 1u) {
        let row = min(u32(round(indices[k])), 66u) * 3u;
        let posed = vec3<f32>(dot(skin.rows[row], p), dot(skin.rows[row + 1u], p), dot(skin.rows[row + 2u], p));
        result += posed * (weights[k] / max(total, 1e-20));
    }
    return result;
}
