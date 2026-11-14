//! Steam: sign in with the player's account (QR code or password + Steam Guard) and download
//! the game's depots, through `steamroom` / `steamroom-client` (MIT OR Apache-2.0).
//!
//! The flow is Steam's own client protocol, as `steamroom-cli download` drives it: product
//! info (PICS) names the depots and their manifests, Steam hands over each depot's key, the
//! manifest comes from a content server, and `DepotJob` fetches, decrypts and checks every
//! chunk. Nothing is cached: no refresh token, manifest or depot key reaches the disk.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use steamroom::cdn::{CdnClient, CdnServerPool};
use steamroom::client::{LoggedIn, SteamClient};
use steamroom::depot::manifest::DepotManifest;
use steamroom::depot::{AppId, CellId, DepotId, ManifestId};
use steamroom::types::KeyValue;
use steamroom_client::download::{CdnChunkFetcher, DepotJob};
use steamroom_client::event::DownloadEvent;
use steamroom_client::login::{CredentialsLoginFlow, GuardType, LoginBuilder, LoginError};

use crate::job::{Ctx, Event, Reply, Result, prepare_dest};
use crate::{GameKind, Login, Request, safe_relative};

/// Steam's app for each game: Origins Ultimate Edition (it holds every DLC), and DA2.
pub fn app_id(game: GameKind) -> u32 {
    match game {
        GameKind::Origins => thedas_launcher::detect::STEAM_ORIGINS_UE,
        GameKind::Da2 => thedas_launcher::detect::STEAM_DA2,
    }
}

/// The name a depot's `config/language` uses for one of the game's language folders.
/// Provisional: Steam's language names, not read from these apps' depots.
pub fn steam_language(lang: &str) -> &'static str {
    match lang.to_ascii_lowercase().as_str() {
        "fr-fr" => "french",
        "de-de" => "german",
        "it-it" => "italian",
        "es-es" => "spanish",
        "pl-pl" => "polish",
        "ru-ru" => "russian",
        "cs-cz" => "czech",
        "hu-hu" => "hungarian",
        _ => "english",
    }
}

/// One depot to download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Depot {
    pub id: u32,
    pub manifest: u64,
    /// The size product info gives, when it gives one.
    pub size: Option<u64>,
    /// Whether the depot belongs to a DLC (`dlcappid`): skipped quietly if the account
    /// does not own it.
    pub dlc: bool,
}

/// The depots of an app's product info to download for Windows in `language`: every depot
/// with a public manifest that is for Windows (or any OS) and for that language (or none),
/// skipping depots another app provides (`depotfromapp`, shared redistributables). When no
/// depot is for `language`, the English ones are taken instead.
pub fn pick_depots(app: &KeyValue, language: &str) -> Vec<Depot> {
    let Some(depots) = app.get("depots") else {
        return Vec::new();
    };
    let want = steam_language(language);
    let mut all = Vec::new();
    let steamroom::types::KvValue::Children(map) = &depots.value else {
        return Vec::new();
    };
    for (key, d) in map {
        let Ok(id) = key.parse::<u32>() else { continue };
        if d.get("depotfromapp").is_some() || d.get("sharedinstall").is_some() {
            continue;
        }
        let config = d.get("config");
        let text = |k: &str| {
            config
                .and_then(|c| c.get(k))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_ascii_lowercase()
        };
        let oslist = text("oslist");
        if !oslist.is_empty() && !oslist.split(',').any(|o| o.trim() == "windows") {
            continue;
        }
        let public = d.get("manifests").and_then(|m| m.get("public"));
        let gid = public
            .and_then(|p| p.get("gid").and_then(|g| g.as_str()).or(p.as_str()))
            .and_then(|g| g.parse::<u64>().ok());
        let Some(manifest) = gid else { continue };
        let size = public
            .and_then(|p| p.get("size"))
            .and_then(kv_u64)
            .or_else(|| d.get("maxsize").and_then(kv_u64));
        all.push((
            text("language"),
            Depot {
                id,
                manifest,
                size,
                dlc: d.get("dlcappid").is_some(),
            },
        ));
    }
    let has_wanted = all.iter().any(|(l, _)| l == want);
    let lang = if has_wanted { want } else { "english" };
    all.into_iter()
        .filter(|(l, _)| l.is_empty() || l == lang)
        .map(|(_, d)| d)
        .collect()
}

