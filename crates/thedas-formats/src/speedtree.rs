//! SpeedTree 4.x tree descriptions (`.spt`): the parameters the trees, bushes and grass of
//! the outdoor levels are grown from at load time.
//!
//! An `.spt` file is a little-endian stream of 32-bit tokens, each followed by a payload whose
//! type the token implies: a float, an int, a one-byte bool or byte, a string (int32 length,
//! then that many bytes, no terminator), a vector of floats, or an "unsigned int" (four bytes
//! of value and four of padding). A token can also open a section that runs until its closing
//! token. Branch shape parameters are strings holding a Bézier curve in text
//! (`BezierSpline min max variance { count x y tangent_x tangent_y tangent_length ... }`),
//! parsed here into [`BezierSpline`].
//!
//! The file opens with the original layout (tree info with its branch levels, general leaf
//! info, legacy wind) up to the end-of-file token; later SpeedTree versions append optional
//! sections after it (leaf clusters, lighting, LOD, texture coordinates, collision, fronds,
//! map bank, ...). Several of those carry one record per branch level, in order, without a
//! count of their own, so they are merged into [`BranchLevel`] here.
//!
//! Token numbers, payload types and section nesting come from the SpeedTree 4 SDK's reader,
//! not from DAOrigins.exe; they are provisional, as are the defaults given to fields a file
//! leaves out (the SDK's constructor values). The parse has been run over every `.spt` in the
//! game's level archives. The SDK source is unlicensed: nothing here is taken from it but the
//! layout.

use std::fmt;

/// Errors from [`parse`] and [`BezierSpline::parse`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SptError {
    #[error("truncated at offset {offset}: need {need} more bytes")]
    Truncated { offset: usize, need: usize },
    #[error("not a SpeedTree file: {0}")]
    NotSpt(String),
    #[error("unexpected token {token} at offset {offset} in {section}")]
    UnexpectedToken {
        token: Token,
        offset: usize,
        section: &'static str,
    },
    #[error("bad count {count} at offset {offset} in {section}")]
    BadCount {
        count: i32,
        offset: usize,
        section: &'static str,
    },
    #[error("bad spline at offset {offset} in {section}: {reason}")]
    BadSpline {
        offset: usize,
        section: &'static str,
        reason: String,
    },
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, SptError>;

macro_rules! tokens {
    ($($(#[$doc:meta])* $name:ident = $value:expr;)*) => {
        /// Token numbers of the `.spt` stream (the SpeedTree 4 SDK's numbering; provisional).
        pub mod tok {
            $($(#[$doc])* pub const $name: i32 = $value;)*
        }

        /// The name of a known token, as [`tok`] spells it.
        pub fn token_name(token: i32) -> Option<&'static str> {
            match token {
                $($value => Some(stringify!($name)),)*
                _ => None,
            }
        }
    };
}

tokens! {
    FILE_START = 1000;
    FILE_END = 1001;
    TREE_START = 1002;
    TREE_END = 1003;
    LEAVES_START = 1004;
    LEAVES_END = 1005;
    LEAF_TEXTURE_COUNT = 1006;
    LEAF_TEXTURE_START = 1007;
    LEAF_TEXTURE_END = 1008;
    LEAF_TEXTURES_START = 1009;
    LEAF_TEXTURES_END = 1010;
    OLD_WIND_START = 1011;
    OLD_WIND_END = 1012;
    BRANCH_LEVEL_COUNT = 1013;
    BRANCHES_START = 1014;
    BRANCHES_END = 1015;
    BRANCH_LEVEL_START = 1016;
    BRANCH_LEVEL_END = 1017;
    STRING = 1018;

    TREE_BRANCH_TEXTURE = 2000;
    TREE_FAR = 2001;
    TREE_LOD_FLAG = 2002;
    TREE_NEAR = 2003;
    TREE_RENDER_STYLE = 2004;
    TREE_SEED = 2005;
    TREE_SIZE = 2006;
    TREE_SIZE_VARIANCE = 2007;

    LEAF_BLOSSOM_DISTANCE = 3000;
    LEAF_BLOSSOM_LEVEL = 3001;
    LEAF_BLOSSOM_WEIGHTING = 3002;
    LEAF_COLOR_SHIFT = 3003;
    LEAF_COLOR_SHIFT_SCALAR = 3004;
    LEAF_LIGHT_SCALAR = 3005;
    LEAF_LIT = 3006;
    LEAF_SPACING_TOLERANCE = 3007;
    LEAF_COLLISION = 3008;
    LEAF_DIMMING = 3009;
    LEAF_DIMMING_SCALAR = 3010;

    LEAF_TEX_BLOSSOM = 4000;
    LEAF_TEX_COLOR = 4001;
    LEAF_TEX_COLOR_VARIANCE = 4002;
    LEAF_TEX_FILENAME = 4003;
    LEAF_TEX_ORIGIN = 4004;
    LEAF_TEX_SIZE = 4005;
    LEAF_TEX_SIZE_USED = 4006;
    LEAF_TEX_RATIO = 4007;

    OLD_WIND_DIRECTION = 5000;
    OLD_WIND_BRANCH_OSCILLATION = 5001;
    OLD_WIND_LEAF_OSCILLATION = 5002;
    OLD_WIND_BRANCH_FACTORS = 5003;
    OLD_WIND_LEAF_FACTORS = 5004;
    OLD_WIND_STRENGTH = 5005;
    OLD_WIND_ENABLED = 5006;

    BRANCH_DISTURBANCE = 6000;
    BRANCH_GRAVITY = 6001;
    BRANCH_FLEXIBILITY = 6002;
    BRANCH_FLEXIBILITY_SCALE = 6003;
    BRANCH_LENGTH = 6004;
    BRANCH_RADIUS = 6005;
    BRANCH_RADIUS_SCALE = 6006;
    BRANCH_START_ANGLE = 6007;
    BRANCH_CROSS_SECTION_SEGMENTS = 6008;
    BRANCH_SEGMENTS = 6009;
    BRANCH_FIRST = 6010;
    BRANCH_LAST = 6011;
    BRANCH_FREQUENCY = 6012;
    BRANCH_S_TILE = 6013;
    BRANCH_T_TILE = 6014;
    BRANCH_S_TILE_ABSOLUTE = 6015;
    BRANCH_T_TILE_ABSOLUTE = 6016;
    BRANCH_ANGLE_PROFILE = 6017;

    CLUSTER_START = 7000;
    CLUSTER_END = 7001;
    CLUSTER_LOD_START = 7002;
    CLUSTER_LOD_END = 7003;
    CLUSTER_LEAF_START = 7004;
    CLUSTER_LEAF_END = 7005;
    CLUSTER_LEAF_COLOR = 7006;
    CLUSTER_LEAF_ANGLE_INDEX = 7007;
    CLUSTER_LEAF_COLOR_SCALE = 7008;
    /// Payload type unknown (the SDK's reader does not accept it).
    CLUSTER_LEAF_FREQUENCY_SCALAR = 7009;
    CLUSTER_LEAF_NORMAL = 7010;
    CLUSTER_LEAF_TEXTURE_INDEX = 7011;
    CLUSTER_LEAF_WIND_WEIGHT = 7012;
    CLUSTER_LEAF_WIND_GROUP = 7013;
    /// Payload type unknown (the SDK's reader does not accept it).
    CLUSTER_LEAF_BRANCH_PERCENT = 7014;
    CLUSTER_LEAF_POSITION = 7015;
    CLUSTER_LEAF_TIME_OFFSET = 7016;

    LIGHTING_START = 8000;
    LIGHTING_END = 8001;
    LIGHTING_BRANCH_METHOD = 8002;
    LIGHTING_BRANCH_MATERIAL = 8003;
    LIGHTING_LEAF_METHOD = 8004;
    LIGHTING_LEAF_MATERIAL = 8005;
    LIGHTING_LEAF_ADJUSTMENT = 8006;
    LIGHTING_STATIC_STYLE = 8007;
    LIGHTING_FROND_METHOD = 8008;
    LIGHTING_FROND_MATERIAL = 8009;

    LOD_START = 9000;
    LOD_END = 9001;
    LOD_LEAF_TRANSITION = 9002;
    LOD_LEAF_TRANSITION_RADIUS = 9003;
    LOD_LEAF_CURVE_EXPONENT = 9004;
    LOD_ENGINE_START = 9005;
    LOD_ENGINE_END = 9006;
    LOD_BRANCH_COUNT = 9007;
    LOD_BRANCH_MIN_VOLUME = 9008;
    LOD_LEAF_SIZE_INCREASE = 9009;
    LOD_LEAF_REDUCTION = 9010;
    LOD_LEAF_COUNT = 9011;
    LOD_BRANCH_MAX_VOLUME = 9012;
    LOD_BRANCH_FUZZINESS = 9013;
    LOD_LARGE_BRANCH_PERCENT = 9014;

    TEXCOORDS_START = 10000;
    TEXCOORDS_END = 10001;
    TEXCOORDS_LEAF = 10002;
    TEXCOORDS_BILLBOARD = 10003;
    TEXCOORDS_FROND = 10004;
    TEXCOORDS_COMPOSITE = 10005;
    TEXCOORDS_HORIZONTAL_BILLBOARD = 10006;
    TEXCOORDS_360_BILLBOARD = 10007;

    WIND_START = 11000;
    WIND_END = 11001;
    WIND_LEVEL = 11002;

    COLLISION_START = 12000;
    COLLISION_END = 12001;
    COLLISION_SPHERE = 12002;
    COLLISION_CAPSULE = 12003;
    COLLISION_BOX = 12004;

    FROND_START = 13000;
    FROND_END = 13001;
    FROND_LEVEL = 13002;
    FROND_KIND = 13003;
    FROND_BLADES = 13004;
    FROND_PROFILE = 13005;
    FROND_PROFILE_SEGMENTS = 13006;
    FROND_ENABLED = 13007;
    FROND_TEXTURE_COUNT = 13008;
    FROND_LOD_COUNT = 13009;
    FROND_MAX_SURFACE = 13010;
    FROND_MIN_SURFACE = 13011;
    FROND_FUZZINESS = 13012;
    FROND_LARGE_PERCENT = 13013;

    FROND_TEX_START = 14000;
    FROND_TEX_END = 14001;
    FROND_TEX_FILENAME = 14002;
    FROND_TEX_ASPECT = 14003;
    FROND_TEX_SIZE_SCALE = 14004;
    FROND_TEX_MIN_ANGLE = 14005;
    FROND_TEX_MAX_ANGLE = 14006;
    FROND_MIN_LENGTH_SEGMENTS = 14007;
    FROND_MIN_CROSS_SEGMENTS = 14008;

    TEX_CONTROLS_START = 15000;
    TEX_CONTROLS_END = 15001;
    TEX_CONTROLS_RANDOM_OFFSET = 15002;
    TEX_CONTROLS_TWIST = 15003;

    FLARES_START = 16000;
    FLARES_END = 16001;
    FLARE_PACKING_EXPONENT = 16002;
    FLARE_COUNT = 16003;
    FLARE_BALANCE = 16004;
    FLARE_RADIAL_INFLUENCE = 16005;
    FLARE_RADIAL_INFLUENCE_VARIANCE = 16006;
    FLARE_RADIAL_EXPONENT = 16007;
    FLARE_RADIAL_DISTANCE = 16008;
    FLARE_RADIAL_DISTANCE_VARIANCE = 16009;
    FLARE_LENGTH_DISTANCE = 16010;
    FLARE_LENGTH_DISTANCE_VARIANCE = 16011;
    FLARE_LENGTH_EXPONENT = 16012;
    FLARE_SEED = 16013;
    LEAF_TRANSITION_FACTOR = 16014;

    SHADOW_START = 18000;
    SHADOW_END = 18001;
    SHADOW_RIGHT = 18002;
    SHADOW_UP = 18003;
    SHADOW_OUT = 18004;
    SHADOW_MAP = 18005;

    USER_DATA_START = 19000;
    USER_DATA_END = 19001;
    USER_DATA = 19002;

    TEXCOORDS2_START = 20000;
    TEXCOORDS2_END = 20001;
    TEXCOORDS2_COMPOSITE = 20002;
    TEXCOORDS2_HORIZONTAL_BILLBOARD = 20003;
    TEXCOORDS2_360_BILLBOARD = 20004;
    TEXCOORDS2_SHADOW = 20005;

    SPEEDWIND_ROCK = 21000;
    SPEEDWIND_RUSTLE = 21001;

    PROPAGATE_FLEXIBILITY = 22000;

    LIGHT_SEAM_START = 23000;
    LIGHT_SEAM_END = 23001;
    LIGHT_SEAM_START_BIAS = 23002;
    LIGHT_SEAM_END_BIAS = 23003;

    LEAF_PLACEMENT_START = 24000;
    LEAF_PLACEMENT_END = 24001;
    LEAF_PLACEMENT_DISTANCE = 24002;
    LEAF_PLACEMENT_LEVEL = 24003;

    FROND2_START = 25000;
    FROND2_END = 25001;
    FROND2_DISTANCE_PERCENT = 25002;
    FROND2_DISTANCE_LEVEL = 25003;
    FROND2_ABOVE = 25004;
    FROND2_BELOW = 25005;
    FROND2_SEGMENT_OVERRIDE = 25006;
    FROND2_USE_SEGMENT_OVERRIDE = 25007;

    BRANCH2_START = 26000;
    BRANCH2_END = 26001;
    BRANCH2_LEVEL_START = 26002;
    BRANCH2_LEVEL_END = 26003;
    BRANCH2_ROUGHNESS = 26004;
    BRANCH2_MIN_LENGTH_PERCENT = 26005;
    BRANCH2_MIN_CROSS_SECTION_PERCENT = 26006;
    BRANCH2_PRUNING_PERCENT = 26007;
    BRANCH2_PRUNING_DEPTH = 26008;
    BRANCH2_FORK_ENABLED = 26009;
    BRANCH2_FORK_BIAS = 26010;
    BRANCH2_FORK_ANGLE = 26011;
    BRANCH2_FORK_LIMIT = 26012;
    BRANCH2_CROSS_SECTION_PROFILE = 26013;
    BRANCH2_LIGHT_SEAM_PROFILE = 26014;
    BRANCH2_ROUGHNESS_V_FREQUENCY = 26015;
    BRANCH2_ROUGHNESS_H_FREQUENCY = 26016;
    BRANCH2_RANDOM_ROUGHNESS = 26017;
    BRANCH2_ROUGHNESS_PROFILE = 26018;
    BRANCH2_FREQUENCY_PROFILE = 26019;
    BRANCH2_LIGHT_SEAM_BIAS = 26020;
    BRANCH2_GNARL = 26021;
    BRANCH2_GNARL_PROFILE = 26022;
    BRANCH2_GNARL_UNISON = 26023;

    FLOOR_START = 27000;
    FLOOR_END = 27001;
    FLOOR_ENABLED = 27002;
    FLOOR_VALUE = 27003;
    FLOOR_LEVEL = 27004;
    FLOOR_EXPONENT = 27005;
    FLOOR_BIAS = 27006;

    LEAF_SMOOTHING_START = 28000;
    LEAF_SMOOTHING_END = 28001;
    LEAF_SMOOTHING_ENABLED = 28002;
    LEAF_SMOOTHING_ANGLE = 28003;
    LEAF_DIMMING_DEPTH = 28004;

    CLUSTER_INFO_START = 29000;
    CLUSTER_INFO_END = 29001;
    CLUSTER_FIRST_BRANCH_LEVEL = 29002;

    SHADER_START = 30000;
    SHADER_END = 30001;
    SHADER_BRANCH_LIGHT = 30002;
    SHADER_FROND_LIGHT = 30003;
    SHADER_LEAF_LIGHT = 30004;
    SHADER_GLOBAL_LIGHT = 30005;
    SHADER_AMBIENT = 30006;
    SHADER_BILLBOARD_DARK_SIDE = 30007;
    SHADER_BILLBOARD_AMBIENT = 30008;
    SHADER_BILLBOARD_BRIGHT_SIDE = 30009;

    ROOTS_START = 40000;
    ROOTS_END = 40001;
    ROOTS_LEVEL = 40002;
    ROOTS_FIRST = 40003;
    ROOTS_LAST = 40004;
    ROOTS_FREQUENCY = 40005;
    ROOTS_BRANCH = 40006;
    ROOTS_EXTRA_START = 40007;
    ROOTS_EXTRA_END = 40008;

    TEX_LAYERS_START = 50000;
    TEX_LAYERS_END = 50001;
    TEX_LAYER_START = 50002;
    TEX_LAYER_END = 50003;
    TEX_LAYER_S_TILE = 50004;
    TEX_LAYER_T_TILE = 50005;
    TEX_LAYER_S_TILE_ABSOLUTE = 50006;
    TEX_LAYER_T_TILE_ABSOLUTE = 50007;
    TEX_LAYER_TWIST = 50008;
    TEX_LAYER_RANDOM_T_OFFSET = 50009;
    TEX_LAYER_T_OFFSET = 50010;
    TEX_LAYER_CLAMP_S = 50011;
    TEX_LAYER_CLAMP_T = 50012;
    TEX_LAYER_CLAMP_LEFT = 50013;
    TEX_LAYER_CLAMP_RIGHT = 50014;
    TEX_LAYER_CLAMP_BOTTOM = 50015;
    TEX_LAYER_CLAMP_TOP = 50016;
    TEX_LAYER_S_OFFSET = 50017;
    TEX_LAYER_SYNC = 50018;

    MAPS_START = 60000;
    MAPS_END = 60001;
    MAPS_BRANCH = 60002;
    MAPS_LEAVES = 60003;
    MAPS_FRONDS = 60004;
    MAPS_COMPOSITE = 60005;
    MAPS_SELF_SHADOW = 60006;
    MAPS_LEAF_COUNT = 60007;
    MAPS_FROND_COUNT = 60008;
    MAPS_BILLBOARD = 60009;

    MAP_SET_START = 70000;
    MAP_SET_END = 70001;
    MAP_SET_DIFFUSE = 70002;
    MAP_SET_DETAIL = 70003;
    MAP_SET_NORMAL = 70004;
    MAP_SET_HEIGHT = 70005;
    MAP_SET_SPECULAR = 70006;
    MAP_SET_USER1 = 70007;
    MAP_SET_USER2 = 70008;

    MESHES_START = 71000;
    MESHES_COUNT = 71001;
    MESH_START = 71002;
    MESH_NAME = 71003;
    MESH_VERTEX_COUNT = 71004;
    MESH_VERTEX_START = 71005;
    MESH_VERTEX_POSITION = 71006;
    MESH_VERTEX_NORMAL = 71007;
    MESH_VERTEX_BINORMAL = 71008;
    MESH_VERTEX_TANGENT = 71009;
    MESH_VERTEX_UV = 71010;
    MESH_VERTEX_END = 71011;
    MESH_INDEX_COUNT = 71012;
    MESH_INDEX = 71013;
    MESH_END = 71014;
    MESHES_END = 71015;

    LEAF_MESHES_START = 72000;
    LEAF_MESH_USE = 72001;
    LEAF_MESH_INDEX_COUNT = 72002;
    LEAF_MESH_INDEX = 72003;
    LEAF_MESHES_END = 72004;
    LEAF_MESH_HANG = 72005;
    LEAF_MESH_ROTATE = 72006;

    COLLISION2_START = 73000;
    COLLISION2_END = 73001;
    COLLISION2_OBJECT_START = 73002;
    COLLISION2_OBJECT_END = 73003;
    COLLISION2_ROTATE_X = 73004;
    COLLISION2_ROTATE_Y = 73005;
    COLLISION2_ROTATE_Z = 73006;

    GLOBAL2_START = 74000;
    GLOBAL2_END = 74001;
    GLOBAL2_TREE_ANGLE = 74002;

    LOD2_START = 75000;
    LOD2_END = 75001;
    LOD2_BRANCH_FADE_DISTANCE = 75002;
    LOD2_FROND_FADE_DISTANCE = 75003;
    LOD2_FADE_EXTRUSIONS = 75004;
    LOD2_BILLBOARD_TRANSITION = 75005;
}

/// A token as read, shown with its [`tok`] name when it has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token(pub i32);

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match token_name(self.0) {
            Some(name) => write!(f, "{} ({name})", self.0),
            None => write!(f, "{}", self.0),
        }
    }
}

/// The header string after the file-start token.
pub const HEADER: &str = "__IdvSpt_02_";

/// Number of texture layers a branch carries coordinate controls for: diffuse, detail,
/// normal, height, specular, user 1, user 2 (in that order; provisional).
pub const TEXTURE_LAYERS: usize = 7;

// ---------------------------------------------------------------------------------------------
// Model

/// A parsed `.spt` file. Sections a file leaves out are `None` (or empty); fields inside a
/// section default to the SDK's constructor values (provisional).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpeedTree {
    pub tree: TreeInfo,
    /// Branch levels, trunk first.
    pub branch_levels: Vec<BranchLevel>,
    pub leaves: LeafInfo,
    /// The original wind block (legacy; most of it unused by SpeedTree 4).
    pub old_wind: OldWind,
    /// Precomputed billboard leaves, one list per leaf LOD level.
    pub leaf_lods: Option<Vec<Vec<BillboardLeaf>>>,
    pub lighting: Option<Lighting>,
    pub lod: Option<Lod>,
    /// The branch level whose motion drives wind weighting.
    pub wind_level: Option<i32>,
    pub texcoords: Option<TexCoordTables>,
    pub collision: Vec<CollisionObject>,
    pub fronds: Option<Fronds>,
    pub flare_seed: Option<i32>,
    pub leaf_transition_factor: Option<f32>,
    pub shadow_projection: Option<ShadowProjection>,
    pub user_data: Option<String>,
    pub speedwind_rock_scalar: Option<f32>,
    pub speedwind_rustle_scalar: Option<f32>,
    pub propagate_flexibility: Option<bool>,
    pub floor: Option<Floor>,
    /// The first branch level leaf clusters attach to.
    pub cluster_first_branch_level: Option<i32>,
    pub shader_scalars: Option<ShaderScalars>,
    pub roots: Option<RootSupport>,
    /// The roots' texture-layer controls: the tex-coord-controls section always carries one
    /// record past the branch levels for them, whether or not the file has root support.
    pub root_layers: Option<[TexLayer; TEXTURE_LAYERS]>,
    pub maps: Option<MapBank>,
    pub meshes: Vec<LeafMesh>,
    /// Degrees.
    pub tree_angle: Option<f32>,
}

