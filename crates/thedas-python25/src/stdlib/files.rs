use std::{
    cell::{Cell, RefCell},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    rc::Rc,
};

use super::{module, no_keywords, optional};
use crate::{Args, Error, NativeObject, Result, Runtime, Value, value::Kind};

pub(super) fn install(runtime: &mut Runtime) {
    let dict = |items: Vec<(Value, Value)>| Value(Kind::Dict(Rc::new(RefCell::new(items))));
    let mut env = std::env::vars_os()
        .filter_map(|(key, value)| {
            Some((Value::bytes(key.to_str()?), Value::bytes(value.to_str()?)))
        })
        .collect::<Vec<_>>();
    if !env
        .iter()
        .any(|(key, _)| key.as_bytes() == Ok(b"NUMBER_OF_PROCESSORS"))
    {
        let count = std::thread::available_parallelism().map_or(1, |n| n.get());
        env.push((
            Value::bytes("NUMBER_OF_PROCESSORS"),
            Value::bytes(count.to_string()),
        ));
    }
    module(
        runtime,
        "os",
        [
            ("environ", dict(env)),
            ("sep", Value::bytes(std::path::MAIN_SEPARATOR.to_string())),
            (
                "getcwd",
                Value::function(|_, args| {
                    let [] = args.take()?;
                    Ok(Value::bytes(
                        std::env::current_dir()
                            .map_err(io_error)?
                            .to_string_lossy()
                            .as_bytes(),
                    ))
                }),
            ),
            (
                "mkdir",
                Value::function(|_, args| {
                    let [path] = args.take()?;
                    fs::create_dir(path.as_text()?).map_err(io_error)?;
                    Ok(Value::NONE)
                }),
            ),
            (
                "makedirs",
                Value::function(|_, args| {
                    let [path] = args.take()?;
                    fs::create_dir_all(path.as_text()?).map_err(io_error)?;
                    Ok(Value::NONE)
                }),
            ),
            (
                "listdir",
                Value::function(|runtime, args| {
                    let [path] = args.take()?;
                    let list = fs::read_dir(runtime.resolve_read_path(Path::new(path.as_text()?)))
                        .map_err(io_error)?
                        .map(|entry| {
                            let entry = entry.map_err(io_error)?;
                            Ok(path_string(&path, &entry.file_name().to_string_lossy()))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    Ok(Value::list(list))
                }),
            ),
            (
                "remove",
                Value::function(|_, args| {
                    let [path] = args.take()?;
                    fs::remove_file(path.as_text()?).map_err(io_error)?;
                    Ok(Value::NONE)
                }),
            ),
        ],
    );
    let path_test = |check: fn(&Path) -> bool| {
        Value::function(move |runtime, args| {
            let [path] = args.take()?;
            Ok(Value::bool(check(
                &runtime.resolve_read_path(Path::new(path.as_text()?)),
            )))
        })
    };
    module(
        runtime,
        "os.path",
        [
            ("exists", path_test(Path::exists)),
            ("isdir", path_test(Path::is_dir)),
            ("isfile", path_test(Path::is_file)),
            (
                "join",
                Value::function(|_, args| {
                    let args = no_keywords(args)?;
                    if args.is_empty() {
                        return Err(Error::type_error("join requires a path"));
                    }
                    let mut path = PathBuf::new();
                    for part in &args {
                        path.push(part.as_text()?);
                    }
                    let path = path.to_string_lossy();
                    Ok(
                        if args.iter().any(|part| matches!(part.0, Kind::Unicode(_))) {
                            Value::unicode(path)
                        } else {
                            Value::bytes(path.as_bytes())
                        },
                    )
                }),
            ),
            (
                "basename",
                Value::function(|_, args| {
                    let [path] = args.take()?;
                    let text = path.as_text()?;
                    let basename = text.rsplit(std::path::is_separator).next().unwrap_or("");
                    Ok(path_string(&path, basename))
                }),
            ),
            (
                "dirname",
                Value::function(|_, args| {
                    let [path] = args.take()?;
                    let text = path.as_text()?;
                    let prefix = text
                        .rsplit_once(std::path::is_separator)
                        .map_or("", |(prefix, _)| prefix);
                    let dirname = prefix.trim_end_matches(std::path::is_separator);
                    Ok(path_string(
                        &path,
                        if dirname.is_empty() && text.starts_with(std::path::is_separator) {
                            &text[..1]
                        } else {
                            dirname
                        },
                    ))
                }),
            ),
            (
                "normpath",
                Value::function(|_, args| {
                    let [path] = args.take()?;
                    Ok(path_string(
                        &path,
                        &normalize(Path::new(path.as_text()?)).to_string_lossy(),
                    ))
                }),
            ),
            (
                "abspath",
                Value::function(|_, args| {
                    let [path] = args.take()?;
                    let full = std::env::current_dir()
                        .map_err(io_error)?
                        .join(path.as_text()?);
                    Ok(path_string(&path, &normalize(&full).to_string_lossy()))
                }),
            ),
            (
                "splitext",
                Value::function(|_, args| {
                    let [path] = args.take()?;
                    let text = path.as_text()?;
                    let file = text.rsplit(std::path::is_separator).next().unwrap_or("");
                    let (base, extension) = match file.rfind('.') {
                        Some(dot) if file[..dot].chars().any(|ch| ch != '.') => {
                            text.split_at(text.len() - file.len() + dot)
                        }
                        _ => (text, ""),
                    };
                    Ok(Value::tuple(vec![
                        path_string(&path, base),
                        path_string(&path, extension),
                    ]))
                }),
            ),
        ],
    );
    module(
        runtime,
        "glob",
        [(
            "glob",
            Value::function(|_, args| {
                let [pattern] = args.take()?;
                let items = glob::glob_with(
                    pattern.as_text()?,
                    glob::MatchOptions {
                        case_sensitive: !cfg!(windows),
                        require_literal_separator: true,
                        require_literal_leading_dot: true,
                    },
                )
                .map_err(|e| Error::new("ValueError", e.to_string()))?;
                let items = items
                    .filter_map(|path| path.ok())
                    .map(|path| path_string(&pattern, &path.to_string_lossy()))
                    .collect();
                Ok(Value::list(items))
            }),
        )],
    );
    module(
        runtime,
        "shutil",
        [
            ("copy", Value::function(|_, args| transfer(args, false))),
            ("move", Value::function(|_, args| transfer(args, true))),
        ],
    );
    module(
        runtime,
        "traceback",
        [(
            "print_exc",
            Value::function(|runtime, args| {
                let args = no_keywords(args)?;
                let text = runtime
                    .pending_exception()
                    .map_or_else(|| "None\n".to_owned(), |error| format!("{error}\n"));
                match args.as_slice() {
                    [] | [_] => runtime.write_stdout(text.as_bytes())?,
                    [_, file] => {
                        let write = runtime.attribute(file, "write")?;
                        runtime.call(
                            &write,
                            Args {
                                positional: vec![Value::bytes(text)],
                                ..Args::default()
                            },
                        )?;
                    }
                    _ => return Err(Error::type_error("print_exc takes up to two arguments")),
                }
                Ok(Value::NONE)
            }),
        )],
    );
    module(
        runtime,
        "subprocess",
        [
            ("PIPE", Value::int(-1)),
            ("STDOUT", Value::int(-2)),
            ("STARTF_USESHOWWINDOW", Value::int(1)),
            (
                "STARTUPINFO",
                Value::function(|_, args| {
                    let [] = args.take()?;
                    Ok(Value::object(StartupInfo(Cell::new(0))))
                }),
            ),
            (
                "Popen",
                Value::function(|runtime, mut args| {
                    let stdout = args
                        .named
                        .remove("stdout")
                        .map(|val| val.as_i64())
                        .transpose()?;
                    let stderr = args
                        .named
                        .remove("stderr")
                        .map(|val| val.as_i64())
                        .transpose()?;
                    args.named.remove("startupinfo");
                    if stdout.is_some_and(|v| v != -1) || stderr.is_some_and(|v| v != -2) {
                        return Err(Error::unsupported("Popen output redirection"));
                    }
                    let [command] = args.take()?;
                    let command = command
                        .items()?
                        .iter()
                        .map(|arg| arg.as_text().map(str::to_owned))
                        .collect::<Result<Vec<_>>>()?;
                    let output = runtime.run_process(&command, stderr == Some(-2))?;
                    Ok(Value::object(Process {
                        code: output.status,
                        stdout: output.stdout,
                        stderr: output.stderr,
                        piped: stdout == Some(-1),
                        status_observed: Rc::new(Cell::new(false)),
                    }))
                }),
            ),
        ],
    );
}

fn path_string(template: &Value, text: &str) -> Value {
    if matches!(template.0, Kind::Unicode(_)) {
        Value::unicode(text)
    } else {
        Value::bytes(text)
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                Some(Component::RootDir) => {}
                _ => normalized.push(part),
            },
            _ => normalized.push(part),
        }
    }
    if normalized.as_os_str().is_empty() {
        normalized.push(".");
    }
    normalized
}

