use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
};

use thedas_python25::{Args, ProcessOutput, Result, Runtime, Value};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static ID: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "thedas-python25-driver-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "requires local EclipseRay 1.2 LightMapper.py via THEDAS_LIGHTMAPPER_SCRIPT"]
fn installed_driver_dispatches_postprocessors() -> Result<()> {
    let source_path = std::env::var_os("THEDAS_LIGHTMAPPER_SCRIPT")
        .expect("set THEDAS_LIGHTMAPPER_SCRIPT to the installed LightMapper.py");
    let source = fs::read_to_string(source_path).unwrap();
    let job_dir = TempDir::new();
    let path = job_dir.0.to_str().unwrap();
    fs::write(job_dir.0.join("job_atlas.xml"), "<Atlas />\n").unwrap();
    let mut runtime = Runtime::new([
        Value::bytes(format!("--inputDir={path}")),
        Value::bytes("--cpus=1"),
        Value::bytes("--terse"),
    ]);
    // This deliberately dispatches no render jobs. Only the script's orchestration
    // is under test. The host records subprocess arguments instead of running them.
    runtime.register_module(
        "aergia.errors",
        Value::module([(
            "hasErrors",
            Value::function(|_, args| {
                let [] = args.take()?;
                Ok(Value::bool(false))
            }),
        )]),
    )?;
    runtime.register_module(
        "win32com.client",
        Value::module([("constants", Value::module([] as [(&str, Value); 0]))]),
    )?;
    runtime.register_module(
        "pywintypes",
        Value::module([("com_error", Value::exception_type("com_error"))]),
    )?;
    let called = Rc::new(RefCell::new(Vec::new()));
    let record = called.clone();
    runtime.set_process_runner(move |command, merge_stderr| {
        assert!(merge_stderr);
        record.borrow_mut().push(command.to_vec());
        Ok(ProcessOutput {
            status: 0,
            stdout: Vec::new(),
            stderr: None,
        })
    });
    let error = runtime.run(&source);
    if let Err(error) = error {
        panic!("installed LightMapper script returned {error}");
    }
    let called = called.borrow();
    assert_eq!(called.len(), 3);
    assert_eq!(called[0][0], "BakedMapProcessor.exe");
    assert_eq!(called[1][0], "CreateAtlas.exe");
    assert_eq!(called[2][0], "TextureProcessor.exe");
    assert_eq!(Path::new(&called[0][2]), job_dir.0.join("TGA"));
    assert_eq!(
        Path::new(called[1].last().unwrap()),
        job_dir.0.join("job_atlas.xml")
    );
    assert_eq!(
        Path::new(called[2].last().unwrap()),
        job_dir.0.join("CombinedMaps")
    );
    let fourcc = runtime.global("MakeFourCC").unwrap();
    let result = runtime.call(
        &fourcc,
        Args {
            positional: vec![Value::bytes("mshh")],
            ..Args::default()
        },
    )?;
    assert_eq!(result.as_i64()?, i64::from(u32::from_le_bytes(*b"mshh")));
    Ok(())
}
