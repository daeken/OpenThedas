// OpenThedas deviation: a separable tent PCF over sixteen red-depth comparisons.
// Unlike filtering depth before comparison, these weights interpolate visibility.
// A nominal 3x3 tent uses a 4x4 stencil for continuous subtexel interpolation.
fn soft_shadow_row(map: texture_2d<f32>, pixel: vec2<i32>, depth: f32) -> vec4<f32> {
    let samples = vec4<f32>(
        textureLoad(map, pixel + vec2<i32>(-1, 0), 0).x,
        textureLoad(map, pixel, 0).x,
        textureLoad(map, pixel + vec2<i32>(1, 0), 0).x,
        textureLoad(map, pixel + vec2<i32>(2, 0), 0).x);
    return select(vec4<f32>(0.0), vec4<f32>(1.0), vec4<f32>(depth) <= samples);
}

fn soft_shadow_border_tap(map: texture_2d<f32>, pixel: vec2<i32>, size: vec2<i32>, depth: f32) -> f32 {
    let stored = textureLoad(map, clamp(pixel, vec2<i32>(0), size - vec2<i32>(1)), 0).x;
    return select(1.0, select(0.0, 1.0, depth <= stored), all(pixel >= vec2<i32>(0)) && all(pixel < size));
}

fn soft_shadow_border_row(map: texture_2d<f32>, pixel: vec2<i32>, size: vec2<i32>, depth: f32) -> vec4<f32> {
    return vec4<f32>(
        soft_shadow_border_tap(map, pixel + vec2<i32>(-1, 0), size, depth),
        soft_shadow_border_tap(map, pixel, size, depth),
        soft_shadow_border_tap(map, pixel + vec2<i32>(1, 0), size, depth),
        soft_shadow_border_tap(map, pixel + vec2<i32>(2, 0), size, depth));
}

fn soft_shadow_filter(map: texture_2d<f32>, t: vec3<f32>) -> f32 {
    let size = vec2<i32>(textureDimensions(map));
    let at = t.xy * vec2<f32>(size) - vec2<f32>(0.5);
    let base = vec2<i32>(floor(at));
    let fraction = fract(at);
    let depth = clamp(t.z, 0.0, 1.0);
    let wx = vec4<f32>(1.0 - fraction.x, 2.0 - fraction.x, 1.0 + fraction.x, fraction.x);
    let wy = vec4<f32>(1.0 - fraction.y, 2.0 - fraction.y, 1.0 + fraction.y, fraction.y);
    var rows: vec4<f32>;
    // Unroll the fixed stencil and check its border once. Keeping a per-texel branch
    // inside nested loops was substantially slower on Metal, despite identical taps.
    if (all(base >= vec2<i32>(1)) && all(base + vec2<i32>(2) < size)) {
        rows = vec4<f32>(
            dot(soft_shadow_row(map, base + vec2<i32>(0, -1), depth), wx),
            dot(soft_shadow_row(map, base, depth), wx),
            dot(soft_shadow_row(map, base + vec2<i32>(0, 1), depth), wx),
            dot(soft_shadow_row(map, base + vec2<i32>(0, 2), depth), wx));
    } else {
        // Explicit white border, including adapters without border samplers.
        rows = vec4<f32>(
            dot(soft_shadow_border_row(map, base + vec2<i32>(0, -1), size, depth), wx),
            dot(soft_shadow_border_row(map, base, size, depth), wx),
            dot(soft_shadow_border_row(map, base + vec2<i32>(0, 1), size, depth), wx),
            dot(soft_shadow_border_row(map, base + vec2<i32>(0, 2), size, depth), wx));
    }
    return dot(rows, wy) / 16.0;
}

fn soft_shadow_visibility(q: vec4<f32>, sd: array<vec4<f32>, 9>, near_map: texture_2d<f32>, far_map: texture_2d<f32>) -> f32 {
    let plane = dot(sd[8], q);
    // Blend over the two metres BEFORE the traced split, so the near map is never
    // sampled beyond its fitted slice. Only this band needs both PCF evaluations.
    var near_lit = 1.0;
    if (plane > 0.0) {
        let t = vec3<f32>(dot(sd[0], q), dot(sd[1], q), dot(sd[2], q)) / dot(sd[3], q);
        near_lit = soft_shadow_filter(near_map, t);
        if (plane >= 2.0) { return near_lit; }
    }
    // Keep the traced final-quarter fade, including exactly lit at/beyond far reach.
    let fade = clamp((sd[7].w - plane) / (0.25 * sd[7].w), 0.0, 1.0);
    if (fade == 0.0) { return 1.0; }
    let t = vec3<f32>(dot(sd[4], q), dot(sd[5], q), dot(sd[6], q)) / dot(vec4<f32>(sd[7].xyz, 1.0), q);
    let far_lit = mix(1.0, soft_shadow_filter(far_map, t), fade);
    return mix(far_lit, near_lit, smoothstep(0.0, 2.0, plane));
}