/// General tree settings (tokens 2000-2007).
#[derive(Clone, Debug, PartialEq)]
pub struct TreeInfo {
    /// The bark texture's file name (often empty when a map bank names it instead).
    pub branch_texture: String,
    /// The random seed the tree is grown from (stored as an int; used unsigned).
    pub seed: u32,
    /// Overall size scalar.
    pub size: f32,
    pub size_variance: f32,
    /// LOD near and far distances.
    pub near: f32,
    pub far: f32,
    /// An obsolete LOD flag, kept as stored.
    pub lod_flag: Option<bool>,
    /// An obsolete render style, kept as stored.
    pub render_style: Option<i32>,
}

impl Default for TreeInfo {
    fn default() -> Self {
        TreeInfo {
            branch_texture: String::new(),
            seed: 1200,
            size: 50.0,
            size_variance: 0.0,
            near: 100.0,
            far: 1100.0,
            lod_flag: None,
            render_style: None,
        }
    }
}

/// One control point of a [`BezierSpline`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SplinePoint {
    pub x: f32,
    pub y: f32,
    /// The tangent's direction.
    pub tangent: [f32; 2],
    /// How far the tangent handle reaches.
    pub tangent_length: f32,
}

/// A branch parameter: a Bézier curve over the branch's length mapping into `min..max`,
/// with a random `variance`.
///
/// Text form: `BezierSpline min max variance { count` then `count` groups of
/// `x y tangent_x tangent_y tangent_length`, then `}`, separated by any whitespace. Layout
/// from the SpeedTree 4 SDK's reader; provisional.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BezierSpline {
    pub min: f32,
    pub max: f32,
    pub variance: f32,
    pub points: Vec<SplinePoint>,
}

/// Why spline text failed to parse.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SplineError {
    #[error("expected {expected}, found {found:?}")]
    Expected {
        expected: &'static str,
        found: String,
    },
    #[error("expected {0}, found the end")]
    Ended(&'static str),
    #[error("bad point count {0}")]
    BadCount(String),
    #[error("trailing text {0:?}")]
    Trailing(String),
}

impl BezierSpline {
    /// Parses spline text (see the type's documentation).
    pub fn parse(text: &str) -> std::result::Result<Self, SplineError> {
        type R<T> = std::result::Result<T, SplineError>;
        fn next<'t>(words: &mut impl Iterator<Item = &'t str>, what: &'static str) -> R<&'t str> {
            words.next().ok_or(SplineError::Ended(what))
        }
        fn word<'t>(words: &mut impl Iterator<Item = &'t str>, want: &'static str) -> R<()> {
            let w = next(words, want)?;
            if w == want {
                Ok(())
            } else {
                Err(SplineError::Expected {
                    expected: want,
                    found: w.into(),
                })
            }
        }
        fn num<'t>(words: &mut impl Iterator<Item = &'t str>, what: &'static str) -> R<f32> {
            let w = next(words, what)?;
            w.parse::<f32>().map_err(|_| SplineError::Expected {
                expected: what,
                found: w.into(),
            })
        }
        let mut words = text.split_ascii_whitespace();
        let words = &mut words;
        word(words, "BezierSpline")?;
        let min = num(words, "a minimum")?;
        let max = num(words, "a maximum")?;
        let variance = num(words, "a variance")?;
        word(words, "{")?;
        let count_word = next(words, "a point count")?;
        let count: usize = count_word
            .parse()
            .map_err(|_| SplineError::BadCount(count_word.into()))?;
        // Each point takes five words, so a count beyond the text's words is bad input.
        if count > text.len() / 2 {
            return Err(SplineError::BadCount(count_word.into()));
        }
        let mut points = Vec::with_capacity(count);
        for _ in 0..count {
            let x = num(words, "a point x")?;
            let y = num(words, "a point y")?;
            let tx = num(words, "a tangent x")?;
            let ty = num(words, "a tangent y")?;
            let tangent_length = num(words, "a tangent length")?;
            points.push(SplinePoint {
                x,
                y,
                tangent: [tx, ty],
                tangent_length,
            });
        }
        word(words, "}")?;
        if let Some(extra) = words.next() {
            return Err(SplineError::Trailing(extra.into()));
        }
        Ok(BezierSpline {
            min,
            max,
            variance,
            points,
        })
    }
}

