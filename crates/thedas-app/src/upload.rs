//! Sends an issue report (see [`crate::report`]) to the project's Discord, when the player
//! has set that up: a channel webhook's address in `discord-webhook` in their data folder
//! (`saves::data_dir`). Without that file nothing ever leaves the machine.
//!
//! What is sent: the player's note and where they were as the message, the screenshot (so
//! the channel shows it), and one zip of the report's files (the two saves, the report's
//! facts, the log, the input and the simulation's recent trace). Paths under the player's
//! home folder are written `~` in what is sent, and the install's own path is left out, so
//! no user name goes with it. Discord takes at most 10 MB a message: a screenshot that would
//! not fit is sent at half size, and then not at all.
//!
//! This is an addition of ours, not the original's.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value as J, json};

/// The file holding the webhook's address, in the player's data folder.
pub const WEBHOOK_FILE: &str = "discord-webhook";
/// The most Discord takes in one message (bytes), less room for the form around it.
const MAX_UPLOAD: usize = 10 * 1024 * 1024 - 64 * 1024;
/// The report's files that go in the zip.
const ZIPPED: &[&str] = &[
    "report.json",
    "note.txt",
    "log.txt",
    "input.txt",
    "trace.json",
    "save_now.json",
    "save_before.json",
];

/// Set when the game is driven by a client or a test (`--control`, a hidden window): then
/// nothing is sent or fetched (reports, verdicts, the update check), so a test under a
/// home that is set up for sending never posts. `THEDAS_REPORT_UPLOAD=1` allows it again
/// (the sending tests' own opt-in).
static AUTOMATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Marks this run as driven by a client or a test (see [`AUTOMATED`]).
pub fn set_automated() {
    AUTOMATED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Whether this run may reach the network for reports and updates.
pub fn online() -> bool {
    !AUTOMATED.load(std::sync::atomic::Ordering::Relaxed)
        || std::env::var_os("THEDAS_REPORT_UPLOAD").is_some_and(|v| v == "1")
}

/// A one-line setup file of this machine's (`discord-webhook`, `update-url`): from the
/// player's data folder, else from beside the program, where a test build's download has
/// it. One found beside the program is copied to the data folder, so a later build
/// (which lives in a folder of its own, see [`crate::update`]) finds it too.
pub fn setup_file(name: &str) -> Option<String> {
    let own = crate::saves::data_dir().join(name);
    if let Ok(text) = std::fs::read_to_string(&own) {
        return Some(text.trim().to_string());
    }
    let beside = std::env::current_exe().ok()?.parent()?.join(name);
    let text = std::fs::read_to_string(beside).ok()?.trim().to_string();
    if let Some(dir) = own.parent() {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(&own, format!("{text}\n"));
    }
    Some(text)
}

/// Where reports are sent, if this machine is set up for it (never in an automated run):
/// a Discord webhook's address in `discord-webhook`, else the test builds' own receiver
/// (`report` under the address in `update-url`), which passes them on to the channel.
pub fn webhook() -> Option<String> {
    if !online() {
        return None;
    }
    if let Some(url) = setup_file(WEBHOOK_FILE) {
        let ok = url.starts_with("https://discord.com/api/webhooks/")
            || url.starts_with("https://discordapp.com/api/webhooks/");
        if !ok {
            log::warn!("report: {WEBHOOK_FILE} does not hold a Discord webhook address");
        }
        return ok.then_some(url);
    }
    crate::update::base_url().map(|base| format!("{base}report"))
}

/// Sends the report in `dir` on a thread of its own; the result is logged and written
/// beside it (`sent.json`).
pub fn send_in_background(dir: PathBuf, url: String) {
    let _ = std::thread::Builder::new()
        .name("report upload".into())
        .spawn(move || {
            let result = send(&dir, &url);
            let record = match &result {
                Ok(id) => {
                    log::info!("report: sent ({id})");
                    json!({ "ok": true, "message": id, "at": crate::saves::now_secs() })
                }
                Err(e) => {
                    log::warn!("report: not sent: {e:#}");
                    json!({ "ok": false, "error": format!("{e:#}"), "at": crate::saves::now_secs() })
                }
            };
            let _ = std::fs::write(
                dir.join("sent.json"),
                serde_json::to_vec_pretty(&record).unwrap_or_default(),
            );
        });
}

/// Sends the report in `dir`; returns the message's id.
pub fn send(dir: &Path, url: &str) -> Result<String> {
    let home = home();
    let mut info: J = serde_json::from_slice(
        &std::fs::read(dir.join("report.json")).context("reading report.json")?,
    )?;
    let message = message(dir, &info);
    if let Some(install) = info["settings"]["install"].as_object_mut() {
        install.remove("root");
    }
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for name in ZIPPED {
        let bytes = match *name {
            "report.json" => serde_json::to_vec_pretty(&info)?,
            _ => match std::fs::read(dir.join(name)) {
                Ok(b) => b,
                Err(_) => continue,
            },
        };
        files.push((name.to_string(), redact(bytes, home.as_deref())));
    }
    let id = info["id"].as_str().unwrap_or("report").to_string();
    let zip = zip(&files)?;
    let mut shot = std::fs::read(dir.join("screenshot.png")).ok();
    if let Some(png) = &shot {
        if zip.len() + png.len() > MAX_UPLOAD {
            shot = half_size(png)
                .ok()
                .filter(|p| zip.len() + p.len() <= MAX_UPLOAD);
        }
    }
    if zip.len() > MAX_UPLOAD {
        bail!(
            "the report is {} bytes zipped, over Discord's limit",
            zip.len()
        );
    }
    let mut parts = vec![(format!("{id}.zip"), "application/zip", zip)];
    if let Some(png) = shot {
        parts.push(("screenshot.png".to_string(), "image/png", png));
    }
    post(url, &message, &parts)
}

/// The message: the player's note first, then where and on what.
fn message(dir: &Path, info: &J) -> String {
    let note = std::fs::read_to_string(dir.join("note.txt")).unwrap_or_default();
    let note = note.trim();
    let game = &info["game"];
    let place = match (game["area_name"].as_str(), game["area"].as_str()) {
        (Some(n), Some(a)) if !n.is_empty() => format!("{n} ({a})"),
        (_, Some(a)) => a.to_string(),
        _ => "the menus".to_string(),
    };
    let build = &info["build"];
    let commit = build["commit"]
        .as_str()
        .map_or("unknown", |c| &c[..c.len().min(9)]);
    let gpu = info["gpu"]["name"].as_str().unwrap_or("unknown GPU");
    let os = build["os"].as_str().unwrap_or("?");
    let head = format!("**Issue report** in {place}\nBuild `{commit}`, {os}, {gpu}");
    let room = 1900usize.saturating_sub(head.len());
    let note = if note.is_empty() {
        "(no note)".to_string()
    } else if note.chars().count() > room {
        format!("{}…", note.chars().take(room).collect::<String>())
    } else {
        note.to_string()
    };
    format!("{note}\n\n{head}")
}

/// Posts `content` with `files` to the webhook (multipart, as Discord takes attachments);
/// returns the message's id.
fn post(url: &str, content: &str, files: &[(String, &str, Vec<u8>)]) -> Result<String> {
    let boundary = format!("openthedas{:016x}", crate::saves::now_secs() ^ 0x5eed_cafe);
    let payload = json!({
        "content": content,
        "allowed_mentions": { "parse": [] },
        "attachments": files.iter().enumerate()
            .map(|(i, (name, _, _))| json!({ "id": i, "filename": name }))
            .collect::<Vec<_>>(),
    });
    let mut body = Vec::new();
    write!(
        body,
        "--{boundary}\r\nContent-Disposition: form-data; name=\"payload_json\"\r\n\
         Content-Type: application/json\r\n\r\n{payload}\r\n"
    )?;
    for (i, (name, ctype, bytes)) in files.iter().enumerate() {
        write!(
            body,
            "--{boundary}\r\nContent-Disposition: form-data; name=\"files[{i}]\"; \
             filename=\"{name}\"\r\nContent-Type: {ctype}\r\n\r\n"
        )?;
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    write!(body, "--{boundary}--\r\n")?;
    let mut resp = ureq::post(format!("{url}?wait=true"))
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .send(&body[..])
        .map_err(|e| anyhow::anyhow!("posting to Discord: {e}"))?;
    let reply: J = serde_json::from_slice(
        &resp
            .body_mut()
            .read_to_vec()
            .context("reading Discord's reply")?,
    )
    .context("Discord's reply")?;
    Ok(reply["id"].as_str().unwrap_or("?").to_string())
}

/// Posts a plain message to the webhook.
pub fn post_text(url: &str, content: &str) -> Result<()> {
    let body = json!({ "content": content, "allowed_mentions": { "parse": [] } });
    ureq::post(url)
        .header("Content-Type", "application/json")
        .send(body.to_string().as_bytes())
        .map_err(|e| anyhow::anyhow!("posting to Discord: {e}"))?;
    Ok(())
}

/// The player's home folder as text, for [`redact`].
fn home() -> Option<String> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| h.to_string_lossy().to_string())
        .filter(|h| h.len() > 3)
}

