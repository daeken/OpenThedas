// The deferred renderer's G-buffer and lighting passes (see `deferred.rs` and
// docs/deferred.md). Ours, not the original's: the lighting follows Static1.psh's sum
// (notes/frame-gamma-lightmap-trace.md) per pixel, over every light in reach.

const PI: f32 = 3.14159265;

// ---------------------------------------------------------------------------------------
// G-buffer pass

struct Frame {
    eye: vec4<f32>,
    // x: 1 / width, y: 1 / height (the D3D9 half-pixel fix-up the translated shaders apply)
    fixup: vec4<f32>,
};

struct FrameDraw {
    mvp: mat4x4<f32>,
    world: mat4x4<f32>,
};

struct DrawData {
    // xy: lightmap atlas offset, zw: scale
    lightmap: vec4<f32>,
    // mml_vFalloffParams
    falloff: vec4<f32>,
    // x: room index (0: none)
    room: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> frame_draws: array<FrameDraw>;
@group(0) @binding(2) var<storage, read> draws: array<DrawData>;

@group(1) @binding(0) var diffuse_tex: texture_2d<f32>;
@group(1) @binding(1) var normal_tex: texture_2d<f32>;
@group(1) @binding(2) var spec_tex: texture_2d<f32>;
@group(1) @binding(3) var wrap_smp: sampler;
@group(1) @binding(4) var lightmap_tex: texture_2d<f32>;
@group(1) @binding(5) var clamp_smp: sampler;

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
    @location(3) world: vec3<f32>,
    @location(4) @interpolate(flat) slot: u32,
};

@vertex
fn vs_gbuffer(v: VsIn, @builtin(instance_index) slot: u32) -> VsOut {
    let f = frame_draws[slot];
    let d = draws[slot];
    var out: VsOut;
    // As Static1.vsh does it (`mul`, `mad`, `mad`, `add` by the matrix's columns), so the
    // depth matches what the original shaders write for the same surface.
    var r = f.mvp[1] * v.position.y;
    r = v.position.x * f.mvp[0] + r;
    r = v.position.z * f.mvp[2] + r;
    let p = r + f.mvp[3];
    out.clip = vec4<f32>(p.x + frame.fixup.x * p.w, p.y - frame.fixup.y * p.w, p.z, p.w);
    out.uv0 = v.uv0;
    out.uv_lm = v.uv1 * d.lightmap.zw + d.lightmap.xy;
    out.normal = (f.world * vec4<f32>(v.normal, 0.0)).xyz;
    out.world = (f.world * vec4<f32>(v.position, 1.0)).xyz;
    out.slot = slot;
    return out;
}

struct GBuffer {
    @location(0) albedo: vec4<f32>,
    @location(1) specular: vec4<f32>,
    @location(2) normal: vec4<f32>,
    @location(3) lightmap: vec4<f32>,
};

// Octahedral encoding of a unit vector into [-1, 1]².
fn oct_encode(n: vec3<f32>) -> vec2<f32> {
    let p = n.xy / (abs(n.x) + abs(n.y) + abs(n.z));
    if (n.z >= 0.0) {
        return p;
    }
    return (1.0 - abs(p.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), p >= vec2<f32>(0.0));
}

@fragment
fn fs_gbuffer(in: VsOut) -> GBuffer {
    let d = draws[in.slot];
    // Tangent frame from the screen-space derivatives of position and texture coordinate
    // (provisional: the meshes' own tangents and binormals, which Static1 reads, are not in
    // the stand-in vertex format).
    let n_v = in.normal;
    let dp1 = dpdx(in.world);
    let dp2 = dpdy(in.world);
    let duv1 = dpdx(in.uv0);
    let duv2 = dpdy(in.uv0);
    let dp2perp = cross(dp2, n_v);
    let dp1perp = cross(n_v, dp1);
    var t = dp2perp * duv1.x + dp1perp * duv2.x;
    var b = dp2perp * duv1.y + dp1perp * duv2.y;
    let scale = inverseSqrt(max(max(dot(t, t), dot(b, b)), 1e-20));
    t = t * scale;
    b = b * scale;

    let nm = textureSample(normal_tex, wrap_smp, in.uv0);
    let xy = nm.wy * 2.0 - 1.0;
    let z = sqrt(max(1.0 - dot(xy, xy), 0.0));
    var n = xy.x * t + xy.y * b + z * n_v;
    n = n * inverseSqrt(max(dot(n, n), 1e-20));

    // The specular falloff Static1.vsh works out per vertex from mml_vFalloffParams.
    let v = normalize(frame.eye.xyz - in.world);
    let x = (d.falloff.z - dot(n_v, v)) / d.falloff.z;
    let falloff = (3.0 - 2.0 * x) * x * x * d.falloff.w;

    var out: GBuffer;
    out.albedo = vec4<f32>(textureSample(diffuse_tex, wrap_smp, in.uv0).rgb, 1.0);
    out.specular = textureSample(spec_tex, wrap_smp, in.uv0);
    out.normal = vec4<f32>(oct_encode(n), d.room.x, falloff);
    out.lightmap = textureSample(lightmap_tex, clamp_smp, in.uv_lm);
    return out;
}

// ---------------------------------------------------------------------------------------
// Lighting pass

