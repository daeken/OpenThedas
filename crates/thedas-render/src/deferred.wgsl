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
    // x: the fog's intensity², y: 1 − its cap (mml_vFogParams[0].w, [1].x); z: 1 to leave
    // the normal maps out (the `flat` debug view)
    fog: vec4<f32>,
};

struct Draw {
    mvp: mat4x4<f32>,
    world: mat4x4<f32>,
    // xy: lightmap atlas offset, zw: scale
    lightmap: vec4<f32>,
    // mml_vFalloffParams
    falloff: vec4<f32>,
    // x: room index (0: none), y: mml_fSpecularReflectionMult
    room: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<uniform> draw: Draw;

@group(1) @binding(0) var diffuse_tex: texture_2d<f32>;
@group(1) @binding(1) var normal_tex: texture_2d<f32>;
@group(1) @binding(2) var spec_tex: texture_2d<f32>;
@group(1) @binding(3) var wrap_smp: sampler;
@group(1) @binding(4) var lightmap_tex: texture_2d<f32>;
@group(1) @binding(5) var clamp_smp: sampler;
// The draw's light probe's cubemap (Static1's `aur_cmLightProbe0`).
@group(1) @binding(6) var env_tex: texture_cube<f32>;

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
};

@vertex
fn vs_gbuffer(v: VsIn) -> VsOut {
    let f = draw;
    var out: VsOut;
    // As Static1.vsh does it (`mul`, `mad`, `mad`, `add` by the matrix's columns), so the
    // depth matches what the original shaders write for the same surface.
    var r = f.mvp[1] * v.position.y;
    r = v.position.x * f.mvp[0] + r;
    r = v.position.z * f.mvp[2] + r;
    let p = r + f.mvp[3];
    out.clip = vec4<f32>(p.x + frame.fixup.x * p.w, p.y - frame.fixup.y * p.w, p.z, p.w);
    out.uv0 = v.uv0;
    out.uv_lm = v.uv1 * f.lightmap.zw + f.lightmap.xy;
    out.normal = (f.world * vec4<f32>(v.normal, 0.0)).xyz;
    out.world = (f.world * vec4<f32>(v.position, 1.0)).xyz;
    return out;
}

struct GBuffer {
    @location(0) albedo: vec4<f32>,
    @location(1) specular: vec4<f32>,
    @location(2) normal: vec4<f32>,
    @location(3) lightmap: vec4<f32>,
};

// The surface's normal (normal-mapped, world space), view direction and specular falloff.
struct Surface {
    n: vec3<f32>,
    v: vec3<f32>,
    falloff: f32,
};

// Octahedral encoding of a unit vector into [-1, 1]².
fn oct_encode(n: vec3<f32>) -> vec2<f32> {
    let p = n.xy / (abs(n.x) + abs(n.y) + abs(n.z));
    if (n.z >= 0.0) {
        return p;
    }
    return (1.0 - abs(p.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), p >= vec2<f32>(0.0));
}

fn surface(in: VsOut) -> Surface {
    let d = draw;
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
    if (frame.fog.z > 0.5) {
        n = normalize(n_v);
    }

    // The specular falloff Static1.vsh works out per vertex from mml_vFalloffParams.
    let v = normalize(frame.eye.xyz - in.world);
    let x = (d.falloff.z - dot(n_v, v)) / d.falloff.z;
    var out: Surface;
    out.n = n;
    out.v = v;
    out.falloff = (3.0 - 2.0 * x) * x * x * d.falloff.w;
    return out;
}

@fragment
fn fs_gbuffer(in: VsOut) -> GBuffer {
    let s = surface(in);
    var out: GBuffer;
    out.albedo = vec4<f32>(textureSample(diffuse_tex, wrap_smp, in.uv0).rgb, 1.0);
    out.specular = textureSample(spec_tex, wrap_smp, in.uv0);
    out.normal = vec4<f32>(oct_encode(s.n), draw.room.x, s.falloff);
    out.lightmap = textureSample(lightmap_tex, clamp_smp, in.uv_lm);
    return out;
}

// Static1's reflection, which no light changes, added over the lit frame by a second draw of
// the same geometry (the G-buffer has no room left for it): the probe at R = 2(N·V)N − V, at
// a mip level of (1 − power / 255) · 9, times mml_fSpecularReflectionMult, the specular mask
// and the falloff.
@fragment
fn fs_env(in: VsOut) -> @location(0) vec4<f32> {
    let s = surface(in);
    let specular = textureSample(spec_tex, wrap_smp, in.uv0);
    let r = 2.0 * dot(s.n, s.v) * s.n - s.v;
    let power = specular.a * 255.0 + 1.0;
    let env = textureSampleLevel(env_tex, wrap_smp, r, (1.0 - power / 255.0) * 9.0).rgb;
    // Fogged as the rest of Static1's colour is: what is added comes through at the fog factor.
    let to_eye = in.world - frame.eye.xyz;
    let f = max(min(exp(-dot(to_eye, to_eye) * frame.fog.x), 1.0), frame.fog.y);
    return vec4<f32>(env * draw.room.y * specular.rgb * s.falloff * f, 1.0);
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
    // x: lights, y: the debug view (`THEDAS_DEFERRED_VIEW`, see deferred.rs)
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
    // x: its shadow map's slot (`deferred_shadow.rs`), −1 for none
    shadow: vec4<f32>,
};

