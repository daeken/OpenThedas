//! Command-line host for the Toolset's Python 2.5 lightmap driver.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use thedas_bake::toolset::{install_aergia, install_gff_reader, install_postprocessors};
use thedas_python25::{Error, ModuleSource, Result, Runtime, Value};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.class == "SystemExit" => {
            ExitCode::from(error.message.parse().unwrap_or(1))
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let script = args.next().map(PathBuf::from).ok_or_else(|| {
        Error::new(
            "Usage",
            "thedas-eclipseray <script.py> [Toolset script arguments]",
        )
    })?;
    let argv = args
        .map(|arg| {
            arg.into_string()
                .map(Value::bytes)
                .map_err(|_| Error::new("UnicodeError", "script argument is not UTF-8"))
        })
        .collect::<Result<Vec<_>>>()?;
    let source = fs::read_to_string(&script)
        .map_err(|error| Error::new("IOError", format!("{}: {error}", script.display())))?;
    let root = script.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut runtime = Runtime::new(argv);
    runtime.set_stdout(std::io::stdout());
    runtime.set_module_loader(move |name| load_module(&root, name));
    install_gff_reader(&mut runtime)?;
    install_aergia(&mut runtime)?;
    install_postprocessors(&mut runtime);
    runtime.run(&source)
}

fn load_module(root: &Path, name: &str) -> Result<Option<ModuleSource>> {
    let mut base = root.to_path_buf();
    for part in name.split('.') {
        if part.is_empty()
            || !part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Ok(None);
        }
        base.push(part);
    }
    for path in [base.with_extension("py"), base.join("__init__.py")] {
        match fs::read_to_string(&path) {
            Ok(source) => {
                return Ok(Some(ModuleSource {
                    source,
                    filename: path.to_string_lossy().into_owned(),
                }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(Error::new(
                    "IOError",
                    format!("{}: {error}", path.display()),
                ));
            }
        }
    }
    Ok(None)
}
