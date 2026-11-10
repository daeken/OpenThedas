//! Drawing materials with their original D3D9 shaders.
//!
//! [`D3dSource`] turns a material's selected technique (vertex shader, pixel shader, render
//! state) into a [`D3dProgram`]: both shaders translated to validated naga IR by
//! `thedas-shader`, linked so the vertex shader writes everything the pixel shader reads, with
//! the fixed-function state from the `.sta` file.
//!
//! [`ConstantContext`] fills the constant registers by name using each shader's constant
//! table. Names resolve in this order:
//!
//! 1. engine values, recognised by the `Usage` the material definition gives the parameter
//!    (`ModelViewProjectionTrans`, `PartTrans`, `LightsArray`, ...);
//! 2. the lightmap atlas region for `mml_vLightmapOffsetAndScale`;
//! 3. material object values (`.mao` `Float` / `Vector4f` / `Vector4fArray`);
//! 4. the `Default` of the parameter in the material definition;
//! 5. zero.
//!
//! Provisional, inferred by reading Static1's bytecode rather than verified against the
//! original executable:
//!
//! - Matrices of class `MatrixRows` occupy one register per column of our column-major
//!   matrices (the shader computes `x*c[n] + y*c[n+1] + z*c[n+2] + c[n+3]`); `MatrixColumns`
//!   one register per row.
//! - `g_vLightParams` layout: `c0` point light A position and radius, `c1` its colour, `c2`
//!   directional light direction, `c3` its colour, `c4` ambient colour, `c5`/`c6` point light B,
//!   `c7`/`c8` point light C, `c9`/`c11` light D (position/radius, colour). Light A is always
//!   evaluated; `b0`..`b3` of `g_bShaderControlBooleans` enable D, B, C and the directional
//!   light. Lights A-C and the directional light are multiplied by the lightmap alpha; D is not.
//! - Fog and the sky's scattering come from the level's atmosphere ([`crate::atmosphere`]).
//!   Shadows, the environment cubemap and the depth-of-field output are disabled (no shadow
//!   maps, black cubemap, `oC1` dropped).

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Mat4, Vec3, Vec4};
use thedas_formats::material::{Parameter, Technique};
use thedas_formats::probe::IrradianceProbe;
use thedas_formats::state::{CompareFunc, MaterialState, PassState};
use thedas_shader::bytecode::{Comparison, TextureType};
use thedas_shader::ctab::{Constant, ConstantTable, ParamClass, RegisterSet};
use thedas_shader::translate::{self, Options, Semantic, Translation};
use thedas_shader::{Shader, Stage};
use thedas_vfs::Vfs;

use crate::material::{Program, ProgramSource};
use crate::scene::{AreaScene, LightmapRef, SceneLight, SceneMaterial};

/// A material drawn with its original shaders.
pub struct D3dProgram {
    /// `<vs>+<ps>+<state>`, lowercase; also the pipeline cache key.
    pub name: String,
    pub vs: Translation,
    pub ps: Translation,
    pub pass: PassState,
    pub state: MaterialState,
}

impl std::fmt::Debug for D3dProgram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("D3dProgram")
            .field("name", &self.name)
            .field("pass", &self.pass)
            .finish()
    }
}