/// Texture-coordinate controls of one texture layer of a branch level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TexLayer {
    /// Repeats around the branch.
    pub s_tile: f32,
    /// Repeats along the branch.
    pub t_tile: f32,
    /// Whether `s_tile` is absolute rather than scaled by the branch's radius.
    pub s_tile_absolute: bool,
    /// Whether `t_tile` is absolute rather than scaled by the branch's length.
    pub t_tile_absolute: bool,
    pub random_t_offset: bool,
    /// Turns of twist over the branch's length.
    pub twist: f32,
    pub s_offset: f32,
    pub t_offset: f32,
    pub clamp_s: bool,
    pub clamp_t: bool,
    /// Left, right, bottom, top.
    pub clamp: [f32; 4],
    /// Whether the layer follows the diffuse layer's controls.
    pub sync: bool,
}

impl Default for TexLayer {
    fn default() -> Self {
        TexLayer {
            s_tile: 1.0,
            t_tile: 1.0,
            s_tile_absolute: true,
            t_tile_absolute: false,
            random_t_offset: false,
            twist: 0.0,
            s_offset: 0.0,
            t_offset: 0.0,
            clamp_s: false,
            clamp_t: false,
            clamp: [0.0, 1.0, 0.0, 1.0],
            sync: true,
        }
    }
}

/// Flares: the buttress-like bulges at a branch's base (section 16000, one per level).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flares {
    pub segment_packing_exponent: f32,
    pub count: i32,
    pub balance: f32,
    pub radial_influence: f32,
    pub radial_influence_variance: f32,
    pub radial_exponent: f32,
    pub radial_distance: f32,
    pub radial_distance_variance: f32,
    pub length_distance: f32,
    pub length_distance_variance: f32,
    pub length_exponent: f32,
}

impl Default for Flares {
    fn default() -> Self {
        Flares {
            segment_packing_exponent: 1.0,
            count: 0,
            balance: 1.0,
            radial_influence: 30.0,
            radial_influence_variance: 10.0,
            radial_exponent: 1.0,
            radial_distance: 0.5,
            radial_distance_variance: 0.25,
            length_distance: 0.3,
            length_distance_variance: 0.1,
            length_exponent: 1.0,
        }
    }
}

/// One branch level: how branches of that generation are shaped and placed. Splines a file
/// leaves out are `None`.
#[derive(Clone, Debug, PartialEq)]
pub struct BranchLevel {
    pub disturbance: Option<BezierSpline>,
    pub gravity: Option<BezierSpline>,
    pub flexibility: Option<BezierSpline>,
    pub flexibility_scale: Option<BezierSpline>,
    pub length: Option<BezierSpline>,
    pub radius: Option<BezierSpline>,
    pub radius_scale: Option<BezierSpline>,
    pub start_angle: Option<BezierSpline>,
    pub angle_profile: Option<BezierSpline>,
    /// Points around a branch's cross-section.
    pub cross_section_segments: i32,
    /// Segments along a branch.
    pub segments: i32,
    /// Where along the parent children start and stop, as fractions of its length.
    pub first_branch: f32,
    pub last_branch: f32,
    pub frequency: f32,
    /// Texture-coordinate controls per layer (see [`TEXTURE_LAYERS`]).
    pub layers: [TexLayer; TEXTURE_LAYERS],
    pub flares: Flares,
    pub light_seam_start_bias: f32,
    pub light_seam_end_bias: f32,
    // Supplemental branch info (section 26000).
    pub roughness: f32,
    pub roughness_vertical_frequency: f32,
    pub roughness_horizontal_frequency: f32,
    pub random_roughness: f32,
    pub min_length_percent: f32,
    pub min_cross_section_percent: f32,
    pub pruning_percent: f32,
    pub pruning_depth: u32,
    pub fork_enabled: bool,
    pub fork_bias: f32,
    pub fork_angle: f32,
    pub fork_limit: i32,
    pub gnarl: f32,
    pub gnarl_unison: bool,
    pub cross_section_profile: Option<BezierSpline>,
    pub light_seam_profile: Option<BezierSpline>,
    pub roughness_profile: Option<BezierSpline>,
    pub frequency_profile: Option<BezierSpline>,
    pub light_seam_bias: Option<BezierSpline>,
    pub gnarl_profile: Option<BezierSpline>,
}

impl Default for BranchLevel {
    fn default() -> Self {
        BranchLevel {
            disturbance: None,
            gravity: None,
            flexibility: None,
            flexibility_scale: None,
            length: None,
            radius: None,
            radius_scale: None,
            start_angle: None,
            angle_profile: None,
            cross_section_segments: 6,
            segments: 3,
            first_branch: 0.3,
            last_branch: 1.0,
            frequency: 0.3,
            layers: [TexLayer::default(); TEXTURE_LAYERS],
            flares: Flares::default(),
            light_seam_start_bias: 1.0,
            light_seam_end_bias: 1.0,
            roughness: 0.0,
            roughness_vertical_frequency: 30.0,
            roughness_horizontal_frequency: 5.0,
            random_roughness: 0.0,
            min_length_percent: 1.0,
            min_cross_section_percent: 1.0,
            pruning_percent: 0.0,
            pruning_depth: 0,
            fork_enabled: false,
            fork_bias: 1.0,
            fork_angle: 0.0,
            fork_limit: 1,
            gnarl: 0.0,
            gnarl_unison: false,
            cross_section_profile: None,
            light_seam_profile: None,
            roughness_profile: None,
            frequency_profile: None,
            light_seam_bias: None,
            gnarl_profile: None,
        }
    }
}

/// How a leaf texture is drawn as a mesh instead of a card (section 72000).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LeafMeshUse {
    pub use_meshes: bool,
    /// Index into [`SpeedTree::meshes`].
    pub mesh: i32,
    pub hang: f32,
    pub rotate: f32,
}

/// One leaf texture (a leaf "level" in the file).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LeafTexture {
    /// The texture's file name, as stored (may carry a path).
    pub filename: String,
    pub blossom: bool,
    pub color: [f32; 3],
    pub color_variance: f32,
    /// The card's pivot.
    pub origin: [f32; 3],
    pub size: [f32; 3],
    pub size_used: [f32; 3],
    /// Height over width, as the tool stored it (redundant with `size`).
    pub ratio: Option<f32>,
    pub mesh: Option<LeafMeshUse>,
}

/// General leaf settings (tokens 3000-3010), the leaf textures, and the leaf placement and
/// normal-smoothing additions.
#[derive(Clone, Debug, PartialEq)]
pub struct LeafInfo {
    pub textures: Vec<LeafTexture>,
    pub blossom_distance: f32,
    pub blossom_level: i32,
    pub blossom_weighting: f32,
    pub dimming: bool,
    pub dimming_scalar: f32,
    pub spacing_tolerance: f32,
    /// 0 none, 1 against branches, 2 against the whole tree.
    pub collision: i32,
    pub placement_distance: f32,
    pub placement_level: i32,
    pub smoothing_enabled: bool,
    /// Degrees.
    pub smoothing_angle: f32,
    pub dimming_depth: i32,
}

impl Default for LeafInfo {
    fn default() -> Self {
        LeafInfo {
            textures: Vec::new(),
            blossom_distance: 0.75,
            blossom_level: 1,
            blossom_weighting: 0.8,
            dimming: true,
            dimming_scalar: 1.0,
            spacing_tolerance: 0.5,
            collision: 2,
            placement_distance: 0.0,
            placement_level: 0,
            smoothing_enabled: false,
            smoothing_angle: 30.0,
            dimming_depth: 1,
        }
    }
}

/// The original wind block (tokens 5000-5006). SpeedTree 4 reads only the leaf values and
/// the strength; the rest is kept as stored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OldWind {
    pub leaf_oscillation: [f32; 3],
    pub leaf_factors: [f32; 3],
    pub strength: f32,
    pub direction: Option<[f32; 3]>,
    pub branch_oscillation: Option<[f32; 3]>,
    pub branch_factors: Option<[f32; 3]>,
    pub enabled: Option<bool>,
}

impl Default for OldWind {
    fn default() -> Self {
        OldWind {
            leaf_oscillation: [0.0; 3],
            leaf_factors: [0.5, 0.5, 0.0],
            strength: 0.0,
            direction: None,
            branch_oscillation: None,
            branch_factors: None,
            enabled: None,
        }
    }
}

/// A precomputed billboard leaf of a leaf cluster LOD.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BillboardLeaf {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// Packed colour, as stored.
    pub color: u32,
    pub angle_index: u8,
    pub color_scale: u8,
    pub texture_index: u8,
    /// Wind weight, 0-255 for 0-1.
    pub wind_weight: u8,
    pub wind_group: i32,
    /// An obsolete per-leaf time offset, kept as stored.
    pub time_offset: Option<f32>,
}

/// A lighting material: diffuse, ambient, specular and emissive colours and shininess.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Material {
    pub diffuse: [f32; 3],
    pub ambient: [f32; 3],
    pub specular: [f32; 3],
    pub emissive: [f32; 3],
    pub shininess: f32,
}

/// Lighting methods and materials (section 8000). Methods: 0 dynamic, 1 static.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Lighting {
    pub branch_method: Option<i32>,
    pub branch_material: Option<Material>,
    pub leaf_method: Option<i32>,
    pub leaf_material: Option<Material>,
    pub frond_method: Option<i32>,
    pub frond_material: Option<Material>,
    pub leaf_adjustment: Option<f32>,
    /// 0 basic, 1 use light sources, 2 simulate shadows.
    pub static_style: Option<i32>,
}

/// LOD settings (section 9000, its engine subsection, and the supplemental LOD section).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Lod {
    /// 0 pop, 1 smooth, 2 an obsolete smooth variant, 3 none.
    pub leaf_transition: Option<i32>,
    pub leaf_transition_radius: Option<f32>,
    pub leaf_curve_exponent: Option<f32>,
    pub leaf_size_increase: Option<f32>,
    pub branch_lod_count: Option<i32>,
    pub branch_min_volume_percent: Option<f32>,
    pub branch_max_volume_percent: Option<f32>,
    pub branch_reduction_fuzziness: Option<f32>,
    pub large_branch_percent: Option<f32>,
    pub leaf_reduction: Option<f32>,
    pub leaf_lod_count: Option<i32>,
    pub branch_fade_distance: Option<f32>,
    pub frond_fade_distance: Option<f32>,
    pub fade_extrusions: Option<bool>,
    pub billboard_transition: Option<f32>,
}

/// Texture coordinates into the composite (atlas) texture: four (s, t) corners per entry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TexCoordTables {
    pub leaves: Vec<[f32; 8]>,
    pub billboards: Vec<[f32; 8]>,
    pub fronds: Vec<[f32; 8]>,
    /// The composite texture's file name, as stored.
    pub composite: Option<String>,
    pub horizontal_billboard: Option<bool>,
    pub billboard_360: Option<bool>,
    pub shadow: Option<[f32; 8]>,
}

/// A collision primitive's shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CollisionShape {
    Sphere { radius: f32 },
    Capsule { radius: f32, height: f32 },
    Box { size: [f32; 3] },
}

/// A collision primitive (section 12000), with its rotation from section 73000.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollisionObject {
    pub shape: CollisionShape,
    pub position: [f32; 3],
    /// Euler angles, degrees.
    pub rotation: [f32; 3],
}

/// A frond texture.
#[derive(Clone, Debug, PartialEq)]
pub struct FrondTexture {
    pub filename: String,
    pub aspect_ratio: f32,
    pub size_scale: f32,
    pub min_angle_offset: f32,
    pub max_angle_offset: f32,
}

impl Default for FrondTexture {
    fn default() -> Self {
        FrondTexture {
            filename: String::new(),
            aspect_ratio: 0.5,
            size_scale: 1.0,
            min_angle_offset: 0.0,
            max_angle_offset: 0.0,
        }
    }
}

/// Fronds: textured blades or extrusions along branches of one level (sections 13000 and
/// 25000).
#[derive(Clone, Debug, PartialEq)]
pub struct Fronds {
    pub enabled: bool,
    /// The branch level fronds grow on.
    pub level: i32,
    /// 0 blades, 1 extrusion.
    pub kind: i32,
    pub blades: i32,
    pub profile: Option<BezierSpline>,
    pub profile_segments: i32,
    pub textures: Vec<FrondTexture>,
    pub lod_count: i32,
    pub max_surface_percent: f32,
    pub min_surface_percent: f32,
    pub reduction_fuzziness: f32,
    pub large_frond_percent: f32,
    pub min_length_segments: i32,
    pub min_cross_segments: i32,
    pub distance_level: i32,
    pub distance_percent: f32,
    /// 0 enabled, 1 disabled, 2 pruned.
    pub above_condition: i32,
    pub below_condition: i32,
    pub segment_override: i32,
    pub use_segment_override: bool,
}

impl Default for Fronds {
    fn default() -> Self {
        Fronds {
            enabled: false,
            level: 1,
            kind: 1,
            blades: 2,
            profile: None,
            profile_segments: 4,
            textures: Vec::new(),
            lod_count: 4,
            max_surface_percent: 1.0,
            min_surface_percent: 0.0,
            reduction_fuzziness: 0.0,
            large_frond_percent: 0.05,
            min_length_segments: 2,
            min_cross_segments: 1,
            distance_level: 0,
            distance_percent: 0.0,
            above_condition: 0,
            below_condition: 0,
            segment_override: 1,
            use_segment_override: false,
        }
    }
}

/// The projection a self-shadow map was rendered with (section 18000).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShadowProjection {
    pub right: Option<[f32; 3]>,
    pub up: Option<[f32; 3]>,
    pub out: Option<[f32; 3]>,
    pub map: Option<String>,
}

