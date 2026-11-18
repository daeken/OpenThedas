//! EclipseRay's script-facing scene, geometry, integrators and TGA output.

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    rc::{Rc, Weak},
};

use glam::{DMat4, Vec2, Vec3};
use thedas_python25::{Args, Error, NativeObject, Result, Runtime, Value};

use crate::{
    bake::{
        BakeLight, LightKind, LightmapCamera, Settings, ToolsetCamera, ToolsetPass,
        bake_region_with_camera, toolset_length,
    },
    bvh::{Bvh, Tri},
    gather::{BakeScene, Region},
};

const POSITION: i64 = 0;
const UV: i64 = 1;
const INDEX: i64 = 2;
const GAUSS: i64 = 2;
// EclipseRay installs EmissionIntegrator (eclipseRay 0x004063b0). With
// no volumes it leaves the optical-depth accumulator at one and exponentiates
// its negative (EmissionIntegrator.dll 0x1000146d–0x10001564).
const EMPTY_VOLUME_TRANSMITTANCE: f32 = 0.367_879_45;
const TGA_SIGNATURE: &[u8] = b"TRUEVISION-XFILE.\0";

/// Install the part of the original Aergia API exercised by the stock driver.
pub fn install_aergia(runtime: &mut Runtime) -> Result<()> {
    let diagnostics = Diagnostics::default();
    let scene_diagnostics = diagnostics.clone();
    let clear_diagnostics = diagnostics.clone();
    runtime.register_module(
        "aergia",
        Value::module([
            (
                "scene",
                diagnostics.bind(move |args| {
                    let [] = args.take()?;
                    Ok(Value::object(Scene::new(scene_diagnostics.clone())))
                }),
            ),
            ("buffer", diagnostics.bind(Buffer::create)),
            ("mesh", diagnostics.bind(Mesh::create)),
            ("film", diagnostics.bind(Film::create)),
            ("lightmapcam", diagnostics.bind(Camera::create)),
            (
                "ClearAll",
                diagnostics.bind(move |args| {
                    let [] = args.take()?;
                    clear_diagnostics.0.borrow_mut().clear();
                    Ok(Value::NONE)
                }),
            ),
            ("BufferUsage_Position", Value::int(POSITION)),
            ("BufferUsage_Texcoord", Value::int(UV)),
            ("BufferUsage_Index", Value::int(INDEX)),
            ("BufferUsage_Raw", Value::int(3)),
            ("FilmFilterType_Box", Value::int(0)),
            ("FilmFilterType_Mitchell", Value::int(1)),
            ("FilmFilterType_Gauss", Value::int(GAUSS)),
        ]),
    )?;
    runtime.register_module(
        "aergia.geometry",
        Value::module([
            ("vector3d", diagnostics.bind(Vector::create)),
            ("matrix4x4", diagnostics.bind(Matrix::create)),
        ]),
    )?;
    runtime.register_module(
        "aergia.materials",
        Value::module([("shinydiffuse", diagnostics.bind(Material::create))]),
    )?;
    runtime.register_module(
        "aergia.integrators",
        Value::module([
            ("directlight", diagnostics.bind(Integrator::direct)),
            ("ambientocclusion", diagnostics.bind(Integrator::ao)),
        ]),
    )?;
    runtime.register_module(
        "aergia.lights",
        Value::module([
            ("ambient", diagnostics.bind(Light::ambient)),
            ("point", diagnostics.bind(Light::point)),
            ("directional", diagnostics.bind(Light::directional)),
            ("spot", diagnostics.bind(Light::spot)),
        ]),
    )?;
    let has_errors = diagnostics.clone();
    let count = diagnostics.clone();
    runtime.register_module(
        "aergia.errors",
        Value::module([
            (
                "hasErrors",
                Value::function(move |_, args| {
                    let [] = args.take()?;
                    Ok(Value::bool(!has_errors.0.borrow().is_empty()))
                }),
            ),
            (
                "getCount",
                Value::function(move |_, args| {
                    let [] = args.take()?;
                    Ok(Value::int(count.0.borrow().len()))
                }),
            ),
            (
                "getErrors",
                Value::function(move |_, args| {
                    let [] = args.take()?;
                    Ok(Value::list(
                        diagnostics.0.borrow().iter().map(Value::bytes).collect(),
                    ))
                }),
            ),
        ]),
    )
}