impl PartialEq for D3dProgram {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

/// Which shader pairs [`D3dSource`] takes over. `None` takes every material whose technique
/// translates; a pixel shader of `*` matches any.
pub type ShaderFilter = Option<Vec<(String, String)>>;

/// Builds [`D3dProgram`]s for the materials of one scene. Shaders and states are read from the
/// resource stack when the source is created, so the source itself owns everything it needs.
pub struct D3dSource {
    programs: HashMap<String, Option<Arc<D3dProgram>>>,
    pub failures: Vec<String>,
}

fn comparison(f: CompareFunc) -> Option<Comparison> {
    Some(match f {
        CompareFunc::Greater => Comparison::Gt,
        CompareFunc::Equal => Comparison::Eq,
        CompareFunc::GreaterEqual => Comparison::Ge,
        CompareFunc::Less => Comparison::Lt,
        CompareFunc::NotEqual => Comparison::Ne,
        CompareFunc::LessEqual => Comparison::Le,
        CompareFunc::Never | CompareFunc::Always => return None,
    })
}

fn read_shader(vfs: &Vfs, name: &str) -> Result<Shader, String> {
    let res = format!("{}.fxo", name.to_ascii_lowercase());
    let (data, _) = vfs.read(&res).map_err(|e| format!("{res}: {e}"))?;
    Shader::parse(&data).map_err(|e| format!("{res}: {e}"))
}

fn build(vfs: &Vfs, vs_name: &str, ps_name: &str, state_name: &str) -> Result<D3dProgram, String> {
    let state_res = format!("{}.xml", state_name.to_ascii_lowercase());
    let (data, _) = vfs
        .read(&state_res)
        .map_err(|e| format!("{state_res}: {e}"))?;
    let state = MaterialState::parse(&data).map_err(|e| format!("{state_res}: {e}"))?;
    let pass = state.pass().map_err(|e| format!("{state_res}: {e}"))?;
    let vs = read_shader(vfs, vs_name)?;
    let ps = read_shader(vfs, ps_name)?;
    if vs.stage != Stage::Vertex || ps.stage != Stage::Pixel {
        return Err(format!("{vs_name}/{ps_name}: wrong shader stages"));
    }
    let alpha_test = pass
        .alpha_test
        .and_then(|(f, r)| comparison(f).map(|c| (c, r as f32 / 255.0)));
    let ps_t = translate::translate(
        &ps,
        &Options {
            color_targets: Some(1),
            alpha_test,
            ..Default::default()
        },
    )
    .map_err(|e| format!("{ps_name}: {e}"))?;
    let extra: Vec<Semantic> = ps_t.interface.inputs.iter().map(|i| i.semantic).collect();
    let vs_t = translate::translate(
        &vs,
        &Options {
            extra_outputs: extra,
            ..Default::default()
        },
    )
    .map_err(|e| format!("{vs_name}: {e}"))?;
    Ok(D3dProgram {
        name: format!(
            "{}+{}+{}",
            vs_name.to_ascii_lowercase(),
            ps_name.to_ascii_lowercase(),
            state_name.to_ascii_lowercase()
        ),
        vs: vs_t,
        ps: ps_t,
        pass,
        state,
    })
}

impl D3dSource {
    /// Prepares programs for every material in the scene whose selected technique's passes
    /// all (if `filter` is set) use one of the listed vertex/pixel shader pairs.
    pub fn new(vfs: &Vfs, scene: &AreaScene, filter: &ShaderFilter) -> Self {
        Self::new_cached(vfs, scene, filter, &mut crate::RenderCache::default())
    }

    /// [`D3dSource::new`], reusing (and keeping) translations in `cache`.
    pub fn new_cached(
        vfs: &Vfs,
        scene: &AreaScene,
        filter: &ShaderFilter,
        cache: &mut crate::RenderCache,
    ) -> Self {
        let mut programs = HashMap::new();
        let mut failures = Vec::new();
        for m in &scene.materials {
            let Some(t) = &m.technique else { continue };
            let Some(passes) = pass_shaders(t) else {
                continue;
            };
            if let Some(f) = filter {
                let listed = |(vs, ps, _): &(&str, &str, &str)| {
                    f.iter().any(|(v, q)| {
                        v.eq_ignore_ascii_case(vs) && (q == "*" || q.eq_ignore_ascii_case(ps))
                    })
                };
                if !passes.iter().all(listed) {
                    continue;
                }
            }
            for (vs, ps, st) in passes {
                let key = format!("{vs}+{ps}+{st}").to_ascii_lowercase();
                if programs.contains_key(&key) {
                    continue;
                }
                if let Some(built) = cache.programs.get(&key) {
                    programs.insert(key, built.clone());
                    continue;
                }
                let built = match build(vfs, vs, ps, st) {
                    Ok(p) => Some(Arc::new(p)),
                    Err(e) => {
                        log::warn!("original shader unavailable: {e}");
                        failures.push(e);
                        None
                    }
                };
                cache.programs.insert(key.clone(), built.clone());
                programs.insert(key, built);
            }
        }
        Self { programs, failures }
    }