/// Branch floor clamping (section 27000).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Floor {
    pub enabled: Option<bool>,
    pub value: Option<f32>,
    pub level: Option<i32>,
    pub exponent: Option<f32>,
    pub bias: Option<f32>,
}

/// Light scalars for SpeedTree's standard shaders (section 30000).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShaderScalars {
    pub branch_light: Option<f32>,
    pub frond_light: Option<f32>,
    pub leaf_light: Option<f32>,
    pub global_light: Option<f32>,
    pub ambient: Option<f32>,
    pub billboard_dark_side: Option<f32>,
    pub billboard_ambient: Option<f32>,
    pub billboard_bright_side: Option<f32>,
}

/// Root support: roots grown from the base of one branch level (section 40000).
#[derive(Clone, Debug, PartialEq)]
pub struct RootSupport {
    pub level: i32,
    pub first: f32,
    pub last: f32,
    pub frequency: f32,
    pub roots: BranchLevel,
}

impl Default for RootSupport {
    fn default() -> Self {
        RootSupport {
            level: 1,
            first: 0.0,
            last: 0.15,
            frequency: 0.2,
            roots: BranchLevel::default(),
        }
    }
}

/// The maps of one surface, by layer. Empty strings are unused layers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapSet {
    pub diffuse: String,
    pub detail: String,
    pub normal: String,
    pub height: String,
    pub specular: String,
    pub user1: String,
    pub user2: String,
}

/// Texture maps by surface (section 60000).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapBank {
    pub branch: Option<MapSet>,
    pub leaves: Vec<MapSet>,
    pub fronds: Vec<MapSet>,
    pub composite: Option<MapSet>,
    pub billboard: Option<MapSet>,
    pub self_shadow: Option<String>,
}

/// A vertex of a leaf mesh.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub binormal: [f32; 3],
    pub tangent: [f32; 3],
    pub uv: [f32; 2],
}

/// A mesh that leaves can be drawn with (section 71000).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LeafMesh {
    pub name: String,
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<i32>,
}

// ---------------------------------------------------------------------------------------------
// Reader

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len());
        match end {
            Some(end) => {
                let s = &self.data[self.pos..end];
                self.pos = end;
                Ok(s)
            }
            None => Err(SptError::Truncated {
                offset: self.pos,
                need: n - (self.data.len() - self.pos.min(self.data.len())),
            }),
        }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let s = self.take(N)?;
        let mut a = [0u8; N];
        a.copy_from_slice(s);
        Ok(a)
    }

    fn int(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn float(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.array()?))
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    fn boolean(&mut self) -> Result<bool> {
        Ok(self.byte()? != 0)
    }

    /// Four bytes of value, then four of padding.
    fn padded_uint(&mut self) -> Result<u32> {
        let v = u32::from_le_bytes(self.array()?);
        self.take(4)?;
        Ok(v)
    }

    fn floats<const N: usize>(&mut self) -> Result<[f32; N]> {
        let mut a = [0.0; N];
        for v in &mut a {
            *v = self.float()?;
        }
        Ok(a)
    }

    fn vec3(&mut self) -> Result<[f32; 3]> {
        self.floats()
    }

    fn string(&mut self, section: &'static str) -> Result<String> {
        let at = self.pos;
        let len = self.int()?;
        let len = usize::try_from(len).map_err(|_| SptError::BadCount {
            count: len,
            offset: at,
            section,
        })?;
        Ok(String::from_utf8_lossy(self.take(len)?).into_owned())
    }

    fn spline(&mut self, section: &'static str) -> Result<BezierSpline> {
        let at = self.pos;
        let text = self.string(section)?;
        BezierSpline::parse(&text).map_err(|e| SptError::BadSpline {
            offset: at,
            section,
            reason: e.to_string(),
        })
    }

    /// A non-negative count; `per_item` is the fewest bytes each item takes, to reject counts
    /// the remaining data cannot hold before allocating.
    fn count(&mut self, section: &'static str, per_item: usize) -> Result<usize> {
        let at = self.pos;
        let n = self.int()?;
        let bad = SptError::BadCount {
            count: n,
            offset: at,
            section,
        };
        let n = usize::try_from(n).map_err(|_| bad.clone())?;
        if n.saturating_mul(per_item) > self.remaining() {
            return Err(bad);
        }
        Ok(n)
    }

    /// Reads a token, returning it with its offset.
    fn token(&mut self) -> Result<(i32, usize)> {
        let at = self.pos;
        Ok((self.int()?, at))
    }

    fn expect(&mut self, want: i32, section: &'static str) -> Result<()> {
        let (t, at) = self.token()?;
        if t == want {
            Ok(())
        } else {
            Err(unexpected(t, at, section))
        }
    }
}

fn unexpected(token: i32, offset: usize, section: &'static str) -> SptError {
    SptError::UnexpectedToken {
        token: Token(token),
        offset,
        section,
    }
}

// ---------------------------------------------------------------------------------------------
// Parser

/// Parses an `.spt` file.
pub fn parse(data: &[u8]) -> Result<SpeedTree> {
    let mut r = Reader { data, pos: 0 };
    let (t, at) = r.token()?;
    if t != tok::FILE_START {
        return Err(SptError::NotSpt(format!(
            "starts with token {t} at offset {at}"
        )));
    }
    let header = r.string("header")?;
    if header != HEADER {
        return Err(SptError::NotSpt(format!("header {header:?}")));
    }

    let mut tree = SpeedTree::default();

    // The original layout, up to the end-of-file token.
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::FILE_END => break,
            tok::TREE_START => parse_tree_info(&mut r, &mut tree)?,
            tok::LEAVES_START => parse_leaf_info(&mut r, &mut tree.leaves)?,
            tok::OLD_WIND_START => parse_old_wind(&mut r, &mut tree.old_wind)?,
            _ => return Err(unexpected(t, at, "file")),
        }
    }

    // Sections appended by later versions, in any order, until the data runs out.
    while !r.at_end() {
        let (t, at) = r.token()?;
        match t {
            tok::CLUSTER_START => tree.leaf_lods = Some(parse_leaf_clusters(&mut r)?),
            tok::LIGHTING_START => {
                parse_lighting(&mut r, tree.lighting.get_or_insert_with(Default::default))?
            }
            tok::LOD_START => parse_lod(&mut r, tree.lod.get_or_insert_with(Default::default))?,
            tok::WIND_START => parse_wind(&mut r, &mut tree.wind_level)?,
            tok::TEXCOORDS_START => {
                parse_texcoords(&mut r, tree.texcoords.get_or_insert_with(Default::default))?
            }
            tok::COLLISION_START => parse_collision(&mut r, &mut tree.collision)?,
            tok::FROND_START => {
                parse_fronds(&mut r, tree.fronds.get_or_insert_with(Default::default))?
            }
            tok::TEX_CONTROLS_START => parse_texture_controls(&mut r, &mut tree.branch_levels)?,
            tok::FLARES_START => parse_flares(&mut r, &mut tree.branch_levels)?,
            tok::FLARE_SEED => tree.flare_seed = Some(r.int()?),
            tok::LEAF_TRANSITION_FACTOR => tree.leaf_transition_factor = Some(r.float()?),
            tok::SHADOW_START => parse_shadow(
                &mut r,
                tree.shadow_projection.get_or_insert_with(Default::default),
            )?,
            tok::USER_DATA_START => parse_user_data(&mut r, &mut tree.user_data)?,
            tok::TEXCOORDS2_START => {
                parse_texcoords2(&mut r, tree.texcoords.get_or_insert_with(Default::default))?
            }
            tok::SPEEDWIND_ROCK => tree.speedwind_rock_scalar = Some(r.float()?),
            tok::SPEEDWIND_RUSTLE => tree.speedwind_rustle_scalar = Some(r.float()?),
            tok::PROPAGATE_FLEXIBILITY => tree.propagate_flexibility = Some(r.boolean()?),
            tok::LIGHT_SEAM_START => parse_light_seams(&mut r, &mut tree.branch_levels)?,
            tok::LEAF_PLACEMENT_START => parse_leaf_placement(&mut r, &mut tree.leaves)?,
            tok::FROND2_START => {
                parse_fronds2(&mut r, tree.fronds.get_or_insert_with(Default::default))?
            }
            tok::BRANCH2_START => parse_branch2(&mut r, &mut tree.branch_levels)?,
            tok::FLOOR_START => {
                parse_floor(&mut r, tree.floor.get_or_insert_with(Default::default))?
            }
            tok::LEAF_SMOOTHING_START => parse_leaf_smoothing(&mut r, &mut tree.leaves)?,
            tok::CLUSTER_INFO_START => {
                parse_cluster_info(&mut r, &mut tree.cluster_first_branch_level)?
            }
            tok::SHADER_START => parse_shader(
                &mut r,
                tree.shader_scalars.get_or_insert_with(Default::default),
            )?,
            tok::ROOTS_START => {
                parse_roots(&mut r, tree.roots.get_or_insert_with(Default::default))?
            }
            tok::TEX_LAYERS_START => parse_tex_layers(&mut r, &mut tree)?,
            tok::MAPS_START => parse_maps(&mut r, tree.maps.get_or_insert_with(Default::default))?,
            tok::MESHES_START => parse_meshes(&mut r, &mut tree.meshes)?,
            tok::LEAF_MESHES_START => parse_leaf_meshes(&mut r, &mut tree.leaves)?,
            tok::COLLISION2_START => parse_collision2(&mut r, &mut tree.collision)?,
            tok::GLOBAL2_START => parse_global2(&mut r, &mut tree.tree_angle)?,
            tok::LOD2_START => parse_lod2(&mut r, tree.lod.get_or_insert_with(Default::default))?,
            _ => return Err(unexpected(t, at, "trailing sections")),
        }
    }
    Ok(tree)
}

fn parse_tree_info(r: &mut Reader<'_>, tree: &mut SpeedTree) -> Result<()> {
    const S: &str = "tree info";
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::TREE_END => return Ok(()),
            tok::TREE_BRANCH_TEXTURE => tree.tree.branch_texture = r.string(S)?,
            tok::TREE_FAR => tree.tree.far = r.float()?,
            tok::TREE_LOD_FLAG => tree.tree.lod_flag = Some(r.boolean()?),
            tok::TREE_NEAR => tree.tree.near = r.float()?,
            tok::TREE_RENDER_STYLE => tree.tree.render_style = Some(r.int()?),
            tok::TREE_SEED => tree.tree.seed = r.int()? as u32,
            tok::TREE_SIZE => tree.tree.size = r.float()?,
            tok::TREE_SIZE_VARIANCE => tree.tree.size_variance = r.float()?,
            tok::BRANCHES_START => {
                // Each level takes at least its start and end tokens.
                let n = r.count("branch info", 8)?;
                let mut levels = Vec::with_capacity(n);
                for _ in 0..n {
                    let mut level = BranchLevel::default();
                    parse_branch_level(r, &mut level)?;
                    levels.push(level);
                }
                r.expect(tok::BRANCHES_END, "branch info")?;
                tree.branch_levels = levels;
            }
            _ => return Err(unexpected(t, at, S)),
        }
    }
}

/// A branch level from its start token to its end token.
fn parse_branch_level(r: &mut Reader<'_>, b: &mut BranchLevel) -> Result<()> {
    const S: &str = "branch level";
    r.expect(tok::BRANCH_LEVEL_START, S)?;
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::BRANCH_LEVEL_END => return Ok(()),
            tok::BRANCH_DISTURBANCE => b.disturbance = Some(r.spline(S)?),
            tok::BRANCH_GRAVITY => b.gravity = Some(r.spline(S)?),
            tok::BRANCH_FLEXIBILITY => b.flexibility = Some(r.spline(S)?),
            tok::BRANCH_FLEXIBILITY_SCALE => b.flexibility_scale = Some(r.spline(S)?),
            tok::BRANCH_LENGTH => b.length = Some(r.spline(S)?),
            tok::BRANCH_RADIUS => b.radius = Some(r.spline(S)?),
            tok::BRANCH_RADIUS_SCALE => b.radius_scale = Some(r.spline(S)?),
            tok::BRANCH_START_ANGLE => b.start_angle = Some(r.spline(S)?),
            tok::BRANCH_ANGLE_PROFILE => b.angle_profile = Some(r.spline(S)?),
            tok::BRANCH_CROSS_SECTION_SEGMENTS => b.cross_section_segments = r.int()?,
            tok::BRANCH_SEGMENTS => b.segments = r.int()?,
            tok::BRANCH_FIRST => b.first_branch = r.float()?,
            tok::BRANCH_LAST => b.last_branch = r.float()?,
            tok::BRANCH_FREQUENCY => b.frequency = r.float()?,
            tok::BRANCH_S_TILE => b.layers[0].s_tile = r.float()?,
            tok::BRANCH_T_TILE => b.layers[0].t_tile = r.float()?,
            tok::BRANCH_S_TILE_ABSOLUTE => b.layers[0].s_tile_absolute = r.boolean()?,
            tok::BRANCH_T_TILE_ABSOLUTE => b.layers[0].t_tile_absolute = r.boolean()?,
            _ => return Err(unexpected(t, at, S)),
        }
    }
}

