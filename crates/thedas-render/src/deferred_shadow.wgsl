// The deferred renderer's point-light shadows (see `deferred_shadow.rs`): one face of a
// light's cube, written as each texel's distance from the light. Ours, not the original's.

struct Draw {
    // The face's view-projection times the caster's model-to-world
    mvp: mat4x4<f32>,
    world: mat4x4<f32>,
    // xyz: the light's position
    light: vec4<f32>,
};

@group(0) @binding(0) var<uniform> d: Draw;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
};

@vertex
fn vs_shadow(@location(0) position: vec3<f32>) -> Out {
    var out: Out;
    out.clip = d.mvp * vec4<f32>(position, 1.0);
    out.world = (d.world * vec4<f32>(position, 1.0)).xyz;
    return out;
}

@fragment
fn fs_shadow(in: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(length(in.world - d.light.xyz), 0.0, 0.0, 1.0);
}
