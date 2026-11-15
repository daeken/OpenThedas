//! Config copies are read and sanitized by the report worker, never copied verbatim.
use anyhow::{Context, Result};
use serde_json::Value;
use std::io::Read as _;
use std::path::Path;

pub(super) fn sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "key",
        "token",
        "secret",
        "password",
        "credential",
        "authorization",
        "webhook",
    ]
    .iter()
    .any(|s| key.contains(s))
}

fn strip_secrets(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|k, _| !sensitive_key(k));
            for v in object.values_mut() {
                strip_secrets(v);
            }
        }
        Value::Array(array) => {
            for v in array {
                strip_secrets(v);
            }
        }
        _ => {}
    }
}

fn sanitize_ini(text: &str) -> String {
    let mut out = String::new();
    let mut secret_section = false;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with([';', '#']) {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            secret_section = sensitive_key(line);
            if !secret_section {
                out.push_str(line);
                out.push('\n');
            }
        } else if !secret_section
            && let Some((key, _)) = line.split_once('=')
            && !sensitive_key(key)
        {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

pub fn copy_configs(
    dir: &Path,
    data: &Path,
    settings: Option<&Path>,
    home: Option<&str>,
) -> Result<()> {
    for (name, path) in [
        (
            "options.ini",
            settings
                .map(Path::to_path_buf)
                .unwrap_or_else(|| data.join("options.ini")),
        ),
        ("launcher.json", data.join("launcher.json")),
    ] {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                log::warn!("report: reading {name}: {e}");
                continue;
            }
        };
        const LIMIT: u64 = 1024 * 1024;
        let mut text = String::new();
        if file.take(LIMIT + 1).read_to_string(&mut text).is_err() || text.len() as u64 > LIMIT {
            log::warn!("report: {name} unreadable or over 1 MiB; omitted");
            continue;
        }
        let bytes = if name.ends_with(".json") {
            let Ok(mut value) = serde_json::from_str::<Value>(&text) else {
                log::warn!("report: invalid {name}; omitted");
                continue;
            };
            strip_secrets(&mut value);
            serde_json::to_vec_pretty(&value)?
        } else {
            sanitize_ini(&text).into_bytes()
        };
        std::fs::write(dir.join(name), crate::upload::redact(bytes, home))
            .with_context(|| format!("writing {name}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configs_drop_nested_credentials_and_ini_secret_sections() {
        let mut json = serde_json::json!({ "size": "1280x720", "installs": [{ "keys_file": "private", "token": "private", "root": "/game" }],
            "AUTHORIZATION": "private", "apiKey": "private", "nested": { "password": "private", "volume": 10 } });
        strip_secrets(&mut json);
        assert!(!json.to_string().contains("private"));
        assert_eq!(json["nested"]["volume"], 10);
        assert_eq!(
            sanitize_ini(
                "; token=private\n; note=private\n# note=private\n[Video]\nWidth=1280\nAPI_KEY=private\n[Tokens]\nvalue=private\n[Audio]\nVolume=10\n"
            ),
            "[Video]\nWidth=1280\n[Audio]\nVolume=10\n"
        );
    }

    #[test]
    fn copies_use_the_active_options_file_and_redact_home_paths() {
        let root = std::env::temp_dir().join(format!("thedas-config-copy-{}", std::process::id()));
        let data = root.join("data");
        let out = root.join("report");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(data.join("options.ini"), "[Video]\nWidth=640\n").unwrap();
        let active = root.join("active.ini");
        std::fs::write(
            &active,
            "[Video]\nWidth=1280\nRoot=/Users/fixture/game\nToken=private\n",
        )
        .unwrap();
        std::fs::write(
            data.join("launcher.json"),
            r#"{"installs":[{"root":"/Users/fixture/game","keys_file":"private"}]}"#,
        )
        .unwrap();
        copy_configs(&out, &data, Some(&active), Some("/Users/fixture")).unwrap();
        let ini = std::fs::read_to_string(out.join("options.ini")).unwrap();
        assert!(ini.contains("Width=1280"));
        assert!(ini.contains("Root=~/game"));
        assert!(!ini.contains("private"));
        let launcher = std::fs::read_to_string(out.join("launcher.json")).unwrap();
        assert!(launcher.contains("~/game"));
        assert!(!launcher.contains("private"));
        let _ = std::fs::remove_dir_all(root);
    }
}
