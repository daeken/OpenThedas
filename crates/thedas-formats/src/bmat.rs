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

/// How a pass of a compiled material is named where a state file's name goes in a
/// [`crate::material::Pass`] made from it ([`Bmat::definition`]): the material's resource,
/// then the semantic's, the technique's and the pass's places in it.
pub fn pass_reference(resource: &str, semantic: usize, technique: usize, pass: usize) -> String {
    format!(
        "{}#{semantic}#{technique}#{pass}",
        resource.to_ascii_lowercase()
    )
}

/// Takes a [`pass_reference`] apart: the material's resource and the three places.
pub fn parse_pass_reference(reference: &str) -> Option<(&str, usize, usize, usize)> {
    let mut parts = reference.split('#');
    let resource = parts.next().filter(|r| !r.is_empty())?;
    let mut place = || parts.next()?.parse().ok();
    let (s, t, p) = (place()?, place()?, place()?);
    parts.next().is_none().then_some((resource, s, t, p))
}

impl Technique {
    /// Whether this draws the picture itself: its second header word is 0, where the depth,
    /// shadow, reflection map and motion vector techniques have 1 to 4. Provisional, from
    /// the shipped files' technique names.
    pub fn is_main(&self) -> bool {
        self.header[1] == 0
    }
}

impl Semantic {
    /// The technique drawn here: of the main ones ([`Technique::is_main`]), the one for
    /// high detail with baked per-pixel lighting when there is one (first header word 1,
    /// third 1: `BakedLightingPerPixelHighLOD…`), else the one with the highest first
    /// word, the earliest among equals; any technique when none is a main one.
    /// Provisional: how the game picks by detail level and lighting mode is not known.
    pub fn technique(&self) -> Option<(usize, &Technique)> {
        let main = || {
            self.techniques
                .iter()
                .enumerate()
                .filter(|(_, t)| t.is_main())
        };
        main()
            .find(|(_, t)| t.header[0] == 1 && t.header[2] == 1)
            .or_else(|| main().max_by_key(|(i, t)| (t.header[0], std::cmp::Reverse(*i))))
            .or_else(|| self.techniques.iter().enumerate().next())
    }

    /// The main technique at high detail for a piece whose lighting is baked per vertex
    /// (third header word 2: `BakedLightingPerVertexHighLOD…`), if the semantic has one.
    /// Provisional, as [`Semantic::technique`].
    pub fn per_vertex_technique(&self) -> Option<(usize, &Technique)> {
        self.techniques
            .iter()
            .enumerate()
            .find(|(_, t)| t.is_main() && t.header[0] == 1 && t.header[2] == 2)
    }
}

/// What [`Bmat::definition`] appends to a semantic's name for its second form: the one drawn
/// with [`Semantic::per_vertex_technique`].
pub const PER_VERTEX: &str = "#PerVertex";

