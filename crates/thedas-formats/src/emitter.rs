//! Particle emitters: the `nemt` nodes of a model hierarchy (`.mmh`), with their spawn
//! volume (`spnv`) and age map (`amap` of `amel` keys).
//!
//! Field meanings come from DAOrigins.exe: the toolset's XML-to-GFF converter
//! (0x59e000-0x5a2000) names each field id it writes (`Birthrate` is 6011, `Life` 6013, ...),
//! and the emitter blueprint loader (0x592d11-0x593f0e) shows where each lands. Angles the
//! loader converts (the spreads, by 0.0174533 at 0x593178) are kept in degrees here, as stored;
//! the simulation converts them.

use crate::gff4::{FourCC, Gff4, StructId};
use crate::typed::{Doc, TypedError, TypedResult};

pub mod field {
    //! GFF4 field ids of emitters, named as DAOrigins.exe's converter names them.

    pub const NAME: u32 = 6000;
    pub const MATERIAL_OBJECT: u32 = 6001;
    pub const MATERIAL_LIBRARY: u32 = 6002;
    pub const BIRTHRATE: u32 = 6011;
    pub const BIRTHRATE_RANGE: u32 = 6012;
    pub const LIFE: u32 = 6013;
    pub const LIFE_RANGE: u32 = 6014;
    pub const SCALE_RANGE: u32 = 6015;
    pub const INITIAL_SPEED: u32 = 6016;
    pub const INITIAL_SPEED_RANGE: u32 = 6017;
    pub const ACCELERATION: u32 = 6018;
    pub const INITIAL_ROTATION_SPEED: u32 = 6019;
    pub const INITIAL_ROTATION_SPEED_RANGE: u32 = 6020;
    pub const ROTATIONAL_ACCELERATION: u32 = 6021;
    pub const INV_MOVEMENT_SPREAD_UPDATE_DELAY: u32 = 6022;
    /// The spawn direction's spread about the emitter's X axis, in degrees.
    pub const SPREAD_X: u32 = 6023;
    /// The spawn direction's spread about the emitter's Y axis, in degrees.
    pub const SPREAD_Y: u32 = 6024;
    pub const MOVEMENT_SPREAD_MIN: u32 = 6025;
    pub const MOVEMENT_SPREAD_MAX: u32 = 6026;
    pub const BIRTHRATE_PER_METER: u32 = 6028;
    pub const AFFECTED_BY_WIND: u32 = 6030;
    pub const GRAVITY_MULTIPLIER: u32 = 6031;
    pub const FOLLOW_PATH: u32 = 6032;
    pub const LINK_PARTICLES: u32 = 6033;
    pub const UPDATE_ONLY_WHEN_VISIBLE: u32 = 6034;
    pub const COLLISIONS: u32 = 6035;
    pub const INHERIT_VELOCITY: u32 = 6036;
    pub const ORIENTATION: u32 = 6037;
    pub const PARTICLE_INHERITANCE: u32 = 6038;
    pub const AGE_MAP_COUNT: u32 = 6039;
    pub const KEY_TIME: u32 = 6040;
    pub const KEY_SCALE_X: u32 = 6041;
    pub const KEY_SCALE_Y: u32 = 6042;
    pub const KEY_COLOR: u32 = 6043;
    pub const SPAWN_WITHIN_VOLUME: u32 = 6045;
    pub const INVERT_VOLUME_NORMALS: u32 = 6046;
    pub const FRAMES_PER_SECOND: u32 = 6180;
    /// Rows of the contact sheet (frames down). The pair is told apart by the sheets: the
    /// torch flame's (`fx_firetorchfb`, 8 frames across and 4 down) gives 6181 = 4 and
    /// 6182 = 8.
    pub const FLIPBOOK_ROWS: u32 = 6181;
    /// Columns of the contact sheet (frames across).
    pub const FLIPBOOK_COLUMNS: u32 = 6182;
    pub const RANDOM_START_FRAME: u32 = 6183;
    pub const TARGET_NAME: u32 = 6184;
    pub const TARGET_ATTRACTION: u32 = 6185;
    pub const TARGET_RADIUS: u32 = 6186;
    pub const SPAWN_TRACKS_TARGET: u32 = 6187;
    pub const KILL_AT_TARGET: u32 = 6188;
    /// Whether the material is a contact sheet (flipbook) read by frame.
    pub const FLIPBOOK: u32 = 6189;
    pub const EMITTER_TYPE: u32 = 6234;
    pub const PHYSICS_EMITTER: u32 = 6239;
    pub const PHYSICS_OBJECT_SPAWN: u32 = 6243;
    pub const AGE_MAP_COLOR_MULTIPLIER: u32 = 6279;
    pub const AGE_MAP_SCALE_X_MULTIPLIER: u32 = 6280;
    pub const AGE_MAP_SCALE_Y_MULTIPLIER: u32 = 6281;
    pub const MESH_PARTICLE_MODEL: u32 = 6284;
    pub const VOLUME_SHAPE: u32 = 6285;
    pub const VOLUME_RADIUS: u32 = 6286;
    pub const VOLUME_LENGTH: u32 = 6287;
    pub const VOLUME_AXIS: u32 = 6288;
    pub const VOLUME_MIN_CORNER: u32 = 6289;
    pub const VOLUME_MAX_CORNER: u32 = 6290;
    pub const VOLUME_NORMALS_FOR_DIRECTION: u32 = 6291;
    pub const WORLD_AXIS_ACCELERATION: u32 = 6294;
    pub const OBJECT_SPACE_ACCELERATION: u32 = 6298;
    pub const INITIAL_ROTATION: u32 = 6299;
    pub const INITIAL_ROTATION_RANGE: u32 = 6300;
    pub const SPLAT_EMITTER: u32 = 6321;
    /// The name of the placement's user parameter (`UPAR`) the emitter takes.
    pub const USER_PARAM_NAME: u32 = 6325;
    pub const CHILDREN: u32 = 6999;
}

