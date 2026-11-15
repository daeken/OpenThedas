//! Texel-by-texel comparison of a baked atlas with the shipped one.

/// Error statistics over the texels both images cover.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub texels: u64,
    /// Mean absolute difference per channel (R, G, B, A), in 8-bit levels.
    pub mae: [f64; 4],
    /// Root mean square difference per channel.
    pub rmse: [f64; 4],
    /// Signed mean (ours − shipped) per channel.
    pub bias: [f64; 4],
    /// Texels whose largest channel difference is within 4 levels.
    pub within4: u64,
    /// Texels where one alpha is zero and the other is not (shadow pass disagreement).
    pub shadow_flips: u64,
}

impl Stats {
    pub fn add(&mut self, ours: [u8; 4], shipped: [u8; 4]) {
        self.texels += 1;
        let mut worst = 0;
        for c in 0..4 {
            let d = ours[c] as f64 - shipped[c] as f64;
            self.mae[c] += d.abs();
            self.rmse[c] += d * d;
            self.bias[c] += d;
            worst = worst.max(d.abs() as u32);
        }
        if worst <= 4 {
            self.within4 += 1;
        }
        if (ours[3] < 8) != (shipped[3] < 8) {
            self.shadow_flips += 1;
        }
    }

    pub fn merge(&mut self, o: &Stats) {
        self.texels += o.texels;
        for c in 0..4 {
            self.mae[c] += o.mae[c];
            self.rmse[c] += o.rmse[c];
            self.bias[c] += o.bias[c];
        }
        self.within4 += o.within4;
        self.shadow_flips += o.shadow_flips;
    }

    /// The finished statistics (means instead of sums).
    pub fn finish(&self) -> Stats {
        let n = self.texels.max(1) as f64;
        let mut s = *self;
        for c in 0..4 {
            s.mae[c] /= n;
            s.rmse[c] = (s.rmse[c] / n).sqrt();
            s.bias[c] /= n;
        }
        s
    }

    pub fn line(&self) -> String {
        let s = self.finish();
        let n = self.texels.max(1) as f64;
        format!(
            "{:>8} texels  MAE r{:5.1} g{:5.1} b{:5.1} a{:5.1}  RMSE r{:5.1} g{:5.1} b{:5.1} a{:5.1}  bias r{:+5.1} g{:+5.1} b{:+5.1} a{:+5.1}  ≤4: {:5.1}%  shadow flips {:4.1}%",
            self.texels,
            s.mae[0],
            s.mae[1],
            s.mae[2],
            s.mae[3],
            s.rmse[0],
            s.rmse[1],
            s.rmse[2],
            s.rmse[3],
            s.bias[0],
            s.bias[1],
            s.bias[2],
            s.bias[3],
            100.0 * self.within4 as f64 / n,
            100.0 * self.shadow_flips as f64 / n,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn averages_differences() {
        let mut s = Stats::default();
        s.add([10, 10, 10, 0], [12, 10, 10, 200]);
        s.add([10, 10, 10, 255], [10, 10, 10, 255]);
        let f = s.finish();
        assert_eq!(f.texels, 2);
        assert!((f.mae[0] - 1.0).abs() < 1e-9);
        assert!((f.bias[3] + 100.0).abs() < 1e-9);
        assert_eq!(s.within4, 1);
        assert_eq!(s.shadow_flips, 1);
    }
}