/// Dragon Age II's numbers for render states and their values, as the names Dragon Age:
/// Origins' state files use ([`crate::state`]). Provisional throughout: worked out from
/// which semantics of the shipped materials set what (blended ones set 11 and the factors 5
/// and 6, punch-through ones 4, 9 and 10, the reflection map techniques flip 7, and so on),
/// not from the executable. Values count on from the D3D9 enumerations at an offset each.
fn render_state(state: u32, value: u32) -> Option<(&'static str, String)> {
    let flag = |v: u32| if v != 0 { "TRUE" } else { "FALSE" }.to_string();
    let compare = |v: u32| {
        Some(
            match v {
                0x1a => "CMP_NEVER",
                0x1b => "CMP_LESS",
                0x1c => "CMP_EQUAL",
                0x1d => "CMP_LESSEQUAL",
                0x1e => "CMP_GREATER",
                0x1f => "CMP_NOTEQUAL",
                0x20 => "CMP_GREATEREQUAL",
                0x21 => "CMP_ALWAYS",
                _ => return None,
            }
            .to_string(),
        )
    };
    let factor = |v: u32| {
        Some(
            match v {
                8 => "BLEND_ZERO",
                9 => "BLEND_ONE",
                10 => "BLEND_SRCCOLOR",
                11 => "BLEND_INVSRCCOLOR",
                12 => "BLEND_SRCALPHA",
                13 => "BLEND_INVSRCALPHA",
                14 => "BLEND_DESTALPHA",
                15 => "BLEND_INVDESTALPHA",
                16 => "BLEND_DESTCOLOR",
                17 => "BLEND_INVDESTCOLOR",
                _ => return None,
            }
            .to_string(),
        )
    };
    let operation = |v: u32| {
        Some(
            match v {
                0x2e => "BLENDOP_ADD",
                0x2f => "BLENDOP_SUBTRACT",
                0x30 => "BLENDOP_REVSUBTRACT",
                0x31 => "BLENDOP_MIN",
                0x32 => "BLENDOP_MAX",
                _ => return None,
            }
            .to_string(),
        )
    };
    Some(match state {
        0 => ("DS_ZENABLE", flag(value)),
        3 => ("DS_ZWRITEENABLE", flag(value)),
        4 => ("DS_ALPHATESTENABLE", flag(value)),
        5 => ("DS_SRCBLEND", factor(value)?),
        6 => ("DS_DESTBLEND", factor(value)?),
        7 => (
            "DS_CULLMODE",
            match value {
                0x17 => "CULL_NONE",
                0x18 => "CULL_CW",
                0x19 => "CULL_CCW",
                _ => return None,
            }
            .to_string(),
        ),
        8 => ("DS_ZFUNC", compare(value)?),
        9 => ("DS_ALPHAREF", value.to_string()),
        10 => ("DS_ALPHAFUNC", compare(value)?),
        11 => ("DS_ALPHABLENDENABLE", flag(value)),
        32 => ("DS_COLORWRITEENABLE", value.to_string()),
        33 => ("DS_BLENDOP", operation(value)?),
        47 => ("DS_SEPARATEALPHABLENDENABLE", flag(value)),
        48 => ("DS_SRCBLENDALPHA", factor(value)?),
        49 => ("DS_DESTBLENDALPHA", factor(value)?),
        50 => ("DS_BLENDOPALPHA", operation(value)?),
        // Fill mode (1), stencil (19, 22 to 24, 36, 39, 40), depth bias (35, 46) and 45 are
        // not carried: nothing here draws with them.
        _ => return None,
    })
}

/// A sampler state and its value likewise (see [`render_state`]): 0 to 2 the address
/// modes, 3 to 5 the filters, 9 whether the texture is read as sRGB.
fn sampler_state(state: u32, value: u32) -> Option<(&'static str, &'static str)> {
    let address = |v: u32| {
        Some(match v {
            0 => "TEXADD_WRAP",
            1 => "TEXADD_MIRROR",
            2 => "TEXADD_CLAMP",
            3 => "TEXADD_BORDER",
            _ => return None,
        })
    };
    let filter = |v: u32| {
        Some(match v {
            5 => "TEXFILT_NONE",
            6 => "TEXFILT_POINT",
            7 => "TEXFILT_LINEAR",
            8 => "TEXFILT_ANISOTROPIC",
            _ => return None,
        })
    };
    Some(match state {
        0 => ("SAMP_ADDRESSU", address(value)?),
        1 => ("SAMP_ADDRESSV", address(value)?),
        2 => ("SAMP_ADDRESSW", address(value)?),
        3 => ("SAMP_MINFILTER", filter(value)?),
        4 => ("SAMP_MAGFILTER", filter(value)?),
        5 => ("SAMP_MIPFILTER", filter(value)?),
        9 => (
            "SAMP_SRGBTEXTURE",
            if value != 0 { "TRUE" } else { "FALSE" },
        ),
        _ => return None,
    })
}

