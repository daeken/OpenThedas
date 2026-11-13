//! Dragon Age II's compiled material definitions (`<name>.mat`, signature `bmat`).
//!
//! Dragon Age: Origins describes a material in XML (`.mat.xml`, with render states in
//! `.sta.xml`; see [`crate::material`]). Dragon Age II ships the same information compiled into
//! one binary file per material in `materialdefinitions.erf`: its semantics, each semantic's
//! techniques, and for each pass the two shaders, the parameters the engine fills in, the
//! parameters a material object (`.mao`) sets with their defaults, and the render and sampler
//! states.
//!
//! Layout, all little-endian, strings as a `u32` length and that many bytes:
//!
//! ```text
//! "bmat", u32 semantic count
//!   semantic:  name, render class ("Opaque,Weapon"), u32 technique count
//!   technique: name, 6 x u32, u32 pass count
//!   pass:      name, u32, vertex shader, pixel shader,
//!              u32 n, n x (name, sampler, u32 source, u32 type, i32),
//!              u32 n, n x (name, sampler, default, u32 type, u32 count, i32),
//!              u32 n, n x (u32 state, u32 value),
//!              u32 n, n x (u32 sampler, u32 state, u32 value)
//! ```
//!
//! Provisional throughout: the layout is read from the shipped files (all 19 DX9 `.mat` files
//! in the PC release parse to their last byte), not from the executable, and the meaning of
//! the numbers (a technique's six header words, a pass's flag word, parameter sources and
//! types, state ids) is not worked out, so they are kept as stored. The `.mat.dx11` variant of
//! these files has a different pass layout and is not read here.

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum BmatError {
    #[error("not a compiled material definition (no bmat signature)")]
    BadSignature,
    #[error("truncated at offset {0:#x}")]
    Truncated(usize),
    #[error("{count} entries at offset {offset:#x} cannot fit in the file")]
    BadCount { offset: usize, count: u32 },
    #[error("{0} bytes left over after the last semantic")]
    Trailing(usize),
}

pub type Result<T> = std::result::Result<T, BmatError>;

/// A parameter the engine supplies (matrices, light and fog values, bound textures).
#[derive(Clone, Debug, PartialEq)]
pub struct EngineParameter {
    /// The shader constant, e.g. `aur_mWorldViewProj`.
    pub name: String,
    /// The sampler, for texture parameters (`smpReflect`); empty otherwise.
    pub sampler: String,
    /// Which engine value this is, as stored.
    pub source: u32,
    /// The value's type, as stored.
    pub ty: u32,
    /// Stored as -1 in every shipped file.
    pub index: i32,
}

