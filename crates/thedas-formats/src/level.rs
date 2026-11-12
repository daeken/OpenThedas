//! Environment layouts: the area layout (`.arl`, GFF4 `ARL `) and its room layouts (`.rml`,
//! GFF4 `RML `).
//!
//! An `.arl` lives loose in `packages/core/env/` and lists the level's rooms. Each room's
//! `.rml` (inside `env/<level>/<level>.rim`) places models in the room and binds their mesh
//! chunks to regions of the room's lightmap atlases.
//!
//! Provisional: every field label below was identified from the data (names, value ranges and
//! cross-checks such as the stored world-space bounding spheres), not from the original
//! executable. The room transform rule (`world = room * placement`) matches the stored sphere
//! centres (`3321`) on the levels checked.

use crate::gff4::{Gff4, StructId};
use crate::typed::{Doc, TypedResult};

pub mod field {
    //! GFF4 field labels used by the layout readers. All provisional.

    pub const ARL_NAME: u32 = 3012;
    pub const ARL_ROOMS: u32 = 3016;
    pub const ARL_PATHING: u32 = 3020;
    pub const ARL_FRAME_EFFECTS: u32 = 3026;
    pub const ARL_SKY_MODEL: u32 = 3025;
    pub const ARL_ATMOSPHERE: u32 = 22500;
    pub const ATMO_SUN_COLOR: u32 = 22519;
    pub const ATMO_SUN_INTENSITY: u32 = 22520;
    pub const ATMO_TURBIDITY: u32 = 22521;
    pub const ATMO_EARTH_REFLECTANCE: u32 = 22522;
    pub const ATMO_MIE: u32 = 22523;
    pub const ATMO_RAYLEIGH: u32 = 22524;
    pub const ATMO_DISTANCE: u32 = 22526;
    pub const ATMO_ALPHA: u32 = 22528;
    pub const ATMO_FOG_COLOR: u32 = 22529;
    pub const ATMO_FOG_INTENSITY: u32 = 22530;
    pub const ATMO_FOG_CAP: u32 = 22531;
    pub const ATMO_FOG_ZENITH: u32 = 22532;
    pub const ATMO_FOG_WATER_INTENSITY: u32 = 22533;
    pub const ATMO_FOG_WATER_CAP: u32 = 22534;
    pub const ATMO_FOG_TACTICAL: u32 = 22535;
    pub const ATMO_MOON_SCALE: u32 = 22700;
    pub const ATMO_MOON_ALPHA: u32 = 22701;
    pub const ATMO_MOON_CLOUD_ALPHA: u32 = 22702;
    pub const ATMO_MOON_ROTATION: u32 = 22703;
    pub const ARL_CLOUDS: u32 = 22600;
    pub const CLOUD_DENSITY: u32 = 22620;
    pub const CLOUD_SHARPNESS: u32 = 22621;
    pub const CLOUD_DEPTH: u32 = 22622;
    pub const CLOUD_RANGE_1: u32 = 22623;
    pub const CLOUD_RANGE_2: u32 = 22624;
    pub const CLOUD_COLOR: u32 = 22625;
    pub const FBE_NAME: u32 = 5520;
    pub const FBE_EFFECT: u32 = 5521;
    pub const FBE_PARAMETERS: u32 = 5522;
    pub const FBEP_NAME: u32 = 5523;
    pub const FBEP_VALUE: u32 = 5524;
    pub const ARL_SUN_COLOR_CHAR: u32 = 3149;
    pub const ARL_SUN_DIRECTION: u32 = 3150;
    pub const ARL_SUN_COLOR: u32 = 3152;
    /// xoreos's "EnvAreaChunkIsChunk"; DAOrigins.exe's layout loader takes the sun from the
    /// layout only when it is set (0x5d1617).
    pub const ARL_LAYOUT_SUN: u32 = 3122;
    pub const PATHING_GRID: u32 = 3110;
    pub const GRID_COLUMNS: u32 = 3086;
    pub const GRID_ROWS: u32 = 3087;
    pub const GRID_CELL_SIZE: u32 = 3088;
    pub const GRID_CLEARANCE: u32 = 3089;
    pub const GRID_BASE: u32 = 3090;
    pub const GRID_NORMAL: u32 = 3091;
    pub const GRID_DATA: u32 = 3092;
    pub const GRID_HEIGHT: u32 = 3093;
    pub const ROOM_ID: u32 = 3031;
    pub const ROOM_NAME: u32 = 3033;
    pub const ROOM_BOUNDS_MIN: u32 = 3140;
    pub const ROOM_BOUNDS_MAX: u32 = 3141;

    pub const RML_MODELS: u32 = 3050;
    pub const POSITION: u32 = 4;
    pub const ROTATION: u32 = 5;
    pub const MDL_SCALE: u32 = 3059;
    pub const MDL_ID: u32 = 3061;
    pub const MDL_NAME: u32 = 3062;
    pub const MDL_RESOURCE: u32 = 3063;
    pub const MDL_LIGHTMAP_NAME: u32 = 3320;
    pub const MDL_SPHERE_CENTER: u32 = 3321;
    pub const MDL_SPHERE_RADIUS: u32 = 3322;
    pub const MDL_LIGHTMAPS: u32 = 3326;
    /// The layout's SpeedTree list (`SPT ` structs naming a tree, 3353); rooms' trees index
    /// it.
    pub const ARL_TREES: u32 = 3355;
    pub const ARL_TREE_NAME: u32 = 3353;
    /// A room's placed trees (`SPT ` structs: 4 position, 5 rotation, 3356 scale, 3357 index
    /// into the layout's list, 3375 colour, 3379 a flag).
    pub const RML_TREES: u32 = 3354;
    pub const TREE_SCALE: u32 = 3356;
    pub const TREE_INDEX: u32 = 3357;
    pub const TREE_COLOR: u32 = 3375;
    pub const TREE_FLAG: u32 = 3379;
    /// A placed model's user parameters (`UPAR` structs: 3507 name, 3508 value), which its
    /// emitters naming that parameter take.
    pub const MDL_USER_PARAMS: u32 = 3506;
    pub const UPAR_NAME: u32 = 3507;
    pub const UPAR_VALUE: u32 = 3508;
    pub const LMAT_CHUNK: u32 = 3235;
    pub const LMAT_TEXTURE: u32 = 3323;
    pub const LMAT_OFFSET_SCALE: u32 = 3324;
}