impl Pass {
    /// The pass's render and sampler states as a state file's ([`crate::state`]), `name`
    /// naming it. Where the pass is silent the engine's defaults are written out first:
    /// clockwise culling (the reflection map techniques, drawn mirrored, set
    /// counter-clockwise), and for every sampler linear filtering with mip maps (the
    /// passes that set filters at all set them to point, for screen passes). Blending
    /// that is turned on without factors blends by the source's alpha. Provisional (see
    /// [`render_state`]): a sampler is named by its register, as the shader has it.
    pub fn material_state(&self, name: &str) -> crate::state::MaterialState {
        use crate::state::{MaterialState, SamplerState};
        let mut device: Vec<(String, String)> = [
            ("DS_CULLMODE", "CULL_CW"),
            ("DS_SRCBLEND", "BLEND_SRCALPHA"),
            ("DS_DESTBLEND", "BLEND_INVSRCALPHA"),
        ]
        .iter()
        .map(|(s, v)| (s.to_string(), v.to_string()))
        .collect();
        device.extend(
            self.render_states
                .iter()
                .filter_map(|&(s, v)| render_state(s, v))
                .map(|(s, v)| (s.to_string(), v)),
        );
        let stages: std::collections::BTreeSet<u32> = (0..16).collect();
        let mut samplers = Vec::new();
        for stage in stages {
            for (state, value) in [
                ("SAMP_MINFILTER", "TEXFILT_LINEAR"),
                ("SAMP_MAGFILTER", "TEXFILT_LINEAR"),
                ("SAMP_MIPFILTER", "TEXFILT_LINEAR"),
            ] {
                samplers.push(SamplerState {
                    sampler: String::new(),
                    stage: Some(stage),
                    state: state.to_string(),
                    value: value.to_string(),
                });
            }
        }
        for &(stage, state, value) in &self.sampler_states {
            if let Some((state, value)) = sampler_state(state, value) {
                samplers.push(SamplerState {
                    sampler: String::new(),
                    stage: Some(stage),
                    state: state.to_string(),
                    value: value.to_string(),
                });
            }
        }
        MaterialState {
            name: name.to_string(),
            device,
            samplers,
        }
    }
}

/// The usage (as Dragon Age: Origins' material definitions spell them, with a few of this
/// reader's own for values that game does not have) of an engine parameter's source number.
///
/// Provisional: the numbers' meanings are taken from the constants' names across every
/// material (source 7 is always `aur_mWorldViewProj` or the sky's, 16 always
/// `g_vLightParams`, and so on), not from the executable. A matrix's usage ends in `Trans`
/// where the shaders given that source read it by columns (`mad` by the vertex's
/// components, as Origins' `…Trans` usages are read: sources 3 and 7, the static and
/// character shaders' world matrices), and is plain where they read it by rows (`dp4`: 2
/// and 6, the water's and effects' forms of the same two, and the view, projection and
/// inverse matrices).
pub fn engine_usage(source: u32) -> Option<&'static str> {
    Some(match source {
        2 => "Part",
        3 => "PartTrans",
        6 => "ModelViewProjection",
        7 => "ModelViewProjectionTrans",
        8 => "ModelView",
        10 => "ModelViewInv",
        16 => "LightsArray",
        20 => "ShaderControlBooleans",
        21 => "ShadowPartitionMatrix",
        22 => "ShadowTexture_Primary",
        24 => "ShadowData",
        25 => "EnvironmentMap",
        31 => "Projection",
        34 => "View",
        36 => "ViewInv",
        38 => "ViewProjection",
        39 => "CameraPosition",
        41 => "ApplicationTime",
        46 => "FogParameters",
        51 => "DebugControlBooleans",
        _ => return None,
    })
}

impl Bmat {
    /// The pass a [`pass_reference`]'s places name.
    pub fn pass_at(&self, semantic: usize, technique: usize, pass: usize) -> Option<&Pass> {
        self.semantics
            .get(semantic)?
            .techniques
            .get(technique)?
            .passes
            .get(pass)
    }

