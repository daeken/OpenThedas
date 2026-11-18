//! TextureProcessor.exe's four-color DXT5 path. The fast principal-axis fit
//! competes with the ordered cluster fit at RVA 0x9c340. The latter searches
//! nearby RGB565 endpoints at 0x79ad0, then 0x944d0 rescores its palette.

mod solid;

const TEXELS: usize = 16;
const WEIGHTS: Color = [0.3086_f32 as f64, 0.6094_f32 as f64, 0.082_f32 as f64];
const BITS: [u8; 3] = [5, 6, 5];
const MAX_CODES: Color = [31.0, 63.0, 31.0];

type Color = [f64; 3];

#[derive(Clone, Copy, Default)]
struct Sample {
    rgb: [u8; 3],
    weighted: Color,
    count: u8,
}

struct Block {
    samples: [Sample; TEXELS],
    sample_for: [Option<usize>; TEXELS],
    unique: usize,
    len: usize,
}

impl Block {
    fn new(texels: [Option<[u8; 3]>; TEXELS]) -> Self {
        let mut samples = [Sample::default(); TEXELS];
        let mut unique = 0;
        let mut len = 0;
        let sample_for = texels.map(|texel| {
            let rgb = texel?;
            let sample = samples[..unique]
                .iter()
                .position(|sample| sample.rgb == rgb)
                .unwrap_or_else(|| {
                    samples[unique] = Sample {
                        rgb,
                        weighted: std::array::from_fn(|channel| {
                            normalized(rgb[channel]) * WEIGHTS[channel]
                        }),
                        count: 0,
                    };
                    unique += 1;
                    unique - 1
                });
            samples[sample].count += 1;
            len += 1;
            Some(sample)
        });
        Self {
            samples,
            sample_for,
            unique,
            len,
        }
    }

    fn samples(&self) -> &[Sample] {
        &self.samples[..self.unique]
    }
}

pub(super) fn encode(texels: [Option<[u8; 3]>; TEXELS]) -> [u8; 8] {
    let block = Block::new(texels);
    let samples = block.samples();
    if samples.is_empty() {
        return [1, 0, 0, 0, 0xff, 0xff, 0xff, 0xff];
    }
    let fast = fast_fit(&block);
    let endpoints = match samples {
        [sample] => solid::endpoints(sample.rgb),
        [a, b] => quantize(samples, [a.weighted, b.weighted]),
        _ => quantize(samples, cluster_fit(&block)),
    };
    let thorough = score(&block, endpoints);
    // The original stores the fast error before comparing it with the
    // recomputed cluster error. Equal errors favored the cluster on the probes.
    let candidate = if thorough.error <= score(&block, fast.endpoints).error {
        thorough
    } else {
        fast
    };
    candidate.write(block.sample_for)
}

#[derive(Clone, Copy)]
struct Candidate {
    endpoints: [u16; 2],
    indices: [u8; TEXELS],
    error: f64,
}

impl Candidate {
    fn write(mut self, sample_for: [Option<usize>; TEXELS]) -> [u8; 8] {
        if self.endpoints[0] == self.endpoints[1] {
            self.endpoints[0] = self.endpoints[0].wrapping_add(1);
            self.indices.fill(1);
        }
        let swapped = self.endpoints[0] < self.endpoints[1];
        if swapped {
            self.endpoints.swap(0, 1);
        }
        let codes = sample_for
            .into_iter()
            .enumerate()
            .fold(0_u32, |bits, (texel, sample)| {
                let index = sample.map_or(3, |i| self.indices[i] ^ u8::from(swapped));
                bits | u32::from(index) << (2 * texel)
            });
        let mut output = [0; 8];
        output[..2].copy_from_slice(&self.endpoints[0].to_le_bytes());
        output[2..4].copy_from_slice(&self.endpoints[1].to_le_bytes());
        output[4..].copy_from_slice(&codes.to_le_bytes());
        output
    }
}

/// The channel values were stored as normalized binary32 before either fit.
fn normalized(byte: u8) -> f64 {
    f64::from(f32::from(byte) / 255.0)
}

fn source_value(byte: u8) -> f64 {
    normalized(byte) * 255.0
}

fn pack(color: [u8; 3]) -> u16 {
    u16::from(color[0]) << 11 | u16::from(color[1]) << 5 | u16::from(color[2])
}