const IDENTITY_QUAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// A rigid transform: translation, then a rotation quaternion `[x, y, z, w]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: IDENTITY_QUAT,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArlRoom {
    pub id: i64,
    /// The room's resource stem: `<name>.rml` in the level rim, `<name>.rim` and
    /// `<name>.gpu.rim` beside it.
    pub name: String,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
}

/// An area layout.
#[derive(Clone, Debug, PartialEq)]
pub struct Arl {
    pub name: Option<String>,
    pub rooms: Vec<ArlRoom>,
    /// The pathing grid the engine routes creatures on, when the layout has one.
    pub grid: Option<NavGrid>,
    /// The level's sun, when it has one (exteriors).
    pub sun: Option<Sun>,
    /// The sunlight direction (3150) as stored, whatever [`Arl::sun`] makes of it: the
    /// direction the sky's celestial body is placed against (DAOrigins.exe's layout loader
    /// keeps it at 0x5d14e4 whether or not the sun lights anything).
    pub sunlight_direction: Option<[f32; 3]>,
    /// The frame buffer effects the level runs over each frame, in the order listed.
    pub effects: Vec<FrameEffect>,
    /// The sky model drawn around the camera (3025, e.g. `sb_day`), when the level has one.
    pub sky_model: Option<String>,
    /// The level's atmosphere (22500): sky scattering and fog, when it has one.
    pub atmosphere: Option<Atmosphere>,
    /// The level's clouds (22600), when it has them.
    pub clouds: Option<Clouds>,
    /// The SpeedTrees the rooms place (`.spt` stems, 3355), indexed by
    /// [`TreePlacement::tree`].
    pub trees: Vec<String>,
}

/// A level's clouds (`CLDS`, 22600), which the sky's cloud layer reads.
///
/// Field names are xoreos's; DAOrigins.exe's loader (0x5d07bc) reads each as 0 when absent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clouds {
    pub density: f32,
    pub sharpness: f32,
    pub depth: f32,
    pub range_1: f32,
    pub range_2: f32,
    pub color: [f32; 3],
}

/// A level's atmosphere (`ATMO`, 22500): what the engine computes the sky's scattering
/// and the fog from.
///
/// Field names are xoreos's (read for the format only). Which fields the engine reads, and
/// their defaults, are DAOrigins.exe's loader's (0x5d00ce): every field reads 0 when absent
/// except `alpha` (1), the water fog (the fog's own intensity and cap), the celestial body
/// (the atmosphere preset 0x55c910 keeps scale 10, the rest 0) and the sun colour (see
/// [`Atmosphere::sun_color`]); 22525 (in-scatter power) and 22527 (phase eccentricity) are
/// not read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Atmosphere {
    /// Field 22519. When absent, the loader takes the layout's sunlight colour (3152) if its
    /// luminance (`0.299 |r| + 0.587 |g| + 0.114 |b|`) exceeds 0.1, white otherwise
    /// (0x5d06d6).
    pub sun_color: [f32; 3],
    pub sun_intensity: f32,
    pub turbidity: f32,
    pub earth_reflectance: f32,
    pub mie: f32,
    pub rayleigh: f32,
    pub distance: f32,
    pub alpha: f32,
    pub fog_color: [f32; 3],
    pub fog_intensity: f32,
    /// The most the fog covers (0-1).
    pub fog_cap: f32,
    /// Degrees.
    pub fog_zenith: f32,
    /// How much of the fog the tactical (zoomed-out) camera sees.
    pub fog_tactical: f32,
    /// The water fog's intensity (22533; the fog's when absent).
    pub water_fog_intensity: f32,
    /// The water fog's cap (22534; the fog's when absent).
    pub water_fog_cap: f32,
    /// The celestial body the sky draws (the sky material's `CelestialBody` texture: the
    /// sun on `sb_day`): its size (22700, as a divisor of the dome's coordinates), its
    /// opacity (22701), its opacity through clouds (22702) and its rotation about its own
    /// axis (22703, radians). xoreos calls these the moon's.
    pub moon_scale: f32,
    pub moon_alpha: f32,
    pub moon_cloud_alpha: f32,
    pub moon_rotation: f32,
}

/// One of a level's frame buffer effects: the `.ffx.xml` effect it names (`da_bloom`,
/// `da_hsvmatrix`, `da_fade`, `da_dof`) and the values it gives that effect's exposed
/// parameters.
///
/// Provisional: the labels were identified from the data. 3026 lists `FBE ` structs; 5520
/// and 5521 both hold the effect's id (equal on every level seen; 5521 is read when 5520 is
/// missing); 5522 lists `FBEP` structs of a parameter name (5523) and a generic value
/// (5524), a float on every level seen. Entries with no id and non-numeric values are
/// skipped.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameEffect {
    pub name: String,
    pub parameters: Vec<(String, f32)>,
}

impl FrameEffect {
    /// The value the level gives parameter `name` (case-insensitive).
    pub fn parameter(&self, name: &str) -> Option<f32> {
        self.parameters
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| *v)
    }
}

