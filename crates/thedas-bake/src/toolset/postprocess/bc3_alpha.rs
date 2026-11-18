//! TextureProcessor.exe's legacy NVIDIA DDS alpha fit, RVAs 0x74d40,
//! 0x96f60 and 0x95b70. The endpoint fit uses the normalized source values,
//! then scores the quantized ramp and assigns the stored indices.

const TEXELS: usize = 16;

pub(super) fn encode(texels: [Option<u8>; TEXELS]) -> [u8; 8] {
    let mut values = texels.iter().flatten().copied();
    let Some(first) = values.next() else {
        return [0; 8];
    };
    if values.all(|alpha| alpha == first) {
        return [first, first.saturating_add(1), 0, 0, 0, 0, 0, 0];
    }

    let mut best = fit_mode(&texels, 8, false, false);
    let has_zero = texels.contains(&Some(0));
    let has_full = texels.contains(&Some(255));
    for (skip_zero, skip_full) in [(false, false), (true, false), (false, true), (true, true)] {
        if (skip_zero && !has_zero) || (skip_full && !has_full) {
            continue;
        }
        let candidate = fit_mode(&texels, 6, skip_zero, skip_full);
        if candidate.error < best.error {
            best = candidate;
        }
    }
    best.write()
}

struct Block {
    endpoints: [u8; 2],
    indices: [Option<u8>; TEXELS],
    levels: u8,
    error: f64,
}

impl Block {
    fn write(self) -> [u8; 8] {
        let [lo, hi] = self.endpoints;
        if lo == hi {
            return [lo, lo.saturating_add(1), 0, 0, 0, 0, 0, 0];
        }
        let eight = self.levels == 8;
        let [first, second] = if eight { [hi, lo] } else { [lo, hi] };
        let mut packed = u64::from(first) | u64::from(second) << 8;
        for (texel, index) in self.indices.into_iter().enumerate() {
            let Some(mut index) = index else { continue };
            if eight {
                index = if index < 2 { 1 - index } else { 9 - index };
            }
            packed |= u64::from(index) << (16 + 3 * texel);
        }
        packed.to_le_bytes()
    }
}

fn fit_mode(texels: &[Option<u8>; TEXELS], levels: u8, skip_zero: bool, skip_full: bool) -> Block {
    let mut values = [0.0; TEXELS];
    let mut sample_for = [None; TEXELS];
    let mut indices = [None; TEXELS];
    let mut len = 0;
    for (texel, alpha) in texels.iter().copied().enumerate() {
        match alpha {
            Some(0) if skip_zero => indices[texel] = Some(6),
            Some(255) if skip_full => indices[texel] = Some(7),
            Some(alpha) => {
                values[len] = f64::from(f32::from(alpha) / 255.0);
                sample_for[texel] = Some(len);
                len += 1;
            }
            None => {}
        }
    }
    let values = &values[..len];
    let steps = levels - 1;
    let endpoints = fit_ramp(values, steps).map(|value| (value * 255.0).round() as u8);
    let [lo, hi] = endpoints.map(u16::from);
    let mut palette = [lo, hi, 0, 0, 0, 0, 0, 0];
    let interval = u16::from(steps);
    for i in 1..interval {
        palette[usize::from(i) + 1] = ((interval - i) * lo + i * hi + interval / 2) / interval;
    }
    let mut error = 0.0;
    for (index, sample) in indices.iter_mut().zip(sample_for) {
        let Some(sample) = sample else { continue };
        let value = values[sample] * 255.0;
        let difference = |color: u16| (f64::from(color) - value).abs();
        let (code, &color) = palette[..usize::from(levels)]
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| difference(**a).total_cmp(&difference(**b)))
            .expect("the ramp includes both endpoints");
        *index = Some(code as u8);
        error += difference(color).powi(2);
    }
    Block {
        endpoints,
        indices,
        levels,
        error,
    }
}

