//! Static1 tints transform the complete lighting sum, before fog. Keep their factors
//! out of the UNORM material channels, which would clip bright tints before lighting.

pub(super) const IDENTITY: [f32; 4] = [1.0; 4];

pub(super) fn transform(tint: [f32; 4]) -> Option<[f32; 4]> {
    if !tint.into_iter().all(f32::is_finite) || tint[..3].contains(&1.0) {
        return None;
    }
    let mut factors = [1.0, 1.0, 1.0, 1.0 - tint[3]];
    for c in 0..3 {
        factors[c] = 1.0 / (1.0 - tint[c]);
    }
    factors.into_iter().all(f32::is_finite).then_some(factors)
}

/// Local palettes are built once at load. Only their small lists are combined when
/// level and object renderers share a frame, using spare 8-bit material codes.
pub(super) struct Table {
    pub factors: [[f32; 4]; 256],
    used: Vec<usize>,
}

impl Table {
    pub fn new() -> Self {
        Self {
            factors: [IDENTITY; 256],
            used: Vec::new(),
        }
    }

    pub fn include(&mut self, palette: &[[f32; 4]]) -> Option<Vec<u8>> {
        let mut codes = vec![0]; // Local zero is the untinted Static1 material.
        for &factors in palette {
            let code = if let Some(&code) = self.used.iter().find(|&&i| self.factors[i] == factors)
            {
                code
            } else {
                let code = (1..255)
                    .find(|i| !matches!(i, 32 | 64 | 128 | 192) && !self.used.contains(i))?;
                self.factors[code] = factors;
                self.used.push(code);
                code
            };
            codes.push(code as u8);
        }
        Some(codes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factors_preserve_clamp_order_and_reject_undefined_materials() {
        assert_eq!(transform([0.0; 4]), Some(IDENTITY));
        assert_eq!(
            transform([0.5, 0.75, -1.0, 0.5]),
            Some([2.0, 4.0, 0.5, 0.5])
        );
        assert_eq!(transform([0.0, 0.5, 0.75, 1.0]), Some([1.0, 2.0, 4.0, 0.0]));
        assert_eq!(transform([1.0, 0.0, 0.0, 0.0]), None);
        assert_eq!(transform([f32::NAN, 0.0, 0.0, 0.0]), None);
        assert_eq!(transform([0.0, 0.0, 0.0, f32::INFINITY]), None);
    }

    #[test]
    fn shared_codes_deduplicate_contributors_and_never_alias_material_families() {
        let mut table = Table::new();
        let first = [2.0, 0.5, 4.0, 1.0];
        assert_eq!(table.include(&[first]), Some(vec![0, 1]));
        assert_eq!(table.include(&[[3.0; 4], first]), Some(vec![0, 2, 1]));
        let palette: Vec<_> = (4..252).map(|i| [i as f32; 4]).collect();
        let codes = table.include(&palette).unwrap();
        assert_eq!(table.used.len(), 250);
        assert!(
            codes[1..]
                .iter()
                .all(|i| !matches!(i, 0 | 32 | 64 | 128 | 192 | 255))
        );
        assert_eq!(table.include(&[[999.0; 4]]), None);
        for i in [0, 32, 64, 128, 192, 255] {
            assert_eq!(table.factors[i], IDENTITY);
        }
    }
}