pub const KIND_SPAWN_VOLUME: FourCC = FourCC::new(b"spnv");
pub const KIND_AGE_MAP: FourCC = FourCC::new(b"amap");
pub const KIND_AGE_KEY: FourCC = FourCC::new(b"amel");

/// Where particles are born (`spnv` 6285; the loader's switch at 0x590996).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum SpawnShape {
    /// At the emitter's origin (no volume, or a shape the loader builds otherwise).
    #[default]
    Point,
    /// 1: a sphere of `radius`.
    Sphere { radius: f32 },
    /// 2: a cylinder of `radius` and `length` along `axis` (a disc when `length` is 0).
    Cylinder {
        radius: f32,
        length: f32,
        axis: [f32; 3],
    },
    /// 3: a box between two corners.
    Box { min: [f32; 3], max: [f32; 3] },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpawnVolume {
    pub shape: SpawnShape,
    /// 6045: anywhere inside rather than on the surface.
    pub within: bool,
    /// 6046: the volume's normals point inwards.
    pub invert_normals: bool,
    /// 6291: particles start moving along the volume's normal.
    pub normals_for_direction: bool,
}

/// One key of an emitter's age map: what a particle looks like at a fraction of its life.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AgeKey {
    /// 6040: the fraction of the particle's life, 0 to 1.
    pub time: f32,
    /// 6041/6042: the particle's width and height.
    pub scale: [f32; 2],
    /// 6043: colour, alpha in `w`.
    pub color: [f32; 4],
}

/// A particle emitter's settings. Defaults are what a node without the field reads as.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Emitter {
    /// The material object (`.mao` stem) its particles draw with.
    pub material: String,
    /// The material library (`.mal`) listing it.
    pub library: Option<String>,
    /// Particles per second (per metre moved with [`Emitter::birthrate_per_meter`]).
    pub birthrate: f32,
    pub birthrate_range: f32,
    /// Seconds.
    pub life: f32,
    pub life_range: f32,
    pub scale_range: f32,
    pub initial_speed: f32,
    pub initial_speed_range: f32,
    /// How fast speed changes, as a fraction of it per second (negative slows).
    pub acceleration: f32,
    pub gravity_multiplier: f32,
    /// Radians.
    pub initial_rotation: f32,
    pub initial_rotation_range: f32,
    /// Turns per second.
    pub rotation_speed: f32,
    pub rotation_speed_range: f32,
    pub rotational_acceleration: f32,
    /// A constant acceleration, in the world's axes (the emitter's with
    /// [`Emitter::object_space_acceleration`]).
    pub world_axis_acceleration: [f32; 3],
    pub object_space_acceleration: bool,
    /// Degrees.
    pub spread: [f32; 2],
    /// Degrees.
    pub movement_spread: [f32; 2],
    pub movement_spread_delay: f32,
    pub target: Option<String>,
    pub target_attraction: f32,
    pub target_radius: f32,
    pub spawn_tracks_target: bool,
    pub kill_at_target: bool,
    /// 0 camera-facing, 1-2 fixed planes, 3 along the velocity, 4 (0x57c4f0).
    pub orientation: i64,
    /// Whether particles move with the emitter after birth.
    pub particle_inheritance: i64,
    /// 0 continuous, 1 a single burst.
    pub emitter_type: i64,
    pub birthrate_per_meter: bool,
    pub affected_by_wind: bool,
    pub follow_path: bool,
    pub link_particles: bool,
    pub update_only_when_visible: bool,
    pub collisions: bool,
    pub inherit_velocity: bool,
    pub physics: bool,
    pub physics_object_spawn: bool,
    pub splat: bool,
    /// A mesh particle emitter's model; such emitters draw meshes, not sprites.
    pub mesh_model: Option<String>,
    pub flipbook: bool,
    pub frames_per_second: f32,
    pub flipbook_columns: u32,
    pub flipbook_rows: u32,
    pub random_start_frame: bool,
    pub age_color_multiplier: [f32; 4],
    pub age_scale_multiplier: [f32; 2],
    pub age_map: Vec<AgeKey>,
    pub volume: SpawnVolume,
    /// The placement's user parameter it takes (`UserParamName`).
    pub user_param: Option<String>,
}

