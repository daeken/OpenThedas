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
    // the normal maps out (the `flat` debug view); w: 1 to replace lightmaps with probes
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
    probe: array<vec4<f32>, 12>,
    tint: array<vec4<f32>, 10>,
    material: vec4<f32>,
    tint_color: vec4<f32>,
    character: Character,
};

struct Character {
    age: array<vec4<f32>, 6>,
    emotion: array<vec4<f32>, 2>,
    hair_weights: array<vec4<f32>, 2>,
    hair_colour: vec4<f32>,
    tattoo: array<vec4<f32>, 5>,
    specular: array<vec4<f32>, 3>,
    ambient: vec4<f32>,
    eye: vec4<f32>,
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
@group(1) @binding(13) var tint_tex: texture_2d<f32>;
@group(1) @binding(14) var emissive_tex: texture_2d<f32>;
@group(1) @binding(15) var face_age_tex: texture_2d<f32>;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv0: vec2<f32>,
    @location(3) uv1: vec2<f32>,
    @location(4) weights: vec4<f32>,
    @location(5) indices: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv0: vec2<f32>,
    @location(1) uv_lm: vec2<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) world: vec3<f32>,
    @location(4) terrain_falloff: f32,
    @location(5) irradiance: vec3<f32>,
};

@vertex
fn vs_gbuffer(v: VsIn) -> VsOut {
    let f = draw;
    let position = skin_vector(vec4<f32>(v.position, 1.0), v.weights, v.indices);
    let normal = skin_vector(vec4<f32>(v.normal, 0.0), v.weights, v.indices);
    var out: VsOut;
    // As Static1.vsh does it (`mul`, `mad`, `mad`, `add` by the matrix's columns), so the
    // depth matches what the original shaders write for the same surface.
    var r = f.mvp[1] * position.y;
    r = position.x * f.mvp[0] + r;
    r = position.z * f.mvp[2] + r;
    let p = r + f.mvp[3];
    out.clip = vec4<f32>(p.x + frame.fixup.x * p.w, p.y - frame.fixup.y * p.w, p.z, p.w);
    out.uv0 = v.uv0;
    out.uv_lm = select(v.uv1, v.uv0, f.room.z > 0.5) * f.lightmap.zw + f.lightmap.xy;
    out.normal = (f.world * vec4<f32>(normal, 0.0)).xyz;
    out.world = (f.world * vec4<f32>(position, 1.0)).xyz;
    out.terrain_falloff = 2.0 - length(frame.eye.xyz - out.world) / 32.0;
    out.irradiance = vec3<f32>(0.0);
    if (draw.material.x > 0.5) {
        // Character/prop shaders interpolate probe light from posed vertex normals;
        // normal maps change direct lighting, not the probe's coarse baked lobes.
        out.irradiance = object_probe(normalize(out.normal));
    }
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

// n^T M n, using rows rather than WGSL's column-major matrix constructors.
fn probe_irradiance(normal: vec3<f32>) -> vec3<f32> {
    let n = vec4<f32>(normal, 1.0);
    var result: vec3<f32>;
    for (var c = 0u; c < 3u; c += 1u) {
        let i = c * 4u;
        let mn = vec4<f32>(dot(draw.probe[i], n), dot(draw.probe[i + 1u], n),
            dot(draw.probe[i + 2u], n), dot(draw.probe[i + 3u], n));
        result[c] = max(dot(n, mn), 0.0);
    }
    return result;
}

fn surface(in: VsOut) -> Surface {
    return surface_normal(in, textureSample(normal_tex, wrap_smp, in.uv0));
}

fn object_probe(n: vec3<f32>) -> vec3<f32> {
    let v = vec4<f32>(n, 1.0);
    var result: vec3<f32>;
    for (var c = 0u; c < 3u; c += 1u) {
        let row = c * 4u;
        result[c] = max(dot(v, vec4<f32>(dot(draw.probe[row], v), dot(draw.probe[row + 1u], v),
            dot(draw.probe[row + 2u], v), dot(draw.probe[row + 3u], v))), 0.0);
    }
    return result;
}

// Lerped tint semantics: independent diffuse/specular opacities, interleaved colours.
fn tint_mode(base: vec3<f32>, mask: vec4<f32>, specular: bool, mode: f32) -> vec3<f32> {
    let channel = select(0u, 1u, specular);
    let weights = mask * draw.tint[channel];
    if (mode > 1.5 && !specular) {
        let skin_colour = base * mix(vec3<f32>(1.0), draw.tint[8].rgb, weights.w);
        let colour = draw.tint[2].rgb * weights.x + draw.tint[4].rgb * weights.y + draw.tint[6].rgb * weights.z;
        let opacity = weights.x + weights.y + weights.z;
        if (mode > 2.5) {
            return mix(skin_colour, colour, opacity);
        }
        return skin_colour * mix(vec3<f32>(1.0), colour, opacity);
    }
    var colour = vec3<f32>(0.0);
    let channels = select(4u, 3u, mode > 2.5);
    for (var k = 0u; k < channels; k += 1u) {
        colour += draw.tint[2u + k * 2u + channel].rgb * weights[k];
    }
    let opacity = weights.x + weights.y + weights.z + select(weights.w, 0.0, mode > 2.5);
    return mix(base, colour, opacity);
}

fn object_tint(base: vec3<f32>, mask: vec4<f32>, specular: bool) -> vec3<f32> {
    return tint_mode(base, mask, specular, draw.material.z);
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

// Face textures reuse the terrain-only and ordinary specular/emissive slots. This keeps
// the material plus sun bindings inside the portable sixteen-texture fragment limit.
fn face_material(uv: vec2<f32>) -> TerrainMaterial {
    let emo0 = textureSample(mask_a, clamp_smp, uv);
    let emo1 = textureSample(mask_a2, clamp_smp, uv);
    let base = textureSample(diffuse_tex, wrap_smp, uv);
    let aged = textureSample(face_age_tex, wrap_smp, uv);
    let tint = textureSample(tint_tex, wrap_smp, uv);
    let normal = textureSample(normal_tex, wrap_smp, uv);
    let age_normal = textureSample(spec_tex, wrap_smp, uv);
    let emotion_normal = textureSample(lightmap_tex, clamp_smp, uv);
    let hair = textureSample(emissive_tex, wrap_smp, uv);
    let hair_normal = textureSample(terrain_diffuse, clamp_smp, uv);
    let tattoo = textureSample(terrain_normal, clamp_smp, uv);
    return compose_face(base, aged, normal, age_normal, emotion_normal, emo0, emo1,
        tint, hair, hair_normal, tattoo);
}

// Texture-independent composition is also exercised by the asset-free GPU fixture.
fn compose_face(base: vec4<f32>, aged: vec4<f32>, normal: vec4<f32>, age_normal: vec4<f32>,
    emotion_normal: vec4<f32>, emo0: vec4<f32>, emo1: vec4<f32>, tint: vec4<f32>,
    hair: vec4<f32>, hair_normal: vec4<f32>, tattoo: vec4<f32>) -> TerrainMaterial {
    let c = draw.character;
    let colour_age = min(dot(emo0, c.age[0]) + dot(emo1, c.age[1]) + c.age[2].w, 1.0);
    let normal_age = min(dot(emo0, c.age[3]) + dot(emo1, c.age[4]) + c.age[5].w, 1.0);
    let expression = dot(emo0, c.emotion[0]) + dot(emo1, c.emotion[1]);
    let hair_coverage = clamp(mix(dot(hair, c.hair_weights[1]), dot(hair, c.hair_weights[0]),
        clamp(emo1.r + emo1.g + emo1.b, 0.0, 1.0)), 0.0, 1.0);
    let xy = mix(normal.wy, age_normal.wy, normal_age)
        + (emotion_normal.wy - vec2<f32>(0.5)) * expression
        + (hair_normal.wy - vec2<f32>(0.5)) * hair_coverage;
    var colour = mix(base, aged, colour_age);
    colour = vec4<f32>(colour.rgb * mix(vec3<f32>(1.0), draw.tint[8].rgb, tint.a), colour.a);
    let tattoo_weights = tattoo * c.tattoo[0];
    let tattoo_colour = tattoo_weights.x * c.tattoo[1].rgb + tattoo_weights.y * c.tattoo[2].rgb
        + tattoo_weights.z * c.tattoo[3].rgb + tattoo_weights.w * c.tattoo[4].rgb;
    colour = vec4<f32>(colour.rgb * mix(vec3<f32>(1.0), tattoo_colour,
        clamp(dot(tattoo_weights, vec4<f32>(1.0)), 0.0, 1.0)), colour.a);
    colour = vec4<f32>(mix(colour.rgb, c.hair_colour.rgb, hair_coverage), colour.a);
    let weights = tint.rgb * draw.tint[0].rgb;
    let tint_colour = weights.x * draw.tint[2].rgb + weights.y * draw.tint[4].rgb + weights.z * draw.tint[6].rgb;
    colour = vec4<f32>(mix(colour.rgb, tint_colour, clamp(dot(weights, vec3<f32>(1.0)), 0.0, 1.0)), colour.a);
    let unmasked = 1.0 - clamp(dot(emo0 + emo1, vec4<f32>(1.0)) + tint.r, 0.0, 1.0);
    let roughness = clamp(dot(emo0, c.specular[0]) + dot(emo1, c.specular[1])
        + tint.r * c.specular[2].x + unmasked * c.specular[2].y, 0.02, 1.0);
    // A single Blinn lobe approximates the skin BRDF lookup; authored regional
    // roughness and diffuse alpha still control its power and amount.
    let power = clamp(2.0 / (roughness * roughness) - 2.0, 1.0, 256.0);
    // A modest dielectric reflectance keeps this unnormalised Blinn approximation
    // from turning the face's detail into bright metallic-looking highlights.
    let amount = 0.04 * max(colour.a * (1.0 - hair_coverage), 0.0);
    let spec_weights = tint.rgb * draw.tint[1].rgb;
    let spec_colour = spec_weights.x * draw.tint[3].rgb + spec_weights.y * draw.tint[5].rgb
        + spec_weights.z * draw.tint[7].rgb;
    let skin_spec = vec3<f32>(amount) * mix(vec3<f32>(1.0), draw.tint[9].rgb, tint.a);
    var out: TerrainMaterial;
    out.albedo = colour;
    out.normal = vec4<f32>(0.5, xy.y, 0.5, xy.x);
    out.specular = vec4<f32>(mix(skin_spec, spec_colour,
        clamp(dot(spec_weights, vec3<f32>(1.0)), 0.0, 1.0)), (power - 1.0) / 255.0);
    return out;
}

fn eye_material(uv: vec2<f32>) -> TerrainMaterial {
    let base = textureSample(diffuse_tex, wrap_smp, uv);
    let tint = textureSample(tint_tex, wrap_smp, uv);
    let normal = textureSample(normal_tex, wrap_smp, uv);
    let cornea = textureSample(spec_tex, wrap_smp, uv);
    return compose_eye(base, tint, normal, cornea);
}

fn compose_eye(base: vec4<f32>, tint: vec4<f32>, normal: vec4<f32>, cornea: vec4<f32>) -> TerrainMaterial {
    let weights = tint.rgb * draw.tint[0].rgb;
    let colour = weights.x * draw.tint[2].rgb + weights.y * draw.tint[4].rgb + weights.z * draw.tint[6].rgb;
    let e = draw.character.eye;
    let cornea_amount = base.a * e.y;
    let sclera_amount = (1.0 - base.a) * e.w;
    let total = max(cornea_amount + sclera_amount, 0.0);
    let blend = clamp(cornea_amount / max(total, 1e-6), 0.0, 1.0);
    var out: TerrainMaterial;
    out.albedo = vec4<f32>(mix(base.rgb, colour, dot(weights, vec3<f32>(1.0))), 1.0);
    // The G-buffer stores one lobe: preserve each authored normal and power by its
    // corresponding mask, approximating their separate forward highlights.
    out.normal = mix(normal, cornea, blend);
    out.specular = vec4<f32>(vec3<f32>(total), (clamp(mix(e.z, e.x, blend), 1.0, 256.0) - 1.0) / 255.0);
    return out;
}

@fragment
fn fs_gbuffer(in: VsOut) -> GBuffer {
    if (draw.material.w > 0.5) {
        var m: TerrainMaterial;
        if (draw.material.w < 1.5) {
            m = face_material(in.uv0);
        } else {
            m = eye_material(in.uv0);
        }
        let s = surface_normal(in, m.normal);
        var out: GBuffer;
        out.albedo = vec4<f32>(mix(m.albedo.rgb, draw.tint_color.rgb, draw.tint_color.a), 192.0 / 255.0);
        out.specular = m.specular;
        out.normal = vec4<f32>(oct_encode(s.n), draw.room.x, 1.0);
        let ambient = select(vec3<f32>(1.0), draw.character.ambient.rgb, draw.material.w < 1.5);
        out.lightmap = vec4<f32>(in.irradiance * ambient, 1.0);
        return out;
    }
    if (draw.room.z > 0.5) {
        let m = terrain_material(in.uv0);
        let s = surface_normal(in, m.normal);
        var out: GBuffer;
        out.albedo = vec4<f32>(m.albedo.rgb, select(1.0, 0.5, draw.room.w > 0.5));
        out.specular = m.specular;
        out.normal = vec4<f32>(oct_encode(s.n), draw.room.x, s.falloff);
        out.lightmap = textureSample(lightmap_tex, clamp_smp, in.uv_lm);
        if (frame.fog.w > 0.5) {
            out.lightmap = vec4<f32>(probe_irradiance(s.n), 1.0);
        }
        return out;
    }
    let s = surface(in);
    var out: GBuffer;
    out.albedo = vec4<f32>(textureSample(diffuse_tex, wrap_smp, in.uv0).rgb, 0.0);
    out.specular = textureSample(spec_tex, wrap_smp, in.uv0);
    out.normal = vec4<f32>(oct_encode(s.n), draw.room.x, s.falloff);
    out.lightmap = textureSample(lightmap_tex, clamp_smp, in.uv_lm);
    if (draw.material.x > 0.5) {
        out.albedo.a = select(64.0 / 255.0, 192.0 / 255.0, draw.material.x > 1.5);
        out.lightmap = vec4<f32>(in.irradiance, 1.0);
        if (draw.material.z > 0.5) {
            let mask = textureSample(tint_tex, wrap_smp, in.uv0);
            out.albedo = vec4<f32>(object_tint(out.albedo.rgb, mask, false), out.albedo.a);
            out.specular = vec4<f32>(object_tint(out.specular.rgb, mask, true), out.specular.a);
        }
        out.albedo = vec4<f32>(mix(out.albedo.rgb, draw.tint_color.rgb, draw.tint_color.a), out.albedo.a);
    } else if (frame.fog.w > 0.5) {
        out.lightmap = vec4<f32>(probe_irradiance(s.n), 1.0);
    }
    return out;
}

// Static1's reflection, which no light changes, added over the lit frame by a second draw of
// the same geometry (the G-buffer has no room left for it): the probe at R = 2(N·V)N − V, at
// a mip level of (1 − power / 255) · 9, times mml_fSpecularReflectionMult, the specular mask
// and the falloff.
@fragment
fn fs_env(in: VsOut) -> @location(0) vec4<f32> {
    if (draw.material.w > 0.5) {
        return vec4<f32>(0.0);
    }
    if (draw.room.z > 0.5) {
        let m = terrain_material(in.uv0);
        let s = surface_normal(in, m.normal);
        let r = 2.0 * dot(s.n, s.v) * s.n - s.v;
        let power = m.specular.a * 255.0 + 1.0;
        let env = textureSampleLevel(env_tex, wrap_smp, r, (1.0 - power / 255.0) * 9.0).rgb;
        let baked_visibility = select(textureSample(lightmap_tex, clamp_smp, in.uv_lm).a, 1.0, frame.fog.w > 0.5);
        let to_eye = in.world - frame.eye.xyz;
        let fog = max(min(exp(-dot(to_eye, to_eye) * frame.fog.x), 1.0), frame.fog.y);
        let shade = sun_visibility(in.world, s.n, draw.room.w < 0.5);
        let reflection = env * m.specular.rgb * (0.75 * shade * baked_visibility + 0.25) * s.falloff;
        return vec4<f32>((reflection + m.albedo.rgb * draw.terrain.tint.rgb) * fog, 1.0);
    }
    let s = surface(in);
    var specular = textureSample(spec_tex, wrap_smp, in.uv0);
    if (draw.material.x > 0.5 && draw.material.z > 0.5) {
        specular = vec4<f32>(object_tint(specular.rgb, textureSample(tint_tex, wrap_smp, in.uv0), true), specular.a);
    }
    let r = 2.0 * dot(s.n, s.v) * s.n - s.v;
    let power = specular.a * 255.0 + 1.0;
    let env = textureSampleLevel(env_tex, wrap_smp, r, (1.0 - power / 255.0) * 9.0).rgb;
    // Fogged as the rest of Static1's colour is: what is added comes through at the fog factor.
    let to_eye = in.world - frame.eye.xyz;
    let f = max(min(exp(-dot(to_eye, to_eye) * frame.fog.x), 1.0), frame.fog.y);
    let emissive = textureSample(emissive_tex, wrap_smp, in.uv0).rgb * draw.material.y;
    return vec4<f32>((env * draw.room.y * specular.rgb * s.falloff + emissive) * f, 1.0);
}

// ---------------------------------------------------------------------------------------
// Lighting pass

struct Lighting {
    inv_view_proj: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // xyz: the direction the sun's light travels, w: 1 when there is a sun
    sun_dir: vec4<f32>,
    sun_colour: vec4<f32>,
    // mml_vFogParams: (colour, intensity²), (1 − cap, ...)
    fog0: vec4<f32>,
    fog1: vec4<f32>,
    // x: lights, y: debug view, z: sun shadows, w: 0 baked, 1 probes, 2 probes + SSAO
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
// 16 x 9 screen tiles, two vec4s each: six ascending 32-bit light masks plus padding.
@group(0) @binding(3) var<uniform> tile_lights: array<vec4<u32>, 288>;

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

fn point_uses_baked_mask(is_static: bool, material: u32) -> bool {
    // Props and characters have probes, not baked direct-light visibility.
    return is_static && material != 64u && material != 192u;
}

fn world_at(pixel: vec2<i32>, depth: f32) -> vec3<f32> {
    // Pixel centres minus the half-pixel fix-up are integer pixel coordinates.
    let ndc = vec2<f32>(f32(pixel.x) / lighting.size.x * 2.0 - 1.0,
        1.0 - f32(pixel.y) / lighting.size.y * 2.0);
    let w = lighting.inv_view_proj * vec4<f32>(ndc, depth, 1.0);
    return w.xyz / w.w;
}

// A deterministic hemisphere kernel, in world metres. Only the probe's diffuse term
// takes this visibility; direct lights keep their shadow maps and reflections stay sharp.
// No noise texture or frame-dependent rotation, to avoid temporal sparkle before TAA.
fn ambient_visibility(p: vec3<f32>, n: vec3<f32>) -> f32 {
    if (lighting.counts.w != 2u) {
        return 1.0;
    }
    let radius = 0.8;
    let axis = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), abs(n.z) > 0.95);
    let t = normalize(cross(axis, n));
    let b = cross(n, t);
    var blocked = 0.0;
    for (var i = 0u; i < 12u; i += 1u) {
        let f = (f32(i) + 0.5) / 12.0;
        let angle = f32(i) * 2.39996323;
        let z = 0.15 + 0.85 * f;
        let xy = sqrt(1.0 - z * z);
        let direction = t * (cos(angle) * xy) + b * (sin(angle) * xy) + n * z;
        let sample = p + n * 0.03 + direction * radius * (0.2 + 0.8 * f);
        let clip = lighting.view_proj * vec4<f32>(sample, 1.0);
        if (clip.w <= 0.0 || clip.z < 0.0 || clip.z >= clip.w) {
            continue;
        }
        let ndc = clip.xy / clip.w;
        // Project back to the same pixel centres used by the G-buffer vertex shader.
        let pixel = vec2<i32>(floor(vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * lighting.size.xy + 0.5));
        if (any(pixel < vec2<i32>(0)) || any(pixel >= vec2<i32>(lighting.size.xy))) {
            continue;
        }
        let depth = textureLoad(g_depth, pixel, 0).x;
        if (depth >= 1.0) {
            continue;
        }
        let hit = world_at(pixel, depth);
        // Reject distant foreground silhouettes, and give a flat surface a metric bias.
        let separation = length(hit - p);
        let closer = length(lighting.eye.xyz - sample) - length(lighting.eye.xyz - hit);
        if (closer > 0.04 && dot(hit - p, n) > 0.025) {
            blocked += 1.0 - smoothstep(radius * 0.5, radius, separation);
        }
    }
    // Missing/offscreen samples are open. Cap darkening until temporal filtering exists.
    return clamp(1.0 - blocked / 12.0, 0.25, 1.0);
}

@fragment
fn fs_lighting(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(frag.xy);
    let depth = textureLoad(g_depth, px, 0).x;
    let p = world_at(px, depth);
    // Depth derivatives describe geometry, rather than normal-map bumps that would
    // falsely occlude a flat wall. Evaluate before the background discard so helper
    // invocations remain available at silhouettes.
    let geometric = cross(dpdx(p), dpdy(p));
    if (depth >= 1.0) {
        discard;
    }

    let ga = textureLoad(g_albedo, px, 0);
    let albedo = ga.rgb;
    let material = u32(round(ga.a * 255.0));
    let terrain = material == 128u || material == 255u;
    let terrain_low = material == 128u;
    let character = material == 192u;
    let object = material == 64u || character;
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
    let tile_x = min(u32(frag.x * 16.0 / lighting.size.x), 15u);
    let tile_y = min(u32(frag.y * 9.0 / lighting.size.y), 8u);
    let tile_base = (tile_y * 16u + tile_x) * 2u;
    for (var word = 0u; word < 6u; word = word + 1u) {
      var bits = tile_lights[tile_base + word / 4u][word % 4u];
      while (bits != 0u) {
        let i = word * 32u + firstTrailingBit(bits);
        bits = bits & (bits - 1u);
        if (i >= lighting.counts.x) { continue; }
        let l = lights[i];
        if ((character && l.shadow.y < 1.5) || (!character && l.shadow.y > 1.5 && l.shadow.y < 2.5)) {
            continue;
        }
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
        // Objects have no baked direct visibility. Their point lights use their own
        // cube visibility, never the sun's mask or Static1's grazing-angle floor.
        if (point_uses_baked_mask(l.colour.w > 0.5, material)) {
            diff_s = diff_s + dif;
            spec_s = spec_s + sp;
        } else {
            diff_d = diff_d + dif;
            spec_d = spec_d + sp;
        }
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
    let sun_shade = sun_visibility(p, n, !terrain_low && !object);
    let plane_n = geometric * inverseSqrt(max(dot(geometric, geometric), 1e-20));
    let oriented_n = select(-plane_n, plane_n, dot(plane_n, n) >= 0.0);
    let ao = ambient_visibility(p, select(n, oriented_n, dot(geometric, geometric) > 1e-20));
    // Static1: alpha masks baked direct visibility once; RGB is baked bounce,
    // dimmed by only 15% in a runtime shadow (exploration camera w = 0).
    // Dynamic lights have their own visibility and never take this sun mask.
    let ambient_shade = select(select(mix(1.0, sun_shade, 0.15), 1.0, object), ao, lighting.counts.w > 0u);
    // Terrain1 masks static diffuse twice; Static1 and Low's Terrain3 only once.
    let diffuse = diff_s * sun_shade * select(lm.a, lm.a * lm.a, terrain && !terrain_low)
        + lm.rgb * ambient_shade + diff_d;
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
        case 12u: { return vec4<f32>(vec3<f32>(ao), 1.0); }
        default: {}
    }

    // The level's fog, as Static1 applies it.
    let to_eye = p - lighting.eye.xyz;
    let f = max(min(exp(-dot(to_eye, to_eye) * lighting.fog0.w), 1.0), lighting.fog1.x);
    colour = f * (colour - lighting.fog0.rgb) + lighting.fog0.rgb;
    return vec4<f32>(colour, 1.0);
}