/// A parameter a material object sets, with the default used when it does not.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialParameter {
    /// The parameter, e.g. `mml_tDiffuse` or `vSunColor`.
    pub name: String,
    /// The sampler, for texture parameters; empty otherwise.
    pub sampler: String,
    /// The default: numbers separated by spaces (`1 1 -0.4 0`), or a texture name.
    pub default: String,
    /// The value's type, as stored.
    pub ty: u32,
    /// The element count, as stored.
    pub count: u32,
    /// Stored as -1 in nearly every shipped file.
    pub index: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pass {
    pub name: String,
    /// A word after the name: 0 in most passes, 2 or 4 in a few. Meaning unknown.
    pub flags: u32,
    /// The vertex shader resource, e.g. `sky_sky2_p0.vsh.fxo`.
    pub vertex_shader: String,
    /// The pixel shader resource, e.g. `sky_sky2_p0.psh.fxo`.
    pub pixel_shader: String,
    pub engine_parameters: Vec<EngineParameter>,
    pub parameters: Vec<MaterialParameter>,
    /// `(state, value)`, as stored.
    pub render_states: Vec<(u32, u32)>,
    /// `(sampler, state, value)`, as stored.
    pub sampler_states: Vec<(u32, u32, u32)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Technique {
    /// E.g. `DefaultTechnique`, `ShadowPass`, `RenderToReflectionMap`.
    pub name: String,
    /// Six words after the name, as stored. They differ between techniques of one name, so
    /// they probably select a technique by detail level and shader model. Meaning unknown.
    pub header: [u32; 6],
    pub passes: Vec<Pass>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Semantic {
    /// The name a material object's default semantic refers to, e.g. `Default` or `Sky2`.
    pub name: String,
    /// Comma-separated render classes, e.g. `Transparent, Water`.
    pub render_class: String,
    pub techniques: Vec<Technique>,
}

/// A compiled material definition.
#[derive(Clone, Debug, PartialEq)]
pub struct Bmat {
    pub semantics: Vec<Semantic>,
}

/// Whether the data starts with the `bmat` signature.
pub fn is_bmat(data: &[u8]) -> bool {
    data.starts_with(b"bmat")
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&end| end <= self.data.len())
            .ok_or(BmatError::Truncated(self.at))?;
        let bytes = &self.data[self.at..end];
        self.at = end;
        Ok(bytes)
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    fn string(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        // The shipped strings are ASCII; decode as Latin-1 so no byte is lost.
        Ok(self.take(n)?.iter().map(|&b| b as char).collect())
    }

    /// A list's entry count, refused when even `min_size` bytes per entry cannot fit.
    fn count(&mut self, min_size: usize) -> Result<usize> {
        let offset = self.at;
        let count = self.u32()?;
        let left = self.data.len() - self.at;
        if count as usize > left / min_size {
            return Err(BmatError::BadCount { offset, count });
        }
        Ok(count as usize)
    }
}

impl Bmat {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if !is_bmat(data) {
            return Err(BmatError::BadSignature);
        }
        let mut r = Reader { data, at: 4 };
        let mut semantics = Vec::new();
        for _ in 0..r.count(12)? {
            let name = r.string()?;
            let render_class = r.string()?;
            let mut techniques = Vec::new();
            for _ in 0..r.count(32)? {
                let name = r.string()?;
                let mut header = [0u32; 6];
                for h in &mut header {
                    *h = r.u32()?;
                }
                let mut passes = Vec::new();
                for _ in 0..r.count(32)? {
                    passes.push(pass(&mut r)?);
                }
                techniques.push(Technique {
                    name,
                    header,
                    passes,
                });
            }
            semantics.push(Semantic {
                name,
                render_class,
                techniques,
            });
        }
        if r.at != data.len() {
            return Err(BmatError::Trailing(data.len() - r.at));
        }
        Ok(Bmat { semantics })
    }

    /// The semantic of this name, compared without case.
    pub fn semantic(&self, name: &str) -> Option<&Semantic> {
        self.semantics
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    }
}