fn flag(d: &Doc<'_>, id: StructId, label: u32) -> TypedResult<bool> {
    Ok(d.opt_i64(id, label)?.unwrap_or(0) != 0)
}

fn nonempty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.is_empty())
}

impl Emitter {
    /// Reads a `nemt` struct.
    #[inline(never)]
    pub fn read(gff: &Gff4, id: StructId) -> TypedResult<Self> {
        use field::*;
        let d = Doc::new(gff, "mmh emitter");
        let f = |label: u32, default: f32| -> TypedResult<f32> {
            Ok(d.opt_f32(id, label)?.unwrap_or(default))
        };
        let small = |label: u32| -> TypedResult<u32> {
            let v = d.opt_i64(id, label)?.unwrap_or(0);
            u32::try_from(v)
                .map_err(|_| TypedError::Invalid(format!("mmh emitter: field {label} is {v}")))
        };
        let mut e = Emitter {
            material: d.opt_str(id, MATERIAL_OBJECT)?.unwrap_or_default(),
            library: nonempty(d.opt_str(id, MATERIAL_LIBRARY)?),
            birthrate: f(BIRTHRATE, 0.0)?,
            birthrate_range: f(BIRTHRATE_RANGE, 0.0)?,
            life: f(LIFE, 0.0)?,
            life_range: f(LIFE_RANGE, 0.0)?,
            scale_range: f(SCALE_RANGE, 0.0)?,
            initial_speed: f(INITIAL_SPEED, 0.0)?,
            initial_speed_range: f(INITIAL_SPEED_RANGE, 0.0)?,
            acceleration: f(ACCELERATION, 0.0)?,
            gravity_multiplier: f(GRAVITY_MULTIPLIER, 0.0)?,
            initial_rotation: f(INITIAL_ROTATION, 0.0)?,
            initial_rotation_range: f(INITIAL_ROTATION_RANGE, 0.0)?,
            rotation_speed: f(INITIAL_ROTATION_SPEED, 0.0)?,
            rotation_speed_range: f(INITIAL_ROTATION_SPEED_RANGE, 0.0)?,
            rotational_acceleration: f(ROTATIONAL_ACCELERATION, 0.0)?,
            world_axis_acceleration: d.opt_vec3(id, WORLD_AXIS_ACCELERATION)?.unwrap_or([0.0; 3]),
            object_space_acceleration: flag(&d, id, OBJECT_SPACE_ACCELERATION)?,
            spread: [f(SPREAD_X, 0.0)?, f(SPREAD_Y, 0.0)?],
            movement_spread: [f(MOVEMENT_SPREAD_MIN, 0.0)?, f(MOVEMENT_SPREAD_MAX, 0.0)?],
            movement_spread_delay: f(INV_MOVEMENT_SPREAD_UPDATE_DELAY, 0.0)?,
            target: nonempty(d.opt_str(id, TARGET_NAME)?),
            target_attraction: f(TARGET_ATTRACTION, 0.0)?,
            target_radius: f(TARGET_RADIUS, 0.0)?,
            spawn_tracks_target: flag(&d, id, SPAWN_TRACKS_TARGET)?,
            kill_at_target: flag(&d, id, KILL_AT_TARGET)?,
            orientation: d.opt_i64(id, ORIENTATION)?.unwrap_or(0),
            particle_inheritance: d.opt_i64(id, PARTICLE_INHERITANCE)?.unwrap_or(0),
            emitter_type: d.opt_i64(id, EMITTER_TYPE)?.unwrap_or(0),
            birthrate_per_meter: flag(&d, id, BIRTHRATE_PER_METER)?,
            affected_by_wind: flag(&d, id, AFFECTED_BY_WIND)?,
            follow_path: flag(&d, id, FOLLOW_PATH)?,
            link_particles: flag(&d, id, LINK_PARTICLES)?,
            update_only_when_visible: flag(&d, id, UPDATE_ONLY_WHEN_VISIBLE)?,
            collisions: flag(&d, id, COLLISIONS)?,
            inherit_velocity: flag(&d, id, INHERIT_VELOCITY)?,
            physics: flag(&d, id, PHYSICS_EMITTER)?,
            physics_object_spawn: flag(&d, id, PHYSICS_OBJECT_SPAWN)?,
            splat: flag(&d, id, SPLAT_EMITTER)?,
            mesh_model: nonempty(d.opt_str(id, MESH_PARTICLE_MODEL)?),
            flipbook: flag(&d, id, FLIPBOOK)?,
            frames_per_second: f(FRAMES_PER_SECOND, 0.0)?,
            flipbook_columns: small(FLIPBOOK_COLUMNS)?,
            flipbook_rows: small(FLIPBOOK_ROWS)?,
            random_start_frame: flag(&d, id, RANDOM_START_FRAME)?,
            age_color_multiplier: d
                .opt_vec4(id, AGE_MAP_COLOR_MULTIPLIER, 1.0)?
                .unwrap_or([1.0; 4]),
            age_scale_multiplier: [
                f(AGE_MAP_SCALE_X_MULTIPLIER, 1.0)?,
                f(AGE_MAP_SCALE_Y_MULTIPLIER, 1.0)?,
            ],
            age_map: Vec::new(),
            volume: SpawnVolume::default(),
            user_param: nonempty(d.opt_str(id, USER_PARAM_NAME)?),
        };
        for c in d.structs(id, CHILDREN)? {
            let kind = d.label(c);
            if kind == KIND_SPAWN_VOLUME {
                e.volume = read_volume(&d, c)?;
            } else if kind == KIND_AGE_MAP {
                for k in d.structs(c, CHILDREN)? {
                    if d.label(k) != KIND_AGE_KEY {
                        continue;
                    }
                    let color = d.opt_vec4(k, KEY_COLOR, 1.0)?.unwrap_or([1.0; 4]);
                    e.age_map.push(AgeKey {
                        time: d.opt_f32(k, KEY_TIME)?.unwrap_or(0.0),
                        scale: [
                            d.opt_f32(k, KEY_SCALE_X)?.unwrap_or(1.0),
                            d.opt_f32(k, KEY_SCALE_Y)?.unwrap_or(1.0),
                        ],
                        color,
                    });
                }
            }
        }
        // The age map is searched in order of time (0x5607c0 walks it from the start).
        e.age_map.sort_by(|a, b| a.time.total_cmp(&b.time));
        Ok(e)
    }
}