fn unpack(color: u16) -> [u8; 3] {
    [
        (color >> 11) as u8,
        (color >> 5 & 63) as u8,
        (color & 31) as u8,
    ]
}

fn expand(code: u8, bits: u8) -> u8 {
    (code << (8 - bits)) | (code >> (2 * bits - 8))
}

fn ramp(a: u8, b: u8, bias: u16) -> [u8; 4] {
    let (x, y) = (u16::from(a), u16::from(b));
    [
        a,
        b,
        ((2 * x + y + bias) / 3) as u8,
        ((x + 2 * y + bias) / 3) as u8,
    ]
}

fn score(block: &Block, endpoints: [u16; 2]) -> Candidate {
    let [a, b] = endpoints.map(unpack);
    let channels =
        std::array::from_fn::<_, 3, _>(|i| ramp(expand(a[i], BITS[i]), expand(b[i], BITS[i]), 0));
    let mut indices = [0; TEXELS];
    let mut errors = [0.0; TEXELS];
    for (i, sample) in block.samples().iter().enumerate() {
        let source = sample.rgb.map(source_value);
        let (code, error) = (0..4)
            .map(|entry| {
                let [r, g, b] = std::array::from_fn(|axis| {
                    (source[axis] - f64::from(channels[axis][entry])) * WEIGHTS[axis]
                });
                (entry, b * b + g * g + r * r)
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .expect("the DXT5 palette has four entries");
        indices[i] = code as u8;
        errors[i] = error;
    }
    let error = block.sample_for.iter().flatten().map(|&i| errors[i]).sum();
    Candidate {
        endpoints,
        indices,
        error,
    }
}

// Search the two bracketing RGB565 codes for each component. The quantizer
// uses rounded interpolants and normalized components. The caller rescores
// the winning endpoints with the integer-truncated DXT5 palette.
fn quantize(samples: &[Sample], endpoints: [Color; 2]) -> [u16; 2] {
    let choices = endpoints.map(|color| {
        std::array::from_fn::<_, 3, _>(|channel| {
            let bits = BITS[channel];
            let value = color[channel] / WEIGHTS[channel] * 255.0;
            let max = (1 << bits) - 1;
            let base = (value.floor().clamp(0.0, 255.0) as u8) >> (8 - bits);
            let low = base.saturating_sub(u8::from(f64::from(expand(base, bits)) > value));
            [low, (low + 1).min(max)]
        })
    });
    let total = f64::from(samples.iter().map(|s| s.count).sum::<u8>());
    let pairs = std::array::from_fn::<_, 3, _>(|channel| {
        std::array::from_fn::<_, 4, _>(|pair| {
            [
                choices[0][channel][pair >> 1],
                choices[1][channel][pair & 1],
            ]
        })
    });
    let errors = std::array::from_fn::<_, 3, _>(|channel| {
        pairs[channel].map(|pair| {
            let [a, b] = pair.map(|code| expand(code, BITS[channel]));
            let palette = ramp(a, b, 1).map(|v| f64::from(v) * WEIGHTS[channel] / 255.0);
            let mut errors = [[0.0; 4]; TEXELS];
            for (sample, out) in samples.iter().zip(&mut errors) {
                let mass = f64::from(sample.count) / total;
                *out = palette.map(|v| {
                    let difference = sample.weighted[channel] - v;
                    (difference * mass) * difference
                });
            }
            errors
        })
    });
    let mut best = [0; 3];
    let mut error = f64::INFINITY;
    for mask in 0..64 {
        let choice = [mask & 3, (mask >> 2) & 3, mask >> 4];
        let [r, g, b] = std::array::from_fn(|axis| &errors[axis][choice[axis]]);
        let mut sum = 0.0;
        for i in (0..samples.len()).rev() {
            let [a, b, c, d] = std::array::from_fn(|entry| g[i][entry] + r[i][entry] + b[i][entry]);
            sum += twice_min(twice_min(a, b), twice_min(c, d));
        }
        if sum < error {
            best = choice;
            error = sum;
        }
    }
    [0, 1].map(|end| pack(std::array::from_fn(|axis| pairs[axis][best[axis]][end])))
}

// Preserve the original quantizer's reduction and accumulation order. An
// ordinary minimum gives the same real-valued error but different float ties.
fn twice_min(a: f64, b: f64) -> f64 {
    (a + b) - (a - b).abs()
}

fn center_and_covariance(
    colors: impl Clone + Iterator<Item = (Color, f64)>,
    len: usize,
) -> (Color, [Color; 3]) {
    let mean = std::array::from_fn(|axis| {
        colors
            .clone()
            .map(|(color, mass)| color[axis] * mass)
            .sum::<f64>()
            / len as f64
    });
    let covariance = std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            colors
                .clone()
                .map(|(color, mass)| (color[i] - mean[i]) * (color[j] - mean[j]) * mass)
                .sum()
        })
    });
    (mean, covariance)
}