    /// This material as a definition of the kind Dragon Age: Origins' `.mat.xml` gives
    /// ([`crate::material::MaterialDef`]), for what draws by those: each semantic with the
    /// one technique drawn here ([`Semantic::technique`]), and after them a second form,
    /// named with [`PER_VERTEX`] after the semantic's name, of each semantic that has a
    /// technique for lighting baked per vertex; each pass with its shaders
    /// (without the `.fxo`, as the XML names them), its parameters (the engine's as
    /// internal ones, textures by their samplers, the others with their defaults), and in
    /// place of a state file's name a [`pass_reference`] into `resource` (this material's
    /// resource name, `static.mat`), by which [`Pass::material_state`] is found again.
    pub fn definition(&self, resource: &str) -> crate::material::MaterialDef {
        use crate::material as m;
        let shader = |name: &str| {
            let name = name.strip_suffix(".fxo").unwrap_or(name);
            (!name.is_empty()).then(|| name.to_string())
        };
        let semantic =
            |si: usize, s: &Semantic, name: String, picked: Option<(usize, &Technique)>| {
                let techniques = picked
                    .map(|(ti, t)| m::Technique {
                        name: Some(t.name.clone()),
                        passes: t
                            .passes
                            .iter()
                            .enumerate()
                            .map(|(pi, p)| m::Pass {
                                name: Some(p.name.clone()),
                                vertex_shader: shader(&p.vertex_shader),
                                pixel_shader: shader(&p.pixel_shader),
                                state: Some(pass_reference(resource, si, ti, pi)),
                                parameter_sets: Vec::new(),
                                parameters: p
                                    .engine_parameters
                                    .iter()
                                    .map(|e| m::Parameter {
                                        internal: true,
                                        ty: String::new(),
                                        name: e.name.clone(),
                                        sampler: (!e.sampler.is_empty()).then(|| e.sampler.clone()),
                                        stage: None,
                                        usage: engine_usage(e.source).map(str::to_string),
                                        default: None,
                                    })
                                    .chain(p.parameters.iter().map(|q| m::Parameter {
                                        internal: false,
                                        ty: if q.sampler.is_empty() {
                                            String::new()
                                        } else {
                                            "Texture".to_string()
                                        },
                                        name: q.name.clone(),
                                        sampler: (!q.sampler.is_empty()).then(|| q.sampler.clone()),
                                        stage: None,
                                        usage: None,
                                        default: (!q.default.is_empty()).then(|| q.default.clone()),
                                    }))
                                    .collect(),
                                streams: Vec::new(),
                            })
                            .collect(),
                    })
                    .into_iter()
                    .collect();
                m::Semantic {
                    name,
                    render_class: (!s.render_class.trim().is_empty())
                        .then(|| s.render_class.clone()),
                    logic: vec![m::Logic {
                        note: None,
                        test: None,
                        techniques,
                        children: Vec::new(),
                    }],
                }
            };
        let mut semantics = Vec::new();
        for (si, s) in self.semantics.iter().enumerate() {
            semantics.push(semantic(si, s, s.name.clone(), s.technique()));
        }
        // After them, so that the first semantic stays the material's first.
        for (si, s) in self.semantics.iter().enumerate() {
            if let Some(picked) = s.per_vertex_technique() {
                semantics.push(semantic(
                    si,
                    s,
                    format!("{}{PER_VERTEX}", s.name),
                    Some(picked),
                ));
            }
        }
        m::MaterialDef {
            name: resource.to_string(),
            parameter_sets: Vec::new(),
            semantics,
        }
    }
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
    fn becomes_a_material_definition() {
        let m = Bmat::parse(&sample()).unwrap();
        let def = m.definition("Sky.mat");
        assert_eq!(def.name, "Sky.mat");
        let sem = &def.semantics[0];
        assert_eq!(sem.name, "Sky2");
        assert_eq!(sem.render_class.as_deref(), Some("sky"));
        // The only technique is not a main one (its second word is 2) and is still drawn.
        let pass = &sem.logic[0].techniques[0].passes[0];
        assert_eq!(pass.vertex_shader.as_deref(), Some("sky_sky2_p0.vsh"));
        assert_eq!(pass.pixel_shader.as_deref(), Some("sky_sky2_p0.psh"));
        assert_eq!(pass.state.as_deref(), Some("sky.mat#0#0#0"));
        assert_eq!(
            parse_pass_reference(pass.state.as_deref().unwrap()),
            Some(("sky.mat", 0, 0, 0))
        );
        let p = &pass.parameters;
        assert_eq!(p[0].name, "aur_mSkyViewProjection");
        assert!(p[0].internal);
        assert_eq!(p[0].usage.as_deref(), Some("ModelViewProjectionTrans"));
        assert_eq!(p[1].name, "vSunColor");
        assert_eq!(p[1].default.as_deref(), Some("1 1 1 5"));
        assert_eq!(p[2].ty, "Texture");
        assert_eq!(p[2].sampler.as_deref(), Some("smpSkyTextureA"));
        assert_eq!(p[2].default.as_deref(), Some("sb03_bcloud"));

        // Its states read as a state file's: culling off (7 = 23), and sampler 0 linear
        // (the default written for every stage) and not sRGB (9 = 0).
        let state = m.pass_at(0, 0, 0).unwrap().material_state("sky.mat#0#0#0");
        assert_eq!(state.device_state("DS_CULLMODE"), Some("CULL_NONE"));
        let pass_state = state.pass().unwrap();
        assert_eq!(pass_state.cull, crate::state::Cull::None);
        assert!(pass_state.blend.is_none());
        let sampler = state.sampler_at(None, Some(0)).unwrap();
        assert_eq!(sampler.min, crate::state::Filter::Linear);
        assert!(!sampler.srgb);
        assert!(m.pass_at(0, 0, 1).is_none());
    }