fn read_volume(d: &Doc<'_>, id: StructId) -> TypedResult<SpawnVolume> {
    use field::*;
    let radius = d.opt_f32(id, VOLUME_RADIUS)?.unwrap_or(0.0);
    let shape = match d.opt_i64(id, VOLUME_SHAPE)?.unwrap_or(0) {
        1 => SpawnShape::Sphere { radius },
        2 => SpawnShape::Cylinder {
            radius,
            length: d.opt_f32(id, VOLUME_LENGTH)?.unwrap_or(0.0),
            axis: optional_vec3(d, id, VOLUME_AXIS)?.unwrap_or([0.0, 0.0, 1.0]),
        },
        3 => SpawnShape::Box {
            min: optional_vec3(d, id, VOLUME_MIN_CORNER)?.unwrap_or([0.0; 3]),
            max: optional_vec3(d, id, VOLUME_MAX_CORNER)?.unwrap_or([0.0; 3]),
        },
        _ => SpawnShape::Point,
    };
    Ok(SpawnVolume {
        shape,
        within: flag(d, id, SPAWN_WITHIN_VOLUME)?,
        invert_normals: flag(d, id, INVERT_VOLUME_NORMALS)?,
        normals_for_direction: flag(d, id, VOLUME_NORMALS_FOR_DIRECTION)?,
    })
}