fn parse_leaf_info(r: &mut Reader<'_>, l: &mut LeafInfo) -> Result<()> {
    const S: &str = "leaf info";
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::LEAVES_END => return Ok(()),
            tok::LEAF_BLOSSOM_DISTANCE => l.blossom_distance = r.float()?,
            tok::LEAF_BLOSSOM_LEVEL => {
                l.blossom_level = r.int()?;
                // The blossom level also sets the dimming depth (a later section may
                // override it).
                l.dimming_depth = l.blossom_level;
            }
            tok::LEAF_BLOSSOM_WEIGHTING => l.blossom_weighting = r.float()?,
            tok::LEAF_COLOR_SHIFT | tok::LEAF_LIT => {
                r.boolean()?;
            }
            tok::LEAF_COLOR_SHIFT_SCALAR | tok::LEAF_LIGHT_SCALAR => {
                r.float()?;
            }
            tok::LEAF_DIMMING => l.dimming = r.boolean()?,
            tok::LEAF_DIMMING_SCALAR => l.dimming_scalar = r.float()?,
            tok::LEAF_SPACING_TOLERANCE => l.spacing_tolerance = r.float()?,
            tok::LEAF_COLLISION => l.collision = r.int()?,
            tok::LEAF_TEXTURES_START => {
                const T: &str = "leaf textures";
                r.expect(tok::LEAF_TEXTURE_COUNT, T)?;
                let n = r.count(T, 8)?;
                let mut textures = Vec::with_capacity(n);
                for _ in 0..n {
                    r.expect(tok::LEAF_TEXTURE_START, T)?;
                    textures.push(parse_leaf_texture(r)?);
                }
                r.expect(tok::LEAF_TEXTURES_END, T)?;
                l.textures = textures;
            }
            _ => return Err(unexpected(t, at, S)),
        }
    }
}

fn parse_leaf_texture(r: &mut Reader<'_>) -> Result<LeafTexture> {
    const S: &str = "leaf texture";
    let mut x = LeafTexture::default();
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::LEAF_TEXTURE_END => return Ok(x),
            tok::LEAF_TEX_BLOSSOM => x.blossom = r.boolean()?,
            tok::LEAF_TEX_COLOR => x.color = r.vec3()?,
            tok::LEAF_TEX_COLOR_VARIANCE => x.color_variance = r.float()?,
            tok::LEAF_TEX_FILENAME => x.filename = r.string(S)?,
            tok::LEAF_TEX_ORIGIN => x.origin = r.vec3()?,
            tok::LEAF_TEX_SIZE => x.size = r.vec3()?,
            tok::LEAF_TEX_SIZE_USED => x.size_used = r.vec3()?,
            tok::LEAF_TEX_RATIO => x.ratio = Some(r.float()?),
            _ => return Err(unexpected(t, at, S)),
        }
    }
}

fn parse_old_wind(r: &mut Reader<'_>, w: &mut OldWind) -> Result<()> {
    const S: &str = "wind info";
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::OLD_WIND_END => return Ok(()),
            tok::OLD_WIND_DIRECTION => w.direction = Some(r.vec3()?),
            tok::OLD_WIND_BRANCH_OSCILLATION => w.branch_oscillation = Some(r.vec3()?),
            tok::OLD_WIND_LEAF_OSCILLATION => w.leaf_oscillation = r.vec3()?,
            tok::OLD_WIND_BRANCH_FACTORS => w.branch_factors = Some(r.vec3()?),
            tok::OLD_WIND_LEAF_FACTORS => w.leaf_factors = r.vec3()?,
            tok::OLD_WIND_STRENGTH => w.strength = r.float()?,
            tok::OLD_WIND_ENABLED => w.enabled = Some(r.boolean()?),
            _ => return Err(unexpected(t, at, S)),
        }
    }
}

fn parse_leaf_clusters(r: &mut Reader<'_>) -> Result<Vec<Vec<BillboardLeaf>>> {
    const S: &str = "leaf clusters";
    let at = r.pos;
    let declared = r.int()?;
    let max = usize::try_from(declared).map_err(|_| SptError::BadCount {
        count: declared,
        offset: at,
        section: S,
    })?;
    let mut lods = Vec::new();
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::CLUSTER_END => return Ok(lods),
            tok::CLUSTER_LOD_START if lods.len() < max => {
                let mut leaves = Vec::new();
                loop {
                    let (t, at) = r.token()?;
                    match t {
                        tok::CLUSTER_LOD_END => break,
                        tok::CLUSTER_LEAF_START => leaves.push(parse_billboard_leaf(r)?),
                        _ => return Err(unexpected(t, at, "leaf cluster LOD")),
                    }
                }
                lods.push(leaves);
            }
            tok::CLUSTER_LOD_START => {
                return Err(SptError::Invalid(format!(
                    "leaf clusters: more LOD levels than the {max} declared, at offset {at}"
                )));
            }
            _ => return Err(unexpected(t, at, S)),
        }
    }
}

fn parse_billboard_leaf(r: &mut Reader<'_>) -> Result<BillboardLeaf> {
    const S: &str = "billboard leaf";
    let mut b = BillboardLeaf::default();
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::CLUSTER_LEAF_END => return Ok(b),
            tok::CLUSTER_LEAF_COLOR => b.color = r.padded_uint()?,
            tok::CLUSTER_LEAF_ANGLE_INDEX => b.angle_index = r.byte()?,
            tok::CLUSTER_LEAF_COLOR_SCALE => b.color_scale = r.byte()?,
            tok::CLUSTER_LEAF_NORMAL => b.normal = r.vec3()?,
            tok::CLUSTER_LEAF_TEXTURE_INDEX => b.texture_index = r.byte()?,
            tok::CLUSTER_LEAF_WIND_WEIGHT => b.wind_weight = r.byte()?,
            tok::CLUSTER_LEAF_WIND_GROUP => b.wind_group = r.int()?,
            tok::CLUSTER_LEAF_POSITION => b.position = r.vec3()?,
            tok::CLUSTER_LEAF_TIME_OFFSET => b.time_offset = Some(r.float()?),
            _ => return Err(unexpected(t, at, S)),
        }
    }
}

fn material(r: &mut Reader<'_>) -> Result<Material> {
    Ok(Material {
        diffuse: r.vec3()?,
        ambient: r.vec3()?,
        specular: r.vec3()?,
        emissive: r.vec3()?,
        shininess: r.float()?,
    })
}

fn parse_lighting(r: &mut Reader<'_>, l: &mut Lighting) -> Result<()> {
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::LIGHTING_END => return Ok(()),
            tok::LIGHTING_BRANCH_METHOD => l.branch_method = Some(r.int()?),
            tok::LIGHTING_BRANCH_MATERIAL => l.branch_material = Some(material(r)?),
            tok::LIGHTING_LEAF_METHOD => l.leaf_method = Some(r.int()?),
            tok::LIGHTING_LEAF_MATERIAL => l.leaf_material = Some(material(r)?),
            tok::LIGHTING_LEAF_ADJUSTMENT => l.leaf_adjustment = Some(r.float()?),
            tok::LIGHTING_STATIC_STYLE => l.static_style = Some(r.int()?),
            tok::LIGHTING_FROND_METHOD => l.frond_method = Some(r.int()?),
            tok::LIGHTING_FROND_MATERIAL => l.frond_material = Some(material(r)?),
            _ => return Err(unexpected(t, at, "lighting")),
        }
    }
}

fn parse_lod(r: &mut Reader<'_>, l: &mut Lod) -> Result<()> {
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::LOD_END => return Ok(()),
            tok::LOD_LEAF_TRANSITION => l.leaf_transition = Some(r.int()?),
            tok::LOD_LEAF_TRANSITION_RADIUS => l.leaf_transition_radius = Some(r.float()?),
            tok::LOD_LEAF_CURVE_EXPONENT => l.leaf_curve_exponent = Some(r.float()?),
            tok::LOD_LEAF_SIZE_INCREASE => l.leaf_size_increase = Some(r.float()?),
            tok::LOD_ENGINE_START => loop {
                let (t, at) = r.token()?;
                match t {
                    tok::LOD_ENGINE_END => break,
                    tok::LOD_BRANCH_COUNT => l.branch_lod_count = Some(r.int()?),
                    tok::LOD_BRANCH_MIN_VOLUME => l.branch_min_volume_percent = Some(r.float()?),
                    tok::LOD_LEAF_REDUCTION => l.leaf_reduction = Some(r.float()?),
                    tok::LOD_LEAF_COUNT => l.leaf_lod_count = Some(r.int()?),
                    tok::LOD_BRANCH_MAX_VOLUME => l.branch_max_volume_percent = Some(r.float()?),
                    tok::LOD_BRANCH_FUZZINESS => l.branch_reduction_fuzziness = Some(r.float()?),
                    tok::LOD_LARGE_BRANCH_PERCENT => l.large_branch_percent = Some(r.float()?),
                    _ => return Err(unexpected(t, at, "engine LOD")),
                }
            },
            _ => return Err(unexpected(t, at, "LOD")),
        }
    }
}

fn parse_lod2(r: &mut Reader<'_>, l: &mut Lod) -> Result<()> {
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::LOD2_END => return Ok(()),
            tok::LOD2_BRANCH_FADE_DISTANCE => l.branch_fade_distance = Some(r.float()?),
            tok::LOD2_FROND_FADE_DISTANCE => l.frond_fade_distance = Some(r.float()?),
            tok::LOD2_FADE_EXTRUSIONS => l.fade_extrusions = Some(r.boolean()?),
            tok::LOD2_BILLBOARD_TRANSITION => l.billboard_transition = Some(r.float()?),
            _ => return Err(unexpected(t, at, "supplemental LOD")),
        }
    }
}

fn parse_wind(r: &mut Reader<'_>, level: &mut Option<i32>) -> Result<()> {
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::WIND_END => return Ok(()),
            tok::WIND_LEVEL => *level = Some(r.int()?),
            _ => return Err(unexpected(t, at, "wind")),
        }
    }
}

fn coord_table(r: &mut Reader<'_>, section: &'static str) -> Result<Vec<[f32; 8]>> {
    let n = r.count(section, 32)?;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(r.floats()?);
    }
    Ok(v)
}

fn parse_texcoords(r: &mut Reader<'_>, c: &mut TexCoordTables) -> Result<()> {
    const S: &str = "texture coordinates";
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::TEXCOORDS_END => return Ok(()),
            tok::TEXCOORDS_LEAF => c.leaves = coord_table(r, S)?,
            tok::TEXCOORDS_BILLBOARD => c.billboards = coord_table(r, S)?,
            tok::TEXCOORDS_FROND => c.fronds = coord_table(r, S)?,
            tok::TEXCOORDS_COMPOSITE => c.composite = Some(r.string(S)?),
            tok::TEXCOORDS_HORIZONTAL_BILLBOARD => c.horizontal_billboard = Some(r.boolean()?),
            tok::TEXCOORDS_360_BILLBOARD => c.billboard_360 = Some(r.boolean()?),
            _ => return Err(unexpected(t, at, S)),
        }
    }
}

fn parse_texcoords2(r: &mut Reader<'_>, c: &mut TexCoordTables) -> Result<()> {
    const S: &str = "supplemental texture coordinates";
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::TEXCOORDS2_END => return Ok(()),
            tok::TEXCOORDS2_COMPOSITE => c.composite = Some(r.string(S)?),
            tok::TEXCOORDS2_HORIZONTAL_BILLBOARD => c.horizontal_billboard = Some(r.boolean()?),
            tok::TEXCOORDS2_360_BILLBOARD => c.billboard_360 = Some(r.boolean()?),
            tok::TEXCOORDS2_SHADOW => c.shadow = Some(r.floats()?),
            _ => return Err(unexpected(t, at, S)),
        }
    }
}

fn parse_collision(r: &mut Reader<'_>, objects: &mut Vec<CollisionObject>) -> Result<()> {
    loop {
        let (t, at) = r.token()?;
        let shape = match t {
            tok::COLLISION_END => return Ok(()),
            tok::COLLISION_SPHERE => {
                let position = r.vec3()?;
                (position, CollisionShape::Sphere { radius: r.float()? })
            }
            tok::COLLISION_CAPSULE => {
                let position = r.vec3()?;
                let [radius, height] = r.floats()?;
                (position, CollisionShape::Capsule { radius, height })
            }
            tok::COLLISION_BOX => {
                let position = r.vec3()?;
                (position, CollisionShape::Box { size: r.vec3()? })
            }
            _ => return Err(unexpected(t, at, "collision")),
        };
        objects.push(CollisionObject {
            shape: shape.1,
            position: shape.0,
            rotation: [0.0; 3],
        });
    }
}

fn parse_collision2(r: &mut Reader<'_>, objects: &mut [CollisionObject]) -> Result<()> {
    const S: &str = "collision rotations";
    let mut index = 0;
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::COLLISION2_END => return Ok(()),
            tok::COLLISION2_OBJECT_START => {
                let Some(object) = objects.get_mut(index) else {
                    return Err(SptError::Invalid(format!(
                        "{S}: object {index} at offset {at}, but only {} collision objects",
                        objects.len()
                    )));
                };
                loop {
                    let (t, at) = r.token()?;
                    match t {
                        tok::COLLISION2_OBJECT_END => break,
                        tok::COLLISION2_ROTATE_X => object.rotation[0] = r.float()?,
                        tok::COLLISION2_ROTATE_Y => object.rotation[1] = r.float()?,
                        tok::COLLISION2_ROTATE_Z => object.rotation[2] = r.float()?,
                        _ => return Err(unexpected(t, at, S)),
                    }
                }
                index += 1;
            }
            _ => return Err(unexpected(t, at, S)),
        }
    }
}