    #[test]
    fn picks_the_technique_to_draw() {
        let technique = |name: &str, header: [u32; 6]| Technique {
            name: name.to_string(),
            header,
            passes: Vec::new(),
        };
        let semantic = |techniques: Vec<Technique>| Semantic {
            name: "Default".into(),
            render_class: "Opaque".into(),
            techniques,
        };
        // Baked per-pixel lighting at high detail wins over what comes before it.
        let s = semantic(vec![
            technique("DepthWrite", [1, 1, 0, 0, 1, 0]),
            technique("VertexLit", [1, 0, 2, 0, 1, 0]),
            technique("Baked", [1, 0, 1, 2, 1, 0]),
            technique("BakedLow", [0, 0, 1, 2, 1, 0]),
        ]);
        assert_eq!(
            s.technique().map(|(i, t)| (i, t.name.as_str())),
            Some((2, "Baked"))
        );
        // Without one, the highest detail of the main techniques, the earliest among equals.
        let s = semantic(vec![
            technique("ShadowPass", [1, 2, 0, 0, 1, 0]),
            technique("Low", [0, 0, 3, 0, 1, 0]),
            technique("High", [1, 0, 3, 0, 1, 0]),
            technique("HighToo", [1, 0, 2, 0, 1, 0]),
        ]);
        assert_eq!(s.technique().map(|(i, _)| i), Some(2));
        assert_eq!(s.per_vertex_technique().map(|(i, _)| i), Some(3));
        assert!(semantic(Vec::new()).technique().is_none());
        assert!(semantic(Vec::new()).per_vertex_technique().is_none());
        // The definition gives the per-vertex technique as a second form of the semantic.
        let def = Bmat { semantics: vec![s] }.definition("static.mat");
        let names: Vec<&str> = def.semantics.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Default", "Default#PerVertex"]);
        assert_eq!(
            def.semantics[1].logic[0].techniques[0].name.as_deref(),
            Some("HighToo")
        );
        assert_eq!(engine_usage(6), Some("ModelViewProjection"));
        assert_eq!(engine_usage(7), Some("ModelViewProjectionTrans"));
        assert_eq!(engine_usage(51), Some("DebugControlBooleans"));
        assert_eq!(engine_usage(1000), None);
        assert_eq!(parse_pass_reference("static.mat#1#2"), None);
        assert_eq!(parse_pass_reference("static.mat#1#2#3#4"), None);
        assert_eq!(parse_pass_reference("static.xml"), None);
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