fn kv_u64(v: &KeyValue) -> Option<u64> {
    v.as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| v.as_u64())
        .or_else(|| v.as_i32().and_then(|i| u64::try_from(i).ok()))
}

/// Product info's key-values, binary or text.
fn parse_app_kv(data: &[u8]) -> Option<KeyValue> {
    if data.first() == Some(&0) {
        steamroom::types::key_value::parse_binary_kv(data).ok()
    } else {
        steamroom::types::key_value::parse_text_kv(&String::from_utf8_lossy(data)).ok()
    }
}

/// A manifest from a content server: a zip holding one file, or the bytes themselves.
fn unzip_manifest(data: &[u8]) -> Result<Vec<u8>> {
    if !data.starts_with(b"PK") {
        return Ok(data.to_vec());
    }
    let bad = |e: &dyn std::fmt::Display| format!("Steam sent a damaged file list: {e}");
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| bad(&e))?;
    if zip.is_empty() {
        return Err(bad(&"empty"));
    }
    let mut f = zip.by_index(0).map_err(|e| bad(&e))?;
    // A manifest of the whole of Origins is a few megabytes; refuse anything absurd.
    if f.size() > 512 << 20 {
        return Err(bad(&"too large"));
    }
    let mut out = Vec::with_capacity(f.size() as usize);
    std::io::Read::read_to_end(&mut f, &mut out).map_err(|e| bad(&e))?;
    Ok(out)
}

/// Checks every path in a manifest before any file is written: all must stay inside the
/// install folder, and links are refused.
fn check_manifest(m: &DepotManifest) -> Result<()> {
    for f in &m.files {
        if safe_relative(&f.filename).is_none() || f.link_target.is_some() {
            return Err(format!(
                "Steam's file list has a path the launcher will not write: {:?}",
                f.filename
            ));
        }
    }
    Ok(())
}

fn login_error(e: LoginError) -> String {
    match e {
        LoginError::InvalidPassword => "Steam says the account name or password is wrong.".into(),
        LoginError::InvalidGuardCode => "Steam did not accept that code.".into(),
        LoginError::NoCmServers => "Could not reach Steam. Check the connection.".into(),
        LoginError::LogonFailed(r) => format!("Steam refused the sign-in ({r:?})."),
        e => format!("Could not sign in to Steam: {e}"),
    }
}

fn steam_error(what: &str, e: impl std::fmt::Display) -> String {
    format!("{what}: {e}")
}

async fn sign_in(login: Login, ctx: &mut Ctx) -> Result<SteamClient<LoggedIn>> {
    let builder = LoginBuilder::new().device_name("OpenThedas launcher");
    let approved = match login {
        Login::SteamQr => {
            ctx.status("Connecting to Steam");
            let flow = builder.with_qr().begin().await.map_err(login_error)?;
            ctx.send(Event::SteamQr(flow.challenge_url().to_string()));
            ctx.status("Scan the code with the Steam app on your phone");
            flow.wait_for_scan().await.map_err(login_error)?
        }
        Login::SteamPassword { account, password } => {
            ctx.status("Signing in to Steam");
            // steamroom keeps its own copy only until the password is encrypted for Steam.
            let flow = builder
                .with_credentials(account, password.expose())
                .begin()
                .await
                .map_err(login_error)?;
            drop(password);
            match flow {
                CredentialsLoginFlow::Approved(a) => a,
                CredentialsLoginFlow::NeedsGuardCode(mut challenge) => {
                    let kinds = challenge.allowed_kinds();
                    let email = kinds.contains(&GuardType::EmailCode)
                        && !kinds.contains(&GuardType::DeviceCode);
                    let kind = if email {
                        GuardType::EmailCode
                    } else {
                        GuardType::DeviceCode
                    };
                    ctx.send(Event::NeedGuardCode { email });
                    loop {
                        let Some(Reply::GuardCode(code)) = ctx.reply().await else {
                            return Err("Signing in was stopped.".into());
                        };
                        let code = code.expose().trim().to_ascii_uppercase();
                        match challenge.submit_code(&code, kind).await {
                            Ok(a) => break a,
                            Err((c, LoginError::InvalidGuardCode)) => {
                                challenge = c;
                                ctx.send(Event::WrongGuardCode);
                            }
                            Err((_, e)) => return Err(login_error(e)),
                        }
                    }
                }
                CredentialsLoginFlow::NeedsMobileConfirm(m) => {
                    ctx.send(Event::NeedMobileConfirm);
                    m.wait_for_confirmation().await.map_err(login_error)?
                }
                _ => return Err("Steam asked for a kind of sign-in the launcher can't do.".into()),
            }
        }
        Login::Gog { .. } => return Err("not a Steam sign-in".into()),
    };
    // `finish` consumes the tokens; they live only inside the client from here on.
    let client = approved.finish().await.map_err(login_error)?;
    ctx.send(Event::SignedIn);
    Ok(client)
}

