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
//! `g_vLightParams` (usage `LightsArray`) is what DAOrigins.exe's per-object light packer
//! (0x46b260, reached through the usage dispatch 0x4f8a10 and 0x46baf0) writes: point
//! lights A, B, C at positions (radius in `w`) `c0`, `c5`, `c7` and colours `c1`, `c6`, `c8`
//! (slot tables at 0xae7fb0 and 0xae7fbc), lights D, E at `c9`/`c10` and `c11`/`c12`
//! (0xae7fc8, 0xae7fd4), unused slots zero; `c2`/`c3` the directional light (below); and the
//! ambient term `c4 = (base + Σ ambient lights, 1)`. Which lights fill them is
//! [`pick_lights`] and [`ambient_of`] (the light manager's lists, its room test and
//! influence functions; notes in `OpenThedas-testdata/notes/character-lights.md`): A-C the
//! static point lights, D-E the others; the ambient light nodes only for placeables. The
//! base is what the render parameters carry at `+0xc0` (0x46baf0's last argument), written
//! only by the graphics detail setter (0x458eb0): `(0.2, 0.2, 0.2)` in its fallback mode 0
//! and zero at every Graphics Detail setting (Low, Medium, High: modes 1-3)
//! ([`BASE_AMBIENT`]).
//! `g_bShaderControlBooleans` `b0`-`b2` say lights D, B and C are present and `b3` that the
//! directional light is (0x46be70).
//!
//! The directional light: every frame the light manager (0x45be65 → 0x4654e0 → 0x464310)
//! gives each node `c2 = (direction, 1)` and `c3 = (colour · v, w)` (0x462470, 0x46bb80),
//! from the layout's sun (0x4625d0: 3150's direction, 3152's colour for the level and
//! 3149's for characters, the nodes flagged 0x4000) in layouts that set 3122, a black light
//! otherwise; `v` is a per-node visibility that eases (at a rate times the frame time,
//! 0x46a520) towards 1, or 0.1 when the sun is occluded from a character in a layout whose
//! 3148 allows it. `b3` is `c3.w` non-zero (0x46bf5d). No multiplier: 3153 and the ATMO sun
//! intensity are not involved.
//!
//! Provisional, inferred by reading Static1's bytecode rather than verified against the
//! original executable:
//!
//! - Matrices of class `MatrixRows` occupy one register per column of our column-major
//!   matrices (the shader computes `x*c[n] + y*c[n+1] + z*c[n+2] + c[n+3]`); `MatrixColumns`
//!   one register per row.
//! - The sun's visibility is taken as 1
//!   (the occlusion test that lowers it is not traced). Light A is always evaluated; lights
//!   A-C and the directional light are multiplied by the lightmap alpha; D is not.
//! - Fog and the sky's scattering come from the level's atmosphere ([`crate::atmosphere`]).
//!   The environment cubemap (`EnvironmentMap`) is the draw's light probe's `.dds` (see
//!   [`sampler_texture`]); the depth-of-field output `oC1` is dropped.

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
use crate::scene::{AreaScene, DOMAIN_CHARACTERS, LightmapRef, SceneLight, SceneMaterial};

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

/// A shadow caster's material: `material` switched to its `PSSM` semantic (the submission
/// queue's `force semantic PSSM`, see [`crate::shadow`]), so its constants take that
/// technique's usages (`aur_mWorld` is `Part` there, `PartTrans` in the Default semantic).
pub type CasterMaterial = (Arc<D3dProgram>, SceneMaterial);