@group(0) @binding(0) var<uniform> lighting: Lighting;
// As many as `MAX_LIGHTS` in deferred.rs: a uniform array, which every backend can read
// (16 KiB at most).
@group(0) @binding(1) var<uniform> lights: array<Light, 192>;
// Per shadow slot, its six faces' view-projections (`deferred_shadow::faces`).
@group(0) @binding(2) var<uniform> shadow_faces: array<mat4x4<f32>, 24>;

@group(1) @binding(0) var g_albedo: texture_2d<f32>;
@group(1) @binding(1) var g_specular: texture_2d<f32>;
@group(1) @binding(2) var g_normal: texture_2d<f32>;
@group(1) @binding(3) var g_lightmap: texture_2d<f32>;
// The depth buffer as plain numbers (the OpenGL backend cannot `textureLoad` a depth
// texture; see cut_effects.rs).
@group(1) @binding(4) var g_depth: texture_2d<f32>;
// The shadow slots' faces: distance from the light to the nearest caster.
@group(1) @binding(5) var shadow_maps: texture_2d_array<f32>;

// How much of a point light at `lp` (shadow slot `slot`) reaches `p` (normal `n`): its
// cube face by the major axis of the direction, a bilinear 2×2 comparison of distances.
fn shadowed(slot: i32, lp: vec3<f32>, p: vec3<f32>, n: vec3<f32>) -> f32 {
    let v = p + n * 0.04 - lp;
    let a = abs(v);
    var face = 0;
    if (a.x >= a.y && a.x >= a.z) {
        face = select(1, 0, v.x > 0.0);
    } else if (a.y >= a.z) {
        face = select(3, 2, v.y > 0.0);
    } else {
        face = select(5, 4, v.z > 0.0);
    }
    let layer = slot * 6 + face;
    let c = shadow_faces[layer] * vec4<f32>(lp + v, 1.0);
    let ndc = c.xy / c.w;
    let size = vec2<f32>(textureDimensions(shadow_maps));
    let at = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * size - 0.5;
    let base = vec2<i32>(floor(at));
    let f = at - floor(at);
    let dist = length(v) - 0.02;
    let hi = vec2<i32>(size) - vec2<i32>(1);
    var s: array<f32, 4>;
    for (var k = 0; k < 4; k = k + 1) {
        let t = clamp(base + vec2<i32>(k % 2, k / 2), vec2<i32>(0), hi);
        s[k] = select(0.0, 1.0, dist <= textureLoad(shadow_maps, t, layer, 0).x);
    }
    return mix(mix(s[0], s[1], f.x), mix(s[2], s[3], f.x), f.y);
}

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
    let depth = textureLoad(g_depth, px, 0).x;
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
        if (lighting.counts.y != 7u && !reaches(l, room)) {
            continue;
        }
        let to = l.position.xyz - p;
        let d = max(length(to), 0.001);
        // Static1's cos²(π/2 · d/R), which repeats past the radius (rising back to full at
        // 2R). The original gives a light to whole objects whose bounding sphere it reaches,
        // so on a large mesh that rise shows; here it ends at the radius (the `wrap` debug
        // view keeps it out to 2R).
        let wrap = lighting.counts.y == 9u;
        if (d >= l.position.w * select(1.0, 2.0, wrap)) {
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
        var shade = 1.0;
        if (l.shadow.x >= 0.0) {
            shade = shadowed(i32(l.shadow.x + 0.5), l.position.xyz, p, n);
            if (lighting.counts.y == 10u) {
                return vec4<f32>(shade, att, 0.0, 1.0);
            }
        }
        let dif = att * ndl * l.colour.rgb * shade;
        let sp = att * pow(ndh, power) * l.colour.rgb * shade;
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
    switch lighting.counts.y {
        case 1u: { return vec4<f32>(albedo, 1.0); }
        case 2u: { return vec4<f32>(n * 0.5 + 0.5, 1.0); }
        case 3u: { return vec4<f32>(lm.rgb, 1.0); }
        case 4u: { return vec4<f32>(vec3<f32>(lm.a), 1.0); }
        case 5u: { return vec4<f32>(diffuse * 0.5, 1.0); }
        case 6u: { return vec4<f32>(spec.rgb * specular * falloff, 1.0); }
        default: {}
    }

    // The level's fog, as Static1 applies it.
    let to_eye = p - lighting.eye.xyz;
    let f = max(min(exp(-dot(to_eye, to_eye) * lighting.fog0.w), 1.0), lighting.fog1.x);
    colour = f * (colour - lighting.fog0.rgb) + lighting.fog0.rgb;
    return vec4<f32>(colour, 1.0);
}