/// A vector field that some files store as a list of three (possibly null) numbers.
fn optional_vec3(d: &Doc<'_>, id: StructId, label: u32) -> TypedResult<Option<[f32; 3]>> {
    match d.opt_vec3(id, label) {
        Ok(v) => Ok(v),
        Err(_) => {
            let n = d.numbers(id, label).unwrap_or_default();
            Ok((n.len() >= 3).then(|| [n[0], n[1], n[2]]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gff4::Value;
    use crate::model::{Mmh, field as mf};
    use crate::typed::testutil::{Builder, generic, s};

    #[test]
    fn reads_an_emitter_with_its_volume_and_age_map() {
        let mut b = Builder::new(b"MMH ");
        let k1 = b.add(
            b"amel",
            vec![
                (field::KEY_TIME, Value::F32(1.0)),
                (field::KEY_SCALE_X, Value::F32(2.0)),
                (field::KEY_SCALE_Y, Value::F32(3.0)),
                (field::KEY_COLOR, Value::Color([1.0, 0.5, 0.25, 0.0])),
            ],
        );
        let k0 = b.add(
            b"amel",
            vec![
                (field::KEY_TIME, Value::F32(0.0)),
                (field::KEY_COLOR, Value::Color([1.0, 1.0, 1.0, 1.0])),
            ],
        );
        let amap = b.add(
            b"amap",
            vec![
                (field::AGE_MAP_COUNT, Value::I32(2)),
                (field::CHILDREN, generic(&[k1, k0])),
            ],
        );
        let spnv = b.add(
            b"spnv",
            vec![
                (field::VOLUME_SHAPE, Value::I32(2)),
                (field::VOLUME_RADIUS, Value::F32(0.75)),
                (field::VOLUME_AXIS, Value::Vec3([0.0, 0.0, 1.0])),
                (field::SPAWN_WITHIN_VOLUME, Value::I32(1)),
            ],
        );
        let t = b.add(
            b"trsl",
            vec![(mf::TRANSLATION, Value::Vec4([0.0, 0.0, 1.0, 1.0]))],
        );
        let nemt = b.add(
            b"nemt",
            vec![
                (field::NAME, s("FireBase")),
                (field::MATERIAL_OBJECT, s("fx_fire_add")),
                (field::MATERIAL_LIBRARY, s("fx_firelibrary.mal")),
                (field::BIRTHRATE, Value::F32(12.0)),
                (field::LIFE, Value::F32(1.5)),
                (field::SPREAD_X, Value::F32(20.0)),
                (field::FLIPBOOK, Value::I32(1)),
                (field::FLIPBOOK_ROWS, Value::I32(4)),
                (field::FLIPBOOK_COLUMNS, Value::I32(8)),
                (field::FRAMES_PER_SECOND, Value::F32(24.0)),
                (field::ORIENTATION, Value::I32(3)),
                (field::TARGET_NAME, s("")),
                (field::USER_PARAM_NAME, s("fog")),
                (field::CHILDREN, generic(&[t, spnv, amap])),
            ],
        );
        let gob = b.add(b"node", vec![(mf::CHILDREN, generic(&[nemt]))]);
        let root = b.add(b"mdlh", vec![(mf::CHILDREN, generic(&[gob]))]);
        let mmh = Mmh::from_gff4(&b.finish(root)).unwrap();
        let em = mmh.emitters();
        assert_eq!(em.len(), 1);
        assert_eq!(em[0].name, Some("FireBase"));
        assert_eq!(em[0].chain[1].position, [0.0, 0.0, 1.0]);
        let e = em[0].emitter;
        assert_eq!(e.material, "fx_fire_add");
        assert_eq!(e.birthrate, 12.0);
        assert_eq!(e.spread, [20.0, 0.0]);
        assert!(e.flipbook);
        assert_eq!((e.flipbook_columns, e.flipbook_rows), (8, 4));
        assert_eq!(e.target, None);
        assert_eq!(e.user_param.as_deref(), Some("fog"));
        assert_eq!(e.age_scale_multiplier, [1.0, 1.0]);
        assert_eq!(
            e.volume.shape,
            SpawnShape::Cylinder {
                radius: 0.75,
                length: 0.0,
                axis: [0.0, 0.0, 1.0]
            }
        );
        assert!(e.volume.within);
        // Sorted by time; a key without scales reads as 1.
        assert_eq!(e.age_map.len(), 2);
        assert_eq!(e.age_map[0].time, 0.0);
        assert_eq!(e.age_map[0].scale, [1.0, 1.0]);
        assert_eq!(e.age_map[1].scale, [2.0, 3.0]);
        assert!(mmh.mesh_pieces().is_empty());
    }

    #[test]
    fn rejects_a_negative_flipbook_size() {
        let mut b = Builder::new(b"MMH ");
        let nemt = b.add(b"nemt", vec![(field::FLIPBOOK_COLUMNS, Value::I32(-4))]);
        let root = b.add(b"mdlh", vec![(mf::CHILDREN, generic(&[nemt]))]);
        assert!(Mmh::from_gff4(&b.finish(root)).is_err());
    }
}
