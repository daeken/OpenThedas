//! The way back from a report (see [`crate::report`], [`crate::upload`]): when a build that
//! fixes a problem this player reported is out, the game says so; one answer saves the game,
//! fetches that build into a folder of its own, and starts it on the game, loaded where the
//! player is now or where they reported the problem; the new build then asks whether it is
//! fixed and sends the answer back beside the report.
//!
//! Set up per machine, never built in: `update-url` in the player's data folder holds the
//! address of the test builds' folder (its `latest.json` lists the builds, newest first,
//! each with its files by platform and the reports it fixes). Without it nothing is
//! checked. Builds live in `builds/<commit>/` in the data folder, so a running program is
//! never replaced.
//!
//! This is an addition of ours, not the original's.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;

/// The file holding the builds' address, in the player's data folder.
pub const URL_FILE: &str = "update-url";
/// The most a build's download may be (bytes).
const MAX_BUILD: u64 = 512 << 20;

/// `latest.json`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Latest {
    pub format: u32,
    #[serde(default)]
    pub builds: Vec<Build>,
}

/// One published build.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Build {
    pub commit: String,
    /// Its files by platform (`windows-x86_64`, `macos-aarch64`, ...).
    #[serde(default)]
    pub files: std::collections::BTreeMap<String, BuildFile>,
    /// The reports it fixes: the Discord messages they were sent as.
    #[serde(default)]
    pub fixes: Vec<String>,
}

/// A build's archive for one platform.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct BuildFile {
    /// Relative to the builds' address.
    pub path: String,
    pub sha256: String,
    pub size: u64,
    /// The program in the archive.
    pub exe: String,
}

/// A report this machine sent, not yet checked against a fix.
#[derive(Clone, Debug)]
pub struct Sent {
    pub dir: PathBuf,
    pub message: String,
    pub note: String,
    pub place: String,
}

/// A fix to offer: the build that has it and the report it fixes.
#[derive(Clone, Debug)]
pub struct Offer {
    pub base: String,
    pub build: Build,
    pub file: BuildFile,
    pub report: Sent,
}

/// This build's platform, as `latest.json` names it.
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// The builds' address, if this machine is set up for them (ends in `/`).
pub fn base_url() -> Option<String> {
    if !crate::upload::online() {
        return None;
    }
    let text = crate::upload::setup_file(URL_FILE)?;
    let url = text.trim();
    if !url.starts_with("https://") {
        log::warn!("update: {URL_FILE} does not hold an https address");
        return None;
    }
    Some(if url.ends_with('/') {
        url.to_string()
    } else {
        format!("{url}/")
    })
}