/// A level's sun: the direction its light travels (normalized, world space), its colour on
/// the level's surfaces and its colour on characters.
///
/// As DAOrigins.exe's layout loader reads them (0x5d1448-0x5d1574): 3152 the colour, 3150
/// the direction, 3149 the characters' colour (3152's when absent). The light manager
/// (0x4625d0) lights with them only in layouts that set 3122 (mode 1, 0x5d168e); every
/// other layout (interiors, whatever colours they store) gets a black directional light
/// (0x456320), which we give as no sun. 3151 and 3153 (xoreos's "enabled" and
/// "multiplier") are not read by the engine, nor present in any layout seen. Field names are
/// xoreos's (read for the format only). Provisional: the normalization (the engine uploads
/// the stored vector; every layout seen stores a unit one); a black or directionless sun
/// is dropped, which draws the same as the engine's zero light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sun {
    pub direction: [f32; 3],
    pub color: [f32; 3],
    pub character_color: [f32; 3],
}

/// The layout's pathing grid: `columns x rows` square cells of `cell_size` metres from
/// `base` (the grid's minimum corner), one data byte and one height per cell, row-major.
///
/// Provisional: field meanings come from their names in xoreos's GFF4 field list (read for
/// the format only) and from the data itself; the meaning of the data byte's bits is not
/// known yet beyond 0 being unwalkable.
#[derive(Clone, Debug, PartialEq)]
pub struct NavGrid {
    pub columns: u32,
    pub rows: u32,
    pub cell_size: f32,
    /// Headroom a creature needs, in metres.
    pub clearance: f32,
    pub base: [f32; 3],
    pub normal: [f32; 3],
    pub data: Vec<u8>,
    pub heights: Vec<f32>,
}

impl NavGrid {
    /// The cell holding a world position, if inside the grid.
    pub fn cell(&self, p: [f32; 3]) -> Option<(u32, u32)> {
        let x = ((p[0] - self.base[0]) / self.cell_size).floor();
        let y = ((p[1] - self.base[1]) / self.cell_size).floor();
        (x >= 0.0 && y >= 0.0 && (x as u32) < self.columns && (y as u32) < self.rows)
            .then_some((x as u32, y as u32))
    }

    /// The centre of a cell, at its stored height when there is one.
    pub fn center(&self, (x, y): (u32, u32)) -> [f32; 3] {
        let z = self
            .heights
            .get((y * self.columns + x) as usize)
            .copied()
            .unwrap_or(self.base[2]);
        [
            self.base[0] + (x as f32 + 0.5) * self.cell_size,
            self.base[1] + (y as f32 + 0.5) * self.cell_size,
            z,
        ]
    }

    pub fn value(&self, (x, y): (u32, u32)) -> u8 {
        self.data
            .get((y * self.columns + x) as usize)
            .copied()
            .unwrap_or(0)
    }
}

impl Arl {
    pub fn parse(data: &[u8]) -> TypedResult<Self> {
        Self::from_gff4(&Gff4::parse(data)?)
    }