fn parse_fronds(r: &mut Reader<'_>, f: &mut Fronds) -> Result<()> {
    const S: &str = "fronds";
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::FROND_END => return Ok(()),
            tok::FROND_LEVEL => f.level = r.int()?,
            tok::FROND_KIND => f.kind = r.int()?,
            tok::FROND_BLADES => f.blades = r.int()?,
            tok::FROND_PROFILE => f.profile = Some(r.spline(S)?),
            tok::FROND_PROFILE_SEGMENTS => f.profile_segments = r.int()?,
            tok::FROND_ENABLED => f.enabled = r.boolean()?,
            tok::FROND_LOD_COUNT => f.lod_count = r.int()?,
            tok::FROND_MAX_SURFACE => f.max_surface_percent = r.float()?,
            tok::FROND_MIN_SURFACE => f.min_surface_percent = r.float()?,
            tok::FROND_FUZZINESS => f.reduction_fuzziness = r.float()?,
            tok::FROND_LARGE_PERCENT => f.large_frond_percent = r.float()?,
            tok::FROND_MIN_LENGTH_SEGMENTS => f.min_length_segments = r.int()?,
            tok::FROND_MIN_CROSS_SEGMENTS => f.min_cross_segments = r.int()?,
            tok::FROND_TEXTURE_COUNT => {
                const T: &str = "frond texture";
                let n = r.count(S, 8)?;
                let mut textures = Vec::with_capacity(n);
                for _ in 0..n {
                    r.expect(tok::FROND_TEX_START, T)?;
                    let mut x = FrondTexture::default();
                    loop {
                        let (t, at) = r.token()?;
                        match t {
                            tok::FROND_TEX_END => break,
                            tok::FROND_TEX_FILENAME => x.filename = r.string(T)?,
                            tok::FROND_TEX_ASPECT => x.aspect_ratio = r.float()?,
                            tok::FROND_TEX_SIZE_SCALE => x.size_scale = r.float()?,
                            tok::FROND_TEX_MIN_ANGLE => x.min_angle_offset = r.float()?,
                            tok::FROND_TEX_MAX_ANGLE => x.max_angle_offset = r.float()?,
                            _ => return Err(unexpected(t, at, T)),
                        }
                    }
                    textures.push(x);
                }
                f.textures = textures;
            }
            _ => return Err(unexpected(t, at, S)),
        }
    }
}

fn parse_fronds2(r: &mut Reader<'_>, f: &mut Fronds) -> Result<()> {
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::FROND2_END => return Ok(()),
            tok::FROND2_DISTANCE_LEVEL => f.distance_level = r.int()?,
            tok::FROND2_DISTANCE_PERCENT => f.distance_percent = r.float()?,
            tok::FROND2_ABOVE => f.above_condition = r.int()?,
            tok::FROND2_BELOW => f.below_condition = r.int()?,
            tok::FROND2_SEGMENT_OVERRIDE => f.segment_override = r.int()?,
            tok::FROND2_USE_SEGMENT_OVERRIDE => f.use_segment_override = r.boolean()?,
            _ => return Err(unexpected(t, at, "supplemental fronds")),
        }
    }
}

/// Section 15000: per branch level, the diffuse layer's random offset then its twist.
fn parse_texture_controls(r: &mut Reader<'_>, levels: &mut [BranchLevel]) -> Result<()> {
    const S: &str = "texture controls";
    for b in levels.iter_mut() {
        r.expect(tok::TEX_CONTROLS_RANDOM_OFFSET, S)?;
        b.layers[0].random_t_offset = r.boolean()?;
        r.expect(tok::TEX_CONTROLS_TWIST, S)?;
        b.layers[0].twist = r.float()?;
    }
    r.expect(tok::TEX_CONTROLS_END, S)
}

/// Section 16000: per branch level, the eleven flare values in a fixed order.
fn parse_flares(r: &mut Reader<'_>, levels: &mut [BranchLevel]) -> Result<()> {
    const S: &str = "flares";
    for b in levels.iter_mut() {
        let f = &mut b.flares;
        let float = |r: &mut Reader<'_>, token: i32| -> Result<f32> {
            r.expect(token, S)?;
            r.float()
        };
        f.segment_packing_exponent = float(r, tok::FLARE_PACKING_EXPONENT)?;
        r.expect(tok::FLARE_COUNT, S)?;
        f.count = r.int()?;
        f.balance = float(r, tok::FLARE_BALANCE)?;
        f.radial_influence = float(r, tok::FLARE_RADIAL_INFLUENCE)?;
        f.radial_influence_variance = float(r, tok::FLARE_RADIAL_INFLUENCE_VARIANCE)?;
        f.radial_exponent = float(r, tok::FLARE_RADIAL_EXPONENT)?;
        f.radial_distance = float(r, tok::FLARE_RADIAL_DISTANCE)?;
        f.radial_distance_variance = float(r, tok::FLARE_RADIAL_DISTANCE_VARIANCE)?;
        f.length_distance = float(r, tok::FLARE_LENGTH_DISTANCE)?;
        f.length_distance_variance = float(r, tok::FLARE_LENGTH_DISTANCE_VARIANCE)?;
        f.length_exponent = float(r, tok::FLARE_LENGTH_EXPONENT)?;
    }
    r.expect(tok::FLARES_END, S)
}

/// Section 23000: per branch level, the light seam start and end biases.
fn parse_light_seams(r: &mut Reader<'_>, levels: &mut [BranchLevel]) -> Result<()> {
    const S: &str = "light seams";
    for b in levels.iter_mut() {
        r.expect(tok::LIGHT_SEAM_START_BIAS, S)?;
        b.light_seam_start_bias = r.float()?;
        r.expect(tok::LIGHT_SEAM_END_BIAS, S)?;
        b.light_seam_end_bias = r.float()?;
    }
    r.expect(tok::LIGHT_SEAM_END, S)
}

fn parse_leaf_placement(r: &mut Reader<'_>, l: &mut LeafInfo) -> Result<()> {
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::LEAF_PLACEMENT_END => return Ok(()),
            tok::LEAF_PLACEMENT_DISTANCE => l.placement_distance = r.float()?,
            tok::LEAF_PLACEMENT_LEVEL => l.placement_level = r.int()?,
            _ => return Err(unexpected(t, at, "leaf placement")),
        }
    }
}

fn parse_leaf_smoothing(r: &mut Reader<'_>, l: &mut LeafInfo) -> Result<()> {
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::LEAF_SMOOTHING_END => return Ok(()),
            tok::LEAF_SMOOTHING_ENABLED => l.smoothing_enabled = r.boolean()?,
            tok::LEAF_SMOOTHING_ANGLE => l.smoothing_angle = r.float()?,
            tok::LEAF_DIMMING_DEPTH => l.dimming_depth = r.int()?,
            _ => return Err(unexpected(t, at, "leaf smoothing")),
        }
    }
}

/// One field of the supplemental branch data, if `t` is one. The per-level section (26000)
/// and the roots' block (40007) share these tokens; the roots' block also takes the flare,
/// light seam and diffuse texture-control tokens, which are accepted in both here.
fn branch_extra(r: &mut Reader<'_>, b: &mut BranchLevel, t: i32, s: &'static str) -> Result<bool> {
    match t {
        tok::BRANCH2_ROUGHNESS => b.roughness = r.float()?,
        tok::BRANCH2_ROUGHNESS_V_FREQUENCY => b.roughness_vertical_frequency = r.float()?,
        tok::BRANCH2_ROUGHNESS_H_FREQUENCY => b.roughness_horizontal_frequency = r.float()?,
        tok::BRANCH2_RANDOM_ROUGHNESS => b.random_roughness = r.float()?,
        tok::BRANCH2_MIN_LENGTH_PERCENT => b.min_length_percent = r.float()?,
        tok::BRANCH2_MIN_CROSS_SECTION_PERCENT => b.min_cross_section_percent = r.float()?,
        tok::BRANCH2_PRUNING_PERCENT => b.pruning_percent = r.float()?,
        tok::BRANCH2_PRUNING_DEPTH => b.pruning_depth = r.int()? as u32,
        tok::BRANCH2_FORK_ENABLED => b.fork_enabled = r.boolean()?,
        tok::BRANCH2_FORK_BIAS => b.fork_bias = r.float()?,
        tok::BRANCH2_FORK_ANGLE => b.fork_angle = r.float()?,
        tok::BRANCH2_FORK_LIMIT => b.fork_limit = r.int()?,
        tok::BRANCH2_CROSS_SECTION_PROFILE => b.cross_section_profile = Some(r.spline(s)?),
        tok::BRANCH2_LIGHT_SEAM_PROFILE => b.light_seam_profile = Some(r.spline(s)?),
        tok::BRANCH2_ROUGHNESS_PROFILE => b.roughness_profile = Some(r.spline(s)?),
        tok::BRANCH2_FREQUENCY_PROFILE => b.frequency_profile = Some(r.spline(s)?),
        tok::BRANCH2_LIGHT_SEAM_BIAS => b.light_seam_bias = Some(r.spline(s)?),
        tok::BRANCH2_GNARL_PROFILE => b.gnarl_profile = Some(r.spline(s)?),
        tok::BRANCH2_GNARL => b.gnarl = r.float()?,
        tok::BRANCH2_GNARL_UNISON => b.gnarl_unison = r.boolean()?,
        tok::FLARE_PACKING_EXPONENT => b.flares.segment_packing_exponent = r.float()?,
        tok::FLARE_COUNT => b.flares.count = r.int()?,
        tok::FLARE_BALANCE => b.flares.balance = r.float()?,
        tok::FLARE_RADIAL_INFLUENCE => b.flares.radial_influence = r.float()?,
        tok::FLARE_RADIAL_INFLUENCE_VARIANCE => b.flares.radial_influence_variance = r.float()?,
        tok::FLARE_RADIAL_EXPONENT => b.flares.radial_exponent = r.float()?,
        tok::FLARE_RADIAL_DISTANCE => b.flares.radial_distance = r.float()?,
        tok::FLARE_RADIAL_DISTANCE_VARIANCE => b.flares.radial_distance_variance = r.float()?,
        tok::FLARE_LENGTH_DISTANCE => b.flares.length_distance = r.float()?,
        tok::FLARE_LENGTH_DISTANCE_VARIANCE => b.flares.length_distance_variance = r.float()?,
        tok::FLARE_LENGTH_EXPONENT => b.flares.length_exponent = r.float()?,
        tok::TEX_CONTROLS_TWIST => b.layers[0].twist = r.float()?,
        tok::TEX_CONTROLS_RANDOM_OFFSET => b.layers[0].random_t_offset = r.boolean()?,
        tok::LIGHT_SEAM_START_BIAS => b.light_seam_start_bias = r.float()?,
        tok::LIGHT_SEAM_END_BIAS => b.light_seam_end_bias = r.float()?,
        _ => return Ok(false),
    }
    Ok(true)
}

/// Section 26000: per branch level, a start token, fields, and an end token.
fn parse_branch2(r: &mut Reader<'_>, levels: &mut [BranchLevel]) -> Result<()> {
    const S: &str = "supplemental branch info";
    for b in levels.iter_mut() {
        r.expect(tok::BRANCH2_LEVEL_START, S)?;
        loop {
            let (t, at) = r.token()?;
            if t == tok::BRANCH2_LEVEL_END {
                break;
            }
            if !branch_extra(r, b, t, S)? {
                return Err(unexpected(t, at, S));
            }
        }
    }
    r.expect(tok::BRANCH2_END, S)
}

fn parse_floor(r: &mut Reader<'_>, f: &mut Floor) -> Result<()> {
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::FLOOR_END => return Ok(()),
            tok::FLOOR_ENABLED => f.enabled = Some(r.boolean()?),
            tok::FLOOR_VALUE => f.value = Some(r.float()?),
            tok::FLOOR_LEVEL => f.level = Some(r.int()?),
            tok::FLOOR_EXPONENT => f.exponent = Some(r.float()?),
            tok::FLOOR_BIAS => f.bias = Some(r.float()?),
            _ => return Err(unexpected(t, at, "floor")),
        }
    }
}

fn parse_cluster_info(r: &mut Reader<'_>, first: &mut Option<i32>) -> Result<()> {
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::CLUSTER_INFO_END => return Ok(()),
            tok::CLUSTER_FIRST_BRANCH_LEVEL => *first = Some(r.int()?),
            _ => return Err(unexpected(t, at, "cluster info")),
        }
    }
}

fn parse_shader(r: &mut Reader<'_>, s: &mut ShaderScalars) -> Result<()> {
    loop {
        let (t, at) = r.token()?;
        let slot = match t {
            tok::SHADER_END => return Ok(()),
            tok::SHADER_BRANCH_LIGHT => &mut s.branch_light,
            tok::SHADER_FROND_LIGHT => &mut s.frond_light,
            tok::SHADER_LEAF_LIGHT => &mut s.leaf_light,
            tok::SHADER_GLOBAL_LIGHT => &mut s.global_light,
            tok::SHADER_AMBIENT => &mut s.ambient,
            tok::SHADER_BILLBOARD_DARK_SIDE => &mut s.billboard_dark_side,
            tok::SHADER_BILLBOARD_AMBIENT => &mut s.billboard_ambient,
            tok::SHADER_BILLBOARD_BRIGHT_SIDE => &mut s.billboard_bright_side,
            _ => return Err(unexpected(t, at, "shader scalars")),
        };
        *slot = Some(r.float()?);
    }
}

