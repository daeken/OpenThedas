// The deferred renderer's point-light shadows (see `deferred_shadow.rs`): one face of a
// light's cube, written as each texel's distance from the light. Ours, not the original's.

struct Draw {
    // The face's view-projection times the caster's model-to-world
    mvp: mat4x4<f32>,
    world: mat4x4<f32>,
    // xyz: the light's position
    light: vec4<f32>,
    // x: comparison (0: opaque), y: 8-bit reference, z: texture channel, w: opacity
    alpha: vec4<f32>,
    // x: an additional shader clip threshold, before 8-bit alpha testing
    clip: vec4<f32>,
};

@group(0) @binding(0) var<uniform> d: Draw;
@group(1) @binding(0) var opacity_tex: texture_2d<f32>;
@group(1) @binding(1) var opacity_smp: sampler;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

@vertex
fn vs_shadow(@location(0) rest: vec3<f32>, @location(2) uv: vec2<f32>, @location(4) weights: vec4<f32>, @location(5) indices: vec4<f32>) -> Out {
    let position = skin_vector(vec4<f32>(rest, 1.0), weights, indices);
    var out: Out;
    out.clip = d.mvp * vec4<f32>(position, 1.0);
    out.world = (d.world * vec4<f32>(position, 1.0)).xyz;
    out.uv = uv;
    return out;
}

fn alpha_passes(alpha: f32, comparison: u32, reference: f32) -> bool {
    // Match the faithful D3D9 translation: test the rounded, saturated 8-bit alpha.
    let a = round(clamp(alpha, 0.0, 1.0) * 255.0);
    switch comparison {
        case 1u: { return false; }
        case 2u: { return a < reference; }
        case 3u: { return a == reference; }
        case 4u: { return a <= reference; }
        case 5u: { return a > reference; }
        case 6u: { return a != reference; }
        case 7u: { return a >= reference; }
        default: { return true; }
    }
}

@fragment
fn fs_shadow(in: Out) -> @location(0) vec4<f32> {
    if (d.alpha.x > 0.5) {
        let opacity = textureSample(opacity_tex, opacity_smp, in.uv);
        let a = opacity[u32(d.alpha.z)] * d.alpha.w;
        if ((d.clip.x >= 0.0 && a < d.clip.x) || !alpha_passes(a, u32(d.alpha.x), d.alpha.y)) { discard; }
    }
    return vec4<f32>(length(in.world - d.light.xyz), 0.0, 0.0, 1.0);
}