/// The program a material's `PSSM` semantic draws shadow casters with (its first pass, at
/// shader model 3) and the material switched to that semantic. `None` when the material
/// has no such semantic or it does not translate.
pub fn pssm_program(
    vfs: &Vfs,
    material: &SceneMaterial,
    detail: &str,
    cache: &mut crate::RenderCache,
) -> Option<CasterMaterial> {
    let def = material.definition.as_ref()?;
    let technique = def.select("PSSM", 3, detail)?;
    let (vs, ps, st) = pass_shaders(technique)?.into_iter().next()?;
    let key = format!("{vs}+{ps}+{st}").to_ascii_lowercase();
    let program = match cache.programs.get(&key) {
        Some(built) => built.clone(),
        None => {
            let built = match build(vfs, vs, ps, st) {
                Ok(p) => Some(Arc::new(p)),
                Err(e) => {
                    log::warn!("shadow caster shader unavailable: {e}");
                    None
                }
            };
            cache.programs.insert(key, built.clone());
            built
        }
    }?;
    let material = SceneMaterial {
        technique: Some(technique.clone()),
        semantic: "PSSM".into(),
        ..material.clone()
    };
    Some((program, material))
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

/// [`static1_filter`] plus the vertex shaders that draw creatures and placeables, and the
/// effects' sprites ([`VFX_VERTEX_SHADERS`]):
/// characters (`Char1`), hair (`Hair0_Pass1`, `Hair0`: four passes), faces (`Face1`), eyes
/// and lashes (`Eye0`), cloth (`Cloth1`), props (`Prop1`) and weapons (`Weapon1`), with any
/// pixel shader; and exterior terrain (`Terrain2`, terrain.mat's pass above Low detail, and
/// `Terrain3`, its Low detail pass); the sky's dome and clouds (`Sky`, `Clouds`); lava
/// (`Lava0`); and water (`Water0`).
pub fn object_filter() -> ShaderFilter {
    let mut v = static1_filter().unwrap_or_default();
    for vs in OBJECT_VERTEX_SHADERS.iter().chain(&VFX_VERTEX_SHADERS) {
        v.push((vs.to_string(), "*".to_string()));
    }
    Some(v)
}

/// The vertex shaders of `VFX.mat`'s sprite semantics (particles and effect cards, read as
/// `POSITION`, `COLOR0`, `TEXCOORD0`), part of [`object_filter`]. The `Chunky` and `VolTex`
/// ones are not drawn yet.
pub const VFX_VERTEX_SHADERS: [&str; 8] = [
    "VFX0.vsh",
    "VFX0_Addv.vsh",
    "VFX0_ContactSheet.vsh",
    "VFX0_CS_Addv.vsh",
    "VFX0_CS_Addv_Alt.vsh",
    "VFX0_CS_Alt.vsh",
    "VFX0_DBA.vsh",
    "VFX0_CS_DBA.vsh",
];

/// See [`object_filter`].
pub const OBJECT_VERTEX_SHADERS: [&str; 14] = [
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
    "Lava0.vsh",
    "Water0.vsh",
];

/// Point lights chosen for one draw.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrawLights {
    /// The static lights, in slot order A, B, C: (position, radius, colour).
    pub points: Vec<(Vec3, f32, Vec3)>,
    /// The other lights (not `IsStatic`), in slot order D, E.
    pub dynamic: Vec<(Vec3, f32, Vec3)>,
}

/// Which of the light manager's lists a draw reads, by its material's `RenderClass`
/// (DAOrigins.exe tags a mesh node 0x4000 for `character` and its model 0x10000 for `prop`,
/// 0x55a0a0-0x55a2da; the per-object pass 0x4654e0 sends 0x4000 nodes to 0x464560 and the
/// rest to 0x464310).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightClass {
    /// Level geometry (and anything else neither character nor prop). Traced: the level's
    /// point lists and no ambient light nodes; not yet applied, these draws keep the old
    /// rule ([`pick_level_lights`]).
    Level,
    /// Placeables (`Prop`): the level's point lists, and the brightest ambient light node.
    Prop,
    /// Characters, their hair, faces, eyes and weapons (`Character`): the characters' point
    /// lists, no ambient light nodes.
    Character,
}

impl LightClass {
    /// The class a `RenderClass` (comma-separated, any case) puts a draw in.
    pub fn of(render_class: Option<&str>) -> LightClass {
        let has = |tag: &str| {
            render_class.is_some_and(|c| c.split(',').any(|k| k.trim().eq_ignore_ascii_case(tag)))
        };
        if has("character") {
            LightClass::Character
        } else if has("prop") {
            LightClass::Prop
        } else {
            LightClass::Level
        }
    }

    /// Whether a point light of this affect domain is in the lists this class reads: the
    /// light manager files domain 1 under the characters' lists and anything else under the
    /// level's (0x464e30).
    fn takes(self, domain: Option<i64>) -> bool {
        (domain == Some(DOMAIN_CHARACTERS)) == (self == LightClass::Character)
    }
}