/// `bytes` with the home folder's path written `~` (as it is, and as JSON escapes it).
fn redact(bytes: Vec<u8>, home: Option<&str>) -> Vec<u8> {
    let Some(home) = home else {
        return bytes;
    };
    let escaped = home.replace('\\', "\\\\");
    let mut out = bytes;
    for pat in [home.to_string(), escaped] {
        out = replace_all(&out, pat.as_bytes(), b"~");
    }
    out
}

fn replace_all(hay: &[u8], pat: &[u8], with: &[u8]) -> Vec<u8> {
    if pat.is_empty() {
        return hay.to_vec();
    }
    let mut out = Vec::with_capacity(hay.len());
    let mut i = 0;
    while i < hay.len() {
        if hay[i..].starts_with(pat) {
            out.extend_from_slice(with);
            i += pat.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

/// The screenshot at half size.
fn half_size(png: &[u8]) -> Result<Vec<u8>> {
    let decoder = png::Decoder::new(std::io::Cursor::new(png));
    let mut reader = decoder.read_info()?;
    let mut buf = vec![
        0;
        reader
            .output_buffer_size()
            .context("screenshot too large")?
    ];
    let frame = reader.next_frame(&mut buf)?;
    if frame.color_type != png::ColorType::Rgba || frame.bit_depth != png::BitDepth::Eight {
        bail!("the screenshot is not 8-bit RGBA");
    }
    buf.truncate(frame.buffer_size());
    let img = thedas_ui::Rgba {
        width: frame.width,
        height: frame.height,
        pixels: buf,
    };
    thedas_ui::stage::encode_png(&crate::shrink(&img, 2)).context("encoding the screenshot")
}

/// A zip of `files` (deflated), as Windows, macOS and Linux open it.
pub fn zip(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    use flate2::Compression;
    use flate2::write::DeflateEncoder;
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let mut crc = flate2::Crc::new();
        crc.update(data);
        let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
        enc.write_all(data)?;
        let packed = enc.finish()?;
        let offset = u32::try_from(out.len()).context("zip too large")?;
        let (csize, usize_) = (
            u32::try_from(packed.len()).context("file too large")?,
            u32::try_from(data.len()).context("file too large")?,
        );
        let name_len = u16::try_from(name.len()).context("name too long")?;
        let header = |sig: u32, central_dir: bool| {
            let mut h = Vec::new();
            h.extend_from_slice(&sig.to_le_bytes());
            if central_dir {
                h.extend_from_slice(&20u16.to_le_bytes()); // made by
            }
            h.extend_from_slice(&20u16.to_le_bytes()); // needed to extract
            h.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
            h.extend_from_slice(&8u16.to_le_bytes()); // deflate
            h.extend_from_slice(&0u32.to_le_bytes()); // time and date
            h.extend_from_slice(&crc.sum().to_le_bytes());
            h.extend_from_slice(&csize.to_le_bytes());
            h.extend_from_slice(&usize_.to_le_bytes());
            h.extend_from_slice(&name_len.to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes()); // extra
            if central_dir {
                h.extend_from_slice(&0u16.to_le_bytes()); // comment
                h.extend_from_slice(&0u16.to_le_bytes()); // disk
                h.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
                h.extend_from_slice(&0u32.to_le_bytes()); // external attributes
                h.extend_from_slice(&offset.to_le_bytes());
            }
            h.extend_from_slice(name.as_bytes());
            h
        };
        out.extend(header(0x0403_4b50, false));
        out.extend_from_slice(&packed);
        central.extend(header(0x0201_4b50, true));
    }
    let count = u16::try_from(files.len()).context("too many files")?;
    let cd_offset = u32::try_from(out.len()).context("zip too large")?;
    let cd_size = u32::try_from(central.len()).context("zip too large")?;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_paths_are_written_as_tilde() {
        let text = br#"{"dir":"/Users/sam/.openthedas/reports/x","win":"C:\\Users\\sam\\x"}"#;
        let out = redact(text.to_vec(), Some("/Users/sam"));
        assert_eq!(
            String::from_utf8(out).unwrap(),
            r#"{"dir":"~/.openthedas/reports/x","win":"C:\\Users\\sam\\x"}"#
        );
        let out = redact(text.to_vec(), Some(r"C:\Users\sam"));
        assert!(String::from_utf8(out).unwrap().contains(r#""win":"~\\x""#));
        assert_eq!(redact(b"abc".to_vec(), None), b"abc");
    }

    #[test]
    fn the_zip_holds_its_files() {
        let files = vec![
            ("a.txt".to_string(), b"hello hello hello hello".to_vec()),
            ("b.json".to_string(), b"{}".to_vec()),
        ];
        let z = zip(&files).unwrap();
        assert_eq!(&z[..4], b"PK\x03\x04");
        // End of central directory: two entries.
        let eocd = z.len() - 22;
        assert_eq!(&z[eocd..eocd + 4], b"PK\x05\x06");
        assert_eq!(u16::from_le_bytes([z[eocd + 10], z[eocd + 11]]), 2);
        // The first entry inflates back.
        let csize = u32::from_le_bytes(z[18..22].try_into().unwrap()) as usize;
        let start = 30 + 5;
        let mut d = flate2::read::DeflateDecoder::new(&z[start..start + csize]);
        let mut back = Vec::new();
        std::io::Read::read_to_end(&mut d, &mut back).unwrap();
        assert_eq!(back, files[0].1);
    }

    #[test]
    fn the_message_leads_with_the_note() {
        let dir = std::env::temp_dir().join(format!("thedas-upload-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("note.txt"), "the door would not open").unwrap();
        let info = json!({
            "game": { "area": "lot100ar", "area_name": "Lothering" },
            "build": { "commit": "0123456789abcdef", "os": "windows" },
            "gpu": { "name": "Test GPU" },
        });
        let m = message(&dir, &info);
        assert!(
            m.starts_with("the door would not open\n\n**Issue report** in Lothering (lot100ar)")
        );
        assert!(m.contains("`012345678`, windows, Test GPU"));
        std::fs::write(dir.join("note.txt"), "x".repeat(5000)).unwrap();
        assert!(message(&dir, &info).chars().count() <= 2000);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
