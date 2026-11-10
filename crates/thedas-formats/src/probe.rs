//! Irradiance probes (`<level>_probe_<id>.mtx`): plain text, three symmetric 4x4 matrices
//! (red, green, blue; one row of four numbers per line) giving the diffuse light arriving
//! from each direction as `n^T M n` for a unit normal `n = (x, y, z, 1)`, the form of
//! Ramamoorthi and Hanrahan's irradiance environment maps. Lines after the twelve rows
//! (`// The following is debugging data ...`, the spherical-harmonic coefficients) are
//! ignored. Each room of a level carries one in its archive; exterior levels one in the level
//! archive.

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ProbeError {
    #[error("probe line {line}: {message}")]
    Malformed { line: usize, message: String },
    #[error("probe has {0} matrix rows, expected 12")]
    Short(usize),
}

/// The three matrices, each as four rows.
#[derive(Clone, Debug, PartialEq)]
pub struct IrradianceProbe {
    pub red: [[f32; 4]; 4],
    pub green: [[f32; 4]; 4],
    pub blue: [[f32; 4]; 4],
}

impl IrradianceProbe {
    pub fn parse(data: &[u8]) -> Result<Self, ProbeError> {
        let text = String::from_utf8_lossy(data);
        let mut rows: Vec<[f32; 4]> = Vec::with_capacity(12);
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.starts_with("//") {
                break;
            }
            if line.is_empty() {
                continue;
            }
            let values: Vec<f32> = line
                .split_whitespace()
                .map(|w| {
                    w.parse::<f32>().map_err(|e| ProbeError::Malformed {
                        line: i + 1,
                        message: format!("{w:?}: {e}"),
                    })
                })
                .collect::<Result<_, _>>()?;
            let row: [f32; 4] =
                values
                    .as_slice()
                    .try_into()
                    .map_err(|_| ProbeError::Malformed {
                        line: i + 1,
                        message: format!("{} numbers, expected 4", values.len()),
                    })?;
            if row.iter().any(|v| !v.is_finite()) {
                return Err(ProbeError::Malformed {
                    line: i + 1,
                    message: "not a finite number".into(),
                });
            }
            rows.push(row);
            if rows.len() == 12 {
                break;
            }
        }
        if rows.len() != 12 {
            return Err(ProbeError::Short(rows.len()));
        }
        let m = |k: usize| [rows[k], rows[k + 1], rows[k + 2], rows[k + 3]];
        Ok(Self {
            red: m(0),
            green: m(4),
            blue: m(8),
        })
    }

    /// The twelve rows in shader order (red, green, blue).
    pub fn rows(&self) -> [[f32; 4]; 12] {
        let mut out = [[0.0; 4]; 12];
        for (i, r) in self
            .red
            .iter()
            .chain(&self.green)
            .chain(&self.blue)
            .enumerate()
        {
            out[i] = *r;
        }
        out
    }

    /// The irradiance (red, green, blue) for a unit normal.
    pub fn irradiance(&self, normal: [f32; 3]) -> [f32; 3] {
        let n = [normal[0], normal[1], normal[2], 1.0];
        let f = |m: &[[f32; 4]; 4]| {
            (0..4)
                .map(|i| n[i] * (0..4).map(|j| m[i][j] * n[j]).sum::<f32>())
                .sum::<f32>()
        };
        [f(&self.red), f(&self.green), f(&self.blue)]
    }

    /// A probe giving the same light `rgb` from every direction.
    pub fn uniform(rgb: [f32; 3]) -> Self {
        let m = |c: f32| {
            let mut m = [[0.0; 4]; 4];
            m[3][3] = c;
            m
        };
        Self {
            red: m(rgb[0]),
            green: m(rgb[1]),
            blue: m(rgb[2]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(constant: [f32; 3]) -> String {
        let mut s = String::new();
        for c in constant {
            for r in 0..4 {
                let row: Vec<String> = (0..4)
                    .map(|k| if r == 3 && k == 3 { c } else { 0.0 })
                    .map(|v| format!("{v:.8}"))
                    .collect();
                s.push_str(&row.join(" "));
                s.push_str(" \r\n");
            }
        }
        s.push_str("\r\n// The following is debugging data\r\nL{00}   1 2 3\r\n");
        s
    }

    #[test]
    fn reads_three_matrices_and_ignores_the_debugging_data() {
        let p = IrradianceProbe::parse(text([0.5, 0.25, 0.125]).as_bytes()).unwrap();
        assert_eq!(p, IrradianceProbe::uniform([0.5, 0.25, 0.125]));
        assert_eq!(p.irradiance([0.0, 0.0, 1.0]), [0.5, 0.25, 0.125]);
        assert_eq!(p.rows()[11], [0.0, 0.0, 0.0, 0.125]);
    }

    #[test]
    fn rejects_bad_input_without_panicking() {
        assert_eq!(IrradianceProbe::parse(b""), Err(ProbeError::Short(0)));
        assert!(matches!(
            IrradianceProbe::parse(b"1 2 3\r\n"),
            Err(ProbeError::Malformed { line: 1, .. })
        ));
        assert!(matches!(
            IrradianceProbe::parse(b"1 2 x 4\n"),
            Err(ProbeError::Malformed { line: 1, .. })
        ));
        assert!(matches!(
            IrradianceProbe::parse(b"1 2 inf 4\n"),
            Err(ProbeError::Malformed { .. })
        ));
        assert_eq!(
            IrradianceProbe::parse(b"1 2 3 4\n// rest\n"),
            Err(ProbeError::Short(1))
        );
        // Bytes that are not text read as replacement characters, which do not parse.
        assert!(IrradianceProbe::parse(&[0xff, 0xfe, 0x00]).is_err());
    }
}