    pub fn from_gff4(gff: &Gff4) -> TypedResult<Self> {
        let d = Doc::new(gff, "arl");
        d.expect_type("ARL ")?;
        let root = gff.root;
        let mut rooms = Vec::new();
        for r in d.structs(root, field::ARL_ROOMS)? {
            let d = Doc::new(gff, "arl room");
            rooms.push(ArlRoom {
                id: d.opt_i64(r, field::ROOM_ID)?.unwrap_or(-1),
                name: d.str(r, field::ROOM_NAME)?,
                bounds_min: d.opt_vec3(r, field::ROOM_BOUNDS_MIN)?.unwrap_or_default(),
                bounds_max: d.opt_vec3(r, field::ROOM_BOUNDS_MAX)?.unwrap_or_default(),
            });
        }
        let grid = match d.opt_struct(root, field::ARL_PATHING)? {
            Some(p) => match d.opt_struct(p, field::PATHING_GRID)? {
                Some(g) => {
                    let n = |l| d.opt_i64(g, l).map(|x| x.unwrap_or(0).max(0) as u32);
                    let (columns, rows) = (n(field::GRID_COLUMNS)?, n(field::GRID_ROWS)?);
                    let data = d.bytes(g, field::GRID_DATA)?.to_vec();
                    let cells = columns as usize * rows as usize;
                    let heights = d.numbers(g, field::GRID_HEIGHT)?;
                    (data.len() == cells && cells > 0).then(|| NavGrid {
                        columns,
                        rows,
                        cell_size: d
                            .opt_f32(g, field::GRID_CELL_SIZE)
                            .ok()
                            .flatten()
                            .unwrap_or(0.5),
                        clearance: d
                            .opt_f32(g, field::GRID_CLEARANCE)
                            .ok()
                            .flatten()
                            .unwrap_or(2.0),
                        base: d
                            .opt_vec3(g, field::GRID_BASE)
                            .ok()
                            .flatten()
                            .unwrap_or_default(),
                        normal: d
                            .opt_vec3(g, field::GRID_NORMAL)
                            .ok()
                            .flatten()
                            .unwrap_or([0.0, 0.0, 1.0]),
                        data,
                        heights: if heights.len() == cells {
                            heights
                        } else {
                            Vec::new()
                        },
                    })
                }
                None => None,
            },
            None => None,
        };
        let layout_sun = d
            .opt_i64(root, field::ARL_LAYOUT_SUN)?
            .is_some_and(|v| v != 0);
        let color = d.opt_vec3(root, field::ARL_SUN_COLOR)?;
        let character_color = d.opt_vec3(root, field::ARL_SUN_COLOR_CHAR)?;
        let sun = match (
            layout_sun,
            color,
            d.opt_vec3(root, field::ARL_SUN_DIRECTION)?,
        ) {
            (true, Some(color), Some([x, y, z])) => {
                let character_color = character_color.unwrap_or(color);
                let len = (x * x + y * y + z * z).sqrt();
                let lit = color.iter().chain(&character_color).any(|c| *c > 0.0);
                let finite = color.iter().chain(&character_color).all(|c| c.is_finite());
                (len.is_finite() && len > 1e-6 && lit && finite).then(|| Sun {
                    direction: [x / len, y / len, z / len],
                    color,
                    character_color,
                })
            }
            _ => None,
        };
        let mut effects = Vec::new();
        for e in d.structs(root, field::ARL_FRAME_EFFECTS)? {
            let name = match d.opt_str(e, field::FBE_NAME)? {
                Some(n) if !n.is_empty() => n,
                _ => match d.opt_str(e, field::FBE_EFFECT)? {
                    Some(n) if !n.is_empty() => n,
                    _ => continue,
                },
            };
            let mut parameters = Vec::new();
            for p in d.structs(e, field::FBE_PARAMETERS)? {
                let Some(pname) = d.opt_str(p, field::FBEP_NAME)?.filter(|n| !n.is_empty()) else {
                    continue;
                };
                if let Some(v) = d.get(p, field::FBEP_VALUE).and_then(|v| v.as_f64()) {
                    parameters.push((pname, v as f32));
                }
            }
            effects.push(FrameEffect { name, parameters });
        }
        let atmosphere = match d.opt_struct(root, field::ARL_ATMOSPHERE)? {
            Some(a) => {
                let o = |l| -> TypedResult<Option<f32>> {
                    Ok(d.opt_f32(a, l)?.filter(|v| v.is_finite()))
                };
                let f = |l| -> TypedResult<f32> { Ok(o(l)?.unwrap_or(0.0)) };
                let oc = |l| -> TypedResult<Option<[f32; 3]>> {
                    Ok(d.opt_vec3(a, l)?
                        .filter(|v| v.iter().all(|x| x.is_finite())))
                };
                let c = |l| -> TypedResult<[f32; 3]> { Ok(oc(l)?.unwrap_or([0.0; 3])) };
                let sun_color = match oc(field::ATMO_SUN_COLOR)? {
                    Some(c) => c,
                    None => {
                        let l = d
                            .opt_vec3(root, field::ARL_SUN_COLOR)?
                            .filter(|v| v.iter().all(|x| x.is_finite()))
                            .unwrap_or([0.0; 3]);
                        let lum = 0.299 * l[0].abs() + 0.587 * l[1].abs() + 0.114 * l[2].abs();
                        if lum > 0.1 { l } else { [1.0; 3] }
                    }
                };
                let fog_intensity = f(field::ATMO_FOG_INTENSITY)?;
                let fog_cap = f(field::ATMO_FOG_CAP)?;
                Some(Atmosphere {
                    sun_color,
                    sun_intensity: f(field::ATMO_SUN_INTENSITY)?,
                    turbidity: f(field::ATMO_TURBIDITY)?,
                    earth_reflectance: f(field::ATMO_EARTH_REFLECTANCE)?,
                    mie: f(field::ATMO_MIE)?,
                    rayleigh: f(field::ATMO_RAYLEIGH)?,
                    distance: f(field::ATMO_DISTANCE)?,
                    alpha: d
                        .opt_f32(a, field::ATMO_ALPHA)?
                        .filter(|v| v.is_finite())
                        .unwrap_or(1.0),
                    fog_color: c(field::ATMO_FOG_COLOR)?,
                    fog_intensity,
                    fog_cap,
                    fog_zenith: f(field::ATMO_FOG_ZENITH)?,
                    fog_tactical: f(field::ATMO_FOG_TACTICAL)?,
                    water_fog_intensity: o(field::ATMO_FOG_WATER_INTENSITY)?
                        .unwrap_or(fog_intensity),
                    water_fog_cap: o(field::ATMO_FOG_WATER_CAP)?.unwrap_or(fog_cap),
                    moon_scale: o(field::ATMO_MOON_SCALE)?.unwrap_or(10.0),
                    moon_alpha: f(field::ATMO_MOON_ALPHA)?,
                    moon_cloud_alpha: f(field::ATMO_MOON_CLOUD_ALPHA)?,
                    moon_rotation: f(field::ATMO_MOON_ROTATION)?,
                })
            }
            None => None,
        };
        let clouds = match d.opt_struct(root, field::ARL_CLOUDS)? {
            Some(c) => {
                let f = |l| -> TypedResult<f32> {
                    Ok(d.opt_f32(c, l)?.filter(|v| v.is_finite()).unwrap_or(0.0))
                };
                Some(Clouds {
                    density: f(field::CLOUD_DENSITY)?,
                    sharpness: f(field::CLOUD_SHARPNESS)?,
                    depth: f(field::CLOUD_DEPTH)?,
                    range_1: f(field::CLOUD_RANGE_1)?,
                    range_2: f(field::CLOUD_RANGE_2)?,
                    color: d
                        .opt_vec3(c, field::CLOUD_COLOR)?
                        .filter(|v| v.iter().all(|x| x.is_finite()))
                        .unwrap_or([0.0; 3]),
                })
            }
            None => None,
        };
        Ok(Arl {
            name: d.opt_str(root, field::ARL_NAME)?,
            rooms,
            grid,
            sun,
            sunlight_direction: d
                .opt_vec3(root, field::ARL_SUN_DIRECTION)?
                .filter(|v| v.iter().all(|x| x.is_finite())),
            effects,
            sky_model: d
                .opt_str(root, field::ARL_SKY_MODEL)?
                .filter(|m| !m.is_empty()),
            atmosphere,
            clouds,
            trees: d
                .structs(root, field::ARL_TREES)?
                .into_iter()
                .map(|t| Ok(d.opt_str(t, field::ARL_TREE_NAME)?.unwrap_or_default()))
                .collect::<TypedResult<_>>()?,
        })
    }