struct Lighting {
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // xyz: the direction the sun's light travels, w: 1 when there is a sun
    sun_dir: vec4<f32>,
    sun_colour: vec4<f32>,
    // mml_vFogParams: (colour, intensity²), (1 − cap, ...)
    fog0: vec4<f32>,
    fog1: vec4<f32>,
    // x: lights
    counts: vec4<u32>,
    // xy: target size
    size: vec4<f32>,
};

struct Light {
    // xyz position, w radius
    position: vec4<f32>,
    // rgb colour times intensity, w: 1 for a static light (masked by the lightmap's alpha)
    colour: vec4<f32>,
    // Bit per room index (256 rooms) the light reaches.
    rooms: array<vec4<u32>, 2>,
};

@group(0) @binding(0) var<uniform> lighting: Lighting;
@group(0) @binding(1) var<storage, read> lights: array<Light>;

@group(1) @binding(0) var g_albedo: texture_2d<f32>;
@group(1) @binding(1) var g_specular: texture_2d<f32>;
@group(1) @binding(2) var g_normal: texture_2d<f32>;
@group(1) @binding(3) var g_lightmap: texture_2d<f32>;
@group(1) @binding(4) var g_depth: texture_depth_2d;

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
}

fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    var n = vec3<f32>(e.x, e.y, 1.0 - abs(e.x) - abs(e.y));
    if (n.z < 0.0) {
        let s = select(vec2<f32>(-1.0), vec2<f32>(1.0), n.xy >= vec2<f32>(0.0));
        n = vec3<f32>((1.0 - abs(n.yx)) * s, n.z);
    }
    return normalize(n);
}

fn reaches(l: Light, room: u32) -> bool {
    if (room == 0u || room > 255u) {
        return true;
    }
    let word = room / 32u;
    let bits = l.rooms[word / 4u][word % 4u];
    return (bits & (1u << (room % 32u))) != 0u;
}

@fragment
fn fs_lighting(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(frag.xy);
    let depth = textureLoad(g_depth, px, 0);
    if (depth >= 1.0) {
        discard;
    }
    let ndc = vec2<f32>(frag.x / lighting.size.x * 2.0 - 1.0, 1.0 - frag.y / lighting.size.y * 2.0);
    let w = lighting.inv_view_proj * vec4<f32>(ndc, depth, 1.0);
    let p = w.xyz / w.w;

    let albedo = textureLoad(g_albedo, px, 0).rgb;
    let spec = textureLoad(g_specular, px, 0);
    let gn = textureLoad(g_normal, px, 0);
    let lm = textureLoad(g_lightmap, px, 0);
    let n = oct_decode(gn.xy);
    let room = u32(gn.z + 0.5);
    let falloff = gn.w;
    let power = spec.a * 255.0 + 1.0;
    let v = normalize(lighting.eye.xyz - p);

    // Static lights and the sun (the original's lights A-C and its directional light,
    // masked by the lightmap's alpha), and the others (its D and E).
    var diff_s = vec3<f32>(0.0);
    var spec_s = vec3<f32>(0.0);
    var diff_d = vec3<f32>(0.0);
    var spec_d = vec3<f32>(0.0);
    for (var i = 0u; i < lighting.counts.x; i = i + 1u) {
        let l = lights[i];
        if (!reaches(l, room)) {
            continue;
        }
        let to = l.position.xyz - p;
        let d = max(length(to), 0.001);
        // Static1's cos²(π/2 · d/R), which repeats past the radius: the original never
        // gives a light to a surface out of its reach, so here it ends there.
        if (d >= l.position.w) {
            continue;
        }
        let c = cos(d / l.position.w * PI * 0.5);
        let att = c * c;
        let ln = to / d;
        let ndl = dot(n, ln);
        if (ndl < 0.0) {
            continue;
        }
        let h = normalize(v + ln);
        let ndh = max(dot(n, h), 1e-6);
        let dif = att * ndl * l.colour.rgb;
        let sp = att * pow(ndh, power) * l.colour.rgb;
        if (l.colour.w > 0.5) {
            diff_s = diff_s + dif;
            spec_s = spec_s + sp;
        } else {
            diff_d = diff_d + dif;
            spec_d = spec_d + sp;
        }
    }
    if (lighting.sun_dir.w > 0.5) {
        let ls = -lighting.sun_dir.xyz;
        let ndl = dot(n, ls);
        if (ndl >= 0.0) {
            let h = normalize(v + ls);
            let ndh = max(dot(n, h), 1e-6);
            diff_s = diff_s + ndl * lighting.sun_colour.rgb;
            spec_s = spec_s + pow(ndh, power) * lighting.sun_colour.rgb;
        }
    }
    // No real-time shadow yet: Static1's shadow term is 1, so its lightmap blend is too.
    let diffuse = diff_s * lm.a + lm.rgb + diff_d;
    let specular = (spec_s * max(lm.a, 0.2) + spec_d) * 3.0;
    var colour = albedo * diffuse + spec.rgb * specular * falloff;

    // The level's fog, as Static1 applies it.
    let to_eye = p - lighting.eye.xyz;
    let f = max(min(exp(-dot(to_eye, to_eye) * lighting.fog0.w), 1.0), lighting.fog1.x);
    colour = f * (colour - lighting.fog0.rgb) + lighting.fog0.rgb;
    return vec4<f32>(colour, 1.0);
}
