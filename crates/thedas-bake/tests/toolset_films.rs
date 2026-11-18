use std::{fs, path::PathBuf};

use thedas_bake::toolset::{install_aergia, install_gff_reader};
use thedas_python25::{Error, Result, Runtime, Value};

#[test]
#[ignore = "requires a local Toolset job, installed script and original reference TGAs"]
fn stock_driver_renders_three_films() -> Result<()> {
    let native_job = required_path("THEDAS_BAKE_NATIVE_JOB")
        .canonicalize()
        .unwrap();
    let original_job = required_path("THEDAS_BAKE_REFERENCE_JOB")
        .canonicalize()
        .unwrap();
    assert_ne!(
        native_job, original_job,
        "use independent native and reference jobs"
    );
    let script = fs::read_to_string(required_path("THEDAS_LIGHTMAPPER_SCRIPT")).unwrap();
    let films = [
        "TGA/lmprobe.StandardLightMap.tga",
        "TGA/lmprobe.ShadowMap.tga",
        "AO/lmprobe.AmbientOcclusionMap.tga",
    ];
    for film in films {
        let path = native_job.join(film);
        if path.is_file() {
            let existing = path.canonicalize().unwrap();
            assert!(existing.starts_with(&native_job) && !existing.starts_with(&original_job));
            fs::remove_file(path).unwrap();
        }
    }

    let mut runtime = Runtime::new([
        Value::bytes(format!("--inputDir={}", native_job.display())),
        Value::bytes("--terse"),
        Value::bytes("--cpus=1"),
    ]);
    runtime.set_stdout(std::io::stdout());
    install_gff_reader(&mut runtime)?;
    install_aergia(&mut runtime)?;
    runtime.set_process_runner(|command, _| {
        if command
            .first()
            .is_some_and(|name| name == "BakedMapProcessor.exe")
        {
            return Err(Error::unsupported("film capture reached the combine stage"));
        }
        Err(Error::unsupported(format!(
            "unexpected command during film capture: {command:?}"
        )))
    });
    let result = runtime.run(&script).unwrap_err();
    assert_eq!(
        result.message, "film capture reached the combine stage",
        "{result}"
    );

    let exact = std::env::var_os("THEDAS_BAKE_EXACT_FILMS").is_some();
    for film in films {
        let original = fs::read(original_job.join(film)).unwrap();
        let candidate = fs::read(native_job.join(film)).unwrap();
        assert_eq!(&original[..18], &candidate[..18], "{film} header");
        assert_eq!(
            &original[original.len() - 26..],
            &candidate[candidate.len() - 26..],
            "{film} footer"
        );
        assert_eq!(original.len(), candidate.len(), "{film} byte count");
        let (pixels, bytes, sum, max) = pixel_difference(&original, &candidate);
        println!(
            "{film}: {pixels} differing pixels, {bytes} differing channels, sum={sum}, max={max}"
        );
        if exact {
            assert_eq!(pixels, 0, "{film} must match byte-for-byte");
        }
    }
    Ok(())
}

fn pixel_difference(original: &[u8], candidate: &[u8]) -> (usize, usize, u32, u8) {
    let tga_data = |bytes: &[u8]| 18..bytes.len() - 26;
    let mut pixels = 0;
    let mut bytes = 0;
    let mut sum = 0;
    let mut max = 0;
    for (a, b) in original[tga_data(original)]
        .chunks_exact(3)
        .zip(candidate[tga_data(candidate)].chunks_exact(3))
    {
        pixels += usize::from(a != b);
        for (&x, &y) in a.iter().zip(b) {
            let error = x.abs_diff(y);
            bytes += usize::from(error != 0);
            sum += u32::from(error);
            max = max.max(error);
        }
    }
    (pixels, bytes, sum, max)
}

fn required_path(name: &str) -> PathBuf {
    PathBuf::from(
        std::env::var_os(name)
            .unwrap_or_else(|| panic!("set {name} for the local film comparison")),
    )
}