/// Signs in and downloads `req` from Steam.
pub(crate) async fn run(req: &Request, login: Login, ctx: &mut Ctx) -> Result<PathBuf> {
    let client = sign_in(login, ctx).await?;
    let app = AppId(app_id(req.game));

    ctx.status("Asking Steam what to download");
    let tokens = client
        .pics_get_access_tokens(&[app])
        .await
        .map_err(|e| steam_error("Could not ask Steam about the game", e))?;
    let token = tokens
        .into_iter()
        .next()
        .unwrap_or(steamroom::apps::AccessToken {
            app_id: app,
            token: 0,
        });
    let info = client
        .pics_get_product_info(&[token])
        .await
        .map_err(|e| steam_error("Could not ask Steam about the game", e))?;
    let kv = info
        .into_iter()
        .next()
        .and_then(|i| i.kv_data)
        .and_then(|d| parse_app_kv(&d))
        .ok_or("Steam sent no details for the game.")?;
    let depots = pick_depots(&kv, &req.language);
    if depots.is_empty() {
        return Err("Steam lists nothing to download for Windows.".into());
    }

    // Keys first: a depot this account can't open is a game it doesn't own (or a DLC it
    // doesn't own, which is skipped).
    let mut keyed = Vec::new();
    for d in &depots {
        match client.get_depot_decryption_key(DepotId(d.id), app).await {
            Ok(key) => keyed.push((d.clone(), key)),
            Err(e) if d.dlc => log::info!("skipping DLC depot {}: {e}", d.id),
            Err(e) => {
                log::info!("no key for depot {}: {e}", d.id);
                return Err(format!(
                    "This Steam account doesn't own {}.",
                    req.game.name()
                ));
            }
        }
    }

    let servers = client
        .get_cdn_servers(CellId(0), Some(20))
        .await
        .map_err(|e| steam_error("Could not find a Steam download server", e))?;
    let Some(first) = servers.first().cloned() else {
        return Err("Steam offered no download servers.".into());
    };
    let cdn = || CdnClient::new().map_err(|e| steam_error("Could not start downloading", e));
    let manifest_cdn = cdn()?;
    let manifest_pool = CdnServerPool::new(servers.clone());

    // Every manifest before any file, so the size is known up front.
    let mut jobs = Vec::new();
    for (d, key) in keyed {
        ctx.status("Fetching the file list");
        let (depot, manifest_id) = (DepotId(d.id), ManifestId(d.manifest));
        let code = client
            .get_manifest_request_code(app, depot, manifest_id, Some("public"), None)
            .await
            .ok()
            .flatten()
            .unwrap_or(0);
        let auth = client
            .get_cdn_auth_token(app, depot, &first.host)
            .await
            .ok()
            .and_then(|t| t.token);
        let raw = manifest_cdn
            .download_manifest_pooled(&manifest_pool, depot, manifest_id, code, auth.as_deref())
            .await
            .map_err(|e| steam_error("Could not fetch the file list", e))?;
        let mut manifest = DepotManifest::parse(&unzip_manifest(&raw)?)
            .map_err(|e| steam_error("Steam sent a damaged file list", e))?;
        if manifest.filenames_encrypted {
            manifest
                .decrypt_filenames(&key)
                .map_err(|e| steam_error("Could not read the file list", e))?;
        }
        check_manifest(&manifest)?;
        jobs.push((depot, key, manifest, auth));
    }
    let total: u64 = jobs
        .iter()
        .flat_map(|(_, _, m, _)| m.files.iter().map(|f| f.size))
        .sum();
    let files: u64 = jobs.iter().map(|(_, _, m, _)| m.files.len() as u64).sum();
    ctx.send(Event::Plan {
        bytes: total,
        files,
    });

    prepare_dest(&req.dest)?;
    let mut before = 0u64;
    for (depot, key, manifest, auth) in jobs {
        ctx.status(format!("Downloading {}", req.game.name()));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let job = DepotJob::builder()
            .depot_id(depot)
            .depot_key(key)
            .install_dir(req.dest.clone())
            .verify(false)
            .event_sender(tx)
            .build()
            .map_err(|e| steam_error("Could not start downloading", e))?;
        let fetcher = Arc::new(CdnChunkFetcher::new(
            cdn()?,
            CdnServerPool::new(servers.clone()),
            auth,
        ));
        let events = ctx.sender();
        let forward = tokio::spawn(async move {
            while let Some(e) = rx.recv().await {
                if let DownloadEvent::DepotProgress {
                    completed_bytes, ..
                } = e
                {
                    let _ = events.send(Event::Progress {
                        done: before + completed_bytes,
                        total,
                    });
                }
            }
        });
        let result = job.download(&manifest, fetcher).await;
        drop(job);
        let _ = forward.await;
        result.map_err(|e| steam_error("The download failed", e))?;
        before += manifest.files.iter().map(|f| f.size).sum::<u64>();
    }
    remove_staging(&req.dest);
    Ok(req.dest.clone())
}