fn transfer(args: Args, rename: bool) -> Result<Value> {
    let [source, target] = args.take()?;
    let source = Path::new(source.as_text()?);
    let target = Path::new(target.as_text()?);
    let target = if target.is_dir() {
        target.join(
            source
                .file_name()
                .ok_or_else(|| Error::new("IOError", "missing source name"))?,
        )
    } else {
        target.to_owned()
    };
    if rename {
        if target.exists() {
            return Err(Error::new("IOError", "destination already exists"));
        }
        match fs::rename(source, &target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
                fs::copy(source, &target).map_err(io_error)?;
                fs::remove_file(source).map_err(io_error)?;
            }
            Err(error) => return Err(io_error(error)),
        }
    } else {
        fs::copy(source, &target).map_err(io_error)?;
    }
    Ok(Value::NONE)
}

fn io_error(error: std::io::Error) -> Error {
    Error::new("IOError", error.to_string())
}

pub(super) fn open(runtime: &mut Runtime, args: Args) -> Result<Value> {
    let args = no_keywords(args)?;
    let (path, mode) = match args.as_slice() {
        [path] => (path.as_text()?, "r"),
        [path, mode] => (path.as_text()?, mode.as_text()?),
        _ => return Err(Error::type_error("open expects a path and optional mode")),
    };
    let mut options = OpenOptions::new();
    let mode = mode.replace('b', "");
    match mode.as_str() {
        "r" => {
            options.read(true);
        }
        "w" => {
            options.write(true).create(true).truncate(true);
        }
        "a" => {
            options.append(true).create(true);
        }
        "r+" => {
            options.read(true).write(true);
        }
        "w+" => {
            options.read(true).write(true).create(true).truncate(true);
        }
        "a+" => {
            options.read(true).append(true).create(true);
        }
        _ => return Err(Error::new("ValueError", "invalid file mode")),
    }
    let path = Path::new(path);
    let file = if mode == "r" {
        options.open(runtime.resolve_read_path(path))
    } else {
        options.open(path)
    }
    .map_err(io_error)?;
    Ok(Value::object(PythonFile(Rc::new(RefCell::new(Some(file))))))
}

