// Ordered colour/depth passes over the shared opaque frame. No G-buffer readback:
// fractional coverage belongs to this material and its authored blend/depth state.
struct ForwardIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv0: vec2<f32>,
    @location(3) uv1: vec2<f32>,
    @location(4) weights: vec4<f32>,
    @location(5) indices: vec4<f32>,
    @location(6) tangent: vec4<f32>,
};

struct ForwardOut {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world: vec3<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) irradiance: vec3<f32>,
    @location(4) tangent: vec3<f32>,
};

@vertex
fn vs_forward(v: ForwardIn) -> ForwardOut {
    let base = VsIn(v.position, v.normal, v.uv0, v.uv1, v.weights, v.indices);
    let vertex = gbuffer_vertex(base,
        skin_vector(vec4<f32>(v.position, 1.0), v.weights, v.indices),
        skin_vector(vec4<f32>(v.normal, 0.0), v.weights, v.indices));
    let tangent = skin_vector(vec4<f32>(v.tangent.xyz, 0.0), v.weights, v.indices);
    return ForwardOut(vertex.clip, vertex.uv0, vertex.world, vertex.normal,
        vertex.irradiance, (draw.world * vec4<f32>(tangent, 0.0)).xyz);
}

@fragment
fn fs_forward_depth(in: ForwardOut) -> @location(0) vec4<f32> {
    if (!alpha_kept(textureSample(diffuse_tex, wrap_smp, in.uv).b)) { discard; }
    return vec4<f32>(0.0);
}

@fragment
fn fs_forward(in: ForwardOut) -> @location(0) vec4<f32> {
    var albedo: vec3<f32>;
    var specular = vec4<f32>(0.0);
    var hair = HairLight();
    let n = normalize(in.normal);
    var alpha: f32;
    if (draw.material.w == 3.0) {
        let packed = textureSample(diffuse_tex, wrap_smp, in.uv);
        let noise = textureSample(normal_tex, clamp_smp, in.uv * draw.character.eye.w).r;
        albedo = draw.tint[2].rgb * packed.g + draw.tint[4].rgb * noise
            + draw.tint[6].rgb * (1.0 - packed.g);
        let secondary = draw.tint[3].rgb * packed.g + draw.tint[5].rgb * noise
            + draw.tint[7].rgb * (1.0 - packed.g);
        let shifted = in.tangent + n * (packed.r * 0.4 - 0.2);
        // Some shipped meshes contain zero tangent slots. Preserve finite authored
        // vectors, and keep a degenerate interpolated strand from generating NaNs.
        let axis = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), abs(n.z) > 0.95);
        let strand = select(normalize(cross(n, axis)),
            shifted * inverseSqrt(max(dot(shifted, shifted), 1e-20)), dot(shifted, shifted) > 1e-20);
        hair = HairLight(strand,
            draw.character.eye, secondary);
        specular = vec4<f32>(vec3<f32>(packed.a), 0.0);
        alpha = packed.b;
        albedo = mix(albedo, draw.tint_color.rgb, draw.tint_color.a);
    } else {
        // Eye0's eyelash shader deliberately biases the diffuse mip selection.
        let diffuse = textureSampleBias(diffuse_tex, wrap_smp, in.uv, -2.5);
        albedo = diffuse.rgb;
        alpha = diffuse.a;
    }
    if (!alpha_kept(alpha)) { discard; }
    // These character surfaces have probes, no baked direct visibility or cubemap
    // reflection. Their own depth does not participate in opaque screen-space AO.
    let ambient = albedo * in.irradiance;
    let colour = light_surface(in.clip, in.world, vec4<f32>(albedo, 192.0 / 255.0),
        specular, vec4<f32>(oct_encode(n), draw.room.x, 1.0),
        vec4<f32>(ambient, 1.0), 1.0, false, hair, true);
    return vec4<f32>(colour.rgb, clamp(alpha * draw.alpha.z, 0.0, 1.0));
}