/// `DepotJob` stages files in `.DepotDownloader/staging`; it is empty once all files land.
fn remove_staging(dest: &Path) {
    let dir = dest.join(".DepotDownloader");
    let _ = std::fs::remove_dir(dir.join("staging"));
    let _ = std::fs::remove_dir(&dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(text: &str) -> KeyValue {
        steamroom::types::key_value::parse_text_kv(text).unwrap()
    }

    const APP: &str = r#""appinfo"
{
    "appid" "47810"
    "depots"
    {
        "228980" { "depotfromapp" "228980" }
        "47811"
        {
            "config" { "oslist" "windows" }
            "manifests" { "public" { "gid" "111" "size" "1000" } }
        }
        "47812"
        {
            "config" { "oslist" "windows" "language" "french" }
            "manifests" { "public" { "gid" "222" } }
            "maxsize" "50"
        }
        "47813"
        {
            "config" { "oslist" "windows" "language" "english" }
            "manifests" { "public" "333" }
        }
        "47814"
        {
            "config" { "oslist" "macos" }
            "manifests" { "public" { "gid" "444" } }
        }
        "47815"
        {
            "dlcappid" "17460"
            "manifests" { "public" { "gid" "555" } }
        }
        "47816" { "config" { "oslist" "windows" } }
        "branches" { "public" { "buildid" "1" } }
    }
}"#;

    #[test]
    fn depots_for_windows_and_the_language() {
        let ids = |lang: &str| -> Vec<u32> {
            pick_depots(&kv(APP), lang)
                .into_iter()
                .map(|d| d.id)
                .collect()
        };
        assert_eq!(ids("en-us"), vec![47811, 47813, 47815]);
        assert_eq!(ids("fr-fr"), vec![47811, 47812, 47815]);
        // No German depot: English instead.
        assert_eq!(ids("de-de"), vec![47811, 47813, 47815]);
        let d = pick_depots(&kv(APP), "fr-fr");
        assert_eq!(d[0].manifest, 111);
        assert_eq!(d[0].size, Some(1000));
        assert_eq!(d[1].size, Some(50));
        assert!(d[2].dlc && !d[0].dlc);
    }

    #[test]
    fn no_depots_is_empty() {
        assert!(pick_depots(&kv(r#""a" { "appid" "1" }"#), "en-us").is_empty());
        assert!(pick_depots(&kv(r#""a" { "depots" "x" }"#), "en-us").is_empty());
    }

    #[test]
    fn manifests_unzip() {
        assert_eq!(unzip_manifest(b"plain").unwrap(), b"plain");
        assert!(unzip_manifest(b"PK\x03\x04junk").is_err());
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            w.start_file("z", zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut w, b"manifest").unwrap();
            w.finish().unwrap();
        }
        assert_eq!(unzip_manifest(&buf).unwrap(), b"manifest");
    }

    #[test]
    fn unsafe_manifest_paths_are_refused() {
        use steamroom::depot::manifest::ManifestFile;
        let ok = DepotManifest::new(vec![ManifestFile::new(r"bin_ship\daorigins.exe".into(), 1)]);
        assert!(check_manifest(&ok).is_ok());
        let bad = DepotManifest::new(vec![ManifestFile::new(r"..\..\evil.dll".into(), 1)]);
        assert!(check_manifest(&bad).is_err());
    }
}