fn fit_ramp(values: &[f64], steps: u8) -> [f64; 2] {
    let Some(&first) = values.first() else {
        return [0.0, 0.0];
    };
    let mut endpoints = values.iter().fold([first; 2], |[lo, hi], &value| {
        [lo.min(value), hi.max(value)]
    });
    let count = values.len() as f64;
    let inv_count = 1.0 / count;
    let mean_alpha = values.iter().sum::<f64>() * inv_count;
    let mut previous = [0; TEXELS];
    for _ in 0..65 {
        let [lo, hi] = endpoints;
        let spacing = (hi - lo) / f64::from(steps);
        let mut bins = [0; TEXELS];
        let mut counts = [0; 8];
        for (&value, bin) in values.iter().zip(&mut bins) {
            *bin = (0..steps)
                .take_while(|&step| value >= lo + (f64::from(step) + 0.5) * spacing)
                .count() as u8;
            counts[usize::from(*bin)] += 1;
        }
        let occupied = counts.iter().filter(|&&count| count != 0).count();
        if occupied == 1 {
            break;
        }
        endpoints = least_squares(values, &bins, steps).endpoints;
        if bins == previous {
            let moments = BinMoments::measure(values, &bins, mean_alpha, count, inv_count);
            let [score, scale] = boundary_score(count, f64::from(steps), mean_alpha, &moments);
            let mut improved = None;
            // Probe adjacent bin assignments for the sample nearest each
            // boundary. The search restarts at the first improvement.
            'search: for higher in [true, false] {
                let range = if higher { 0..steps } else { 1..steps + 1 };
                for bin in range {
                    let bin_count = counts[usize::from(bin)];
                    if bin_count == 0 || (bin_count == 1 && occupied <= 2) {
                        continue;
                    }
                    let mut sample = None;
                    for (i, (&value, &assigned)) in values.iter().zip(&bins).enumerate() {
                        if assigned == bin
                            && sample.is_none_or(|j| {
                                if higher {
                                    value > values[j]
                                } else {
                                    value < values[j]
                                }
                            })
                        {
                            sample = Some(i);
                        }
                    }
                    let i = sample.expect("the bin has at least one sample");
                    let direction = if higher { 1.0 } else { -1.0 };
                    let moved =
                        moments.move_sample(bin, values[i] - mean_alpha, inv_count, direction);
                    let [next_score, next_scale] =
                        boundary_score(count, f64::from(steps), mean_alpha, &moved);
                    if next_score * scale < next_scale * score {
                        let mut trial = bins;
                        trial[i] = if higher { bin + 1 } else { bin - 1 };
                        improved = Some(least_squares(values, &trial, steps).endpoints);
                        break 'search;
                    }
                }
            }
            let Some(next) = improved else { break };
            endpoints = next;
        }
        previous = bins;
    }
    endpoints
}

struct BinMoments {
    mean: f64,
    variance: f64,
    covariance: f64,
}

impl BinMoments {
    fn measure(
        values: &[f64],
        bins: &[u8; TEXELS],
        mean_alpha: f64,
        count: f64,
        inv_count: f64,
    ) -> Self {
        let mut sum = 0;
        let mut squares = 0;
        let mut covariance = 0.0;
        for (&value, &bin) in values.iter().zip(bins) {
            let bin = u16::from(bin);
            sum += bin;
            squares += bin * bin;
            covariance += (value - mean_alpha) * f64::from(bin);
        }
        let mean = f64::from(sum) * inv_count;
        Self {
            mean,
            variance: f64::from(squares) - count * mean * mean,
            covariance,
        }
    }

    fn move_sample(&self, bin: u8, centered_alpha: f64, inv_count: f64, direction: f64) -> Self {
        let twice_direction = direction + direction;
        Self {
            mean: self.mean + inv_count * direction,
            variance: twice_direction * f64::from(bin)
                + (self.variance + 1.0 - twice_direction * self.mean - inv_count),
            covariance: centered_alpha * direction + self.covariance,
        }
    }
}