/// Whether a light (or probe) of room `light_room` reaches an object in room `room`
/// (DAOrigins.exe 0x53d210): yes when the object has no room, the light has none, they are
/// the same room, or the light's room's reach list ([`AreaScene::light_rooms`], RML 3164)
/// is the single entry -1 or holds the object's room. A room whose list is empty (or
/// missing) reaches only itself.
pub fn reaches(
    light_room: Option<i64>,
    light_rooms: &HashMap<i64, Vec<i64>>,
    room: Option<i64>,
) -> bool {
    let (Some(l), Some(r)) = (light_room, room) else {
        return true;
    };
    if l == r {
        return true;
    }
    match light_rooms.get(&l).map(Vec::as_slice) {
        Some([-1]) => true,
        Some(list) => list.contains(&r),
        None => false,
    }
}

/// A point light's influence on an object (DAOrigins.exe 0x53d280, `ObjectNodePointLight`'s
/// `vfunc+0x84`): with `R` its radius plus the object's bound radius and `d` the distance
/// between their centres, 0 beyond `R`; else `R - d` for `Version` 0 and
/// `B · (1 - d / R)` for `Version` 1, `B` the colour's brightness `(|r| + |g| + |b|) / 3`
/// (0x53ce60). Both scale by the light's intensity, 1 for a light that does not animate.
/// Provisional: other versions (none seen) take version 0's formula; the exe leaves the
/// last value.
pub fn influence(
    position: Vec3,
    radius: f32,
    color: Vec3,
    version: i64,
    sphere: (Vec3, f32),
) -> f32 {
    let reach = radius + sphere.1;
    let d2 = position.distance_squared(sphere.0);
    if reach * reach < d2 {
        return 0.0;
    }
    let d = d2.max(0.0).sqrt();
    if version == 1 {
        let brightness = (color.x.abs() + color.y.abs() + color.z.abs()) / 3.0;
        brightness * (1.0 - d / reach)
    } else {
        reach - d
    }
}

/// Picks a draw's point lights as DAOrigins.exe's light manager does (0x463e40 over the
/// lists 0x464e30 fills): from the point lights of the draw's class ([`LightClass::takes`])
/// whose room reaches the draw's room ([`reaches`]), ranked by [`influence`] on the draw's
/// bounding sphere, those above zero; up to three static lights (A, B, C) and two others
/// (D, E). Ties keep the scene's order (the exe's sort is not stable).
///
/// Not mirrored (provisional): the light groups (a `CanBeOccluded` light reaches only the
/// objects whose group set, ARL 3800, holds its id), the hidden-owner test of the D/E
/// lists, and an object's second room.
pub fn pick_lights(
    lights: &[SceneLight],
    light_rooms: &HashMap<i64, Vec<i64>>,
    sphere: (Vec3, f32),
    room: Option<i64>,
    class: LightClass,
) -> DrawLights {
    pick_where(lights, sphere, |domain, light_room| {
        class.takes(domain) && reaches(light_room, light_rooms, room)
    })
}

/// The light choice for level draws ([`LightClass::Level`]), kept as it was before the
/// light manager was traced: up to three point lights whose radius reaches the draw's
/// bounding sphere and whose affect domain (read as a mask) includes `domain` (a missing
/// domain matches any), strongest first by colour brightness times the shader's
/// attenuation at the sphere centre; no room test, no D/E lights.
///
/// Not yet applied to level draws: the traced rule ([`pick_lights`] with
/// [`LightClass::Level`]: domains other than 1, the room test with RML 3164, `R − d`
/// ranking, static lights in A-C and the others in D-E; and no ambient light node) is the
/// same selection function the exe runs for every object, but whether level meshes reach
/// it by the per-object pass (0x4654e0) is not verified, and applied to them it loses the
/// bright window light on the Cousland hall's floor and darkens the Highever hall, which
/// the original appears to keep. With no reference for those views it waits for one.
/// Provisional.
pub fn pick_level_lights(lights: &[SceneLight], sphere: (Vec3, f32), domain: i64) -> DrawLights {
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
        dynamic: Vec::new(),
    }
}

/// [`pick_lights`] for a light rig (the paperdoll's): every light whose domain is unset or
/// the characters', whatever its room.
pub fn pick_rig_lights(lights: &[SceneLight], sphere: (Vec3, f32)) -> DrawLights {
    pick_where(lights, sphere, |domain, _| {
        domain.is_none_or(|d| d == DOMAIN_CHARACTERS)
    })
}