fn principal_axis(covariance: [Color; 3], iterations: usize) -> Color {
    let mut start = 0;
    for axis in 1..3 {
        if covariance[axis][axis] > covariance[start][start] {
            start = axis;
        }
    }
    let mut axis = unit(covariance[start]);
    for _ in 0..iterations {
        axis = unit(covariance.map(|row| dot(row, axis)));
    }
    axis
}

fn fast_axis(mut covariance: [Color; 3]) -> Option<Color> {
    let square = |matrix: [Color; 3]| {
        std::array::from_fn(|i| std::array::from_fn(|j| float_store(dot(matrix[i], matrix[j]))))
    };
    for _ in 0..9 {
        covariance = square(square(covariance));
        let trace = float_store(covariance[0][0] + covariance[1][1] + covariance[2][2]);
        if trace == 0.0 {
            return None;
        }
        let reciprocal = float_store(1.0 / float_store(trace.sqrt()));
        let scale = float_store(3.0 * reciprocal * reciprocal);
        covariance = covariance.map(|row| row.map(|v| float_store(v * scale)));
    }
    let mut axis = 0;
    for i in 1..3 {
        if covariance[i][i] > covariance[axis][axis] {
            axis = i;
        }
    }
    let diagonal = covariance[axis][axis];
    if diagonal < f64::from(f32::EPSILON) {
        return None;
    }
    let reciprocal = float_store(1.0 / float_store(diagonal.sqrt()));
    Some(covariance[axis].map(|value| float_store(value * reciprocal)))
}

fn fast_fit(block: &Block) -> Candidate {
    let samples = block.samples();
    let rgb_weighted = |i: usize| {
        std::array::from_fn(|axis| {
            let normalized = f64::from(f32::from(samples[i].rgb[axis]) / 255.0);
            float_store(normalized * WEIGHTS[axis])
        })
    };
    let solid_endpoint = || {
        pack(std::array::from_fn(|i| {
            (source_value(samples[0].rgb[i]) / 255.0 * MAX_CODES[i]).round_ties_even() as u8
        }))
    };
    let mut indices = [0; TEXELS];
    let endpoints = if block.unique == 1 {
        [solid_endpoint(); 2]
    } else {
        let mut mean = [0.0; 3];
        for &sample in block.sample_for.iter().flatten() {
            let color = rgb_weighted(sample);
            mean = std::array::from_fn(|i| float_store(mean[i] + color[i]));
        }
        let mean = mean.map(|v| float_store(v * float_store(1.0 / block.len as f64)));
        let centered: [Color; TEXELS] = std::array::from_fn(|i| {
            if i < block.unique {
                subtract(rgb_weighted(i), mean).map(float_store)
            } else {
                [0.0; 3]
            }
        });
        let mut cov = [[0.0; 3]; 3];
        for &sample in block.sample_for.iter().flatten() {
            let point = centered[sample];
            for i in 0..3 {
                for j in i..3 {
                    cov[i][j] = float_store(point[i] * point[j] + cov[i][j]);
                    cov[j][i] = cov[i][j];
                }
            }
        }
        let Some(axis) = fast_axis(cov) else {
            return Candidate {
                endpoints: [solid_endpoint(); 2],
                indices,
                error: 0.0,
            };
        };
        let inverse_length_squared = float_store(1.0 / float_store(dot(axis, axis)));
        let (lo, hi) = centered[..block.unique].iter().fold(
            (f64::INFINITY, f64::NEG_INFINITY),
            |(lo, hi), &color| {
                // RVAs 0xa4ccb–0xa4cf9 store the partial projection in
                // binary32 and round it again after adding 128.
                let red = float_store(color[0] * axis[0]);
                let red_green = float_store(red + color[1] * axis[1]);
                let projection = float_store(red_green + color[2] * axis[2]);
                let t = float_store(projection * inverse_length_squared + 128.0) - 128.0;
                (lo.min(t), hi.max(t))
            },
        );
        let mut line: [Color; 2] =
            [lo, hi].map(|t| std::array::from_fn(|i| float_store(mean[i] + t * axis[i])));
        for (channel, weight) in WEIGHTS.into_iter().enumerate() {
            for limit in [0.0, weight] {
                let [a, b] = line;
                if (a[channel] < limit) != (b[channel] < limit) {
                    let t = (limit - a[channel]) / (b[channel] - a[channel]);
                    let end = usize::from((a[channel] < limit) != (limit == 0.0));
                    let start = line[end];
                    line[end] = std::array::from_fn(|i| {
                        float_store(start[i] + (t - end as f64) * (b[i] - a[i]))
                    });
                }
            }
        }
        let scaled = line.map(|color| {
            std::array::from_fn::<_, 3, _>(|i| {
                let normalized = float_store(color[i] / WEIGHTS[i]);
                float_store(normalized.clamp(0.0, 1.0) * MAX_CODES[i])
            })
        });
        let mut codes = scaled.map(|color| color.map(|value| value.round_ties_even() as u8));
        for i in 0..3 {
            if codes[0][i] == codes[1][i] {
                codes[0][i] = scaled[0][i].floor() as u8;
                codes[1][i] = scaled[1][i].ceil() as u8;
            }
        }
        let mut endpoints = codes.map(pack);
        if endpoints[0] < endpoints[1] {
            endpoints.swap(0, 1);
        }
        let [a, b] = endpoints.map(unpack).map(|color| {
            std::array::from_fn(|i| {
                float_store(float_store(f64::from(color[i]) / MAX_CODES[i]) * WEIGHTS[i])
            })
        });
        let direction = subtract(b, a);
        let length_squared = dot(direction, direction);
        if length_squared > 0.0 {
            for (i, index) in indices[..samples.len()].iter_mut().enumerate() {
                let coordinate =
                    (dot(subtract(rgb_weighted(i), a), direction) / length_squared).clamp(0.0, 1.0);
                *index = [0, 2, 3, 1][(coordinate * 3.0).round_ties_even() as usize];
            }
        }
        endpoints
    };
    Candidate {
        endpoints,
        indices,
        error: 0.0,
    }
}