    pub fn programs(&self) -> impl Iterator<Item = &Arc<D3dProgram>> {
        self.programs.values().flatten()
    }
}

impl ProgramSource for D3dSource {
    fn program(&self, m: &SceneMaterial) -> Option<Program> {
        let passes = pass_shaders(m.technique.as_ref()?)?;
        let programs: Option<Vec<Arc<D3dProgram>>> = passes
            .iter()
            .map(|(vs, ps, st)| {
                let key = format!("{vs}+{ps}+{st}").to_ascii_lowercase();
                self.programs.get(&key).cloned().flatten()
            })
            .collect();
        programs.map(Program::D3d)
    }
}

/// Each pass's (vertex shader, pixel shader, state), if every pass names all three.
fn pass_shaders(t: &Technique) -> Option<Vec<(&str, &str, &str)>> {
    if t.passes.is_empty() {
        return None;
    }
    t.passes
        .iter()
        .map(|p| {
            Some((
                p.vertex_shader.as_deref()?,
                p.pixel_shader.as_deref()?,
                p.state.as_deref()?,
            ))
        })
        .collect()
}

/// Static1 and its variants plus `Black`: every shader the M1 area's geometry uses.
pub fn static1_filter() -> ShaderFilter {
    let mut v: Vec<(String, String)> = [
        "Static1.psh",
        "Static1_Alpha.psh",
        "Static1_Punch.psh",
        "Static1_Fast.psh",
    ]
    .iter()
    .map(|ps| ("Static1.vsh".to_string(), ps.to_string()))
    .collect();
    v.push(("Black.vsh".into(), "Black.psh".into()));
    Some(v)
}

/// [`static1_filter`] plus the vertex shaders that draw creatures and placeables:
/// characters (`Char1`), hair (`Hair0_Pass1`, `Hair0`: four passes), faces (`Face1`), eyes
/// and lashes (`Eye0`), cloth (`Cloth1`), props (`Prop1`) and weapons (`Weapon1`), with any
/// pixel shader; and exterior terrain (`Terrain2`, terrain.mat's pass above Low detail, and
/// `Terrain3`, its Low detail pass); and the sky's dome and clouds (`Sky`, `Clouds`).
pub fn object_filter() -> ShaderFilter {
    let mut v = static1_filter().unwrap_or_default();
    for vs in OBJECT_VERTEX_SHADERS {
        v.push((vs.to_string(), "*".to_string()));
    }
    Some(v)
}

/// See [`object_filter`].
pub const OBJECT_VERTEX_SHADERS: [&str; 12] = [
    "Char1.vsh",
    "Hair0_Pass1.vsh",
    "Hair0.vsh",
    "Face1.vsh",
    "Eye0.vsh",
    "Cloth1.vsh",
    "Prop1.vsh",
    "Weapon1.vsh",
    "Terrain2.vsh",
    "Terrain3.vsh",
    "Sky.vsh",
    "Clouds.vsh",
];

/// Point lights chosen for one draw, in slot order A, B, C.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrawLights {
    pub points: Vec<(Vec3, f32, Vec3)>,
}

/// Picks up to three point lights whose radius reaches the draw's bounding sphere and whose
/// affect domain includes `domain` (level geometry or characters), strongest first by colour
/// brightness times the shader's own attenuation at the sphere centre. The domain keeps the
/// rooms' character lights (`Char_sun`, `Char_bounce`, reaching 600 units) off the
/// lightmapped level. Provisional: the original's light selection is unknown beyond that.
pub fn pick_lights(lights: &[SceneLight], sphere: (Vec3, f32), domain: i64) -> DrawLights {
    let mut cands: Vec<(f32, Vec3, f32, Vec3)> = lights
        .iter()
        .filter_map(|l| match l {
            SceneLight::Point {
                position,
                color,
                radius,
                domain: lit,
                ..
            } => {
                if lit.is_some_and(|m| m & domain == 0) {
                    return None;
                }
                let d = position.distance(sphere.0);
                if d > radius + sphere.1 || *radius <= 0.0 {
                    return None;
                }
                let t = (d / radius).clamp(0.0, 1.0);
                let att = (t * std::f32::consts::FRAC_PI_2).cos().powi(2);
                let weight = (color.x + color.y + color.z) * att.max(1e-3);
                Some((weight, *position, *radius, *color))
            }
            SceneLight::Ambient { .. } => None,
        })
        .collect();
    cands.sort_by(|a, b| b.0.total_cmp(&a.0));
    DrawLights {
        points: cands
            .into_iter()
            .take(3)
            .map(|(_, p, r, c)| (p, r, c))
            .collect(),
    }
}

/// Everything needed to fill one draw's constants.
pub struct ConstantContext<'a> {
    pub view: Mat4,
    pub projection: Mat4,
    pub world: Mat4,
    pub camera: Vec3,
    pub material: &'a SceneMaterial,
    pub lightmap: Option<&'a LightmapRef>,
    pub lights: &'a DrawLights,
    pub ambient: Vec3,
    /// The skinning palette (`bsm_bones`): three rows of a 3x4 matrix per bone, in the
    /// mesh chunk's bone order. Empty means every bone at rest (identity), i.e. bind pose.
    pub bones: &'a [[f32; 4]],
    /// The irradiance probe lighting the draw (`g_mLightParams`), if the level has one.
    pub irradiance: Option<&'a IrradianceProbe>,
    /// The level's sun: the direction its light travels and its colour.
    pub sun: Option<(Vec3, Vec3)>,
    /// The level's fog registers (`FogParameters`, see [`crate::atmosphere`]).
    pub fog: [[f32; 4]; 2],
    /// The level's sky registers (`SkyParameters`), when it has an atmosphere.
    pub sky: Option<&'a [[f32; 4]; 9]>,
    /// The level's cloud registers (`CloudParameters`).
    pub clouds: [[f32; 4]; 2],
}

