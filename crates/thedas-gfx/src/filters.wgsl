struct Args {
    color: vec4<f32>,
    effect: vec4<f32>,
    mode: vec4<f32>,
};
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var blurred: texture_2d<f32>;
@group(0) @binding(2) var smp: sampler;
@group(0) @binding(3) var<uniform> args: Args;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

// Sampling outside the bitmap is transparent, including fractional edge samples.
fn pixel(p: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(blurred));
    if (any(p < vec2<i32>(0)) || any(p >= size)) { return vec4<f32>(0.0); }
    return textureLoad(blurred, p, 0);
}

fn alpha_at(p: vec2<f32>) -> f32 {
    let lo = vec2<i32>(floor(p));
    let f = fract(p);
    return mix(mix(pixel(lo).a, pixel(lo + vec2<i32>(1, 0)).a, f.x),
               mix(pixel(lo + vec2<i32>(0, 1)).a, pixel(lo + vec2<i32>(1, 1)).a, f.x), f.y);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    if (args.mode.w > 0.0) {
        let radius = (args.mode.z - 1.0) * 0.5;
        let whole = i32(floor(radius));
        let edge = floor(fract(radius) * 255.0) / 255.0;
        var direction = vec2<i32>(1, 0);
        if (args.mode.w > 1.5) { direction = vec2<i32>(0, 1); }
        var sum = pixel(p);
        for (var i = 1; i <= whole; i += 1) {
            sum += pixel(p + direction * i) + pixel(p - direction * i);
        }
        sum += edge * (pixel(p + direction * (whole + 1)) + pixel(p - direction * (whole + 1)));
        return sum / args.mode.z;
    }
    let src = textureLoad(source, p, 0);
    let mask = alpha_at(pos.xy - vec2<f32>(0.5) - args.effect.xy);
    let inner = args.effect.w > 0.5;
    let knockout = args.mode.x > 0.5;
    let composite = args.mode.y > 0.5;
    let coverage = select(mask, 1.0 - mask, inner);
    let alpha = args.color.a * clamp(coverage * args.effect.z, 0.0, 1.0);
    let tint = vec4<f32>(args.color.rgb, 1.0) * alpha;
    if (inner) {
        if (knockout || !composite) { return tint * src.a; }
        return tint * src.a + src * (1.0 - alpha);
    }
    if (knockout) { return tint * (1.0 - src.a); }
    if (composite) { return tint * (1.0 - src.a) + src; }
    return tint;
}