fn pick_where(
    lights: &[SceneLight],
    sphere: (Vec3, f32),
    keep: impl Fn(Option<i64>, Option<i64>) -> bool,
) -> DrawLights {
    let mut points: Vec<(f32, (Vec3, f32, Vec3))> = Vec::new();
    let mut dynamic: Vec<(f32, (Vec3, f32, Vec3))> = Vec::new();
    for l in lights {
        let SceneLight::Point {
            position,
            color,
            radius,
            domain,
            is_static,
            version,
            room,
            ..
        } = l
        else {
            continue;
        };
        if !keep(*domain, *room) {
            continue;
        }
        let w = influence(*position, *radius, *color, *version, sphere);
        if w > 0.0 {
            let list = if *is_static {
                &mut points
            } else {
                &mut dynamic
            };
            list.push((w, (*position, *radius, *color)));
        }
    }
    let best = |mut v: Vec<(f32, (Vec3, f32, Vec3))>, n: usize| {
        v.sort_by(|a, b| b.0.total_cmp(&a.0));
        v.into_iter().take(n).map(|(_, l)| l).collect()
    };
    DrawLights {
        points: best(points, 3),
        dynamic: best(dynamic, 2),
    }
}

/// The ambient light node term of a draw (`c4` beside the base): DAOrigins.exe selects the
/// ambient list only for placeables (0x464310's 4th argument, set for models tagged `prop`,
/// 0x465527), with one slot and an influence of the colour's brightness
/// `(|r| + |g| + |b|) / 3` whatever the distance or room (0x53d040): the brightest ambient
/// light of the level. Characters (0x465570 passes 0) get none.
///
/// Level geometry gets none from this path either (0x464310 without the `prop` flag skips
/// the ambient list at 0x4644eb), but that rule is not applied to level draws yet (see
/// [`pick_level_lights`]): `D3dPass` keeps their room's ambient and this returns zero for
/// them.
pub fn ambient_of(lights: &[SceneLight], class: LightClass) -> Vec3 {
    if class != LightClass::Prop {
        return Vec3::ZERO;
    }
    let brightness = |c: &Vec3| (c.x.abs() + c.y.abs() + c.z.abs()) / 3.0;
    lights
        .iter()
        .filter_map(|l| match l {
            SceneLight::Ambient { color, .. } => Some(*color),
            _ => None,
        })
        .fold(None, |best: Option<Vec3>, c| match best {
            Some(b) if brightness(&b) >= brightness(&c) => Some(b),
            _ => Some(c),
        })
        .unwrap_or(Vec3::ZERO)
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
    pub clouds: [[f32; 4]; 3],
    /// The shaders' `ApplicationTime`, in seconds (see [`crate::atmosphere`]).
    pub time: f32,
    /// `g_vShadowData` (see [`crate::shadow`]); [`NO_SHADOW_DATA`] with no partitions drawn.
    pub shadow: [[f32; 4]; 9],
    /// The shadow partition being drawn into (`ShadowPartitionMatrix`, world to the
    /// partition's clip space), for the casters' `PSSM` programs.
    pub partition: Option<Mat4>,
}

/// The ambient term's base (see the module docs): zero at every Graphics Detail setting
/// (DAOrigins.exe 0x458eb0 writes `(0.2, 0.2, 0.2)` only in its fallback mode 0).
pub const BASE_AMBIENT: [f32; 3] = [0.0; 3];

/// `g_vShadowData` (usage `ShadowData`, the nine vectors at 0xc1e750 the usage dispatch
/// hands out, 0x4f8bc5) with no shadow partition drawn: both partitions' projections the
/// identity with its depth row zeroed, as the shadow reset writes them (0x452cb0), the
/// partition-2 range `w` 1 and the view-depth plane zero (never written). With these the
/// Static shaders' shadow tests read every point as lit, whatever the shadow textures hold:
/// every lookup compares the map against a depth of 0.
///
/// The original draws shadow partitions at every Graphics Detail setting (0x458ff7 → 0x4556e0
/// with the detail mode as the shadow quality: at Low, 512-texel maps, 0x45569e, with ranges
/// of 5 and 15 m, 0x455777); a point those find in shadow gets the sun and point lights
/// scaled by the shadow and the lightmap by `0.5 · (shadow − 1) + 1`. Those partitions are
/// not drawn here (provisional): this is the engine's own unshadowed state.
pub const NO_SHADOW_DATA: [[f32; 4]; 9] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 0.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 0.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
    [0.0, 0.0, 0.0, 0.0],
];