/// Light from every direction for draws in a level with no irradiance probe. Provisional.
pub const NO_PROBE_IRRADIANCE: [f32; 3] = [0.35, 0.35, 0.35];

/// The rows of an identity 3x4 bone matrix.
pub const IDENTITY_BONE: [[f32; 4]; 3] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

/// Register contents for one stage: floats, ints, bools.
pub struct StageConstants {
    pub floats: Vec<[f32; 4]>,
    pub ints: Vec<[i32; 4]>,
    pub bools: Vec<[u32; 4]>,
}

impl StageConstants {
    fn new() -> Self {
        Self {
            floats: vec![[0.0; 4]; translate::FLOAT_CONSTANTS as usize],
            ints: vec![[0; 4]; translate::INT_CONSTANTS as usize],
            bools: vec![[0; 4]; translate::BOOL_CONSTANTS as usize],
        }
    }
}

enum Value {
    Matrix(Mat4),
    Vectors(Vec<[f32; 4]>),
    Bools(Vec<bool>),
}

fn parse_floats(s: &str) -> Vec<f32> {
    s.split_whitespace()
        .filter_map(|x| x.parse().ok())
        .collect()
}

/// The parameters a material's draws see: those its technique's passes declare, then its
/// definition's parameter sets.
fn parameters(m: &SceneMaterial) -> impl Iterator<Item = &Parameter> {
    let passes = m
        .technique
        .iter()
        .flat_map(|t| t.passes.iter())
        .flat_map(|p| p.parameters.iter());
    let sets = m
        .definition
        .iter()
        .flat_map(|d| d.parameter_sets.iter())
        .flat_map(|s| s.parameters.iter());
    passes.chain(sets)
}

fn find_param<'a>(m: &'a SceneMaterial, name: &str) -> Option<&'a Parameter> {
    parameters(m).find(|p| p.name.eq_ignore_ascii_case(name))
}