    /// The union of all room bounds, if there are rooms.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut it = self.rooms.iter();
        let first = it.next()?;
        let (mut lo, mut hi) = (first.bounds_min, first.bounds_max);
        for r in it {
            for i in 0..3 {
                lo[i] = lo[i].min(r.bounds_min[i]);
                hi[i] = hi[i].max(r.bounds_max[i]);
            }
        }
        Some((lo, hi))
    }
}

/// Binds one mesh chunk of a placed model to a region of a lightmap atlas.
#[derive(Clone, Debug, PartialEq)]
pub struct LightmapBinding {
    /// Matches the lightmap id of a mesh piece in the model (`_7u` and similar).
    pub chunk: String,
    /// The atlas texture, e.g. `lgt101d_room000.dds`.
    pub texture: String,
    /// Atlas UV = mesh lightmap UV * scale + offset. The materials name this parameter
    /// `mml_vLightmapOffsetAndScale`, which fixes the component order.
    pub offset: [f32; 2],
    pub scale: [f32; 2],
}

/// One model placed in a room.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub id: Option<i64>,
    /// The base name, e.g. `tve_pil_05`.
    pub name: Option<String>,
    /// The model resource stem (`<model>.mmh`), e.g. `tve_pil_05_0`. Falls back to `name`
    /// when the resource field is missing.
    pub model: String,
    /// Relative to the room.
    pub transform: Transform,
    pub scale: f32,
    pub lightmap_name: Option<String>,
    /// World-space bounding sphere as stored by the toolset.
    pub sphere: Option<([f32; 3], f32)>,
    pub lightmaps: Vec<LightmapBinding>,
    /// Named values the placement gives its model (`UPAR`), e.g. char_stage's floor fog
    /// emitter's `fog`.
    pub user_params: Vec<(String, [f32; 4])>,
}

/// A room layout.
#[derive(Clone, Debug, PartialEq)]
pub struct Rml {
    /// Where the room sits in the level; placements are relative to it.
    pub transform: Transform,
    pub placements: Vec<Placement>,
    /// The trees, bushes and grass the room places.
    pub trees: Vec<TreePlacement>,
}

/// A SpeedTree placed in a room (`SPT `, 3354). Provisional: field meanings are read from
/// the data (3356 values near 0.3–1 per instance, 3357 small indices into the layout's tree
/// list), not traced in DAOrigins.exe.
#[derive(Clone, Debug, PartialEq)]
pub struct TreePlacement {
    /// Index into [`Arl::trees`].
    pub tree: usize,
    /// Relative to the room.
    pub transform: Transform,
    pub scale: f32,
    /// A per-instance colour (rgba), e.g. a tint.
    pub color: [f32; 4],
    pub flag: i64,
}

impl Rml {
    pub fn parse(data: &[u8]) -> TypedResult<Self> {
        Self::from_gff4(&Gff4::parse(data)?)
    }

    pub fn from_gff4(gff: &Gff4) -> TypedResult<Self> {
        let d = Doc::new(gff, "rml");
        d.expect_type("RML ")?;
        let root = gff.root;
        let transform = read_transform(&d, root)?;
        let mut placements = Vec::new();
        for m in d.structs(root, field::RML_MODELS)? {
            placements.push(read_placement(gff, m)?);
        }
        let mut trees = Vec::new();
        for t in d.structs(root, field::RML_TREES)? {
            let index = d.opt_i64(t, field::TREE_INDEX)?.unwrap_or(0);
            let tree = usize::try_from(index).map_err(|_| {
                crate::typed::TypedError::Invalid(format!("rml tree: index {index}"))
            })?;
            trees.push(TreePlacement {
                tree,
                transform: read_transform(&d, t)?,
                scale: d.opt_f32(t, field::TREE_SCALE)?.unwrap_or(1.0),
                color: d.opt_vec4(t, field::TREE_COLOR, 1.0)?.unwrap_or([1.0; 4]),
                flag: d.opt_i64(t, field::TREE_FLAG)?.unwrap_or(0),
            });
        }
        Ok(Rml {
            transform,
            placements,
            trees,
        })
    }
}

fn read_transform(d: &Doc, id: StructId) -> TypedResult<Transform> {
    Ok(Transform {
        position: d.opt_vec3(id, field::POSITION)?.unwrap_or_default(),
        rotation: d
            .opt_vec4(id, field::ROTATION, 1.0)?
            .unwrap_or(IDENTITY_QUAT),
    })
}