/// Light from every direction for draws in a level with no irradiance probe. Provisional.
pub const NO_PROBE_IRRADIANCE: [f32; 3] = [0.35, 0.35, 0.35];

/// The rows of an identity 3x4 bone matrix.
pub const IDENTITY_BONE: [[f32; 4]; 3] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

/// Register contents for one stage: floats, ints, bools.
#[derive(Clone)]
pub struct StageConstants {
    pub floats: Vec<[f32; 4]>,
    pub ints: Vec<[i32; 4]>,
    pub bools: Vec<[u32; 4]>,
}

impl Default for StageConstants {
    fn default() -> Self {
        Self::new()
    }
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

/// A parameter's default (its material definition's `Default`) as four-float vectors.
pub(crate) fn default_vectors(m: &SceneMaterial, name: &str) -> Option<Vec<[f32; 4]>> {
    let f = parse_floats(find_param(m, name)?.default.as_deref()?);
    (!f.is_empty()).then(|| {
        f.chunks(4)
            .map(|q| {
                let mut o = [0.0; 4];
                o[..q.len()].copy_from_slice(q);
                o
            })
            .collect()
    })
}

impl ConstantContext<'_> {
    fn light_params(&self) -> Vec<[f32; 4]> {
        let mut r = vec![[0.0f32; 4]; 13];
        let slots = [0usize, 5, 7];
        for (i, (p, radius, c)) in self.lights.points.iter().take(3).enumerate() {
            let at = slots[i];
            r[at] = [p.x, p.y, p.z, *radius];
            r[at + 1] = [c.x, c.y, c.z, 1.0];
        }
        // D and E: positions in 9 and 10, colours in 11 and 12 (0xae7fc8, 0xae7fd4).
        for (i, (p, radius, c)) in self.lights.dynamic.iter().take(2).enumerate() {
            r[9 + i] = [p.x, p.y, p.z, *radius];
            r[11 + i] = [c.x, c.y, c.z, 1.0];
        }
        let base = Vec3::from_array(BASE_AMBIENT);
        let ambient = base + self.ambient;
        r[4] = [ambient.x, ambient.y, ambient.z, 1.0];
        // The sun takes registers 2 (direction, w 1) and 3 (colour, w 1: the stored colour's
        // w, which boolean 3 tests), as the light manager writes them (0x4625d0 → 0x462470
        // → 0x46bb80).
        if let Some((d, c)) = self.sun {
            r[2] = [d.x, d.y, d.z, 1.0];
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
                // `w` is the camera's +0x138 (DAOrigins.exe): 0 for the exploration camera
                // (area modes 4 and 5, and the default); Terrain1 blends its lightmap with the
                // shadow term by 0.35·w + 0.15.
                Value::Vectors(vec![[self.camera.x, self.camera.y, self.camera.z, 0.0]])
            }
            ("FocalPlane", _) => Value::Vectors(vec![[1.0, 0.0, 0.0, 0.0]]),
            ("LightsArray", _) | (_, "g_vLightParams") => Value::Vectors(self.light_params()),
            ("ShaderControlBooleans", _) | (_, "g_bShaderControlBooleans") => {
                // b0: the D/E list's first entry; b1, b2: lights B and C (0x46be70).
                let n = self.lights.points.len();
                Value::Bools(vec![
                    !self.lights.dynamic.is_empty(),
                    n > 1,
                    n > 2,
                    self.sun.is_some(),
                ])
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
            ("ApplicationTime", _) => Value::Vectors(vec![[self.time, 0.0, 0.0, 0.0]]),
            ("CloudParameters", _) => Value::Vectors(self.clouds.to_vec()),
            ("ShadowData", _) => Value::Vectors(self.shadow.to_vec()),
            // Four rows of the partition's matrix, as `PSSMMesh.vsh` reads them (`dp4`).
            ("ShadowPartitionMatrix", _) => Value::Vectors(
                self.partition
                    .map(|m| [0, 1, 2, 3].map(|i| m.row(i).to_array()).to_vec())
                    .unwrap_or_default(),
            ),
            ("IrradianceParams", _) => Value::Vectors(Vec::new()),
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

    /// Whether a constant's value follows the camera, the draw's placement and pose, the
    /// clock or the shadow partitions, and so changes from frame to frame; every other
    /// constant (the material's, the draw's lights, the level's fog and sky) stays as the
    /// draw was made. Mirrors the cases of [`ConstantContext::value`].
    pub fn per_frame(&self, c: &Constant) -> bool {
        let usage = find_param(self.material, &c.name).and_then(|p| p.usage.as_deref());
        matches!(
            usage.unwrap_or(""),
            "ModelViewProjectionTrans"
                | "ModelViewProjection"
                | "PartTrans"
                | "Part"
                | "ModelView"
                | "View"
                | "ViewInv"
                | "Projection"
                | "ViewProjection"
                | "CameraPosition"
                | "ApplicationTime"
                | "ShadowData"
                | "ShadowPartitionMatrix"
        ) || matches!(
            c.name.as_str(),
            "aur_mWorldViewProj"
                | "aur_mWorld"
                | "aur_mModelView"
                | "g_vCameraPosition"
                | "bsm_bones"
        )
    }

    /// Fills the registers a shader's constant table names.
    pub fn fill(&self, table: Option<&ConstantTable>) -> StageConstants {
        let mut out = StageConstants::new();
        self.fill_into(table, &mut out, |_| true);
        out
    }

    /// Fills into `out` the registers of the constants (by index in the table) `pick`
    /// accepts, leaving the rest as they are.
    pub fn fill_into(
        &self,
        table: Option<&ConstantTable>,
        out: &mut StageConstants,
        pick: impl Fn(usize) -> bool,
    ) {
        let Some(table) = table else { return };
        for (k, c) in table.constants.iter().enumerate() {
            if !pick(k) {
                continue;
            }
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
    }
}

/// What a sampler register is bound to.
#[derive(Clone, Debug, PartialEq)]
pub enum TextureSource {
    /// A texture resource, e.g. `fca_master_d.dds`.
    Resource(String),
    /// A material's texture, else (when it does not load) its parameter's default: the
    /// terrain palette's `DirtRocks` names `ter_dirtrock_d.dds`, which the game does not
    /// ship, and draws `Default_Missing` (mid grey) rather than black. Provisional: the
    /// exe's fallback for a texture that fails to load is not traced.
    ResourceOr(String, String),
    /// Opaque white (shadow maps: every texel "lit").
    White,
    /// Opaque black.
    Black,
    /// Shadow partition 0 (`ShadowTexture_Primary`) or 1 (`ShadowTexture_Secondary`); opaque
    /// white (every texel lit) when no partitions are drawn.
    Shadow(u8),
}

/// The texture a sampler register reads, via the constant table's sampler name and the
/// material definition's `SamplerName` mapping.
pub fn sampler_texture(
    table: Option<&ConstantTable>,
    register: u32,
    ty: TextureType,
    material: &SceneMaterial,
    lightmap: Option<&LightmapRef>,
    probe: Option<&str>,
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
        let slot = u8::from(usage.eq_ignore_ascii_case("ShadowTexture_Secondary"));
        return (Some(sampler), TextureSource::Shadow(slot));
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
    let default = param
        .default
        .as_deref()
        .filter(|d| !d.is_empty())
        .map(|d| format!("{}.dds", d.to_ascii_lowercase()));
    // The environment cubemap (`aur_cmLightProbe0`, usage `EnvironmentMap`): the light
    // probe's own cubemap, the `.dds` the level ships beside each probe's `.mtx`
    // (`ost000t_probe_7yo.dds`, 256² DXT1, six faces). Provisional: inferred from the names
    // and the files; the exe's binding is not traced.
    if usage.eq_ignore_ascii_case("EnvironmentMap")
        && let Some(p) = probe
    {
        let p = p.to_ascii_lowercase();
        let cube = format!("{}.dds", p.trim_end_matches(".mtx"));
        return (
            Some(sampler),
            match default {
                Some(d) => TextureSource::ResourceOr(cube, d),
                None => TextureSource::Resource(cube),
            },
        );
    }
    if let Some(t) = material.texture(&param.name) {
        return (
            Some(sampler),
            match default {
                Some(d) => TextureSource::ResourceOr(t, d),
                None => TextureSource::Resource(t),
            },
        );
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
            dynamic: Vec::new(),
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
            clouds: crate::atmosphere::cloud_params(None, None),
            time: 0.0,
            shadow: NO_SHADOW_DATA,
            partition: None,
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
        assert_eq!(c.floats[2], [0.0, 0.0, -1.0, 1.0]);
        assert_eq!(c.floats[3], [0.6, 0.5, 0.3, 1.0]);
        assert_eq!(c.bools[3][0], 1);
        assert_eq!(c.floats[40], [0.0; 4]);
        // Only the matrix follows the frame; filled in two halves, what stays and what
        // changes, the registers come out the same.
        let per_frame: Vec<bool> = t.constants.iter().map(|k| cc.per_frame(k)).collect();
        assert_eq!(per_frame, [true, false, false, false, false, false, false]);
        let mut split = StageConstants::default();
        cc.fill_into(Some(&t), &mut split, |k| !per_frame[k]);
        cc.fill_into(Some(&t), &mut split, |k| per_frame[k]);
        assert_eq!(split.floats, c.floats);
        assert_eq!(split.ints, c.ints);
        assert_eq!(split.bools, c.bools);
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
        let tex = |r| sampler_texture(Some(&t), r, TextureType::Tex2D, &material, None, None).1;
        // The material's own texture, else (missing) the parameter's default.
        assert_eq!(
            tex(0),
            TextureSource::ResourceOr("w_d.dds".into(), "default_missing.dds".into())
        );
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
            clouds: crate::atmosphere::cloud_params(None, None),
            time: 0.0,
            shadow: NO_SHADOW_DATA,
            partition: None,
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

    /// The Static1_fast shadow test (registers c12-c20, shadow maps s7 and s15) on the CPU.
    fn shadow_lit(sd: &[[f32; 4]; 9], pos: [f32; 3], map: [f32; 2]) -> f32 {
        let p = [pos[0], pos[1], pos[2], 1.0];
        let dp4 = |r: [f32; 4], v: [f32; 4]| r.iter().zip(v).map(|(a, b)| a * b).sum::<f32>();
        let sat = |x: f32| if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
        let d = dp4([sd[8][0], sd[8][1], sd[8][2], 0.0], p) + sd[8][3];
        let near = sat(d * 10000.0);
        // Partition 2.
        let w2 = dp4([sd[7][0], sd[7][1], sd[7][2], 1.0], p);
        let z2 = sat(dp4(sd[6], p) / w2);
        let lit2 = if map[0] - z2 >= 0.0 { 1.0 } else { 0.0 };
        let fade = sat((sd[7][3] - d) / (0.25 * sd[7][3]));
        let far = fade * (lit2 - 1.0) + 1.0;
        // Partition 1.
        let z1 = sat(dp4(sd[2], p) / dp4(sd[3], p));
        let lit1 = if map[0] - z1 >= 0.0 { 1.0 } else { sat(map[1]) };
        if -near >= 0.0 { far } else { lit1 }
    }

    #[test]
    fn without_partitions_every_point_is_lit() {
        for pos in [[0.0, 0.0, 0.0], [12.5, -40.0, 3.0], [300.0, 250.0, 5.0]] {
            for map in [[0.0, 0.0], [1.0, 1.0], [0.3, 0.0]] {
                assert_eq!(
                    shadow_lit(&NO_SHADOW_DATA, pos, map),
                    1.0,
                    "{pos:?} {map:?}"
                );
            }
        }
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
                clouds: crate::atmosphere::cloud_params(None, None),
                time: 0.0,
                shadow: NO_SHADOW_DATA,
                partition: None,
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

    /// A static point light of version 0 at (x, 0, 0).
    fn point(x: f32, radius: f32, domain: Option<i64>, room: Option<i64>) -> SceneLight {
        SceneLight::Point {
            position: Vec3::new(x, 0.0, 0.0),
            color: Vec3::ONE,
            radius,
            name: String::new(),
            domain,
            is_static: true,
            version: 0,
            room,
        }
    }

    fn xs(l: &[(Vec3, f32, Vec3)]) -> Vec<f32> {
        l.iter().map(|p| p.0.x).collect()
    }

    #[test]
    fn ranks_by_overlap_not_colour() {
        let mut bright = point(1.0, 5.0, None, None);
        if let SceneLight::Point { color, .. } = &mut bright {
            *color = Vec3::splat(10.0);
        }
        let lights = vec![
            bright,                        // R - d = 6 - 1 = 5
            point(100.0, 5.0, None, None), // out of reach
            point(2.0, 20.0, None, None),  // 21 - 2 = 19
            point(0.5, 5.0, None, None),   // 5.5
            point(3.0, 5.0, None, None),   // 3
            SceneLight::Ambient {
                color: Vec3::ONE,
                name: String::new(),
                room: String::new(),
            },
        ];
        let picked = pick_lights(
            &lights,
            &HashMap::new(),
            (Vec3::ZERO, 1.0),
            None,
            LightClass::Level,
        );
        assert_eq!(xs(&picked.points), [2.0, 0.5, 1.0]);
        assert!(picked.dynamic.is_empty());
    }

    #[test]
    fn version_one_weighs_brightness_and_dynamic_lights_fill_d_and_e() {
        assert_eq!(
            influence(Vec3::X * 2.0, 3.0, Vec3::splat(3.0), 1, (Vec3::ZERO, 1.0)),
            1.5
        );
        assert_eq!(
            influence(Vec3::X * 2.0, 3.0, Vec3::splat(3.0), 0, (Vec3::ZERO, 1.0)),
            2.0
        );
        assert_eq!(
            influence(Vec3::X * 5.0, 3.0, Vec3::ONE, 0, (Vec3::ZERO, 1.0)),
            0.0
        );
        let mut lights: Vec<SceneLight> =
            (0..4).map(|i| point(i as f32, 10.0, None, None)).collect();
        for l in &mut lights[1..] {
            if let SceneLight::Point { is_static, .. } = l {
                *is_static = false;
            }
        }
        let picked = pick_lights(
            &lights,
            &HashMap::new(),
            (Vec3::ZERO, 1.0),
            None,
            LightClass::Prop,
        );
        assert_eq!(xs(&picked.points), [0.0]);
        assert_eq!(xs(&picked.dynamic), [1.0, 2.0]);
    }

    #[test]
    fn characters_take_domain_one_and_the_rest_take_the_others() {
        let lights = vec![
            point(1.0, 600.0, Some(DOMAIN_CHARACTERS), None),
            point(2.0, 600.0, Some(DOMAIN_LEVEL), None),
            point(3.0, 600.0, Some(0), None),
            point(4.0, 600.0, None, None),
        ];
        let at = |class| {
            xs(&pick_lights(&lights, &HashMap::new(), (Vec3::ZERO, 1.0), None, class).points)
        };
        assert_eq!(at(LightClass::Character), [1.0]);
        assert_eq!(at(LightClass::Level), [2.0, 3.0, 4.0]);
        assert_eq!(at(LightClass::Prop), [2.0, 3.0, 4.0]);
        assert_eq!(
            LightClass::of(Some("Opaque,Weapon,Character")),
            LightClass::Character
        );
        assert_eq!(
            LightClass::of(Some("Opaque,Prop,PunchThrough")),
            LightClass::Prop
        );
        assert_eq!(LightClass::of(Some("Opaque,Static1")), LightClass::Level);
        assert_eq!(LightClass::of(None), LightClass::Level);
    }

    #[test]
    fn a_rooms_lights_reach_the_rooms_it_lists() {
        let rooms: HashMap<i64, Vec<i64>> = [(10, vec![47, 48]), (20, vec![]), (30, vec![-1])]
            .into_iter()
            .collect();
        let lights = vec![
            point(1.0, 600.0, Some(1), Some(10)),
            point(2.0, 600.0, Some(1), Some(20)),
            point(3.0, 600.0, Some(1), Some(30)),
            point(4.0, 600.0, Some(1), Some(99)),
            point(5.0, 600.0, Some(1), None),
        ];
        let at = |room| {
            xs(&pick_lights(
                &lights,
                &rooms,
                (Vec3::ZERO, 1.0),
                room,
                LightClass::Character,
            )
            .points)
        };
        assert_eq!(at(Some(47)), [1.0, 3.0, 5.0]);
        assert_eq!(at(Some(20)), [2.0, 3.0, 5.0]);
        assert_eq!(at(None), [1.0, 2.0, 3.0]);
        assert!(!reaches(Some(20), &rooms, Some(47)));
        assert!(reaches(Some(99), &rooms, Some(99)));
    }

    #[test]
    fn only_props_take_the_brightest_ambient() {
        let amb = |c: f32| SceneLight::Ambient {
            color: Vec3::splat(c),
            name: String::new(),
            room: String::new(),
        };
        let lights = vec![amb(0.2), amb(0.5), amb(0.3)];
        assert_eq!(ambient_of(&lights, LightClass::Prop), Vec3::splat(0.5));
        assert_eq!(ambient_of(&lights, LightClass::Character), Vec3::ZERO);
        assert_eq!(ambient_of(&lights, LightClass::Level), Vec3::ZERO);
    }
}