impl ConstantContext<'_> {
    fn light_params(&self) -> Vec<[f32; 4]> {
        let mut r = vec![[0.0f32; 4]; 12];
        let slots = [0usize, 5, 7];
        for (i, (p, radius, c)) in self.lights.points.iter().enumerate() {
            let at = slots[i];
            r[at] = [p.x, p.y, p.z, *radius];
            r[at + 1] = [c.x, c.y, c.z, 1.0];
        }
        r[4] = [self.ambient.x, self.ambient.y, self.ambient.z, 1.0];
        // The sun takes registers 2 (direction) and 3 (colour), which the shaders read under
        // control boolean 3. Provisional, from the shaders' use of them.
        if let Some((d, c)) = self.sun {
            r[2] = [d.x, d.y, d.z, 0.0];
            r[3] = [c.x, c.y, c.z, 1.0];
        }
        r
    }

    /// Whether the material object or its definition gives `name` a value.
    fn material_value(&self, name: &str) -> Option<()> {
        let mao = &self.material.object;
        let set = mao.floats.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
            || mao
                .vectors
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case(name));
        let default = find_param(self.material, name)
            .and_then(|p| p.default.as_deref())
            .is_some_and(|d| !parse_floats(d).is_empty());
        (set || default).then_some(())
    }

    fn value(&self, c: &Constant) -> Option<Value> {
        let usage = find_param(self.material, &c.name).and_then(|p| p.usage.clone());
        let usage = usage.as_deref().unwrap_or("");
        let world_view = self.view * self.world;
        // Matrices: a usage ending in `Trans` (`ModelViewProjectionTrans`, `PartTrans`) is
        // the layout the shaders bound to it read as rows (`mad` by the vertex's
        // components); the plain usages (`ModelViewProjection`, `Part`, `ModelView`, `View`,
        // `ViewInv`) are its transpose, which their shaders read with `dp4` (Terrain2,
        // Static1's view depth). A matrix known only by name takes the `Trans` layout.
        // Provisional, from the shaders' code and the material definitions' usages.
        let matrix = match (usage, c.name.as_str()) {
            ("ModelViewProjectionTrans" | "ModelViewProjection", _) | (_, "aur_mWorldViewProj") => {
                Some(self.projection * world_view)
            }
            ("PartTrans" | "Part", _) | (_, "aur_mWorld") => Some(self.world),
            ("ModelView", _) | (_, "aur_mModelView") => Some(world_view),
            ("View", _) => Some(self.view),
            ("ViewInv", _) => Some(self.view.inverse()),
            ("Projection", _) => Some(self.projection),
            ("ViewProjection", _) => Some(self.projection * self.view),
            _ => None,
        };
        if let Some(m) = matrix {
            let plain = !usage.is_empty() && !usage.ends_with("Trans");
            return Some(Value::Matrix(if plain { m.transpose() } else { m }));
        }
        Some(match (usage, c.name.as_str()) {
            ("CameraPosition", _) | (_, "g_vCameraPosition") => {
                Value::Vectors(vec![[self.camera.x, self.camera.y, self.camera.z, 1.0]])
            }
            ("FocalPlane", _) => Value::Vectors(vec![[1.0, 0.0, 0.0, 0.0]]),
            ("LightsArray", _) | (_, "g_vLightParams") => Value::Vectors(self.light_params()),
            ("ShaderControlBooleans", _) | (_, "g_bShaderControlBooleans") => {
                let n = self.lights.points.len();
                Value::Bools(vec![false, n > 1, n > 2, self.sun.is_some()])
            }
            // Three 4x4 matrices, red, green and blue: the vertex shaders light a normal n
            // with n^T M n (see `thedas_formats::probe`).
            (_, "g_mLightParams") => Value::Vectors(
                self.irradiance
                    .cloned()
                    .unwrap_or_else(|| IrradianceProbe::uniform(NO_PROBE_IRRADIANCE))
                    .rows()
                    .to_vec(),
            ),
            // Provisional: rows of 3x4 bone matrices, as the character vertex shaders read
            // them (three `dp4`s per bone).
            (_, "bsm_bones") => {
                let n = c.count.max(1) as usize;
                let mut v: Vec<[f32; 4]> = self.bones.iter().take(n).copied().collect();
                while v.len() < n {
                    v.push(IDENTITY_BONE[v.len() % 3]);
                }
                Value::Vectors(v)
            }
            // The engine's fade: fully opaque unless the material says otherwise.
            (_, "mml_fOpacityMultiplier") if self.material_value(&c.name).is_none() => {
                Value::Vectors(vec![[1.0, 0.0, 0.0, 0.0]])
            }
            ("FogParameters", _) => Value::Vectors(self.fog.to_vec()),
            ("SkyParameters", _) => {
                Value::Vectors(self.sky.map(|s| s.to_vec()).unwrap_or_default())
            }
            ("ApplicationTime", _) => Value::Vectors(vec![[0.0; 4]]),
            ("CloudParameters", _) => Value::Vectors(self.clouds.to_vec()),
            ("ShadowData", _) | ("IrradianceParams", _) => Value::Vectors(Vec::new()),
            (_, "mml_vLightmapOffsetAndScale") if self.lightmap.is_some() => {
                let l = self.lightmap?;
                Value::Vectors(vec![[l.offset[0], l.offset[1], l.scale[0], l.scale[1]]])
            }
            _ => {
                let mao = &self.material.object;
                if let Some((_, v)) = mao
                    .floats
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(&c.name))
                {
                    Value::Vectors(vec![[*v, 0.0, 0.0, 0.0]])
                } else if let Some((_, v)) = mao
                    .vectors
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(&c.name))
                {
                    Value::Vectors(vec![*v])
                } else if let Some((_, v)) = mao
                    .vector_arrays
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(&c.name))
                {
                    Value::Vectors(
                        v.chunks(4)
                            .map(|q| {
                                let mut o = [0.0; 4];
                                o[..q.len()].copy_from_slice(q);
                                o
                            })
                            .collect(),
                    )
                } else {
                    // A default holds as many vectors as the parameter has elements.
                    let d =
                        find_param(self.material, &c.name).and_then(|p| p.default.as_deref())?;
                    let f = parse_floats(d);
                    if f.is_empty() {
                        return None;
                    }
                    Value::Vectors(
                        f.chunks(4)
                            .map(|q| {
                                let mut o = [0.0; 4];
                                o[..q.len()].copy_from_slice(q);
                                o
                            })
                            .collect(),
                    )
                }
            }
        })
    }

    /// Fills the registers a shader's constant table names.
    pub fn fill(&self, table: Option<&ConstantTable>) -> StageConstants {
        let mut out = StageConstants::new();
        let Some(table) = table else { return out };
        for c in &table.constants {
            let Some(v) = self.value(c) else { continue };
            let base = c.register as usize;
            let count = c.count.max(1) as usize;
            match (c.set, v) {
                (RegisterSet::Float4, Value::Matrix(m)) => {
                    let regs: [Vec4; 4] = if c.class == ParamClass::MatrixColumns {
                        [m.row(0), m.row(1), m.row(2), m.row(3)]
                    } else {
                        [m.x_axis, m.y_axis, m.z_axis, m.w_axis]
                    };
                    for (i, r) in regs.iter().enumerate().take(count) {
                        if let Some(slot) = out.floats.get_mut(base + i) {
                            *slot = r.to_array();
                        }
                    }
                }
                (RegisterSet::Float4, Value::Vectors(vs)) => {
                    for (i, v) in vs.iter().enumerate().take(count) {
                        if let Some(slot) = out.floats.get_mut(base + i) {
                            *slot = *v;
                        }
                    }
                }
                (RegisterSet::Bool, Value::Bools(bs)) => {
                    for (i, b) in bs.iter().enumerate().take(count) {
                        if let Some(slot) = out.bools.get_mut(base + i) {
                            *slot = [*b as u32, 0, 0, 0];
                        }
                    }
                }
                (RegisterSet::Int4, Value::Vectors(vs)) => {
                    for (i, v) in vs.iter().enumerate().take(count) {
                        if let Some(slot) = out.ints.get_mut(base + i) {
                            *slot = v.map(|x| x as i32);
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }
}

/// What a sampler register is bound to.
#[derive(Clone, Debug, PartialEq)]
pub enum TextureSource {
    /// A texture resource, e.g. `fca_master_d.dds`.
    Resource(String),
    /// Opaque white (shadow maps: every texel "lit").
    White,
    /// Opaque black.
    Black,
}

/// The texture a sampler register reads, via the constant table's sampler name and the
/// material definition's `SamplerName` mapping.
pub fn sampler_texture(
    table: Option<&ConstantTable>,
    register: u32,
    ty: TextureType,
    material: &SceneMaterial,
    lightmap: Option<&LightmapRef>,
) -> (Option<String>, TextureSource) {
    let Some(c) = table.and_then(|t| t.at(RegisterSet::Sampler, register)) else {
        return (None, TextureSource::Black);
    };
    let sampler = c.name.clone();
    let param = parameters(material).find(|p| {
        p.sampler
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case(&sampler))
    });
    let Some(param) = param else {
        return (Some(sampler), TextureSource::Black);
    };
    let usage = param.usage.as_deref().unwrap_or("");
    if usage.starts_with("ShadowTexture") {
        return (Some(sampler), TextureSource::White);
    }
    if param.name.eq_ignore_ascii_case("mml_tLightmap") {
        return (
            Some(sampler),
            match lightmap {
                Some(l) => TextureSource::Resource(l.texture.clone()),
                None => TextureSource::Resource("default_black.dds".into()),
            },
        );
    }
    if let Some(t) = material.texture(&param.name) {
        return (Some(sampler), TextureSource::Resource(t));
    }
    let _ = ty;
    match param.default.as_deref() {
        Some(d) if !d.is_empty() => (
            Some(sampler),
            TextureSource::Resource(format!("{}.dds", d.to_ascii_lowercase())),
        ),
        _ => (Some(sampler), TextureSource::Black),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{DOMAIN_CHARACTERS, DOMAIN_LEVEL};
    use thedas_formats::MaterialObject;
    use thedas_formats::material::MaterialDef;

    fn table(constants: Vec<Constant>) -> ConstantTable {
        ConstantTable {
            creator: String::new(),
            target: "vs_3_0".into(),
            constants,
        }
    }

    fn constant(
        name: &str,
        set: RegisterSet,
        register: u16,
        count: u16,
        class: ParamClass,
    ) -> Constant {
        Constant {
            name: name.into(),
            set,
            register,
            count,
            class,
            ty: 3,
            rows: 1,
            columns: 4,
            elements: 1,
            default: None,
        }
    }

    #[test]
    fn fills_constants_by_name() {
        let xml = br#"<MaterialObject Name="t"><Material Name="static.mat"/>
            <Float Name="mml_fOpacityMultiplier" value="0.5"/></MaterialObject>"#;
        let mat = MaterialDef::parse(
            br#"<Material Name="Static"><ParameterSet Name="p">
                <InternalParameter Type="Matrix4x4f" Name="aur_mWorldViewProj" Usage="ModelViewProjectionTrans"/>
                <Parameter Type="Vector4f" Name="mml_vTintColor" Default="0.25 0 0 1"/>
            </ParameterSet></Material>"#,
        )
        .unwrap();
        let material = SceneMaterial {
            mao: "t.mao".into(),
            object: MaterialObject::parse(xml).unwrap(),
            definition: Some(Arc::new(mat)),
            semantic: "Default".into(),
            render_class: None,
            technique: None,
        };
        let lights = DrawLights {
            points: vec![(Vec3::new(1.0, 2.0, 3.0), 12.0, Vec3::new(0.5, 0.25, 0.0)); 2],
        };
        let lm = LightmapRef {
            texture: "lm.dds".into(),
            offset: [0.5, 0.25],
            scale: [0.125, 0.0625],
        };
        let world = Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0));
        let cc = ConstantContext {
            view: Mat4::IDENTITY,
            projection: Mat4::IDENTITY,
            world,
            camera: Vec3::ZERO,
            material: &material,
            lightmap: Some(&lm),
            lights: &lights,
            ambient: Vec3::new(0.1, 0.2, 0.3),
            bones: &[],
            irradiance: None,
            sun: Some((Vec3::new(0.0, 0.0, -1.0), Vec3::new(0.6, 0.5, 0.3))),
            fog: crate::atmosphere::fog_params(None),
            sky: None,
            clouds: crate::atmosphere::cloud_params(None),
        };
        let t = table(vec![
            constant(
                "aur_mWorldViewProj",
                RegisterSet::Float4,
                5,
                4,
                ParamClass::MatrixRows,
            ),
            constant(
                "g_vLightParams",
                RegisterSet::Float4,
                0,
                5,
                ParamClass::Vector,
            ),
            constant(
                "mml_fOpacityMultiplier",
                RegisterSet::Float4,
                25,
                1,
                ParamClass::Scalar,
            ),
            constant(
                "mml_vTintColor",
                RegisterSet::Float4,
                27,
                1,
                ParamClass::Vector,
            ),
            constant(
                "mml_vLightmapOffsetAndScale",
                RegisterSet::Float4,
                28,
                1,
                ParamClass::Vector,
            ),
            constant(
                "g_bShaderControlBooleans",
                RegisterSet::Bool,
                0,
                4,
                ParamClass::Scalar,
            ),
            constant("unknown", RegisterSet::Float4, 40, 1, ParamClass::Vector),
        ]);
        let c = cc.fill(Some(&t));
        // Translation lands in the fourth register of a MatrixRows matrix.
        assert_eq!(c.floats[8], [10.0, 0.0, 0.0, 1.0]);
        assert_eq!(c.floats[0], [1.0, 2.0, 3.0, 12.0]);
        assert_eq!(c.floats[1], [0.5, 0.25, 0.0, 1.0]);
        assert_eq!(c.floats[4], [0.1, 0.2, 0.3, 1.0]);
        assert_eq!(c.floats[25][0], 0.5);
        assert_eq!(c.floats[27], [0.25, 0.0, 0.0, 1.0]);
        assert_eq!(c.floats[28], [0.5, 0.25, 0.125, 0.0625]);
        assert_eq!(c.bools[0][0], 0, "light D stays off");
        assert_eq!(c.bools[1][0], 1, "second point light enables B");
        assert_eq!(c.bools[2][0], 0);
        // The sun: its direction and colour in registers 2 and 3, and boolean 3 on.
        assert_eq!(c.floats[2], [0.0, 0.0, -1.0, 0.0]);
        assert_eq!(c.floats[3], [0.6, 0.5, 0.3, 1.0]);
        assert_eq!(c.bools[3][0], 1);
        assert_eq!(c.floats[40], [0.0; 4]);
    }

    #[test]
    fn parameters_declared_in_the_pass_bind_samplers_and_defaults() {
        // weapon.mat and hairalpha.mat declare their parameters in the pass, in no set.
        let def = MaterialDef::parse(
            br#"<Material Name="Weapon"><Semantic Name="Default"><Logic><LogicTest ShaderModel="3"/>
                <Technique Name="LOD1"><Pass VertexShader="W.vsh" PixelShader="W.psh" State="W.sta">
                <Parameter Type="Texture" Name="mml_tDiffuse" SamplerName="smpDecal" Default="Default_Missing"/>
                <Parameter Type="Texture" Name="mml_tTintMask" SamplerName="smpTintMask" Default="Default_Black"/>
                <Parameter Type="Vector4f" Name="mml_vTintColor" Default="0.25 0 0 1"/>
                </Pass></Technique></Logic></Semantic></Material>"#,
        )
        .unwrap();
        let xml = br#"<MaterialObject Name="w"><Material Name="weapon.mat"/>
            <Texture Name="mml_tDiffuse" ResName="w_d.dds"/></MaterialObject>"#;
        let material = SceneMaterial {
            mao: "w.mao".into(),
            object: MaterialObject::parse(xml).unwrap(),
            technique: def.select("Default", 3, "HIGH").cloned(),
            definition: Some(Arc::new(def)),
            semantic: "Default".into(),
            render_class: None,
        };
        let sampler = |name: &str, register| Constant {
            ty: 12,
            ..constant(name, RegisterSet::Sampler, register, 1, ParamClass::Object)
        };
        let t = table(vec![
            sampler("smpDecal", 0),
            sampler("smpTintMask", 1),
            sampler("smpUnknown", 2),
        ]);
        let tex = |r| sampler_texture(Some(&t), r, TextureType::Tex2D, &material, None).1;
        assert_eq!(tex(0), TextureSource::Resource("w_d.dds".into()));
        assert_eq!(tex(1), TextureSource::Resource("default_black.dds".into()));
        assert_eq!(tex(2), TextureSource::Black);
        let lights = DrawLights::default();
        let cc = ConstantContext {
            view: Mat4::IDENTITY,
            projection: Mat4::IDENTITY,
            world: Mat4::IDENTITY,
            camera: Vec3::ZERO,
            material: &material,
            lightmap: None,
            lights: &lights,
            ambient: Vec3::ZERO,
            bones: &[],
            irradiance: None,
            sun: None,
            fog: crate::atmosphere::fog_params(None),
            sky: None,
            clouds: crate::atmosphere::cloud_params(None),
        };
        let c = cc.fill(Some(&table(vec![constant(
            "mml_vTintColor",
            RegisterSet::Float4,
            3,
            1,
            ParamClass::Vector,
        )])));
        assert_eq!(c.floats[3], [0.25, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn plain_matrix_usages_are_the_transpose_of_trans_ones() {
        let def = |usage: &str| {
            MaterialDef::parse(
                format!(
                    r#"<Material Name="m"><ParameterSet Name="p">
                    <InternalParameter Type="Matrix4x4f" Name="aur_mWorld" Usage="{usage}"/>
                    </ParameterSet></Material>"#
                )
                .as_bytes(),
            )
            .unwrap()
        };
        let world = Mat4::from_translation(Vec3::new(7.0, 8.0, 9.0));
        let lights = DrawLights::default();
        let regs = |usage: &str| {
            let material = SceneMaterial {
                mao: "m.mao".into(),
                object: MaterialObject::parse(
                    br#"<MaterialObject Name="m"><Material Name="m.mat"/></MaterialObject>"#,
                )
                .unwrap(),
                definition: Some(Arc::new(def(usage))),
                semantic: "Default".into(),
                render_class: None,
                technique: None,
            };
            let cc = ConstantContext {
                view: Mat4::IDENTITY,
                projection: Mat4::IDENTITY,
                world,
                camera: Vec3::ZERO,
                material: &material,
                lightmap: None,
                lights: &lights,
                ambient: Vec3::ZERO,
                bones: &[],
                irradiance: None,
                sun: None,
                fog: crate::atmosphere::fog_params(None),
                sky: None,
                clouds: crate::atmosphere::cloud_params(None),
            };
            let c = cc.fill(Some(&table(vec![constant(
                "aur_mWorld",
                RegisterSet::Float4,
                0,
                4,
                ParamClass::MatrixRows,
            )])));
            c.floats[..4].to_vec()
        };
        // `PartTrans` (read with mad): the translation is the fourth register.
        assert_eq!(regs("PartTrans")[3], [7.0, 8.0, 9.0, 1.0]);
        // `Part` (read with dp4): the translation is each register's w.
        let plain = regs("Part");
        assert_eq!([plain[0][3], plain[1][3], plain[2][3]], [7.0, 8.0, 9.0]);
        assert_eq!(plain[3], [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn picks_the_strongest_lights_in_range() {
        let light = |x: f32, r: f32, c: f32| SceneLight::Point {
            position: Vec3::new(x, 0.0, 0.0),
            color: Vec3::splat(c),
            radius: r,
            name: String::new(),
            domain: None,
        };
        let lights = vec![
            light(1.0, 5.0, 1.0),
            light(100.0, 5.0, 10.0), // out of range
            light(2.0, 5.0, 0.1),
            light(0.5, 5.0, 0.5),
            light(3.0, 5.0, 0.9),
            SceneLight::Ambient {
                color: Vec3::ONE,
                name: String::new(),
                room: String::new(),
            },
        ];
        let picked = pick_lights(&lights, (Vec3::ZERO, 1.0), DOMAIN_LEVEL);
        assert_eq!(picked.points.len(), 3);
        assert_eq!(picked.points[0].0.x, 1.0);
        assert!(picked.points.iter().all(|p| p.0.x < 50.0));
    }

    #[test]
    fn lights_only_their_domain() {
        let light = |x: f32, c: f32, domain| SceneLight::Point {
            position: Vec3::new(x, 0.0, 0.0),
            color: Vec3::splat(c),
            radius: 600.0,
            name: String::new(),
            domain,
        };
        let lights = vec![
            light(1.0, 9.0, Some(DOMAIN_CHARACTERS)),
            light(2.0, 1.0, Some(DOMAIN_LEVEL)),
            light(3.0, 0.5, Some(DOMAIN_LEVEL | DOMAIN_CHARACTERS)),
            light(4.0, 0.1, None),
        ];
        let at = |domain| {
            pick_lights(&lights, (Vec3::ZERO, 1.0), domain)
                .points
                .iter()
                .map(|p| p.0.x)
                .collect::<Vec<_>>()
        };
        assert_eq!(at(DOMAIN_LEVEL), [2.0, 3.0, 4.0]);
        assert_eq!(at(DOMAIN_CHARACTERS), [1.0, 3.0, 4.0]);
    }
}