fn parse_roots(r: &mut Reader<'_>, roots: &mut RootSupport) -> Result<()> {
    const S: &str = "root support";
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::ROOTS_END => return Ok(()),
            tok::ROOTS_LEVEL => roots.level = r.int()?,
            tok::ROOTS_FIRST => roots.first = r.float()?,
            tok::ROOTS_LAST => roots.last = r.float()?,
            tok::ROOTS_FREQUENCY => roots.frequency = r.float()?,
            tok::ROOTS_BRANCH => parse_branch_level(r, &mut roots.roots)?,
            tok::ROOTS_EXTRA_START => loop {
                let (t, at) = r.token()?;
                if t == tok::ROOTS_EXTRA_END {
                    break;
                }
                if !branch_extra(r, &mut roots.roots, t, S)? {
                    return Err(unexpected(t, at, S));
                }
            },
            _ => return Err(unexpected(t, at, S)),
        }
    }
}

fn parse_tex_layer(r: &mut Reader<'_>, x: &mut TexLayer) -> Result<()> {
    const S: &str = "texture layer";
    r.expect(tok::TEX_LAYER_START, S)?;
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::TEX_LAYER_END => return Ok(()),
            tok::TEX_LAYER_S_TILE => x.s_tile = r.float()?,
            tok::TEX_LAYER_T_TILE => x.t_tile = r.float()?,
            tok::TEX_LAYER_S_TILE_ABSOLUTE => x.s_tile_absolute = r.boolean()?,
            tok::TEX_LAYER_T_TILE_ABSOLUTE => x.t_tile_absolute = r.boolean()?,
            tok::TEX_LAYER_RANDOM_T_OFFSET => x.random_t_offset = r.boolean()?,
            tok::TEX_LAYER_TWIST => x.twist = r.float()?,
            tok::TEX_LAYER_T_OFFSET => x.t_offset = r.float()?,
            tok::TEX_LAYER_S_OFFSET => x.s_offset = r.float()?,
            tok::TEX_LAYER_CLAMP_S => x.clamp_s = r.boolean()?,
            tok::TEX_LAYER_CLAMP_T => x.clamp_t = r.boolean()?,
            tok::TEX_LAYER_CLAMP_LEFT => x.clamp[0] = r.float()?,
            tok::TEX_LAYER_CLAMP_RIGHT => x.clamp[1] = r.float()?,
            tok::TEX_LAYER_CLAMP_BOTTOM => x.clamp[2] = r.float()?,
            tok::TEX_LAYER_CLAMP_TOP => x.clamp[3] = r.float()?,
            tok::TEX_LAYER_SYNC => x.sync = r.boolean()?,
            _ => return Err(unexpected(t, at, S)),
        }
    }
}

/// Section 50000: for each branch level and then the roots, one block per texture layer.
fn parse_tex_layers(r: &mut Reader<'_>, tree: &mut SpeedTree) -> Result<()> {
    for b in tree.branch_levels.iter_mut() {
        for x in b.layers.iter_mut() {
            parse_tex_layer(r, x)?;
        }
    }
    let mut roots = tree
        .roots
        .as_ref()
        .map_or([TexLayer::default(); TEXTURE_LAYERS], |s| s.roots.layers);
    for x in roots.iter_mut() {
        parse_tex_layer(r, x)?;
    }
    if let Some(s) = tree.roots.as_mut() {
        s.roots.layers = roots;
    }
    tree.root_layers = Some(roots);
    r.expect(tok::TEX_LAYERS_END, "texture layers")
}

fn parse_map_set(r: &mut Reader<'_>) -> Result<MapSet> {
    const S: &str = "map set";
    r.expect(tok::MAP_SET_START, S)?;
    let mut m = MapSet::default();
    loop {
        let (t, at) = r.token()?;
        let slot = match t {
            tok::MAP_SET_END => return Ok(m),
            tok::MAP_SET_DIFFUSE => &mut m.diffuse,
            tok::MAP_SET_DETAIL => &mut m.detail,
            tok::MAP_SET_NORMAL => &mut m.normal,
            tok::MAP_SET_HEIGHT => &mut m.height,
            tok::MAP_SET_SPECULAR => &mut m.specular,
            tok::MAP_SET_USER1 => &mut m.user1,
            tok::MAP_SET_USER2 => &mut m.user2,
            _ => return Err(unexpected(t, at, S)),
        };
        *slot = r.string(S)?;
    }
}

fn parse_maps(r: &mut Reader<'_>, m: &mut MapBank) -> Result<()> {
    const S: &str = "map bank";
    let sets = |r: &mut Reader<'_>, count_token: i32| -> Result<Vec<MapSet>> {
        r.expect(count_token, S)?;
        let n = r.count(S, 8)?;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(parse_map_set(r)?);
        }
        Ok(v)
    };
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::MAPS_END => return Ok(()),
            tok::MAPS_BRANCH => m.branch = Some(parse_map_set(r)?),
            tok::MAPS_LEAVES => m.leaves = sets(r, tok::MAPS_LEAF_COUNT)?,
            tok::MAPS_FRONDS => m.fronds = sets(r, tok::MAPS_FROND_COUNT)?,
            tok::MAPS_COMPOSITE => m.composite = Some(parse_map_set(r)?),
            tok::MAPS_BILLBOARD => m.billboard = Some(parse_map_set(r)?),
            tok::MAPS_SELF_SHADOW => m.self_shadow = Some(r.string(S)?),
            _ => return Err(unexpected(t, at, S)),
        }
    }
}

fn parse_meshes(r: &mut Reader<'_>, meshes: &mut Vec<LeafMesh>) -> Result<()> {
    const S: &str = "meshes";
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::MESHES_END => return Ok(()),
            // A capacity hint only.
            tok::MESHES_COUNT => {
                r.int()?;
            }
            tok::MESH_START => {
                let mut mesh = LeafMesh::default();
                loop {
                    let (t, at) = r.token()?;
                    match t {
                        tok::MESH_END => break,
                        tok::MESH_NAME => mesh.name = r.string(S)?,
                        tok::MESH_VERTEX_COUNT | tok::MESH_INDEX_COUNT => {
                            r.int()?;
                        }
                        tok::MESH_INDEX => mesh.indices.push(r.int()?),
                        tok::MESH_VERTEX_START => {
                            let mut v = MeshVertex::default();
                            loop {
                                let (t, at) = r.token()?;
                                match t {
                                    tok::MESH_VERTEX_END => break,
                                    tok::MESH_VERTEX_POSITION => v.position = r.vec3()?,
                                    tok::MESH_VERTEX_NORMAL => v.normal = r.vec3()?,
                                    tok::MESH_VERTEX_BINORMAL => v.binormal = r.vec3()?,
                                    tok::MESH_VERTEX_TANGENT => v.tangent = r.vec3()?,
                                    tok::MESH_VERTEX_UV => v.uv = r.floats()?,
                                    _ => return Err(unexpected(t, at, "mesh vertex")),
                                }
                            }
                            mesh.vertices.push(v);
                        }
                        _ => return Err(unexpected(t, at, "mesh")),
                    }
                }
                meshes.push(mesh);
            }
            _ => return Err(unexpected(t, at, S)),
        }
    }
}

/// Section 72000: one block per leaf texture, in order. Blocks past the leaf textures are
/// read and dropped, as SpeedTree does.
fn parse_leaf_meshes(r: &mut Reader<'_>, l: &mut LeafInfo) -> Result<()> {
    const S: &str = "leaf meshes";
    let mut index = 0;
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::LEAF_MESHES_END => return Ok(()),
            tok::CLUSTER_START => {
                let mut m = LeafMeshUse::default();
                loop {
                    let (t, at) = r.token()?;
                    match t {
                        tok::CLUSTER_END => break,
                        tok::LEAF_MESH_INDEX_COUNT => {
                            r.int()?;
                        }
                        tok::LEAF_MESH_INDEX => m.mesh = r.int()?,
                        tok::LEAF_MESH_USE => m.use_meshes = r.boolean()?,
                        tok::LEAF_MESH_HANG => m.hang = r.float()?,
                        tok::LEAF_MESH_ROTATE => m.rotate = r.float()?,
                        _ => return Err(unexpected(t, at, S)),
                    }
                }
                if let Some(x) = l.textures.get_mut(index) {
                    x.mesh = Some(m);
                }
                index += 1;
            }
            _ => return Err(unexpected(t, at, S)),
        }
    }
}

fn parse_shadow(r: &mut Reader<'_>, s: &mut ShadowProjection) -> Result<()> {
    const S: &str = "shadow projection";
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::SHADOW_END => return Ok(()),
            tok::SHADOW_RIGHT => s.right = Some(r.vec3()?),
            tok::SHADOW_UP => s.up = Some(r.vec3()?),
            tok::SHADOW_OUT => s.out = Some(r.vec3()?),
            tok::SHADOW_MAP => s.map = Some(r.string(S)?),
            _ => return Err(unexpected(t, at, S)),
        }
    }
}

fn parse_user_data(r: &mut Reader<'_>, u: &mut Option<String>) -> Result<()> {
    const S: &str = "user data";
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::USER_DATA_END => return Ok(()),
            tok::USER_DATA => *u = Some(r.string(S)?),
            _ => return Err(unexpected(t, at, S)),
        }
    }
}