fn cluster_fit(block: &Block) -> [Color; 2] {
    let samples = block.samples();
    let colors = samples.iter().map(|s| (s.weighted, f64::from(s.count)));
    let (center, covariance) = center_and_covariance(colors, block.len);
    let mut widest_axis = [0.0; 3];
    for (i, a) in samples.iter().enumerate() {
        for b in &samples[i + 1..] {
            let delta = subtract(a.weighted, b.weighted);
            if dot(delta, delta) > dot(widest_axis, widest_axis) {
                widest_axis = delta;
            }
        }
    }

    let mut best = (f64::INFINITY, [[0.0; 3]; 2]);
    for start in [principal_axis(covariance, 8), unit(widest_axis)] {
        let fit = refine_axis(block, center, start, false);
        if fit.0 < best.0 {
            best = fit;
        }
    }
    if best.1.iter().any(|endpoint| outside_gamut(*endpoint)) {
        let axis = unit(subtract(best.1[1], best.1[0]));
        return refine_axis(block, center, axis, true).1;
    }
    best.1
}

fn outside_gamut(color: Color) -> bool {
    color
        .into_iter()
        .zip(WEIGHTS)
        .any(|(v, max)| v < 0.0 || v > max)
}

fn refine_axis(block: &Block, center: Color, mut axis: Color, bounded: bool) -> (f64, [Color; 2]) {
    let mut best = (f64::INFINITY, [[0.0; 3]; 2]);
    let mut visited = [[0; TEXELS]; 16];
    for iteration in 0..visited.len() {
        let projections = block.samples.map(|s| dot(s.weighted, axis));
        let mut order = std::array::from_fn::<_, TEXELS, _>(|i| i);
        order[..block.unique].sort_by(|&a, &b| projections[a].total_cmp(&projections[b]));
        if visited[..iteration].contains(&order) {
            break;
        }
        visited[iteration] = order;
        let fit = fit_partitions(
            block.samples(),
            &order[..block.unique],
            center,
            block.len as f64,
            bounded,
        );
        if fit.0 < best.0 {
            best = fit;
        }
        axis = unit(subtract(fit.1[1], fit.1[0]));
    }
    best
}