fn pass(r: &mut Reader) -> Result<Pass> {
    let name = r.string()?;
    let flags = r.u32()?;
    let vertex_shader = r.string()?;
    let pixel_shader = r.string()?;
    let mut engine_parameters = Vec::new();
    for _ in 0..r.count(20)? {
        engine_parameters.push(EngineParameter {
            name: r.string()?,
            sampler: r.string()?,
            source: r.u32()?,
            ty: r.u32()?,
            index: r.i32()?,
        });
    }
    let mut parameters = Vec::new();
    for _ in 0..r.count(24)? {
        parameters.push(MaterialParameter {
            name: r.string()?,
            sampler: r.string()?,
            default: r.string()?,
            ty: r.u32()?,
            count: r.u32()?,
            index: r.i32()?,
        });
    }
    let mut render_states = Vec::new();
    for _ in 0..r.count(8)? {
        render_states.push((r.u32()?, r.u32()?));
    }
    let mut sampler_states = Vec::new();
    for _ in 0..r.count(12)? {
        sampler_states.push((r.u32()?, r.u32()?, r.u32()?));
    }
    Ok(Pass {
        name,
        flags,
        vertex_shader,
        pixel_shader,
        engine_parameters,
        parameters,
        render_states,
        sampler_states,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(out: &mut Vec<u8>, text: &str) {
        out.extend((text.len() as u32).to_le_bytes());
        out.extend(text.as_bytes());
    }

    fn w(out: &mut Vec<u8>, words: &[u32]) {
        for v in words {
            out.extend(v.to_le_bytes());
        }
    }

    /// One semantic, one technique, one pass with one entry in each list.
    fn sample() -> Vec<u8> {
        let mut b = b"bmat".to_vec();
        w(&mut b, &[1]);
        s(&mut b, "Sky2");
        s(&mut b, "sky");
        w(&mut b, &[1]);
        s(&mut b, "DefaultTechnique");
        w(&mut b, &[0, 2, 5, 0, 1, 0, 1]);
        s(&mut b, "Pass 1");
        w(&mut b, &[0]);
        s(&mut b, "sky_sky2_p0.vsh.fxo");
        s(&mut b, "sky_sky2_p0.psh.fxo");
        w(&mut b, &[1]);
        s(&mut b, "aur_mSkyViewProjection");
        s(&mut b, "");
        w(&mut b, &[7, 5, u32::MAX]);
        w(&mut b, &[2]);
        s(&mut b, "vSunColor");
        s(&mut b, "");
        s(&mut b, "1 1 1 5");
        w(&mut b, &[2, 1, u32::MAX]);
        s(&mut b, "Cloud1");
        s(&mut b, "smpSkyTextureA");
        s(&mut b, "sb03_bcloud");
        w(&mut b, &[0, 1, u32::MAX]);
        w(&mut b, &[1, 7, 23]);
        w(&mut b, &[1, 0, 9, 0]);
        b
    }

    #[test]
    fn reads_a_compiled_material() {
        let m = Bmat::parse(&sample()).unwrap();
        let sem = m.semantic("sky2").unwrap();
        assert_eq!(sem.render_class, "sky");
        let t = &sem.techniques[0];
        assert_eq!(t.name, "DefaultTechnique");
        assert_eq!(t.header, [0, 2, 5, 0, 1, 0]);
        let p = &t.passes[0];
        assert_eq!(p.name, "Pass 1");
        assert_eq!(p.vertex_shader, "sky_sky2_p0.vsh.fxo");
        assert_eq!(p.pixel_shader, "sky_sky2_p0.psh.fxo");
        assert_eq!(
            p.engine_parameters,
            vec![EngineParameter {
                name: "aur_mSkyViewProjection".into(),
                sampler: String::new(),
                source: 7,
                ty: 5,
                index: -1,
            }]
        );
        assert_eq!(p.parameters.len(), 2);
        assert_eq!(p.parameters[0].default, "1 1 1 5");
        assert_eq!(p.parameters[1].sampler, "smpSkyTextureA");
        assert_eq!(p.parameters[1].default, "sb03_bcloud");
        assert_eq!(p.render_states, vec![(7, 23)]);
        assert_eq!(p.sampler_states, vec![(0, 9, 0)]);
    }

    #[test]
    fn errors_instead_of_panicking() {
        let good = sample();
        assert_eq!(Bmat::parse(b"GFF V4.0"), Err(BmatError::BadSignature));
        // Every truncation is an error, never a panic.
        for n in 4..good.len() {
            assert!(Bmat::parse(&good[..n]).is_err(), "cut at {n}");
        }
        // Bytes after the last semantic.
        let mut long = good.clone();
        long.push(0);
        assert_eq!(Bmat::parse(&long), Err(BmatError::Trailing(1)));
        // A count that cannot fit.
        let mut huge = b"bmat".to_vec();
        w(&mut huge, &[u32::MAX]);
        assert!(matches!(
            Bmat::parse(&huge),
            Err(BmatError::BadCount { .. })
        ));
    }
}