#[test]
#[ignore = "requires the original EclipseRay camera captures"]
fn camera_charts_and_background_match_eclipseray() -> Result<()> {
    let reference = required_path("THEDAS_BAKE_CAMERA_REFERENCE");
    let script = fs::read_to_string(reference.join("LightmapCameraProbe.py")).unwrap();
    let output =
        std::env::temp_dir().join(format!("thedas-aergia-camera-{}.tga", std::process::id()));
    for (chart, mode, filename) in [
        ("touch", "direct", "touch"),
        ("edge", "direct", "edge"),
        ("overlap", "direct", "overlap"),
        ("duplicate", "direct", "duplicate"),
        ("degenerate", "direct", "degenerate"),
        ("outside", "direct", "outside"),
        ("outside", "direct", "outside-direct"),
        ("outside", "ao", "outside-ao"),
        ("outside", "shadow", "outside-shadow"),
        ("outside", "shadowgamma", "outside-shadowgamma"),
        ("outside", "custom", "outside-custom"),
    ] {
        let mut runtime = Runtime::new([
            Value::bytes(chart),
            Value::bytes(output.to_str().unwrap()),
            Value::bytes(mode),
        ]);
        install_aergia(&mut runtime)?;
        runtime.run(&script)?;
        let candidate = fs::read(&output).unwrap();
        fs::remove_file(&output).unwrap();
        let original = fs::read(reference.join(format!("{filename}.reference.tga"))).unwrap();
        assert_eq!(original.len(), candidate.len(), "{filename}");
        assert_eq!(
            original.iter().zip(candidate).position(|(&a, b)| a != b),
            None,
            "{filename}",
        );
    }
    Ok(())
}

#[test]
fn native_aergia_renders_a_triangle() -> Result<()> {
    let path = std::env::temp_dir().join(format!("thedas-aergia-{}.tga", std::process::id()));
    let direct = path.with_extension("direct.tga");
    let mut runtime = Runtime::new([]);
    runtime.set_global("direct_output", Value::unicode(direct.to_str().unwrap()));
    runtime.set_global("output", Value::unicode(path.to_str().unwrap()));
    install_aergia(&mut runtime)?;
    runtime.run(
        "
import aergia
scene = aergia.scene()
surface = aergia.materials.shinydiffuse('white', aergia.geometry.vector3d(1, 1, 1), 0, 0, 1, 0, 0, 1.3, 0)
positions = aergia.buffer(aergia.BufferUsage_Position, 9)
positions.setData([-1., -1., 0., 1., -1., 0., -1., 1., 0.])
uv = aergia.buffer(aergia.BufferUsage_Texcoord, 6)
uv.setData([0., 1., 1., 1., 0., 0.])
indices = aergia.buffer(aergia.BufferUsage_Index, 3)
indices.setData([0, 1, 2])
transform = aergia.geometry.matrix4x4([1,0,0,0, 0,1,0,0, 0,0,1,0, 0,0,0,1])
mesh = aergia.mesh('triangle', positions, 0, 3, uv, 0, 3, indices, 0, 1, transform, (0,0,0,2), scene, surface)
scene.addObject(aergia.integrators.directlight('direct', 6, 0, 0, 0))
scene.addObject(aergia.lights.ambient('ambient', aergia.geometry.vector3d(.2, .4, .6)))
film = aergia.film(output, 16, 16, aergia.FilmFilterType_Gauss, 1, 1.5, 1, 0)
scene.render(film, aergia.lightmapcam(film, mesh))
assert not aergia.errors.hasErrors()
direct_scene = aergia.scene()
mesh = aergia.mesh('triangle', positions, 0, 3, uv, 0, 3, indices, 0, 1, transform, (0,0,-3,2), direct_scene, surface)
direct_scene.addObject(aergia.integrators.directlight('direct2', 6, 0, 0, 0))
direct_scene.addObject(aergia.lights.directional('sun', aergia.geometry.vector3d(1, .5, .25), 1, aergia.geometry.vector3d(0,0,1)))
# The point light reaches the triangle, but its sphere only touches the authored mesh sphere.
direct_scene.addObject(aergia.lights.point('point', aergia.geometry.vector3d(0,0,1), aergia.geometry.vector3d(1,0,0), 2, 2))
film = aergia.film(direct_output, 16, 16, aergia.FilmFilterType_Gauss, 1, 1, 1, 0)
direct_scene.render(film, aergia.lightmapcam(film, mesh))
assert not aergia.errors.hasErrors()
",
    )?;
    let data = fs::read(&path).unwrap();
    fs::remove_file(path).unwrap();
    assert_eq!(&data[18..21], &[181, 138, 87]);
    assert_eq!(data.len(), 18 + 16 * 16 * 3 + 26);
    let data = fs::read(&direct).unwrap();
    fs::remove_file(direct).unwrap();
    assert_eq!(&data[18..21], &[23, 46, 93]);
    Ok(())
}