fn read_placement(gff: &Gff4, m: StructId) -> TypedResult<Placement> {
    let d = Doc::new(gff, "rml model");
    let name = d.opt_str(m, field::MDL_NAME)?.filter(|s| !s.is_empty());
    let resource = d.opt_str(m, field::MDL_RESOURCE)?.filter(|s| !s.is_empty());
    let model = resource.or_else(|| name.clone()).ok_or_else(|| {
        crate::typed::TypedError::Invalid("rml model: no model name (3062/3063)".into())
    })?;
    let sphere = match (
        d.opt_vec3(m, field::MDL_SPHERE_CENTER)?,
        d.opt_f32(m, field::MDL_SPHERE_RADIUS)?,
    ) {
        (Some(c), Some(r)) => Some((c, r)),
        _ => None,
    };
    let mut lightmaps = Vec::new();
    for l in d.structs(m, field::MDL_LIGHTMAPS)? {
        let d = Doc::new(gff, "rml lightmap");
        let [ox, oy, sx, sy] = d
            .opt_vec4(l, field::LMAT_OFFSET_SCALE, 0.0)?
            .unwrap_or([0.0, 0.0, 1.0, 1.0]);
        lightmaps.push(LightmapBinding {
            chunk: d.str(l, field::LMAT_CHUNK)?,
            texture: d.str(l, field::LMAT_TEXTURE)?,
            offset: [ox, oy],
            scale: [sx, sy],
        });
    }
    let mut user_params = Vec::new();
    for u in d.structs(m, field::MDL_USER_PARAMS)? {
        let d = Doc::new(gff, "rml user parameter");
        let Some(name) = d.opt_str(u, field::UPAR_NAME)?.filter(|s| !s.is_empty()) else {
            continue;
        };
        let value = d.opt_vec4(u, field::UPAR_VALUE, 1.0)?.unwrap_or([1.0; 4]);
        user_params.push((name, value));
    }
    Ok(Placement {
        id: d.opt_i64(m, field::MDL_ID)?,
        name,
        model,
        transform: read_transform(&d, m)?,
        scale: d.opt_f32(m, field::MDL_SCALE)?.unwrap_or(1.0),
        lightmap_name: d.opt_str(m, field::MDL_LIGHTMAP_NAME)?,
        sphere,
        lightmaps,
        user_params,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gff4::Value;
    use crate::typed::testutil::{Builder, generic, s, structs};

    #[test]
    fn reads_area_layout() {
        let mut b = Builder::new(b"ARL ");
        let r0 = b.add(
            b"ROOM",
            vec![
                (field::ROOM_ID, Value::I32(0)),
                (field::ROOM_NAME, s("lvl_0")),
                (field::ROOM_BOUNDS_MIN, Value::Vec3([-1.0, -2.0, -3.0])),
                (field::ROOM_BOUNDS_MAX, Value::Vec3([1.0, 2.0, 3.0])),
            ],
        );
        let r1 = b.add(
            b"ROOM",
            vec![
                (field::ROOM_ID, Value::I32(7)),
                (field::ROOM_NAME, s("lvl_7")),
                (field::ROOM_BOUNDS_MIN, Value::Vec3([0.0, 0.0, 0.0])),
                (field::ROOM_BOUNDS_MAX, Value::Vec3([5.0, 1.0, 1.0])),
            ],
        );
        let root = b.add(
            b"AREA",
            vec![
                (field::ARL_NAME, s("Test Area")),
                (field::ARL_ROOMS, structs(&[r0, r1])),
            ],
        );
        let gff = b.finish(root);
        let arl = Arl::from_gff4(&gff).unwrap();
        assert_eq!(arl.name.as_deref(), Some("Test Area"));
        assert_eq!(arl.rooms.len(), 2);
        assert_eq!(arl.rooms[1].name, "lvl_7");
        assert_eq!(arl.bounds(), Some(([-1.0, -2.0, -3.0], [5.0, 2.0, 3.0])));
        assert_eq!(arl.sun, None);
        assert!(arl.effects.is_empty());
        assert!(arl.trees.is_empty());
    }

    #[test]
    fn reads_trees() {
        let mut b = Builder::new(b"ARL ");
        let t0 = b.add(b"SPT ", vec![(field::ARL_TREE_NAME, s("lvl_tre_pine"))]);
        let t1 = b.add(b"SPT ", vec![(field::ARL_TREE_NAME, s("lvl_gra_short"))]);
        let root = b.add(b"AREA", vec![(field::ARL_TREES, structs(&[t0, t1]))]);
        let arl = Arl::from_gff4(&b.finish(root)).unwrap();
        assert_eq!(arl.trees, vec!["lvl_tre_pine", "lvl_gra_short"]);

        let mut b = Builder::new(b"RML ");
        let p = b.add(
            b"SPT ",
            vec![
                (field::POSITION, Value::Vec3([1.0, 2.0, 3.0])),
                (field::ROTATION, Value::Quat([0.0, 0.0, 0.6, 0.8])),
                (field::TREE_SCALE, Value::F32(0.35)),
                (field::TREE_INDEX, Value::I32(1)),
                (field::TREE_COLOR, Value::Color([0.1, 0.2, 0.3, 1.0])),
            ],
        );
        let root = b.add(b"ROOM", vec![(field::RML_TREES, structs(&[p]))]);
        let rml = Rml::from_gff4(&b.finish(root)).unwrap();
        assert_eq!(rml.trees.len(), 1);
        let t = &rml.trees[0];
        assert_eq!((t.tree, t.scale, t.flag), (1, 0.35, 0));
        assert_eq!(t.transform.position, [1.0, 2.0, 3.0]);
        assert_eq!(t.color, [0.1, 0.2, 0.3, 1.0]);

        let mut b = Builder::new(b"RML ");
        let p = b.add(b"SPT ", vec![(field::TREE_INDEX, Value::I32(-1))]);
        let root = b.add(b"ROOM", vec![(field::RML_TREES, structs(&[p]))]);
        assert!(Rml::from_gff4(&b.finish(root)).is_err());
    }

    #[test]
    fn reads_an_exteriors_sun() {
        let sun = |fields: Vec<(u32, Value)>| {
            let mut b = Builder::new(b"ARL ");
            let root = b.add(b"AREA", fields);
            Arl::from_gff4(&b.finish(root)).unwrap().sun
        };
        let dir = (field::ARL_SUN_DIRECTION, Value::Vec3([0.0, 0.0, -2.0]));
        let level = (field::ARL_SUN_COLOR, Value::Vec4([0.6, 0.5, 0.3, 1.0]));
        let chars = (field::ARL_SUN_COLOR_CHAR, Value::Vec4([0.4, 0.4, 0.4, 1.0]));
        let outdoors = (field::ARL_LAYOUT_SUN, Value::I32(1));
        let s = sun(vec![
            dir.clone(),
            level.clone(),
            chars.clone(),
            outdoors.clone(),
        ])
        .unwrap();
        assert_eq!(s.direction, [0.0, 0.0, -1.0]);
        assert_eq!(s.color, [0.6, 0.5, 0.3]);
        assert_eq!(s.character_color, [0.4, 0.4, 0.4]);
        // Only a layout that takes its sun from the layout (3122) has one: an interior's
        // stored colours light nothing.
        assert_eq!(
            sun(vec![
                dir.clone(),
                level.clone(),
                (field::ARL_LAYOUT_SUN, Value::I32(0))
            ]),
            None
        );
        assert_eq!(sun(vec![dir.clone(), level.clone()]), None);
        // No characters' colour: the level's.
        let s = sun(vec![dir.clone(), level.clone(), outdoors.clone()]).unwrap();
        assert_eq!(s.character_color, s.color);
        let black = (field::ARL_SUN_COLOR, Value::Vec4([0.0, 0.0, 0.0, 1.0]));
        assert_eq!(sun(vec![dir.clone(), black, outdoors.clone()]), None);
        assert_eq!(
            sun(vec![
                (field::ARL_SUN_DIRECTION, Value::Vec3([0.0; 3])),
                level,
                outdoors
            ]),
            None
        );
    }

    #[test]
    fn reads_room_layout() {
        let mut b = Builder::new(b"RML ");
        let lm = b.add(
            b"LMAT",
            vec![
                (field::LMAT_CHUNK, s("_7u")),
                (field::LMAT_TEXTURE, s("lvl_room000.dds")),
                (
                    field::LMAT_OFFSET_SCALE,
                    Value::Vec4([0.5, 0.25, 0.125, 0.0625]),
                ),
            ],
        );
        let m = b.add(
            b"MDL ",
            vec![
                (field::MDL_NAME, s("wall_01")),
                (field::MDL_RESOURCE, s("wall_01_0")),
                (field::MDL_SCALE, Value::F32(2.0)),
                (field::POSITION, Value::Vec3([1.0, 2.0, 3.0])),
                (field::ROTATION, Value::Quat([0.0, 0.0, 1.0, 0.0])),
                (field::MDL_LIGHTMAPS, generic(&[lm])),
            ],
        );
        // A model with only the base name and no lightmaps.
        let m2 = b.add(b"MDL ", vec![(field::MDL_NAME, s("prop"))]);
        let root = b.add(
            b"ROOM",
            vec![
                (field::POSITION, Value::Vec3([10.0, 0.0, 0.0])),
                (field::RML_MODELS, structs(&[m, m2])),
            ],
        );
        let rml = Rml::from_gff4(&b.finish(root)).unwrap();
        assert_eq!(rml.transform.position, [10.0, 0.0, 0.0]);
        assert_eq!(rml.transform.rotation, IDENTITY_QUAT);
        let p = &rml.placements[0];
        assert_eq!(p.model, "wall_01_0");
        assert_eq!(p.name.as_deref(), Some("wall_01"));
        assert_eq!(p.scale, 2.0);
        assert_eq!(p.transform.rotation, [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(
            p.lightmaps,
            vec![LightmapBinding {
                chunk: "_7u".into(),
                texture: "lvl_room000.dds".into(),
                offset: [0.5, 0.25],
                scale: [0.125, 0.0625],
            }]
        );
        assert_eq!(rml.placements[1].model, "prop");
        assert_eq!(rml.placements[1].scale, 1.0);
    }

    #[test]
    fn errors_instead_of_panicking() {
        // Wrong file type.
        let mut b = Builder::new(b"MESH");
        let root = b.add(b"ROOM", vec![]);
        assert!(Rml::from_gff4(&b.finish(root)).is_err());
        // Wrong field type.
        let mut b = Builder::new(b"ARL ");
        let root = b.add(b"AREA", vec![(field::ARL_ROOMS, Value::F32(1.0))]);
        assert!(Arl::from_gff4(&b.finish(root)).is_err());
        // Model with no name at all.
        let mut b = Builder::new(b"RML ");
        let m = b.add(b"MDL ", vec![]);
        let root = b.add(b"ROOM", vec![(field::RML_MODELS, structs(&[m]))]);
        assert!(Rml::from_gff4(&b.finish(root)).is_err());
        // Garbage bytes.
        assert!(Arl::parse(b"not a gff").is_err());
    }

    #[test]
    fn reads_frame_effects() {
        let mut b = Builder::new(b"ARL ");
        let param = |b: &mut Builder, name: &str, v: Value| {
            b.add(
                b"FBEP",
                vec![
                    (field::FBEP_NAME, s(name)),
                    (
                        field::FBEP_VALUE,
                        Value::Generic(Box::new(crate::gff4::GenericSlot {
                            type_code: 8,
                            flags: 0,
                            value: Some(v),
                        })),
                    ),
                ],
            )
        };
        let glow = param(&mut b, "GlowIntensity", Value::F32(0.25));
        let blur = param(&mut b, "HBlurWidth", Value::F32(3.0));
        let odd = param(&mut b, "Label", s("not a number"));
        let bloom = b.add(
            b"FBE ",
            vec![
                (field::FBE_NAME, s("da_bloom")),
                (field::FBE_EFFECT, s("da_bloom")),
                (field::FBE_PARAMETERS, structs(&[glow, blur, odd])),
            ],
        );
        let hsv = b.add(b"FBE ", vec![(field::FBE_EFFECT, s("da_hsvmatrix"))]);
        let empty = b.add(b"FBE ", vec![]);
        let root = b.add(
            b"AREA",
            vec![(field::ARL_FRAME_EFFECTS, structs(&[bloom, hsv, empty]))],
        );
        let arl = Arl::from_gff4(&b.finish(root)).unwrap();
        assert_eq!(arl.effects.len(), 2);
        assert_eq!(arl.effects[0].name, "da_bloom");
        assert_eq!(arl.effects[0].parameter("glowintensity"), Some(0.25));
        assert_eq!(arl.effects[0].parameter("HBlurWidth"), Some(3.0));
        assert_eq!(arl.effects[0].parameter("Label"), None);
        assert_eq!(arl.effects[1].name, "da_hsvmatrix");
        assert!(arl.effects[1].parameters.is_empty());
    }

    #[test]
    fn reads_the_atmosphere_and_sky() {
        let mut b = Builder::new(b"ARL ");
        let atmo = b.add(
            b"ATMO",
            vec![
                (field::ATMO_SUN_COLOR, Value::Vec3([0.74, 0.32, 0.14])),
                (field::ATMO_SUN_INTENSITY, Value::F32(17.4)),
                (field::ATMO_TURBIDITY, Value::F32(1.03)),
                (field::ATMO_FOG_COLOR, Value::Vec3([0.16, 0.19, 0.16])),
                (field::ATMO_FOG_INTENSITY, Value::F32(0.008)),
                (field::ATMO_FOG_CAP, Value::F32(0.65)),
                (field::ATMO_FOG_ZENITH, Value::F32(42.0)),
            ],
        );
        let clouds = b.add(
            b"CLDS",
            vec![
                (field::CLOUD_DENSITY, Value::F32(0.67)),
                (field::CLOUD_COLOR, Value::Vec3([0.12, 0.14, 0.12])),
            ],
        );
        let root = b.add(
            b"AREA",
            vec![
                (field::ARL_SKY_MODEL, s("sb_day")),
                (field::ARL_ATMOSPHERE, Value::Struct(Some(atmo))),
                (field::ARL_CLOUDS, Value::Struct(Some(clouds))),
            ],
        );
        let arl = Arl::from_gff4(&b.finish(root)).unwrap();
        assert_eq!(arl.sky_model.as_deref(), Some("sb_day"));
        let a = arl.atmosphere.unwrap();
        assert_eq!(a.sun_intensity, 17.4);
        assert_eq!(a.fog_cap, 0.65);
        assert_eq!(a.mie, 0.0, "absent fields read 0");
        assert_eq!(a.alpha, 1.0, "absent alpha reads 1");
        assert_eq!(
            a.water_fog_intensity, 0.008,
            "absent water fog is the fog's"
        );
        assert_eq!(a.water_fog_cap, 0.65);
        assert_eq!(a.moon_scale, 10.0, "absent celestial body keeps the preset");
        assert_eq!((a.moon_alpha, a.moon_rotation), (0.0, 0.0));
        assert_eq!(arl.sunlight_direction, None);
        let c = arl.clouds.unwrap();
        assert_eq!(c.density, 0.67);
        assert_eq!(c.color, [0.12, 0.14, 0.12]);
        assert_eq!(c.range_1, 0.0);
    }

    #[test]
    fn reads_the_celestial_body_and_falls_back_to_the_sunlight_colour() {
        let arl = |atmo_fields: Vec<(u32, Value)>, sun: [f32; 3]| {
            let mut b = Builder::new(b"ARL ");
            let atmo = b.add(b"ATMO", atmo_fields);
            let root = b.add(
                b"AREA",
                vec![
                    (field::ARL_ATMOSPHERE, Value::Struct(Some(atmo))),
                    (field::ARL_SUN_COLOR, Value::Vec3(sun)),
                    (field::ARL_SUN_DIRECTION, Value::Vec3([0.0, -0.6, -0.8])),
                ],
            );
            Arl::from_gff4(&b.finish(root)).unwrap()
        };
        let a = arl(
            vec![
                (field::ATMO_MOON_SCALE, Value::F32(4.0)),
                (field::ATMO_MOON_ALPHA, Value::F32(0.75)),
                (field::ATMO_MOON_CLOUD_ALPHA, Value::F32(0.25)),
                (field::ATMO_MOON_ROTATION, Value::F32(1.5)),
                (field::ATMO_FOG_WATER_INTENSITY, Value::F32(0.02)),
                (field::ATMO_FOG_WATER_CAP, Value::F32(0.3)),
            ],
            [0.5, 0.4, 0.3],
        );
        assert_eq!(a.sunlight_direction, Some([0.0, -0.6, -0.8]));
        let m = a.atmosphere.unwrap();
        assert_eq!(
            (
                m.moon_scale,
                m.moon_alpha,
                m.moon_cloud_alpha,
                m.moon_rotation
            ),
            (4.0, 0.75, 0.25, 1.5)
        );
        assert_eq!((m.water_fog_intensity, m.water_fog_cap), (0.02, 0.3));
        // No ATMO sun colour: the layout's sunlight colour, bright enough here...
        assert_eq!(m.sun_color, [0.5, 0.4, 0.3]);
        // ...or white when its luminance is 0.1 or less.
        let dim = arl(vec![], [0.1, 0.1, 0.1]).atmosphere.unwrap();
        assert_eq!(dim.sun_color, [1.0; 3]);
        let own = arl(
            vec![(field::ATMO_SUN_COLOR, Value::Vec3([0.2, 0.0, 0.0]))],
            [0.5, 0.4, 0.3],
        );
        assert_eq!(own.atmosphere.unwrap().sun_color, [0.2, 0.0, 0.0]);
    }
}
