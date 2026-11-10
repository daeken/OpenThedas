// Stand-in area shader: diffuse texture times lightmap. NOT the original look; the original
// Static1 shaders are translated from their D3D9 bytecode in a later milestone step.
//
// Lightmap use is a guess (provisional): DA:O atlases are DXT5 with dim RGB and a busy alpha
// channel, so the stand-in treats RGB as baked light and alpha as extra light from the
// room's dynamic lights. Draws without a lightmap binding get a flat light level.

struct Frame {
    view_proj: mat4x4<f32>,
    camera: vec4<f32>,
    // x: lightmap RGB scale, y: lightmap alpha weight, z: flat light level, w: unused
    light: vec4<f32>,
};

struct Draw {
    model: mat4x4<f32>,
    // xy: atlas offset, zw: atlas scale
    lightmap: vec4<f32>,
    // x: 1 when the draw has a lightmap
    flags: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var diffuse_tex: texture_2d<f32>;
@group(1) @binding(1) var diffuse_smp: sampler;
@group(2) @binding(0) var lightmap_tex: texture_2d<f32>;
@group(2) @binding(1) var lightmap_smp: sampler;
@group(3) @binding(0) var<uniform> draw: Draw;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv0: vec2<f32>,
    @location(3) uv1: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv0: vec2<f32>,
    @location(1) uv_lm: vec2<f32>,
    @location(2) normal: vec3<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    let world = draw.model * vec4<f32>(v.position, 1.0);
    out.clip = frame.view_proj * world;
    out.uv0 = v.uv0;
    out.uv_lm = v.uv1 * draw.lightmap.zw + draw.lightmap.xy;
    out.normal = (draw.model * vec4<f32>(v.normal, 0.0)).xyz;
    return out;
}

fn light(in: VsOut) -> vec3<f32> {
    if (draw.flags.x > 0.5) {
        let lm = textureSample(lightmap_tex, lightmap_smp, in.uv_lm);
        return lm.rgb * frame.light.x + vec3<f32>(lm.a * frame.light.y);
    }
    return vec3<f32>(frame.light.z);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let d = textureSample(diffuse_tex, diffuse_smp, in.uv0);
    return vec4<f32>(d.rgb * light(in), d.a);
}

@fragment
fn fs_punch(in: VsOut) -> @location(0) vec4<f32> {
    let d = textureSample(diffuse_tex, diffuse_smp, in.uv0);
    if (d.a < 0.5) {
        discard;
    }
    return vec4<f32>(d.rgb * light(in), 1.0);
}

@fragment
fn fs_black(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}