fn parse_global2(r: &mut Reader<'_>, angle: &mut Option<f32>) -> Result<()> {
    loop {
        let (t, at) = r.token()?;
        match t {
            tok::GLOBAL2_END => return Ok(()),
            tok::GLOBAL2_TREE_ANGLE => *angle = Some(r.float()?),
            _ => return Err(unexpected(t, at, "supplemental global info")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a token stream.
    #[derive(Default)]
    struct W(Vec<u8>, Vec<usize>);

    impl W {
        /// Records a point where the stream may end (before a trailing section).
        fn mark(&mut self) {
            self.1.push(self.0.len());
        }
        fn t(&mut self, token: i32) -> &mut Self {
            self.i(token)
        }
        fn i(&mut self, v: i32) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn f(&mut self, v: f32) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn b(&mut self, v: bool) -> &mut Self {
            self.0.push(v as u8);
            self
        }
        fn byte(&mut self, v: u8) -> &mut Self {
            self.0.push(v);
            self
        }
        fn s(&mut self, v: &str) -> &mut Self {
            self.i(v.len() as i32);
            self.0.extend_from_slice(v.as_bytes());
            self
        }
        fn v3(&mut self, v: [f32; 3]) -> &mut Self {
            v.iter().for_each(|&x| {
                self.f(x);
            });
            self
        }
    }

    const SPLINE: &str = "BezierSpline 0 1 5\n{\n\t2\n\t0 0 1 0 0.25\n\t1 1 0.7 0.7 0.5\n\n}\n";

    /// A small but complete tree: the original layout and a selection of later sections.
    fn sample() -> Vec<u8> {
        sample_with_ends().0
    }

    /// The sample and the lengths at which it may end.
    fn sample_with_ends() -> (Vec<u8>, Vec<usize>) {
        let mut w = W::default();
        w.t(tok::FILE_START).s(HEADER);
        w.t(tok::TREE_START)
            .t(tok::TREE_BRANCH_TEXTURE)
            .s("bark.dds")
            .t(tok::TREE_FAR)
            .f(500.0)
            .t(tok::TREE_LOD_FLAG)
            .b(false)
            .t(tok::TREE_NEAR)
            .f(15.0)
            .t(tok::TREE_SEED)
            .i(532_513)
            .t(tok::TREE_SIZE)
            .f(40.0)
            .t(tok::TREE_SIZE_VARIANCE)
            .f(5.0);
        w.t(tok::BRANCHES_START).i(2);
        for level in 0..2 {
            w.t(tok::BRANCH_LEVEL_START)
                .t(tok::BRANCH_LENGTH)
                .s(SPLINE)
                .t(tok::BRANCH_SEGMENTS)
                .i(4 + level)
                .t(tok::BRANCH_S_TILE_ABSOLUTE)
                .b(false)
                .t(tok::BRANCH_FREQUENCY)
                .f(2.0)
                .t(tok::BRANCH_LEVEL_END);
        }
        w.t(tok::BRANCHES_END).t(tok::TREE_END);
        w.t(tok::LEAVES_START)
            .t(tok::LEAF_BLOSSOM_LEVEL)
            .i(3)
            .t(tok::LEAF_COLOR_SHIFT)
            .b(false)
            .t(tok::LEAF_TEXTURES_START)
            .t(tok::LEAF_TEXTURE_COUNT)
            .i(1)
            .t(tok::LEAF_TEXTURE_START)
            .t(tok::LEAF_TEX_FILENAME)
            .s("leaf.dds")
            .t(tok::LEAF_TEX_SIZE)
            .v3([1.0, 2.0, 0.0])
            .t(tok::LEAF_TEX_RATIO)
            .f(2.0)
            .t(tok::LEAF_TEXTURE_END)
            .t(tok::LEAF_TEXTURES_END)
            .t(tok::LEAVES_END);
        w.t(tok::OLD_WIND_START)
            .t(tok::OLD_WIND_STRENGTH)
            .f(0.25)
            .t(tok::OLD_WIND_ENABLED)
            .b(true)
            .t(tok::OLD_WIND_END);
        w.t(tok::FILE_END);

        // Trailing sections.
        w.mark();
        w.t(tok::CLUSTER_START).i(2).t(tok::CLUSTER_LOD_START);
        w.t(tok::CLUSTER_LEAF_START)
            .t(tok::CLUSTER_LEAF_COLOR)
            .i(0x11223344)
            .i(0)
            .t(tok::CLUSTER_LEAF_ANGLE_INDEX)
            .byte(7)
            .t(tok::CLUSTER_LEAF_POSITION)
            .v3([1.0, 2.0, 3.0])
            .t(tok::CLUSTER_LEAF_WIND_WEIGHT)
            .byte(255)
            .t(tok::CLUSTER_LEAF_END);
        w.t(tok::CLUSTER_LOD_END).t(tok::CLUSTER_END);
        w.mark();
        w.t(tok::LIGHTING_START).t(tok::LIGHTING_LEAF_MATERIAL);
        (0..13).for_each(|k| {
            w.f(k as f32);
        });
        w.t(tok::LIGHTING_END);
        w.mark();
        w.t(tok::LOD_START)
            .t(tok::LOD_LEAF_TRANSITION)
            .i(1)
            .t(tok::LOD_ENGINE_START)
            .t(tok::LOD_LEAF_COUNT)
            .i(3)
            .t(tok::LOD_ENGINE_END)
            .t(tok::LOD_END);
        w.mark();
        w.t(tok::WIND_START)
            .t(tok::WIND_LEVEL)
            .i(1)
            .t(tok::WIND_END);
        w.mark();
        w.t(tok::TEXCOORDS_START).t(tok::TEXCOORDS_LEAF).i(1);
        (0..8).for_each(|k| {
            w.f(k as f32 / 8.0);
        });
        w.t(tok::TEXCOORDS_COMPOSITE)
            .s("tree_diffuse.dds")
            .t(tok::TEXCOORDS_END);
        w.mark();
        w.t(tok::COLLISION_START)
            .t(tok::COLLISION_CAPSULE)
            .v3([0.0, 0.0, 1.0])
            .f(0.5)
            .f(3.0)
            .t(tok::COLLISION_END);
        w.mark();
        w.t(tok::COLLISION2_START)
            .t(tok::COLLISION2_OBJECT_START)
            .t(tok::COLLISION2_ROTATE_Z)
            .f(90.0)
            .t(tok::COLLISION2_OBJECT_END)
            .t(tok::COLLISION2_END);
        w.mark();
        w.t(tok::FROND_START)
            .t(tok::FROND_ENABLED)
            .b(true)
            .t(tok::FROND_PROFILE)
            .s(SPLINE)
            .t(tok::FROND_TEXTURE_COUNT)
            .i(1)
            .t(tok::FROND_TEX_START)
            .t(tok::FROND_TEX_FILENAME)
            .s("frond.dds")
            .t(tok::FROND_TEX_END)
            .t(tok::FROND_MIN_CROSS_SEGMENTS)
            .i(3)
            .t(tok::FROND_END);
        w.mark();
        w.t(tok::FLARES_START);
        for _ in 0..2 {
            w.t(tok::FLARE_PACKING_EXPONENT).f(1.0);
            w.t(tok::FLARE_COUNT).i(4);
            for k in tok::FLARE_BALANCE..=tok::FLARE_LENGTH_EXPONENT {
                w.t(k).f(0.5);
            }
        }
        w.t(tok::FLARES_END);
        w.mark();
        w.t(tok::BRANCH2_START);
        for _ in 0..2 {
            w.t(tok::BRANCH2_LEVEL_START)
                .t(tok::BRANCH2_GNARL)
                .f(0.3)
                .t(tok::BRANCH2_ROUGHNESS_PROFILE)
                .s(SPLINE)
                .t(tok::BRANCH2_LEVEL_END);
        }
        w.t(tok::BRANCH2_END);
        w.mark();
        w.t(tok::TEX_LAYERS_START);
        for _ in 0..3 * TEXTURE_LAYERS {
            w.t(tok::TEX_LAYER_START)
                .t(tok::TEX_LAYER_TWIST)
                .f(1.5)
                .t(tok::TEX_LAYER_END);
        }
        w.t(tok::TEX_LAYERS_END);
        w.mark();
        w.t(tok::MAPS_START)
            .t(tok::MAPS_LEAVES)
            .t(tok::MAPS_LEAF_COUNT)
            .i(1)
            .t(tok::MAP_SET_START)
            .t(tok::MAP_SET_NORMAL)
            .s("leaf_n.dds")
            .t(tok::MAP_SET_END)
            .t(tok::MAPS_END);
        w.mark();
        w.t(tok::SPEEDWIND_ROCK).f(0.75);
        w.mark();
        w.t(tok::LOD2_START)
            .t(tok::LOD2_FADE_EXTRUSIONS)
            .b(true)
            .t(tok::LOD2_END);
        w.mark();
        (w.0, w.1)
    }

    #[test]
    fn parses_a_synthetic_tree() {
        let t = parse(&sample()).unwrap();
        assert_eq!(t.tree.branch_texture, "bark.dds");
        assert_eq!(t.tree.seed, 532_513);
        assert_eq!((t.tree.size, t.tree.size_variance), (40.0, 5.0));
        assert_eq!((t.tree.near, t.tree.far), (15.0, 500.0));
        assert_eq!(t.tree.lod_flag, Some(false));

        assert_eq!(t.branch_levels.len(), 2);
        let b = &t.branch_levels[1];
        assert_eq!(b.segments, 5);
        assert_eq!(b.frequency, 2.0);
        assert!(!b.layers[0].s_tile_absolute);
        assert_eq!(b.length.as_ref().unwrap().points.len(), 2);
        assert_eq!(b.gravity, None);
        assert_eq!(b.cross_section_segments, 6, "a default");
        assert_eq!(b.flares.count, 4);
        assert_eq!(b.flares.length_exponent, 0.5);
        assert_eq!(b.gnarl, 0.3);
        assert!(b.roughness_profile.is_some());
        assert_eq!(b.layers[3].twist, 1.5);
        assert_eq!(t.root_layers.unwrap()[6].twist, 1.5);

        assert_eq!(t.leaves.blossom_level, 3);
        assert_eq!(t.leaves.dimming_depth, 3);
        assert_eq!(t.leaves.textures.len(), 1);
        assert_eq!(t.leaves.textures[0].filename, "leaf.dds");
        assert_eq!(t.leaves.textures[0].size, [1.0, 2.0, 0.0]);
        assert_eq!(t.leaves.textures[0].ratio, Some(2.0));
        assert_eq!(t.old_wind.strength, 0.25);
        assert_eq!(t.old_wind.enabled, Some(true));

        let lods = t.leaf_lods.unwrap();
        assert_eq!(lods.len(), 1);
        let leaf = lods[0][0];
        assert_eq!(leaf.color, 0x11223344);
        assert_eq!(leaf.angle_index, 7);
        assert_eq!(leaf.position, [1.0, 2.0, 3.0]);
        assert_eq!(leaf.wind_weight, 255);

        let m = t.lighting.unwrap().leaf_material.unwrap();
        assert_eq!(m.ambient, [3.0, 4.0, 5.0]);
        assert_eq!(m.shininess, 12.0);
        let lod = t.lod.unwrap();
        assert_eq!(lod.leaf_transition, Some(1));
        assert_eq!(lod.leaf_lod_count, Some(3));
        assert_eq!(lod.fade_extrusions, Some(true));
        assert_eq!(t.wind_level, Some(1));
        let tc = t.texcoords.unwrap();
        assert_eq!(tc.leaves.len(), 1);
        assert_eq!(tc.leaves[0][4], 0.5);
        assert_eq!(tc.composite.as_deref(), Some("tree_diffuse.dds"));
        assert_eq!(
            t.collision,
            vec![CollisionObject {
                shape: CollisionShape::Capsule {
                    radius: 0.5,
                    height: 3.0
                },
                position: [0.0, 0.0, 1.0],
                rotation: [0.0, 0.0, 90.0],
            }]
        );
        let fronds = t.fronds.unwrap();
        assert!(fronds.enabled);
        assert!(fronds.profile.is_some());
        assert_eq!(fronds.textures[0].filename, "frond.dds");
        assert_eq!(fronds.textures[0].aspect_ratio, 0.5, "a default");
        assert_eq!(fronds.min_cross_segments, 3);
        assert_eq!(t.maps.unwrap().leaves[0].normal, "leaf_n.dds");
        assert_eq!(t.speedwind_rock_scalar, Some(0.75));
    }

    #[test]
    fn parses_a_spline() {
        let s = BezierSpline::parse(SPLINE).unwrap();
        assert_eq!((s.min, s.max, s.variance), (0.0, 1.0, 5.0));
        assert_eq!(
            s.points[1],
            SplinePoint {
                x: 1.0,
                y: 1.0,
                tangent: [0.7, 0.7],
                tangent_length: 0.5
            }
        );
        // One line, scientific notation with a padded exponent.
        let s =
            BezierSpline::parse("BezierSpline -90 -90 5 { 1 0 1 -6.24607e-008 0.3 0.1 }").unwrap();
        assert_eq!(s.min, -90.0);
        assert!(s.points[0].tangent[0] < 0.0);
    }

    #[test]
    fn rejects_bad_splines() {
        for bad in [
            "",
            "Spline 0 1 0 { 0 }",
            "BezierSpline 0 1 { 0 }",
            "BezierSpline 0 1 0 { 2 0 0 1 0 1 }",
            "BezierSpline 0 1 0 { 1 0 0 1 0 1",
            "BezierSpline 0 1 0 { 1 0 0 x 0 1 }",
            "BezierSpline 0 1 0 { 99999999999 }",
            "BezierSpline 0 1 0 { -1 }",
            "BezierSpline 0 1 0 { 0 } extra",
        ] {
            assert!(BezierSpline::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn every_truncation_is_an_error() {
        let (data, ends) = sample_with_ends();
        for n in 0..=data.len() {
            assert_eq!(
                parse(&data[..n]).is_ok(),
                ends.contains(&n),
                "prefix of {n} bytes"
            );
        }
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(matches!(parse(b""), Err(SptError::Truncated { .. })));
        assert!(matches!(
            parse(b"\x00\x00\x00\x00junk"),
            Err(SptError::NotSpt(_))
        ));
        let mut w = W::default();
        w.t(tok::FILE_START).s("__IdvSpt_01_");
        assert!(matches!(parse(&w.0), Err(SptError::NotSpt(_))));

        // A huge string length.
        let mut w = W::default();
        w.t(tok::FILE_START).i(i32::MAX);
        assert!(parse(&w.0).is_err());
        let mut w = W::default();
        w.t(tok::FILE_START).i(-4);
        assert!(matches!(parse(&w.0), Err(SptError::BadCount { .. })));

        // A count beyond the data.
        let mut w = W::default();
        w.t(tok::FILE_START)
            .s(HEADER)
            .t(tok::TREE_START)
            .t(tok::BRANCHES_START)
            .i(1_000_000);
        assert!(matches!(parse(&w.0), Err(SptError::BadCount { .. })));

        // Pseudo-random bytes after a valid header never panic.
        let mut seed = 0x1234_5678u32;
        for _ in 0..200 {
            let mut w = W::default();
            w.t(tok::FILE_START).s(HEADER);
            for _ in 0..64 {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                w.byte((seed >> 16) as u8);
            }
            let _ = parse(&w.0);
        }
        // Valid tokens in a random order never panic either.
        let mut seed = 7u32;
        for _ in 0..500 {
            let mut w = W::default();
            w.t(tok::FILE_START).s(HEADER).t(tok::FILE_END);
            for _ in 0..16 {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let pick = [1001, 7000, 8000, 13000, 26000, 50000, 60000, 71000, 72000]
                    [(seed >> 16) as usize % 9];
                w.t(pick + ((seed >> 8) % 4) as i32);
            }
            let _ = parse(&w.0);
        }
    }

    #[test]
    fn unknown_tokens_are_named() {
        let mut w = W::default();
        w.t(tok::FILE_START).s(HEADER).t(tok::FILE_END);
        w.t(tok::CLUSTER_START)
            .i(1)
            .t(tok::CLUSTER_LOD_START)
            .t(tok::CLUSTER_LEAF_START)
            .t(tok::CLUSTER_LEAF_FREQUENCY_SCALAR)
            .f(1.0);
        let e = parse(&w.0).unwrap_err();
        let SptError::UnexpectedToken { token, section, .. } = &e else {
            panic!("{e}");
        };
        assert_eq!(token.0, 7009);
        assert_eq!(*section, "billboard leaf");
        assert!(e.to_string().contains("CLUSTER_LEAF_FREQUENCY_SCALAR"));

        let mut w = W::default();
        w.t(tok::FILE_START).s(HEADER).t(tok::FILE_END).t(99_999);
        assert!(matches!(
            parse(&w.0),
            Err(SptError::UnexpectedToken {
                token: Token(99_999),
                ..
            })
        ));
    }

    #[test]
    fn per_level_sections_follow_the_branch_count() {
        // Light seams for one level when the tree has none: the level record is left over.
        let mut w = W::default();
        w.t(tok::FILE_START).s(HEADER).t(tok::FILE_END);
        w.t(tok::LIGHT_SEAM_START)
            .t(tok::LIGHT_SEAM_START_BIAS)
            .f(1.0);
        assert!(matches!(parse(&w.0), Err(SptError::UnexpectedToken { .. })));
        // Rotations for a collision object that does not exist.
        let mut w = W::default();
        w.t(tok::FILE_START).s(HEADER).t(tok::FILE_END);
        w.t(tok::COLLISION2_START)
            .t(tok::COLLISION2_OBJECT_START)
            .t(tok::COLLISION2_OBJECT_END)
            .t(tok::COLLISION2_END);
        assert!(matches!(parse(&w.0), Err(SptError::Invalid(_))));
    }
}