struct PythonFile(Rc<RefCell<Option<File>>>);

impl NativeObject for PythonFile {
    fn get(&self, name: &str) -> Result<Value> {
        let file = self.0.clone();
        let operation = match name {
            "read" => 0,
            "write" => 1,
            "flush" => 2,
            "close" => 3,
            _ => return Err(Error::new("AttributeError", name)),
        };
        Ok(Value::function(move |_, args| {
            let mut file = file.borrow_mut();
            if operation == 3 {
                let [] = args.take()?;
                *file = None;
                return Ok(Value::NONE);
            }
            let file = file
                .as_mut()
                .ok_or_else(|| Error::new("ValueError", "I/O on closed file"))?;
            match operation {
                0 => {
                    let count = optional(args)?
                        .map(|value| value.as_i64())
                        .transpose()?
                        .unwrap_or(-1);
                    let mut bytes = Vec::new();
                    if count < 0 {
                        file.read_to_end(&mut bytes).map_err(io_error)?;
                    } else {
                        file.take(count as u64)
                            .read_to_end(&mut bytes)
                            .map_err(io_error)?;
                    }
                    Ok(Value::bytes(bytes))
                }
                1 => {
                    let [value] = args.take()?;
                    let bytes = match &value.0 {
                        Kind::Bytes(bytes) => bytes.as_ref(),
                        Kind::Unicode(text) if text.is_ascii() => text.as_bytes(),
                        Kind::Unicode(_) => {
                            return Err(Error::new("UnicodeEncodeError", "non-ASCII Unicode"));
                        }
                        _ => return Err(Error::type_error("expected a string")),
                    };
                    file.write_all(bytes).map_err(io_error)?;
                    Ok(Value::NONE)
                }
                _ => {
                    let [] = args.take()?;
                    file.flush().map_err(io_error)?;
                    Ok(Value::NONE)
                }
            }
        }))
    }
}

struct StartupInfo(Cell<i64>);

impl NativeObject for StartupInfo {
    fn get(&self, name: &str) -> Result<Value> {
        if name == "dwFlags" {
            Ok(Value::int(self.0.get()))
        } else {
            Err(Error::new("AttributeError", name))
        }
    }
    fn set(&self, name: &str, value: Value) -> Result<()> {
        if name != "dwFlags" {
            return Err(Error::new("AttributeError", name));
        }
        self.0.set(value.as_i64()?);
        Ok(())
    }
}

struct Process {
    code: i32,
    stdout: Vec<u8>,
    stderr: Option<Vec<u8>>,
    piped: bool,
    status_observed: Rc<Cell<bool>>,
}

impl NativeObject for Process {
    fn get(&self, name: &str) -> Result<Value> {
        match name {
            "returncode" => Ok(if self.status_observed.get() {
                Value::int(self.code)
            } else {
                Value::NONE
            }),
            "wait" | "poll" => {
                let code = self.code;
                let status_observed = self.status_observed.clone();
                // Command handlers have already finished when Popen returns.
                Ok(Value::function(move |_, args| {
                    let [] = args.take()?;
                    status_observed.set(true);
                    Ok(Value::int(code))
                }))
            }
            "communicate" => {
                // The host runs the command synchronously. Expose its completed result
                // when the script asks for it, matching the driver's Popen/communicate use.
                let stdout = if self.piped {
                    Value::bytes(&self.stdout)
                } else {
                    Value::NONE
                };
                let stderr = self.stderr.as_ref().map_or(Value::NONE, Value::bytes);
                let status_observed = self.status_observed.clone();
                Ok(Value::function(move |_, args| {
                    let [] = args.take()?;
                    status_observed.set(true);
                    Ok(Value::tuple(vec![stdout.clone(), stderr.clone()]))
                }))
            }
            _ => Err(Error::new("AttributeError", name)),
        }
    }
}
