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

struct Terrain {
    dimensions: vec4<f32>,
    parameters: vec4<f32>,
    scales: array<vec4<f32>, 2>,
    specular: array<vec4<f32>, 8>,
    tint: vec4<f32>,
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
    terrain: Terrain,
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
@group(1) @binding(7) var mask_v: texture_2d<f32>;
@group(1) @binding(8) var mask_a: texture_2d<f32>;
@group(1) @binding(9) var mask_a2: texture_2d<f32>;
@group(1) @binding(10) var point_smp: sampler;
@group(1) @binding(11) var terrain_diffuse: texture_2d<f32>;
@group(1) @binding(12) var terrain_normal: texture_2d<f32>;

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
    @location(4) terrain_falloff: f32,
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
    out.uv_lm = select(v.uv1, v.uv0, f.room.z > 0.5) * f.lightmap.zw + f.lightmap.xy;
    out.normal = (f.world * vec4<f32>(v.normal, 0.0)).xyz;
    out.world = (f.world * vec4<f32>(v.position, 1.0)).xyz;
    out.terrain_falloff = 2.0 - length(frame.eye.xyz - out.world) / 32.0;
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

fn surface_normal(in: VsOut, nm: vec4<f32>) -> Surface {
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

    let xy = nm.wy * 2.0 - 1.0;
    let z = sqrt(max(1.0 - dot(xy, xy), 0.0));
    var n = xy.x * t + xy.y * b + z * n_v;
    n = n * inverseSqrt(max(dot(n, n), 1e-20));
    if (frame.fog.z > 0.5 || (draw.room.z > 0.5 && draw.room.w > 0.5)) {
        n = normalize(n_v);
    }

    // The specular falloff Static1.vsh works out per vertex from mml_vFalloffParams.
    let v = normalize(frame.eye.xyz - in.world);
    let x = (d.falloff.z - dot(n_v, v)) / d.falloff.z;
    var out: Surface;
    out.n = n;
    out.v = v;
    out.falloff = (3.0 - 2.0 * x) * x * x * d.falloff.w;
    if (draw.room.z > 0.5) {
        // Terrain2.vsh's specular distance factor, interpolated per vertex.
        out.falloff = in.terrain_falloff;
    }
    return out;
}

fn surface(in: VsOut) -> Surface {
    return surface_normal(in, textureSample(normal_tex, wrap_smp, in.uv0));
}

struct TerrainMaterial {
    albedo: vec4<f32>,
    normal: vec4<f32>,
    specular: vec4<f32>,
};

// MaskV picks three of eight layers, encoded at 0, 2/15, ..., 14/15.
// MaskA/MaskA2 hold all eight weights. The palette's tile padding and each
// layer's UV scale come from the material, never a fixed world-space tiling.
fn terrain_material(uv: vec2<f32>) -> TerrainMaterial {
    let t = draw.terrain;
    let indices = textureSampleLevel(mask_v, point_smp, uv, 0.0).rgb;
    let a = textureSample(mask_a, clamp_smp, uv);
    let b = textureSample(mask_a2, clamp_smp, uv);
    let weights = array<f32, 8>(a.x, a.y, a.z, a.w, b.x, b.y, b.z, b.w);
    let dx = dpdx(uv) * t.parameters.zw;
    let dy = dpdy(uv) * t.parameters.zw;
    var result: TerrainMaterial;
    result.albedo = vec4<f32>(0.0);
    result.normal = vec4<f32>(0.0);
    result.specular = vec4<f32>(0.0);
    var total = 0.0;
    for (var layer = 0u; layer < 8u; layer += 1u) {
        result.specular += weights[layer] * t.specular[layer];
    }
    for (var k = 0u; k < 3u; k += 1u) {
        let encoded = indices[k];
        var weight = 0.0;
        var scale = 0.0;
        for (var layer = 0u; layer < 8u; layer += 1u) {
            let selected = abs(encoded - f32(layer) * (2.0 / 15.0)) <= 0.0625;
            weight += select(0.0, weights[layer], selected);
            scale += select(0.0, t.scales[layer / 4u][layer % 4u], selected);
        }
        let column = floor(encoded * t.dimensions.z);
        let row = floor(fract(encoded * t.dimensions.z) * t.dimensions.w);
        let tile = vec2<f32>(column, row) * t.dimensions.xy + t.parameters.xy;
        let coord = fract(uv * scale) * t.parameters.zw + tile;
        result.albedo += weight * textureSampleGrad(terrain_diffuse, clamp_smp, coord, dx * scale, dy * scale);
        if (draw.room.w < 0.5) {
            result.normal += weight * textureSampleGrad(terrain_normal, clamp_smp, coord, dx * scale, dy * scale);
        }
        total += weight;
    }
    result.albedo /= max(total, 1e-6);
    result.normal /= max(total, 1e-6);
    result.specular = vec4<f32>(result.specular.rgb * result.albedo.a, result.specular.a);
    if (draw.room.w > 0.5) {
        result.specular = vec4<f32>(0.0);
    }
    return result;
}

@fragment
fn fs_gbuffer(in: VsOut) -> GBuffer {
    if (draw.room.z > 0.5) {
        let m = terrain_material(in.uv0);
        let s = surface_normal(in, m.normal);
        var out: GBuffer;
        out.albedo = vec4<f32>(m.albedo.rgb, select(1.0, 0.5, draw.room.w > 0.5));
        out.specular = m.specular;
        out.normal = vec4<f32>(oct_encode(s.n), draw.room.x, s.falloff);
        out.lightmap = textureSample(lightmap_tex, clamp_smp, in.uv_lm);
        return out;
    }
    let s = surface(in);
    var out: GBuffer;
    out.albedo = vec4<f32>(textureSample(diffuse_tex, wrap_smp, in.uv0).rgb, 0.0);
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
    if (draw.room.z > 0.5) {
        let m = terrain_material(in.uv0);
        let s = surface_normal(in, m.normal);
        let r = 2.0 * dot(s.n, s.v) * s.n - s.v;
        let power = m.specular.a * 255.0 + 1.0;
        let env = textureSampleLevel(env_tex, wrap_smp, r, (1.0 - power / 255.0) * 9.0).rgb;
        let lm = textureSample(lightmap_tex, clamp_smp, in.uv_lm);
        let to_eye = in.world - frame.eye.xyz;
        let fog = max(min(exp(-dot(to_eye, to_eye) * frame.fog.x), 1.0), frame.fog.y);
        let shade = sun_visibility(in.world, s.n, draw.room.w < 0.5);
        let reflection = env * m.specular.rgb * (0.75 * shade * lm.a + 0.25) * s.falloff;
        return vec4<f32>((reflection + m.albedo.rgb * draw.terrain.tint.rgb) * fog, 1.0);
    }
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
struct Sun {
    data: array<vec4<f32>, 9>,
    direction: vec4<f32>,
    enabled: vec4<u32>,
};
@group(2) @binding(0) var<uniform> sun: Sun;
@group(2) @binding(1) var sun_near: texture_2d<f32>;
@group(2) @binding(2) var sun_far: texture_2d<f32>;
@group(2) @binding(3) var sun_sampler: sampler;

fn sun_sample(uv: vec2<f32>, near: bool) -> vec4<f32> {
    // The original uses a white border. Keep off-map taps lit on backends without
    // clamp-to-border samplers (including GL).
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return vec4<f32>(1.0);
    }
    if (near) {
        return textureSampleLevel(sun_near, sun_sampler, uv, 0.0);
    }
    return textureSampleLevel(sun_far, sun_sampler, uv, 0.0);
}

// Static1's original five-tap near filter (red depth, blurred green coverage),
// four-tap far PCF, distance fade and grazing-angle floor. The faithful pass owns
// both maps, their caster selection, quality, fitting and temporal smoothing.
fn sun_visibility(p: vec3<f32>, n: vec3<f32>, grazing_floor: bool) -> f32 {
    if (sun.enabled.x == 0u || sun.direction.w < 0.5) {
        return 1.0;
    }
    let q = vec4<f32>(p, 1.0);
    let sd = sun.data;
    let plane = dot(sd[8], q);
    let offsets = array<vec2<f32>, 4>(
        vec2<f32>(-0.00141302, -0.000598593),
        vec2<f32>(0.00141838, -0.00115336),
        vec2<f32>(-0.000141276, -0.00139408),
        vec2<f32>(0.000517439, 0.000440816));
    var shade = 1.0;
    if (plane > 0.0) {
        let t = vec3<f32>(dot(sd[0], q), dot(sd[1], q), dot(sd[2], q)) / dot(sd[3], q);
        if (all(t.xy >= vec2<f32>(0.0)) && all(t.xy <= vec2<f32>(1.0))) {
            let centre = sun_sample(t.xy, true);
            var green = centre.y;
            for (var k = 0u; k < 4u; k = k + 1u) {
                green += sun_sample(t.xy + offsets[k], true).y;
            }
            shade = select(green * 0.2, 1.0, t.z <= centre.x);
        }
    } else {
        // sd[7].w holds near reach minus far reach, not a matrix element.
        let t = vec3<f32>(dot(sd[4], q), dot(sd[5], q), dot(sd[6], q))
            / dot(vec4<f32>(sd[7].xyz, 1.0), q);
        if (all(t.xy >= vec2<f32>(0.0)) && all(t.xy <= vec2<f32>(1.0))) {
            var lit = select(0.0, 1.0, t.z <= sun_sample(t.xy, false).x);
            // The original uses the centre and the first three offsets here.
            for (var k = 0u; k < 3u; k = k + 1u) {
                lit += select(0.0, 1.0, t.z <= sun_sample(t.xy + offsets[k], false).x);
            }
            let fade = clamp((sd[7].w - plane) / (0.25 * sd[7].w), 0.0, 1.0);
            shade = mix(1.0, lit * 0.25, fade);
        }
    }
    return clamp(select(shade, max(shade, 1.0 - 10.0 * dot(n, -sun.direction.xyz)), grazing_floor), 0.0, 1.0);
}

// How much of a point light at `lp` (shadow slot `slot`) reaches `p` (normal `n`): its
// cube face by the major axis of the direction, a bilinear 2×2 comparison of distances.
fn shadowed(slot: i32, lp: vec3<f32>, p: vec3<f32>, n: vec3<f32>) -> f32 {
    let v = p + n * 0.04 - lp;
    let a = abs(v);
    // Inside the cube's near planes there is no caster. In particular, do not divide
    // by a zero projection w when a receiver coincides with the light.
    if (max(max(a.x, a.y), a.z) < 0.35) {
        return 1.0;
    }
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
    // Undo the D3D9 half-pixel fix-up used by vs_gbuffer before reconstructing
    // world position, so shadow comparisons refer to the actual surface.
    let ndc = vec2<f32>((frag.x - 0.5) / lighting.size.x * 2.0 - 1.0, 1.0 - (frag.y - 0.5) / lighting.size.y * 2.0);
    let w = lighting.inv_view_proj * vec4<f32>(ndc, depth, 1.0);
    let p = w.xyz / w.w;

    let ga = textureLoad(g_albedo, px, 0);
    let albedo = ga.rgb;
    let terrain = ga.a > 0.25;
    let terrain_low = terrain && ga.a < 0.75;
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
    let sun_shade = sun_visibility(p, n, !terrain_low);
    // Static1: alpha masks baked direct visibility once; RGB is baked bounce,
    // dimmed by only 15% in a runtime shadow (exploration camera w = 0).
    // Dynamic lights have their own visibility and never take this sun mask.
    // Terrain1 masks static diffuse twice; Static1 and Low's Terrain3 only once.
    let diffuse = diff_s * sun_shade * select(lm.a, lm.a * lm.a, terrain && !terrain_low)
        + lm.rgb * mix(1.0, sun_shade, 0.15) + diff_d;
    let specular = select((spec_s * sun_shade * max(lm.a, 0.2) + spec_d) * 3.0,
        (spec_s * (0.75 * sun_shade * lm.a + 0.25) + spec_d) * 3.0, terrain);
    var colour = albedo * diffuse + spec.rgb * specular * falloff;
    switch lighting.counts.y {
        case 1u: { return vec4<f32>(albedo, 1.0); }
        case 2u: { return vec4<f32>(n * 0.5 + 0.5, 1.0); }
        case 3u: { return vec4<f32>(lm.rgb, 1.0); }
        case 4u: { return vec4<f32>(vec3<f32>(lm.a), 1.0); }
        case 5u: { return vec4<f32>(diffuse * 0.5, 1.0); }
        case 6u: { return vec4<f32>(spec.rgb * specular * falloff, 1.0); }
        case 11u: { return vec4<f32>(vec3<f32>(sun_shade), 1.0); }
        default: {}
    }

    // The level's fog, as Static1 applies it.
    let to_eye = p - lighting.eye.xyz;
    let f = max(min(exp(-dot(to_eye, to_eye) * lighting.fog0.w), 1.0), lighting.fog1.x);
    colour = f * (colour - lighting.fog0.rgb) + lighting.fog0.rgb;
    return vec4<f32>(colour, 1.0);
}