/// The reports this machine sent that no build has been checked against yet.
pub fn sent_reports(reports: &Path) -> Vec<Sent> {
    let Ok(rd) = std::fs::read_dir(reports) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let dir = e.path();
        if dir.join("checked.json").exists() {
            continue;
        }
        let Some(sent) = std::fs::read(dir.join("sent.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        else {
            continue;
        };
        let Some(message) = sent["message"].as_str().filter(|_| sent["ok"] == true) else {
            continue;
        };
        let info: serde_json::Value = std::fs::read(dir.join("report.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let place = info["game"]["area_name"]
            .as_str()
            .or(info["game"]["area"].as_str())
            .unwrap_or("the game")
            .to_string();
        let note = std::fs::read_to_string(dir.join("note.txt")).unwrap_or_default();
        out.push(Sent {
            dir: dir.clone(),
            message: message.to_string(),
            note: note.trim().to_string(),
            place,
        });
    }
    out.sort_by(|a, b| a.dir.cmp(&b.dir));
    out
}

/// The fix to offer, if any: the newest build that is not this one, has a file for this
/// platform, and fixes one of `sent`.
pub fn offer(base: &str, latest: &Latest, own: Option<&str>, sent: &[Sent]) -> Option<Offer> {
    let build = latest.builds.first()?;
    if own.is_some_and(|c| c == build.commit) {
        return None;
    }
    let file = build.files.get(&platform())?;
    let report = sent.iter().find(|s| build.fixes.contains(&s.message))?;
    Some(Offer {
        base: base.to_string(),
        build: build.clone(),
        file: file.clone(),
        report: report.clone(),
    })
}

/// Looks for a fix to offer on a thread of its own.
pub fn check_in_background(reports: PathBuf) -> Option<mpsc::Receiver<Option<Offer>>> {
    let base = base_url()?;
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("update check".into())
        .spawn(move || {
            let sent = sent_reports(&reports);
            let found = if sent.is_empty() {
                None
            } else {
                match fetch_latest(&base) {
                    Ok(latest) => offer(&base, &latest, option_env!("THEDAS_GIT_COMMIT"), &sent),
                    Err(e) => {
                        log::info!("update: no check: {e:#}");
                        None
                    }
                }
            };
            let _ = tx.send(found);
        });
    Some(rx)
}

fn fetch_latest(base: &str) -> Result<Latest> {
    let mut resp = ureq::get(format!("{base}latest.json"))
        .call()
        .map_err(|e| anyhow::anyhow!("fetching latest.json: {e}"))?;
    let bytes = resp
        .body_mut()
        .with_config()
        .limit(1 << 20)
        .read_to_vec()
        .context("reading latest.json")?;
    let latest: Latest = serde_json::from_slice(&bytes).context("latest.json")?;
    if latest.format != 1 {
        bail!("latest.json format {} (expected 1)", latest.format);
    }
    Ok(latest)
}

/// Fetches `offer`'s build into `builds/<commit>/` in the data folder (checked against its
/// hash) and returns the program to start. A build already there is used as it is.
pub fn fetch_build(offer: &Offer) -> Result<PathBuf> {
    let commit = &offer.build.commit;
    if commit.is_empty() || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("the build's commit {commit:?} is not a commit");
    }
    let dir = crate::saves::data_dir().join("builds").join(commit);
    let exe = dir.join(safe_name(&offer.file.exe)?);
    if exe.is_file() {
        return Ok(exe);
    }
    if offer.file.size > MAX_BUILD {
        bail!("the build is {} bytes", offer.file.size);
    }
    let mut resp = ureq::get(format!("{}{}", offer.base, offer.file.path))
        .call()
        .map_err(|e| anyhow::anyhow!("downloading the build: {e}"))?;
    let bytes = resp
        .body_mut()
        .with_config()
        .limit(MAX_BUILD)
        .read_to_vec()
        .context("downloading the build")?;
    let sum = sha256_hex(&bytes);
    if !sum.eq_ignore_ascii_case(offer.file.sha256.trim()) {
        bail!("the build's download does not match its hash");
    }
    let partial = dir.with_extension("partial");
    let _ = std::fs::remove_dir_all(&partial);
    for (name, data) in unzip(&bytes)? {
        let path = partial.join(safe_name(&name)?);
        std::fs::create_dir_all(&partial)?;
        std::fs::write(&path, data).with_context(|| format!("writing {name}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&partial, &dir).context("putting the build in place")?;
    if !exe.is_file() {
        bail!("the build has no {}", offer.file.exe);
    }
    Ok(exe)
}

/// A file name from an archive, with no folders in it.
fn safe_name(name: &str) -> Result<&str> {
    let ok = !name.is_empty()
        && !name.contains(['/', '\\', ':'])
        && name != "."
        && name != ".."
        && !name.starts_with('.');
    if !ok {
        bail!("the build holds a file named {name:?}");
    }
    Ok(name)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let d = sha2::Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// The files of a zip (stored or deflated; no folders). Bounds-checked throughout.
pub fn unzip(z: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let u16_at = |i: usize| -> Result<u16> {
        Ok(u16::from_le_bytes(
            z.get(i..i + 2).context("zip cut short")?.try_into()?,
        ))
    };
    let u32_at = |i: usize| -> Result<u32> {
        Ok(u32::from_le_bytes(
            z.get(i..i + 4).context("zip cut short")?.try_into()?,
        ))
    };
    // The end of the central directory: the last signature within its comment's reach.
    let lowest = z.len().saturating_sub(22 + 0xffff);
    let eocd = (lowest..=z.len().saturating_sub(22))
        .rev()
        .find(|&i| z.get(i..i + 4) == Some(b"PK\x05\x06"))
        .context("not a zip")?;
    let count = u16_at(eocd + 10)? as usize;
    let mut at = u32_at(eocd + 16)? as usize;
    let mut out = Vec::new();
    for _ in 0..count {
        if u32_at(at)? != 0x0201_4b50 {
            bail!("zip directory damaged");
        }
        let method = u16_at(at + 10)?;
        let csize = u32_at(at + 20)? as usize;
        let usize_ = u32_at(at + 24)? as usize;
        let name_len = u16_at(at + 28)? as usize;
        let extra_len = u16_at(at + 30)? as usize;
        let comment_len = u16_at(at + 32)? as usize;
        let local = u32_at(at + 42)? as usize;
        let name = String::from_utf8_lossy(
            z.get(at + 46..at + 46 + name_len)
                .context("zip cut short")?,
        )
        .to_string();
        at += 46 + name_len + extra_len + comment_len;
        if name.ends_with('/') {
            continue;
        }
        if u32_at(local)? != 0x0403_4b50 {
            bail!("zip entry {name} damaged");
        }
        let start = local + 30 + u16_at(local + 26)? as usize + u16_at(local + 28)? as usize;
        let packed = z.get(start..start + csize).context("zip cut short")?;
        if usize_ as u64 > MAX_BUILD {
            bail!("zip entry {name} too large");
        }
        let data = match method {
            0 => packed.to_vec(),
            8 => {
                let mut d = Vec::with_capacity(usize_);
                flate2::read::DeflateDecoder::new(packed)
                    .take(usize_ as u64 + 1)
                    .read_to_end(&mut d)?;
                d
            }
            m => bail!("zip entry {name}: method {m}"),
        };
        if data.len() != usize_ {
            bail!("zip entry {name}: size differs");
        }
        out.push((name, data));
    }
    Ok(out)
}

/// Starts `exe` on the game: the install, `save` loaded, and `report` to ask about.
pub fn start_build(
    exe: &Path,
    install: &Path,
    save: &Path,
    report: &Path,
    size: &str,
) -> Result<()> {
    std::process::Command::new(exe)
        .arg("--install")
        .arg(install)
        .arg("--save")
        .arg(save)
        .arg("--check-report")
        .arg(report)
        .arg("--size")
        .arg(size)
        .arg("--no-intro")
        .current_dir(exe.parent().unwrap_or(Path::new(".")))
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))?;
    Ok(())
}

/// Sends the player's answer about a report's fix beside it, and marks it checked.
pub fn send_verdict_in_background(report: PathBuf, fixed: bool, comment: String) {
    let _ = std::thread::Builder::new()
        .name("report verdict".into())
        .spawn(move || {
            let sent: serde_json::Value = std::fs::read(report.join("sent.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            let message = sent["message"].as_str().unwrap_or("?").to_string();
            let commit =
                option_env!("THEDAS_GIT_COMMIT").map_or("unknown", |c| &c[..c.len().min(9)]);
            let mut text = format!(
                "{} Report {message}: the reporter says it is {} in build `{commit}`.",
                if fixed { "✅" } else { "❌" },
                if fixed { "fixed" } else { "NOT fixed" }
            );
            let comment = comment.trim();
            if !comment.is_empty() {
                text.push_str(&format!(
                    "\n> {}",
                    comment.chars().take(1500).collect::<String>()
                ));
            }
            let result = match crate::upload::webhook() {
                Some(url) => crate::upload::post_text(&url, &text),
                None => Err(anyhow::anyhow!("no webhook set up")),
            };
            if let Err(e) = &result {
                log::warn!("update: the answer was not sent: {e:#}");
            }
            let _ = std::fs::write(
                report.join("checked.json"),
                serde_json::to_vec_pretty(&json!({
                    "fixed": fixed,
                    "comment": comment,
                    "build": option_env!("THEDAS_GIT_COMMIT"),
                    "sent": result.is_ok(),
                    "at": crate::saves::now_secs(),
                }))
                .unwrap_or_default(),
            );
        });
}

/// The words for the question a build started on `report` asks.
pub fn verdict_prompt(report: &Path) -> String {
    let what = sent_reports(report.parent().unwrap_or(report))
        .iter()
        .find(|s| s.dir == report)
        .map(|s| {
            let note: String = s.note.chars().take(120).collect();
            format!(" in {} (\"{note}\")", s.place)
        })
        .unwrap_or_default();
    format!(
        "Is the problem you reported{what} fixed? Anything you write here goes with your answer."
    )
}

/// The words for an offer's question.
pub fn offer_prompt(o: &Offer) -> String {
    let note: String = o.report.note.chars().take(120).collect();
    let note = if note.is_empty() {
        String::new()
    } else {
        format!(" (\"{note}\")")
    };
    format!(
        "We think the problem you reported in {}{note} is fixed in a new build. Update now? \
         Your game is saved first, and the new build starts where you choose.",
        o.report.place
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latest(commit: &str, fixes: &[&str]) -> Latest {
        let mut files = std::collections::BTreeMap::new();
        files.insert(
            platform(),
            BuildFile {
                path: format!("builds/{commit}/game.zip"),
                sha256: "00".into(),
                size: 10,
                exe: "thedas-app".into(),
            },
        );
        Latest {
            format: 1,
            builds: vec![Build {
                commit: commit.into(),
                files,
                fixes: fixes.iter().map(|s| s.to_string()).collect(),
            }],
        }
    }

    fn sent(message: &str) -> Sent {
        Sent {
            dir: PathBuf::from(message),
            message: message.into(),
            note: "the door".into(),
            place: "Lothering".into(),
        }
    }

    #[test]
    fn a_fix_is_offered_only_for_this_players_report_in_another_build() {
        let l = latest("abc123", &["111", "222"]);
        let o = offer(
            "https://x/",
            &l,
            Some("def456"),
            &[sent("333"), sent("222")],
        )
        .unwrap();
        assert_eq!(o.report.message, "222");
        assert_eq!(o.file.exe, "thedas-app");
        assert!(offer_prompt(&o).contains("Lothering (\"the door\")"));
        // Already on that build, or no report of ours fixed, or no file for us.
        assert!(offer("https://x/", &l, Some("abc123"), &[sent("222")]).is_none());
        assert!(offer("https://x/", &l, None, &[sent("333")]).is_none());
        let mut other = l.clone();
        other.builds[0].files.clear();
        assert!(offer("https://x/", &other, None, &[sent("222")]).is_none());
    }

    #[test]
    fn sent_reports_are_found_until_checked() {
        let dir = std::env::temp_dir().join(format!("thedas-update-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let r = dir.join("2026-11-13-lot");
        std::fs::create_dir_all(&r).unwrap();
        std::fs::write(r.join("sent.json"), r#"{"ok":true,"message":"42"}"#).unwrap();
        std::fs::write(r.join("note.txt"), "stuck\n").unwrap();
        std::fs::write(
            r.join("report.json"),
            r#"{"game":{"area":"lot100ar","area_name":"Lothering"}}"#,
        )
        .unwrap();
        let unsent = dir.join("2026-11-13-unsent");
        std::fs::create_dir_all(&unsent).unwrap();
        let s = sent_reports(&dir);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].message.as_str(), s[0].note.as_str()), ("42", "stuck"));
        assert_eq!(s[0].place, "Lothering");
        std::fs::write(r.join("checked.json"), "{}").unwrap();
        assert!(sent_reports(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn archives_unpack_and_bad_names_are_refused() {
        // A zip as the report's sender writes it reads back.
        let files = vec![
            ("thedas-app".to_string(), b"#!/bin/sh\necho hi\n".to_vec()),
            ("README.txt".to_string(), vec![b'x'; 5000]),
        ];
        let z = crate::upload::zip(&files).unwrap();
        assert_eq!(unzip(&z).unwrap(), files);
        assert!(unzip(b"not a zip at all, not even close......").is_err());
        assert!(unzip(&z[..z.len() / 2]).is_err());
        for bad in ["../x", "a/b", "C:x", "..", ".hidden", ""] {
            assert!(safe_name(bad).is_err(), "{bad}");
        }
        assert_eq!(sha256_hex(b"abc").len(), 64);
        assert!(sha256_hex(b"abc").starts_with("ba7816bf"));
    }
}