#[derive(Clone, Default)]
struct Diagnostics(Rc<RefCell<Vec<String>>>);

impl Diagnostics {
    fn bind(&self, callback: impl Fn(Args) -> Result<Value> + 'static) -> Value {
        let diagnostics = self.clone();
        Value::function(move |_, args| {
            let result = callback(args);
            if let Err(error) = &result {
                diagnostics.0.borrow_mut().push(error.to_string());
            }
            result
        })
    }
}

fn native<'a, T: NativeObject + 'static>(value: &'a Value, name: &str) -> Result<&'a T> {
    value
        .downcast_object()
        .ok_or_else(|| Error::new("TypeError", format!("expected an aergia {name}")))
}

fn value_error(message: impl Into<String>) -> Error {
    Error::new("ValueError", message)
}

fn attr_error(name: &str) -> Error {
    Error::new("AttributeError", name)
}

fn number(value: &Value) -> Result<f32> {
    let number = value.as_f64()? as f32;
    if !number.is_finite() {
        return Err(value_error("expected a finite number"));
    }
    Ok(number)
}

fn count(value: &Value) -> Result<usize> {
    usize::try_from(value.as_i64()?).map_err(|_| value_error("expected a nonnegative count"))
}

fn floats<const N: usize>(value: &Value) -> Result<[f32; N]> {
    let values: [Value; N] = value
        .items()?
        .try_into()
        .map_err(|_| value_error(format!("expected a sequence of {N} values")))?;
    let mut result = [0.0; N];
    for (number_out, value) in result.iter_mut().zip(values) {
        *number_out = number(&value)?;
    }
    Ok(result)
}

#[derive(Clone)]
struct Vector(Rc<Cell<Vec3>>);

impl Vector {
    fn create(args: Args) -> Result<Value> {
        let [x, y, z] = args.take()?;
        let value = Vec3::new(number(&x)?, number(&y)?, number(&z)?);
        Ok(Value::object(Self(Rc::new(Cell::new(value)))))
    }

    fn read(value: &Value) -> Result<Vec3> {
        Ok(native::<Self>(value, "vector3d")?.0.get())
    }
}

impl NativeObject for Vector {
    fn get(&self, name: &str) -> Result<Value> {
        if name != "transform" {
            return Err(attr_error(name));
        }
        let state = self.0.clone();
        Ok(Value::function(move |_, args| {
            let [matrix] = args.take()?;
            let matrix = native::<Matrix>(&matrix, "matrix4x4")?;
            state.set(matrix.transform_point3(state.get()));
            Ok(Value::NONE)
        }))
    }
}

struct Matrix(DMat4);

impl Matrix {
    fn create(args: Args) -> Result<Value> {
        let [array] = args.take()?;
        // Aergia accepts the row-major matrix list constructed by the driver.
        let coefficients = floats::<16>(&array)?.map(f64::from);
        let matrix = DMat4::from_cols_array(&coefficients).transpose();
        Ok(Value::object(Self(matrix)))
    }

    fn transform_point3(&self, point: Vec3) -> Vec3 {
        // EclipseRay RVA 0x4680 accumulates each row in x87 registers
        // and stores the resulting coordinate once as binary32.
        self.0.transform_point3(point.as_dvec3()).as_vec3()
    }
}

impl NativeObject for Matrix {}

enum BufferData {
    Floats(Vec<f32>),
    Indices(Vec<usize>),
}

#[derive(Clone)]
struct Buffer {
    usage: i64,
    len: usize,
    contents: Rc<RefCell<Option<BufferData>>>,
}

impl Buffer {
    fn create(args: Args) -> Result<Value> {
        let [usage, len] = args.take()?;
        let usage = usage.as_i64()?;
        if !matches!(usage, POSITION | UV | INDEX) {
            return Err(Error::unsupported(format!("aergia buffer usage {usage}")));
        }
        Ok(Value::object(Self {
            usage,
            len: count(&len)?,
            contents: Rc::default(),
        }))
    }

    fn floats<const N: usize>(
        &self,
        start: usize,
        len: usize,
        usage: i64,
    ) -> Result<Vec<[f32; N]>> {
        if self.usage != usage {
            return Err(value_error("incorrect mesh buffer usage"));
        }
        if len == 0 {
            return Ok(Vec::new());
        }
        let contents = self.contents.borrow();
        let Some(BufferData::Floats(values)) = contents.as_ref() else {
            return Err(value_error("mesh buffer has no data"));
        };
        Ok(fixed_chunks(subrange(values, start, len, N)?))
    }

    fn indices(&self, start: usize, len: usize) -> Result<Vec<[usize; 3]>> {
        let contents = self.contents.borrow();
        let Some(BufferData::Indices(values)) = contents.as_ref() else {
            return Err(value_error("mesh index buffer has no data"));
        };
        Ok(fixed_chunks(subrange(values, start, len, 3)?))
    }
}

fn fixed_chunks<T: Copy, const N: usize>(values: &[T]) -> Vec<[T; N]> {
    values
        .chunks_exact(N)
        .map(|chunk| std::array::from_fn(|index| chunk[index]))
        .collect()
}

fn subrange<T>(values: &[T], start: usize, len: usize, stride: usize) -> Result<&[T]> {
    let range = start
        .checked_mul(stride)
        .zip(start.checked_add(len).and_then(|n| n.checked_mul(stride)))
        .and_then(|(start, end)| values.get(start..end));
    range.ok_or_else(|| value_error("mesh buffer range exceeds the supplied data"))
}

impl NativeObject for Buffer {
    fn get(&self, name: &str) -> Result<Value> {
        if name != "setData" {
            return Err(attr_error(name));
        }
        let this = self.clone();
        Ok(Value::function(move |_, args| {
            let [sequence] = args.take()?;
            let values = sequence.items()?;
            if values.len() != this.len {
                return Err(value_error(
                    "buffer data does not match its declared element count",
                ));
            }
            let data = if this.usage == INDEX {
                BufferData::Indices(values.iter().map(count).collect::<Result<_>>()?)
            } else {
                BufferData::Floats(values.iter().map(number).collect::<Result<_>>()?)
            };
            *this.contents.borrow_mut() = Some(data);
            Ok(Value::NONE)
        }))
    }
}

struct Material;

impl Material {
    fn create(args: Args) -> Result<Value> {
        let [
            id,
            color,
            transparency,
            translucency,
            diffuse,
            specular,
            emit,
            ior,
            sigma,
        ] = args.take()?;
        id.as_text()?;
        let color = Vector::read(&color)?;
        if color != Vec3::ONE
            || number(&transparency)? != 0.0
            || number(&translucency)? != 0.0
            || number(&diffuse)? != 1.0
            || number(&specular)? != 0.0
            || number(&emit)? != 0.0
            || number(&sigma)? > 0.0
        {
            return Err(Error::unsupported(
                "aergia shinydiffuse settings outside the white opaque material path",
            ));
        }
        number(&ior)?;
        Ok(Value::object(Self))
    }
}

impl NativeObject for Material {}

#[derive(Clone)]
struct Scene {
    state: Rc<RefCell<SceneState>>,
    diagnostics: Diagnostics,
}

struct SceneState {
    cores: usize,
    samples: u32,
    meshes: Vec<MeshGeometry>,
    lights: Vec<BakeLight>,
    integrator: Option<Integrator>,
    geometry: Option<BakeScene>,
    background: Vec3,
    active_background: Option<Vec3>,
}

impl Scene {
    fn new(diagnostics: Diagnostics) -> Self {
        Self {
            state: Rc::new(RefCell::new(SceneState {
                cores: 1,
                samples: 8,
                meshes: Vec::new(),
                lights: Vec::new(),
                integrator: None,
                geometry: None,
                background: Vec3::new(0.0, 0.0, 0.8),
                active_background: None,
            })),
            diagnostics,
        }
    }

    fn render(&self, film: &Film, camera: &Camera) -> Result<()> {
        if !Rc::ptr_eq(&film.identity, &camera.film_id)
            || !camera.mesh.owner.ptr_eq(&Rc::downgrade(&self.state))
        {
            return Err(value_error(
                "camera, film and mesh belong to different render targets",
            ));
        }
        let mut state = self.state.borrow_mut();
        let mode = state
            .integrator
            .ok_or_else(|| value_error("scene has no integrator"))?;
        // EclipseRay installs the constant background when the scene first renders.
        let background = state.background;
        let background = *state.active_background.get_or_insert(background);
        if state.geometry.is_none() {
            state.geometry = Some(build_geometry(&state.meshes)?);
        }
        let Some(geometry) = &state.geometry else {
            unreachable!("initialized above");
        };
        let region = geometry
            .regions
            .get(camera.mesh.id)
            .ok_or_else(|| value_error("mesh no longer exists in scene"))?;
        let mut settings = Settings {
            samples: state.samples,
            threads: state.cores,
            gamma: film.gamma,
            direct_transmittance: EMPTY_VOLUME_TRANSMITTANCE,
            ..Settings::default()
        };
        if let Integrator::Ao { samples, length } = mode {
            settings.ao_rays = samples;
            settings.ao_length = length;
        }
        let pass = match mode {
            Integrator::Ao { .. } => ToolsetPass::Occlusion,
            Integrator::Direct { monochrome: false } => ToolsetPass::Light,
            Integrator::Direct { monochrome: true } => ToolsetPass::Shadow,
        };
        let mesh = &state.meshes[camera.mesh.id];
        let mut lights = Vec::new();
        if !matches!(pass, ToolsetPass::Occlusion) {
            lights.extend(
                state
                    .lights
                    .iter()
                    .filter(|light| mesh.affected_by(light.kind))
                    .cloned(),
            );
        }
        let background = if matches!(pass, ToolsetPass::Shadow) {
            Vec3::splat(f32::from(u8::from(background.max_element() > 0.0)))
        } else {
            background
        };
        let primary_count =
            u64::from(film.width) * u64::from(film.height) * u64::from(settings.samples);
        if primary_count > 134_217_728 {
            return Err(Error::unsupported(
                "film exceeds the primary-sample working set",
            ));
        }
        let width = film.width.into();
        let height = film.height.into();
        let uv_camera = LightmapCamera::new(region, &mesh.texcoords, width, height);
        let films = bake_region_with_camera(
            geometry,
            region,
            width,
            height,
            &lights,
            &settings,
            Some(ToolsetCamera {
                frame: &uv_camera,
                background,
                pass,
            }),
        );
        write_tga(film, films.light.into_iter())
    }
}

impl NativeObject for Scene {
    fn get(&self, name: &str) -> Result<Value> {
        let this = self.clone();
        Ok(match name {
            "setNumCores" => self.diagnostics.bind(move |args| {
                let [cores] = args.take()?;
                let cores = count(&cores)?;
                if cores == 0 {
                    return Err(value_error("expected at least one render core"));
                }
                this.state.borrow_mut().cores = cores;
                Ok(Value::NONE)
            }),
            "setAntialias" => self.diagnostics.bind(move |args| {
                let [samples, passes, increment, threshold] = args.take()?;
                if passes.as_i64()? != 1 {
                    return Err(Error::unsupported("more than one Aergia antialiasing pass"));
                }
                count(&increment)?;
                number(&threshold)?;
                this.state.borrow_mut().samples = samples_in_range(&samples)?;
                Ok(Value::NONE)
            }),
            "setBackgroundColor" => self.diagnostics.bind(move |args| {
                let [color] = args.take()?;
                this.state.borrow_mut().background = Vector::read(&color)?;
                Ok(Value::NONE)
            }),
            "addObject" => self.diagnostics.bind(move |args| {
                let [object] = args.take()?;
                let mut state = this.state.borrow_mut();
                if let Some(light) = object.downcast_object::<Light>() {
                    state.lights.push(light.0.clone());
                } else if let Some(mode) = object.downcast_object::<Integrator>() {
                    state.integrator = Some(*mode);
                } else {
                    return Err(Error::unsupported("aergia.scene.addObject argument"));
                }
                Ok(Value::NONE)
            }),
            "render" => self.diagnostics.bind(move |args| {
                let [film, camera] = args.take()?;
                this.render(native(&film, "film")?, native(&camera, "lightmapcam")?)?;
                Ok(Value::NONE)
            }),
            _ => return Err(attr_error(name)),
        })
    }
}

#[derive(Clone)]
struct Mesh {
    owner: Weak<RefCell<SceneState>>,
    id: usize,
}

struct MeshGeometry {
    positions: Vec<Vec3>,
    texcoords: Vec<Vec2>,
    triangles: Vec<[usize; 3]>,
    sphere_center: Vec3,
    sphere_radius: f32,
}

impl MeshGeometry {
    fn affected_by(&self, light: LightKind) -> bool {
        let (position, radius) = match light {
            LightKind::Point { position, radius }
            | LightKind::Spot {
                position, radius, ..
            } => (position, radius),
            LightKind::Ambient | LightKind::Directional { .. } => return true,
        };
        // EclipseRay RVA 0x65a6 rounds the distance through vector3d_t::length
        // before comparing it to the sum of the two radii in x87 registers.
        let distance = toolset_length(position - self.sphere_center);
        f64::from(distance) < f64::from(radius) + f64::from(self.sphere_radius)
    }
}

impl Mesh {
    fn create(args: Args) -> Result<Value> {
        let [
            name,
            positions,
            vertex_start,
            vertex_count,
            texcoords,
            uv_start,
            uv_count,
            indices,
            face_start,
            face_count,
            matrix,
            sphere,
            scene,
            material,
        ] = args.take()?;
        name.as_text()?;
        native::<Material>(&material, "material")?;
        let transform = native::<Matrix>(&matrix, "matrix4x4")?;
        let owner = native::<Scene>(&scene, "scene")?;
        let positions = native::<Buffer>(&positions, "position buffer")?.floats::<3>(
            count(&vertex_start)?,
            count(&vertex_count)?,
            POSITION,
        )?;
        let texcoords = native::<Buffer>(&texcoords, "UV buffer")?.floats::<2>(
            count(&uv_start)?,
            count(&uv_count)?,
            UV,
        )?;
        let triangles = native::<Buffer>(&indices, "index buffer")?
            .indices(count(&face_start)?, count(&face_count)?)?;
        let [center_x, center_y, center_z, sphere_radius] = floats::<4>(&sphere)?;
        if !texcoords.is_empty() && texcoords.len() != positions.len() {
            return Err(value_error(
                "mesh UV count must equal position count or be zero",
            ));
        }
        if triangles
            .iter()
            .flatten()
            .any(|index| *index >= positions.len())
        {
            return Err(value_error("mesh triangle index outside vertex buffer"));
        }
        let mesh = MeshGeometry {
            positions: positions
                .into_iter()
                .map(|vertex| transform.transform_point3(Vec3::from(vertex)))
                .collect(),
            // The driver supplies the camera's bottom-origin UV coordinates.
            texcoords: texcoords.into_iter().map(Vec2::from_array).collect(),
            triangles,
            // EclipseRay RVA 0x93d0 transforms the authored center and copies the radius.
            sphere_center: transform.transform_point3(Vec3::new(center_x, center_y, center_z)),
            sphere_radius,
        };
        let mut state = owner.state.borrow_mut();
        let id = state.meshes.len();
        state.meshes.push(mesh);
        state.geometry = None;
        Ok(Value::object(Self {
            owner: Rc::downgrade(&owner.state),
            id,
        }))
    }
}

impl NativeObject for Mesh {}

fn build_geometry(meshes: &[MeshGeometry]) -> Result<BakeScene> {
    let mut triangles = Vec::new();
    let mut regions = Vec::with_capacity(meshes.len());
    for (draw, mesh) in meshes.iter().enumerate() {
        let draw_id = u32::try_from(draw).map_err(|_| value_error("too many meshes in scene"))?;
        let mut tris = Vec::new();
        for &triangle in &mesh.triangles {
            let pos = triangle.map(|index| mesh.positions[index]);
            if !mesh.texcoords.is_empty() {
                let uv = triangle.map(|index| mesh.texcoords[index]);
                tris.push((pos, uv));
            }
            triangles.push(Tri {
                a: pos[0],
                b: pos[1],
                c: pos[2],
                draw: draw_id,
                casts: true,
            });
        }
        regions.push(Region {
            draw,
            atlas: String::new(),
            offset: Vec2::ZERO,
            scale: Vec2::ONE,
            tris,
        });
    }
    Ok(BakeScene {
        bvh: Bvh::new_toolset(triangles),
        regions,
        atlases: BTreeMap::new(),
        non_casters: 0,
    })
}

struct Film {
    path: PathBuf,
    width: u16,
    height: u16,
    gamma: f32,
    identity: Rc<()>,
}

impl Film {
    fn create(args: Args) -> Result<Value> {
        let [
            path,
            width,
            height,
            filter,
            filter_width,
            gamma,
            depth,
            clamp,
        ] = args.take()?;
        if filter.as_i64()? != GAUSS || number(&filter_width)? != 1.0 {
            return Err(Error::unsupported(
                "aergia film filter outside the stock Gaussian settings",
            ));
        }
        depth.as_i64()?;
        clamp.as_i64()?;
        let dimension = |value: Value| {
            u16::try_from(value.as_i64()?)
                .ok()
                .filter(|n| (1..=4096).contains(n))
                .ok_or_else(|| value_error("film dimension outside 1..=4096"))
        };
        let gamma = number(&gamma)?;
        if gamma <= 0.0 {
            return Err(value_error("film gamma must be positive"));
        }
        Ok(Value::object(Self {
            path: Path::new(path.as_text()?).to_path_buf(),
            width: dimension(width)?,
            height: dimension(height)?,
            gamma,
            identity: Rc::default(),
        }))
    }
}

impl NativeObject for Film {}

#[derive(Clone)]
struct Camera {
    film_id: Rc<()>,
    mesh: Mesh,
}

impl Camera {
    fn create(args: Args) -> Result<Value> {
        let [film, mesh] = args.take()?;
        Ok(Value::object(Self {
            film_id: native::<Film>(&film, "film")?.identity.clone(),
            mesh: native::<Mesh>(&mesh, "mesh")?.clone(),
        }))
    }
}

impl NativeObject for Camera {}

#[derive(Clone, Copy)]
enum Integrator {
    Direct { monochrome: bool },
    Ao { samples: u32, length: f32 },
}

impl Integrator {
    fn direct(args: Args) -> Result<Value> {
        let [
            id,
            ray_depth,
            photon_count,
            transparent_shadow_depth,
            monochrome,
        ] = args.take()?;
        id.as_text()?;
        ray_depth.as_i64()?;
        if photon_count.as_i64()? != 0 || transparent_shadow_depth.as_i64()? > 0 {
            return Err(Error::unsupported("aergia photons or transparent shadows"));
        }
        Ok(Value::object(Self::Direct {
            monochrome: monochrome.as_i64()? != 0,
        }))
    }

    fn ao(args: Args) -> Result<Value> {
        let [id, samples, length, color] = args.take()?;
        id.as_text()?;
        if Vector::read(&color)? != Vec3::ONE {
            return Err(Error::unsupported(
                "aergia non-white ambient occlusion color",
            ));
        }
        let length = number(&length)?;
        if length <= 0.0 {
            return Err(value_error("AO length must be positive"));
        }
        Ok(Value::object(Self::Ao {
            samples: samples_in_range(&samples)?,
            length,
        }))
    }
}

impl NativeObject for Integrator {}

fn samples_in_range(value: &Value) -> Result<u32> {
    u32::try_from(value.as_i64()?)
        .ok()
        .filter(|samples| (1..=1024).contains(samples))
        .ok_or_else(|| value_error("sample count outside 1..=1024"))
}

struct Light(BakeLight);

impl Light {
    fn create(kind: LightKind, color: Vec3, brightness: f32) -> Value {
        Value::object(Self(BakeLight {
            kind,
            color,
            brightness,
            in_lightmap: true,
            cast_shadows: (color * brightness).max_element() > 0.0,
            shadow_reach: 1.0,
        }))
    }

    fn ambient(args: Args) -> Result<Value> {
        let [id, color] = args.take()?;
        id.as_text()?;
        Ok(Self::create(LightKind::Ambient, Vector::read(&color)?, 1.0))
    }

    fn point(args: Args) -> Result<Value> {
        let [id, position, color, brightness, radius] = args.take()?;
        id.as_text()?;
        let radius = number(&radius)?;
        if radius <= 0.0 {
            return Err(value_error("point light radius must be positive"));
        }
        let kind = LightKind::Point {
            position: Vector::read(&position)?,
            radius,
        };
        Ok(Self::create(
            kind,
            Vector::read(&color)?,
            number(&brightness)?,
        ))
    }

    fn directional(args: Args) -> Result<Value> {
        let (id, color, brightness, toward) =
            match (args.positional.as_slice(), args.named.is_empty()) {
                ([id, color, brightness, toward], true) => (id, color, brightness, toward),
                ([id, color, brightness, toward, position], true) => {
                    Vector::read(position)?;
                    (id, color, brightness, toward)
                }
                _ => {
                    return Err(Error::new(
                        "TypeError",
                        "directional expects four or five positional arguments",
                    ));
                }
            };
        id.as_text()?;
        let kind = LightKind::Directional {
            toward: Vector::read(toward)?,
        };
        Ok(Self::create(
            kind,
            Vector::read(color)?,
            number(brightness)?,
        ))
    }

    fn spot(args: Args) -> Result<Value> {
        let [id, color, outer_angle, from, to, brightness, blend, radius] = args.take()?;
        id.as_text()?;
        let position = Vector::read(&from)?;
        let axis = position - Vector::read(&to)?;
        let length = toolset_length(axis);
        let backward = if length != 0.0 {
            axis * (1.0 / length)
        } else {
            axis
        };
        let radius = number(&radius)?;
        let outer_angle = number(&outer_angle)?;
        let blend = number(&blend)?;
        if radius <= 0.0 || !(0.0..=180.0).contains(&outer_angle) || !(0.0..=1.0).contains(&blend) {
            return Err(Error::unsupported("aergia spotlight parameter range"));
        }
        // spotlight.dll RVA 0x168e keeps the converted cone angles wide through cos.
        let angle = f64::from(outer_angle) * std::f64::consts::PI / 180.0;
        let kind = LightKind::Spot {
            position,
            backward,
            radius,
            inner_cos: ((1.0 - f64::from(blend)) * angle).cos() as f32,
            outer_cos: angle.cos() as f32,
        };
        Ok(Self::create(
            kind,
            Vector::read(&color)?,
            number(&brightness)?,
        ))
    }
}

impl NativeObject for Light {}

fn write_tga(film: &Film, rgb: impl Iterator<Item = [u8; 3]>) -> Result<()> {
    let save = || -> std::io::Result<()> {
        let mut output = BufWriter::new(File::create(&film.path)?);
        let mut header = [0; 18];
        header[2] = 2;
        header[12..14].copy_from_slice(&film.width.to_le_bytes());
        header[14..16].copy_from_slice(&film.height.to_le_bytes());
        header[16] = 24;
        header[17] = 0x20;
        output.write_all(&header)?;
        for [r, g, b] in rgb {
            output.write_all(&[b, g, r])?;
        }
        output.write_all(&[0; 8])?;
        output.write_all(TGA_SIGNATURE)?;
        output.flush()
    };
    save().map_err(|error| Error::new("IOError", format!("{}: {error}", film.path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_rounds_after_accumulating_a_coordinate() {
        let mut coefficients = DMat4::IDENTITY.to_cols_array();
        coefficients[4] = 1.0;
        coefficients[8] = -1.0;
        let matrix = Matrix(DMat4::from_cols_array(&coefficients));
        let point = Vec3::new(16_777_216.0, 1.0, 16_777_216.0);
        assert_eq!(matrix.transform_point3(point), Vec3::new(1.0, 1.0, point.z));
    }
}
