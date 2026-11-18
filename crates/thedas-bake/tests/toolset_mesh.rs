use std::{fs, path::PathBuf};

use thedas_bake::toolset::install_gff_reader;
use thedas_python25::{Result, Runtime, Value};

#[test]
#[ignore = "requires the installed LightMapper.py, original MMH/MSH, and EclipseRay mesh dumps"]
fn stock_driver_mesh_values_match_eclipseray() -> Result<()> {
    let source = fs::read_to_string(path_var("THEDAS_LIGHTMAPPER_SCRIPT")).unwrap();
    let model = path_var("THEDAS_GFF_REFERENCE_MODEL");
    let reference = path_var("THEDAS_GFF_REFERENCE_CASE");
    let mut runtime = Runtime::new([]);
    runtime.set_stdout(std::io::stdout());
    runtime.set_global("__name__", Value::bytes("mesh_probe"));
    runtime.set_global("model_path", Value::unicode(model.to_str().unwrap()));
    install_gff_reader(&mut runtime)?;
    // The driver's asset readers run independently of the renderer.
    runtime.register_module("aergia", Value::module([] as [(&str, Value); 0]))?;
    runtime.run(&source)?;
    runtime.run(
        "
reader = win32com.client.gencache.EnsureDispatch('Bioware.Eclipse.EngGFFReader')
reader.LoadGFFFromFile(model_path)
assert os.path.isfile(os.path.join(os.path.dirname(model_path), reader.TopLevelStruct.GetValueByLabel(6005)))
asset = GFFMMHReader(model_path)
assert asset.meshreader is not None
assert len(asset.parts) == 1
part = asset.parts[0]
chunk = asset.meshreader.chunks[part.meshgroup]
positions = chunk.positions
indices = chunk.indices
texcoords = chunk.texcoords
normals = chunk.normals
sphere = chunk.bsphere
transform = part.transform
assert asset.meshreader.reader.MagicNumber == 541476423
assert asset.meshreader.reader.GFFVersion == 808334422
try:
    asset.meshreader.reader.TopLevelStruct.GetValueByLabel(999999)
except pywintypes.com_error:
    pass
else:
    raise AssertionError('absent field succeeded')
",
    )?;
    for name in [
        "positions",
        "indices",
        "texcoords",
        "normals",
        "sphere",
        "transform",
    ] {
        let values = runtime.global(name).unwrap().items()?;
        let mut bytes = Vec::new();
        for value in values {
            if name == "indices" {
                bytes.extend(i32::try_from(value.as_i64()?).unwrap().to_le_bytes());
            } else {
                bytes.extend(value.as_f64()?.to_le_bytes());
            }
        }
        let expected = fs::read(reference.join(format!("{name}.bin"))).unwrap();
        assert_eq!(bytes.len(), expected.len(), "{name} byte count");
        assert_eq!(
            bytes.iter().zip(&expected).position(|(a, b)| a != b),
            None,
            "{name} first differing byte",
        );
    }
    Ok(())
}

fn path_var(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("set {name} for the EclipseRay mesh comparison"))
}