fn fit_partitions(
    samples: &[Sample],
    order: &[usize],
    center: Color,
    mass: f64,
    bounded: bool,
) -> (f64, [Color; 2]) {
    let mut prefix = [([0.0; 3], 0.0); TEXELS + 1];
    let n = order.len();
    for (i, &entry) in order.iter().enumerate() {
        let sample = samples[entry];
        let count = f64::from(sample.count);
        let (sum, accumulated) = prefix[i];
        let centered = subtract(sample.weighted, center);
        prefix[i + 1] = (
            std::array::from_fn(|j| sum[j] + centered[j] * count),
            accumulated + count,
        );
    }
    let (total, _) = prefix[n];
    let mut best_error = 0.0;
    let mut best = [center, center];
    for high in (1..=n).rev() {
        for mid in (1..=high).rev() {
            for low in (1..=mid.min(n - 1)).rev() {
                let (a, ma) = prefix[low];
                let (b, mb) = prefix[mid];
                let (c, mc) = prefix[high];
                let x = 3.0 * mass - ma - mb - mc;
                let xx = 9.0 * mass - ma - 3.0 * mb - 5.0 * mc;
                let variance = xx - x * x / mass;
                let weighted = std::array::from_fn(|i| 3.0 * total[i] - a[i] - b[i] - c[i]);
                let mut error = -dot(weighted, weighted) / variance;
                if error >= best_error && !bounded {
                    continue;
                }
                let steps =
                    (usize::from(low != n) + usize::from(mid != n) + usize::from(high != n)) as f64;
                let slope = weighted.map(|v| v / variance);
                let lo = std::array::from_fn(|i| center[i] - x / mass * slope[i]);
                let hi = std::array::from_fn(|i| lo[i] + steps * slope[i]);
                let mut fit = [lo, hi];
                if bounded {
                    let t = x / steps;
                    let tt = xx / (steps * steps);
                    let a2 = mass - 2.0 * t + tt;
                    let b2 = tt;
                    let ab = t - tt;
                    for channel in 0..3 {
                        let moment = weighted[channel] / steps + t * center[channel];
                        let a_y = mass * center[channel] - moment;
                        let b_y = moment;
                        let max = WEIGHTS[channel];
                        let [a, b] = [lo[channel], hi[channel]];
                        if (0.0..=max).contains(&a) && (0.0..=max).contains(&b) {
                            continue;
                        }
                        let fit_error = |a: f64, b: f64| {
                            a * (a2 * a + ab * b - 2.0 * a_y) + b * (b2 * b + ab * a - 2.0 * b_y)
                        };
                        let mut best = (f64::INFINITY, [a, b]);
                        for value in [0.0, max] {
                            for [a, b] in [
                                [value, ((b_y - ab * value) / b2).clamp(0.0, max)],
                                [((a_y - ab * value) / a2).clamp(0.0, max), value],
                            ] {
                                let error = fit_error(a, b);
                                if error < best.0 {
                                    best = (error, [a, b]);
                                }
                            }
                        }
                        error += best.0 - fit_error(a, b);
                        fit[0][channel] = best.1[0];
                        fit[1][channel] = best.1[1];
                    }
                }
                if error < best_error {
                    best_error = error;
                    best = fit;
                }
            }
        }
    }
    (best_error, best)
}

fn float_store(value: f64) -> f64 {
    f64::from(value as f32)
}

fn subtract(a: Color, b: Color) -> Color {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: Color, b: Color) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(value: Color) -> Color {
    let norm = dot(value, value).sqrt();
    if norm == 0.0 {
        [1.0, 0.0, 0.0]
    } else {
        value.map(|v| v / norm)
    }
}

#[cfg(test)]
mod tests {
    use super::encode;

    #[test]
    fn fast_fit_rounds_the_biased_projection_before_quantizing() {
        let colors = [
            [14, 29, 49],
            [14, 31, 51],
            [14, 30, 51],
            [15, 32, 53],
            [14, 29, 49],
            [11, 24, 40],
            [13, 27, 45],
            [9, 20, 33],
            [0, 0, 0],
            [0, 0, 0],
            [0, 0, 0],
            [0, 0, 0],
            [0, 0, 0],
            [0, 0, 0],
            [0, 0, 0],
            [0, 0, 0],
        ];
        assert_eq!(
            encode(colors.map(Some)),
            [0x06, 0x11, 0x00, 0x00, 0x00, 0x88, 0x55, 0x55]
        );
    }
}