// TextureProcessor compares scaled residuals at RVA 0x96769. Preserve
// the centered moments and operation order because equal exact residuals
// can compare differently after the intermediate products round.
fn boundary_score(count: f64, steps: f64, mean_alpha: f64, moments: &BinMoments) -> [f64; 2] {
    let BinMoments {
        mean,
        variance,
        covariance,
    } = *moments;
    let low = -mean;
    let high = steps - mean;
    let low_weight = low * count * low + variance;
    let high_weight = high * count * high + variance;
    let cross_weight = high * low * count + variance;
    let product = low_weight * high_weight;
    let scale = product * variance * product * steps * steps;
    let min_alpha = -mean_alpha * variance;
    let max_alpha = (1.0 - mean_alpha) * variance;
    let min_low = min_alpha - low * covariance;
    let max_low = max_alpha - low * covariance;
    let min_high = (min_alpha - high * covariance) * low_weight;
    let max_high = (max_alpha - high * covariance) * low_weight;

    // The original constrains each endpoint with these absolute-value
    // expressions. A direct min/max clamp can round differently.
    let clamp = |value: f64, min: f64, max: f64| {
        ((min - value).abs() + min + (max - (max - value).abs())) * 0.5
    };
    let high_delta = clamp(
        clamp(0.0, min_low, max_low) * cross_weight,
        min_high,
        max_high,
    );
    let low_offset = clamp(
        high_delta * cross_weight,
        min_low * product,
        max_low * product,
    );
    let high_offset = high_delta * high_weight;
    let slope_offset = low_offset - high_offset;
    let mean_offset = high_offset * low - low_offset * high;
    [
        slope_offset * variance * slope_offset + mean_offset * count * mean_offset
            - covariance * covariance * scale,
        scale * variance,
    ]
}

struct Fit {
    endpoints: [f64; 2],
    error: f64,
}

fn least_squares(values: &[f64], bins: &[u8; TEXELS], steps: u8) -> Fit {
    let coordinate = |bin| f64::from(bin) / f64::from(steps);
    let n = values.len() as f64;
    let (mut x, mut y, mut xx, mut xy) = (0.0, 0.0, 0.0, 0.0);
    for (&value, &bin) in values.iter().zip(bins) {
        let t = coordinate(bin);
        x += t;
        y += value;
        xx += t * t;
        xy += t * value;
    }
    let evaluate = |lo, hi| Fit {
        endpoints: [lo, hi],
        error: values
            .iter()
            .zip(bins)
            .map(|(&value, &bin)| (value - (lo + coordinate(bin) * (hi - lo))).powi(2))
            .sum(),
    };
    let slope = (xy - x * y / n) / (xx - x * x / n);
    let lo = (y - slope * x) / n;
    let hi = lo + slope;
    // If the fit leaves [0, 1], resolve it with one endpoint pinned.
    let with_lo_pinned = || evaluate(0.0, (xy / xx).clamp(0.0, 1.0));
    let with_hi_pinned = || {
        evaluate(
            ((y - xy - x + xx) / (n - 2.0 * x + xx)).clamp(0.0, 1.0),
            1.0,
        )
    };
    match (lo < 0.0, hi > 1.0) {
        (true, true) => {
            let a = with_lo_pinned();
            let b = with_hi_pinned();
            if b.error < a.error { b } else { a }
        }
        (true, false) => with_lo_pinned(),
        (false, true) => with_hi_pinned(),
        (false, false) => evaluate(lo, hi),
    }
}

#[cfg(test)]
mod tests {
    use super::encode;

    #[test]
    fn keeps_original_alpha_fit_when_adjacent_assignments_tie() {
        let alpha = [0, 0, 0, 0, 0, 0, 102, 110, 0, 0, 105, 124, 0, 121, 113, 116];
        assert_eq!(
            encode(alpha.map(Some)),
            [0x66, 0x7d, 0xb6, 0x6d, 0x63, 0xb6, 0xe2, 0x8e]
        );
    }
}
