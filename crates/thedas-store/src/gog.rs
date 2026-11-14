//! GOG: sign in through GOG's own page in the player's browser, then download the game with
//! GOG's content system (generation 2), the way GOG Galaxy does.
//!
//! Provisional throughout: written from public descriptions of the protocol and checked
//! against GOG's public build lists and file lists for Origins, not traced from Galaxy. The
//! secure-link step (the signed download addresses) needs a signed-in account and has not
//! been seen working yet.
//!
//! - Sign-in: the player opens [`login_url`] in the browser and signs in on GOG's page; GOG
//!   then shows a page whose address carries a one-time code. The player pastes that address
//!   (or the code) into the launcher, and [`code_from`] picks the code out. The launcher
//!   never sees the password. The code buys an access token and a refresh token, kept in
//!   memory for the download (the refresh token renews the access token on a long
//!   download) and dropped after it.
//! - [`CLIENT_ID`] / [`CLIENT_SECRET`] are GOG Galaxy's public client pair, the one every
//!   Galaxy install carries; committed with Sera's approval (2026-11-14), the one exception
//!   to "no key material in the repository".
//! - Builds: `content-system.gog.com/products/<id>/os/windows/builds?generation=2` lists
//!   them, newest first; a build's `link` is a zlib-compressed JSON naming its depots.
//! - A depot's file list is `.../content-system/v2/meta/<aa>/<bb>/<hash>`, zlib JSON again:
//!   each file is a list of chunks with the MD5 of the chunk both compressed and not.
//! - Chunks come from the addresses `secure_link` signs for the account, at
//!   `<path>/<aa>/<bb>/<compressedMd5>`. Each is zlib data; both MD5s are checked.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use md5::{Digest, Md5};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, Semaphore};

use crate::job::{Ctx, Event, Result, prepare_dest};
use crate::{GameKind, Request, Secret, safe_relative};

/// GOG Galaxy's public OAuth client id.
pub const CLIENT_ID: &str = "46899977096215655";
/// GOG Galaxy's public OAuth client secret (public: in every Galaxy install).
pub const CLIENT_SECRET: &str = "9d85c43b1482497dbbce61f6e4aa173a433796eeae2ca8c5f6129f2dc4de46d9";
/// Where GOG sends the browser after sign-in; the code is in its address.
pub const REDIRECT: &str = "https://embed.gog.com/on_login_success?origin=client";

const AUTH: &str = "https://auth.gog.com";
const CONTENT: &str = "https://content-system.gog.com";
const META: &str = "https://gog-cdn-fastly.gog.com/content-system/v2/meta";

/// GOG's product for each game it sells: Origins Ultimate Edition only.
pub fn product_id(game: GameKind) -> Option<&'static str> {
    match game {
        GameKind::Origins => Some(thedas_launcher::detect::GOG_ORIGINS),
        GameKind::Da2 => None,
    }
}

/// The address of GOG's sign-in page, for the player's browser.
pub fn login_url() -> String {
    format!(
        "{AUTH}/auth?client_id={CLIENT_ID}&redirect_uri={}&response_type=code&layout=client2",
        encode(REDIRECT)
    )
}

/// The sign-in code from what the player pasted: the whole address GOG ended on, or just
/// the code. `None` when it holds no code.
pub fn code_from(pasted: &str) -> Option<Secret> {
    let s = pasted.trim();
    let code = match s.find("code=") {
        Some(i) => s[i + 5..].split(['&', '#']).next().unwrap_or(""),
        None if !s.contains(['/', '?', '=']) => s,
        None => "",
    };
    let ok = (8..=4096).contains(&code.len())
        && code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~'));
    ok.then(|| Secret::new(code))
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Whether `h` is an MD5 in hex, as GOG names files on its servers.
fn is_hash(h: &str) -> bool {
    h.len() == 32 && h.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `ab/cd/abcd...`, where GOG keeps a file named by its hash.
fn galaxy_path(hash: &str) -> String {
    format!("{}/{}/{hash}", &hash[0..2], &hash[2..4])
}

// --- What GOG sends -----------------------------------------------------------------------

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_in: u64,
}

#[derive(Deserialize)]
struct Builds {
    #[serde(default)]
    items: Vec<BuildItem>,
}

#[derive(Deserialize)]
struct BuildItem {
    #[serde(default)]
    generation: u32,
    #[serde(default)]
    branch: Option<String>,
    link: String,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Build {
    #[serde(default)]
    pub base_product_id: String,
    #[serde(default)]
    pub depots: Vec<DepotRef>,
}

#[derive(Deserialize, Debug, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DepotRef {
    pub product_id: String,
    #[serde(default)]
    pub languages: Vec<String>,
    pub manifest: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Deserialize)]
struct DepotDoc {
    depot: Depot,
}

#[derive(Deserialize)]
struct Depot {
    #[serde(default)]
    items: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    #[serde(default)]
    chunks: Vec<Chunk>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Chunk {
    pub md5: String,
    pub size: u64,
    pub compressed_md5: String,
    pub compressed_size: u64,
}

#[derive(Deserialize)]
struct SecureLinks {
    #[serde(default)]
    urls: Vec<Endpoint>,
}

#[derive(Deserialize, Clone)]
struct Endpoint {
    url_format: String,
    #[serde(default)]
    parameters: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    supports_generation: Vec<u32>,
}

/// A file to download.
#[derive(Clone, Debug)]
pub(crate) struct FileJob {
    pub rel: PathBuf,
    pub product: String,
    pub chunks: Vec<Chunk>,
    pub size: u64,
}

/// The depots of `build` to download for `language` (`en-us`...): those for every language
/// (`*`) and those for `language`, or for English when no depot is for `language`.
pub(crate) fn pick_depots(build: &Build, language: &str) -> Vec<DepotRef> {
    let is = |d: &DepotRef, l: &str| d.languages.iter().any(|x| x.eq_ignore_ascii_case(l));
    let lang = if build.depots.iter().any(|d| is(d, language)) {
        language
    } else {
        "en-US"
    };
    build
        .depots
        .iter()
        .filter(|d| is(d, "*") || is(d, lang))
        .cloned()
        .collect()
}

/// The files of a depot's file list, checked: every path stays inside the install folder
/// and every hash is a hash. Directories and links are skipped (directories are made as
/// files land; Origins has no links).
pub(crate) fn depot_files(doc: &[u8], product: &str) -> Result<Vec<FileJob>> {
    let doc: DepotDoc =
        serde_json::from_slice(doc).map_err(|e| format!("GOG sent a damaged file list: {e}"))?;
    let mut out = Vec::new();
    for it in doc.depot.items {
        if it.kind != "DepotFile" {
            continue;
        }
        let rel = safe_relative(&it.path).ok_or_else(|| {
            format!(
                "GOG's file list has a path the launcher will not write: {:?}",
                it.path
            )
        })?;
        if it
            .chunks
            .iter()
            .any(|c| !is_hash(&c.md5) || !is_hash(&c.compressed_md5))
            || it
                .chunks
                .iter()
                .any(|c| c.size > 1 << 30 || c.compressed_size > 1 << 30)
        {
            return Err(format!("GOG's file list is damaged at {:?}", it.path));
        }
        let size = it.chunks.iter().map(|c| c.size).sum();
        out.push(FileJob {
            rel,
            product: product.to_string(),
            chunks: it.chunks,
            size,
        });
    }
    Ok(out)
}

/// Inflates a zlib stream of at most `limit` bytes.
fn inflate(data: &[u8], limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(data)
        .take(limit + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("damaged data from GOG: {e}"))?;
    if out.len() as u64 > limit {
        return Err("damaged data from GOG: too large".into());
    }
    Ok(out)
}

fn md5_hex(data: &[u8]) -> String {
    let d = Md5::digest(data);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Checks a downloaded chunk and returns its data.
pub(crate) fn unpack_chunk(c: &Chunk, compressed: &[u8]) -> Result<Vec<u8>> {
    if compressed.len() as u64 != c.compressed_size
        || !md5_hex(compressed).eq_ignore_ascii_case(&c.compressed_md5)
    {
        return Err("a downloaded piece did not match GOG's checksum".into());
    }
    let data = inflate(compressed, c.size)?;
    if data.len() as u64 != c.size || !md5_hex(&data).eq_ignore_ascii_case(&c.md5) {
        return Err("an unpacked piece did not match GOG's checksum".into());
    }
    Ok(data)
}

/// A chunk's address at one endpoint: the endpoint's `url_format` with its parameters filled
/// in, the chunk's place appended to `path`.
fn chunk_url(e: &Endpoint, compressed_md5: &str) -> String {
    let mut url = e.url_format.clone();
    for (k, v) in &e.parameters {
        let mut v = match v {
            serde_json::Value::String(s) => s.clone(),
            v => v.to_string(),
        };
        if k == "path" {
            v = format!(
                "{}/{}",
                v.trim_end_matches('/'),
                galaxy_path(compressed_md5)
            );
        }
        url = url.replace(&format!("{{{k}}}"), &v);
    }
    url
}

// --- Talking to GOG -----------------------------------------------------------------------

/// The signed-in session: tokens in memory only, renewed as they run out.
struct Session {
    http: reqwest::Client,
    access: Secret,
    refresh: Secret,
    expires: Instant,
}

impl Session {
    async fn sign_in(http: reqwest::Client, code: Secret) -> Result<Session> {
        let t = token(
            &http,
            &[
                ("grant_type", "authorization_code"),
                ("code", code.expose()),
                ("redirect_uri", REDIRECT),
            ],
        )
        .await
        .map_err(|e| {
            log::info!("GOG sign-in failed: {e}");
            "GOG did not accept the sign-in. Sign in again and paste the new address.".to_string()
        })?;
        Ok(Session::from_tokens(http, t))
    }

    fn from_tokens(http: reqwest::Client, t: Tokens) -> Session {
        Session {
            http,
            access: Secret::new(t.access_token),
            refresh: Secret::new(t.refresh_token),
            expires: Instant::now() + Duration::from_secs(t.expires_in.clamp(60, 86_400)),
        }
    }

    /// A current access token, renewed first if it is about to run out.
    async fn access(&mut self) -> Result<Secret> {
        if Instant::now() + Duration::from_secs(120) >= self.expires && !self.refresh.is_empty() {
            let t = token(
                &self.http,
                &[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", self.refresh.expose()),
                ],
            )
            .await
            .map_err(|e| format!("GOG ended the session: {e}"))?;
            *self = Session::from_tokens(self.http.clone(), t);
        }
        Ok(self.access.clone())
    }
}

async fn token(
    http: &reqwest::Client,
    extra: &[(&str, &str)],
) -> std::result::Result<Tokens, String> {
    let mut q = vec![("client_id", CLIENT_ID), ("client_secret", CLIENT_SECRET)];
    q.extend_from_slice(extra);
    let r = http
        .get(format!("{AUTH}/token"))
        .query(&q)
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if !r.status().is_success() {
        return Err(format!("status {}", r.status().as_u16()));
    }
    r.json::<Tokens>()
        .await
        .map_err(|e| e.without_url().to_string())
}

async fn get_bytes(
    http: &reqwest::Client,
    url: &str,
    limit: u64,
) -> std::result::Result<Vec<u8>, Fetch> {
    let r = http.get(url).send().await.map_err(|_| Fetch::Retry)?;
    match r.status().as_u16() {
        200 => {}
        401 | 403 => return Err(Fetch::Denied),
        404 => return Err(Fetch::Missing),
        _ => return Err(Fetch::Retry),
    }
    if r.content_length().is_some_and(|n| n > limit) {
        return Err(Fetch::Missing);
    }
    let b = r.bytes().await.map_err(|_| Fetch::Retry)?;
    if b.len() as u64 > limit {
        return Err(Fetch::Missing);
    }
    Ok(b.to_vec())
}

#[derive(Debug, PartialEq)]
enum Fetch {
    /// Worth trying again, or at another endpoint.
    Retry,
    /// The signed addresses ran out, or the account may not fetch this.
    Denied,
    Missing,
}

/// GOG's zlib-compressed JSON (a build or a file list), by address.
async fn get_meta(http: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let mut last = Fetch::Retry;
    for attempt in 0..4 {
        match get_bytes(http, url, 256 << 20).await {
            Ok(b) => return inflate(&b, 1 << 30),
            Err(Fetch::Retry) => {
                last = Fetch::Retry;
                tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
            }
            Err(e) => {
                last = e;
                break;
            }
        }
    }
    Err(format!("Could not fetch GOG's file list ({last:?})."))
}

/// Signed download addresses for `product`, for this session.
async fn secure_links(
    session: &Mutex<Session>,
    product: &str,
) -> std::result::Result<Vec<Endpoint>, Fetch> {
    let (http, access) = {
        let mut s = session.lock().await;
        let a = s.access().await.map_err(|_| Fetch::Denied)?;
        (s.http.clone(), a)
    };
    let r = http
        .get(format!("{CONTENT}/products/{product}/secure_link"))
        .query(&[("generation", "2"), ("_version", "2"), ("path", "/")])
        .bearer_auth(access.expose())
        .send()
        .await
        .map_err(|_| Fetch::Retry)?;
    match r.status().as_u16() {
        200 => {}
        401 | 403 => return Err(Fetch::Denied),
        404 => return Err(Fetch::Missing),
        _ => return Err(Fetch::Retry),
    }
    let links: SecureLinks = r.json().await.map_err(|_| Fetch::Retry)?;
    let urls: Vec<Endpoint> = links
        .urls
        .into_iter()
        .filter(|e| e.supports_generation.is_empty() || e.supports_generation.contains(&2))
        .collect();
    if urls.is_empty() {
        return Err(Fetch::Missing);
    }
    Ok(urls)
}

type Signed = (Instant, Arc<Vec<Endpoint>>);

/// Each product's signed addresses, fetched again when they run out.
struct Links {
    session: Mutex<Session>,
    /// Product id to when its addresses were signed, and the addresses.
    by_product: Mutex<std::collections::HashMap<String, Signed>>,
}

impl Links {
    async fn get(
        &self,
        product: &str,
        stale: bool,
    ) -> std::result::Result<Arc<Vec<Endpoint>>, Fetch> {
        let mut map = self.by_product.lock().await;
        if let Some((at, e)) = map.get(product)
            && !stale
            && at.elapsed() < Duration::from_secs(15 * 60)
        {
            return Ok(e.clone());
        }
        let mut last = Fetch::Retry;
        for attempt in 0..4 {
            match secure_links(&self.session, product).await {
                Ok(e) => {
                    let e = Arc::new(e);
                    map.insert(product.to_string(), (Instant::now(), e.clone()));
                    return Ok(e);
                }
                Err(Fetch::Retry) => tokio::time::sleep(Duration::from_secs(1 << attempt)).await,
                Err(e) => {
                    last = e;
                    break;
                }
            }
        }
        Err(last)
    }
}

/// One chunk, checked, from whichever endpoint answers.
async fn fetch_chunk(
    http: &reqwest::Client,
    links: &Links,
    product: &str,
    c: &Chunk,
) -> Result<Vec<u8>> {
    let mut stale = false;
    for attempt in 0..6u32 {
        let endpoints = links.get(product, stale).await.map_err(|e| match e {
            Fetch::Denied => {
                "GOG refused the download. Is the game on this GOG account?".to_string()
            }
            _ => "Could not reach GOG's download servers.".to_string(),
        })?;
        stale = false;
        let e = &endpoints[attempt as usize % endpoints.len()];
        match get_bytes(
            http,
            &chunk_url(e, &c.compressed_md5),
            c.compressed_size + 1,
        )
        .await
        {
            Ok(b) => match unpack_chunk(c, &b) {
                Ok(data) => return Ok(data),
                Err(err) => log::info!("GOG chunk {}: {err}", c.compressed_md5),
            },
            Err(Fetch::Denied) => stale = true,
            Err(f) => log::info!("GOG chunk {}: {f:?}", c.compressed_md5),
        }
        tokio::time::sleep(Duration::from_millis(500 << attempt.min(4))).await;
    }
    Err("A part of the game would not download. Try again later.".into())
}

/// Downloads one file to `<file>.part`, then renames it into place. A file already in
/// place at the right size is kept (a download started again picks up from there).
async fn fetch_file(
    http: reqwest::Client,
    links: Arc<Links>,
    chunks_sem: Arc<Semaphore>,
    dest: PathBuf,
    f: FileJob,
    progress: Arc<dyn Fn(u64) + Send + Sync>,
) -> Result<()> {
    let path = dest.join(&f.rel);
    if std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == f.size) {
        progress(f.size);
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| format!("Could not make {}: {e}", dir.display()))?;
    }
    let mut part = path.clone().into_os_string();
    part.push(".part");
    let part = PathBuf::from(part);
    let mut out = tokio::fs::File::create(&part)
        .await
        .map_err(|e| format!("Could not write {}: {e}", part.display()))?;
    // Up to four chunks of a file in flight, written in order.
    let mut ahead: VecDeque<tokio::task::JoinHandle<Result<Vec<u8>>>> = VecDeque::new();
    let mut next = 0;
    while next < f.chunks.len() || !ahead.is_empty() {
        while next < f.chunks.len() && ahead.len() < 4 {
            let (http, links, sem, c, product) = (
                http.clone(),
                links.clone(),
                chunks_sem.clone(),
                f.chunks[next].clone(),
                f.product.clone(),
            );
            ahead.push_back(tokio::spawn(async move {
                let _permit = sem.acquire_owned().await.map_err(|e| e.to_string())?;
                fetch_chunk(&http, &links, &product, &c).await
            }));
            next += 1;
        }
        if let Some(h) = ahead.pop_front() {
            let data = h.await.map_err(|e| e.to_string())??;
            out.write_all(&data)
                .await
                .map_err(|e| format!("Could not write {}: {e}", part.display()))?;
            progress(data.len() as u64);
        }
    }
    out.flush()
        .await
        .map_err(|e| format!("Could not write {}: {e}", part.display()))?;
    drop(out);
    tokio::fs::rename(&part, &path)
        .await
        .map_err(|e| format!("Could not write {}: {e}", path.display()))
}

/// Signs in with `code` and downloads `req` from GOG.
pub(crate) async fn run(req: &Request, code: Secret, ctx: &mut Ctx) -> Result<PathBuf> {
    let product = product_id(req.game).ok_or("GOG does not sell this game.")?;
    let http = reqwest::Client::builder()
        .user_agent("OpenThedas launcher")
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| format!("Could not start downloading: {e}"))?;

    ctx.status("Signing in to GOG");
    let session = Session::sign_in(http.clone(), code).await?;
    ctx.send(Event::SignedIn);

    ctx.status("Asking GOG what to download");
    let builds: Builds = http
        .get(format!("{CONTENT}/products/{product}/os/windows/builds"))
        .query(&[("generation", "2")])
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Could not ask GOG about the game: {}", e.without_url()))?
        .json()
        .await
        .map_err(|e| format!("GOG sent a damaged build list: {}", e.without_url()))?;
    let build = builds
        .items
        .iter()
        .find(|b| b.generation == 2 && b.branch.is_none())
        .ok_or("GOG lists no build to download.")?;
    if !build.link.starts_with("https://") {
        return Err("GOG sent an odd build address.".into());
    }
    let build: Build = serde_json::from_slice(&get_meta(&http, &build.link).await?)
        .map_err(|e| format!("GOG sent a damaged build: {e}"))?;

    let links = Arc::new(Links {
        session: Mutex::new(session),
        by_product: Mutex::new(Default::default()),
    });
    ctx.status("Fetching the file list");
    let mut files = Vec::new();
    for d in pick_depots(&build, &req.language) {
        if !is_hash(&d.manifest) || !d.product_id.bytes().all(|b| b.is_ascii_digit()) {
            return Err("GOG's build is damaged.".into());
        }
        if d.product_id != build.base_product_id {
            // A DLC's own product: only if this account may fetch it.
            if links.get(&d.product_id, false).await.is_err() {
                log::info!(
                    "skipping GOG product {} (not on this account)",
                    d.product_id
                );
                continue;
            }
        }
        let doc = get_meta(&http, &format!("{META}/{}", galaxy_path(&d.manifest))).await?;
        files.extend(depot_files(&doc, &d.product_id)?);
    }
    // The same file in two depots: the later depot's copy wins, as in Galaxy's install.
    let mut seen = std::collections::HashSet::new();
    files.reverse();
    files.retain(|f| seen.insert(f.rel.to_string_lossy().to_ascii_lowercase()));
    files.reverse();
    if files.is_empty() {
        return Err("GOG lists nothing to download.".into());
    }
    // Ownership check before anything lands: the base product must sign addresses.
    links.get(product, false).await.map_err(|e| match e {
        Fetch::Denied => format!("This GOG account doesn't own {}.", req.game.name()),
        _ => "Could not reach GOG's download servers.".into(),
    })?;

    let total: u64 = files.iter().map(|f| f.size).sum();
    ctx.send(Event::Plan {
        bytes: total,
        files: files.len() as u64,
    });
    prepare_dest(&req.dest)?;
    ctx.status(format!("Downloading {}", req.game.name()));

    let done = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let events = ctx.sender();
    let progress: Arc<dyn Fn(u64) + Send + Sync> = {
        let done = done.clone();
        let last = std::sync::Mutex::new(Instant::now() - Duration::from_secs(1));
        Arc::new(move |n| {
            let d = done.fetch_add(n, std::sync::atomic::Ordering::Relaxed) + n;
            let mut last = last.lock().unwrap_or_else(|p| p.into_inner());
            if last.elapsed() >= Duration::from_millis(200) || d >= total {
                *last = Instant::now();
                let _ = events.send(Event::Progress { done: d, total });
            }
        })
    };
    let chunks_sem = Arc::new(Semaphore::new(12));
    let mut running = tokio::task::JoinSet::new();
    let mut queue = files.into_iter();
    loop {
        while running.len() < 4 {
            let Some(f) = queue.next() else { break };
            running.spawn(fetch_file(
                http.clone(),
                links.clone(),
                chunks_sem.clone(),
                req.dest.clone(),
                f,
                progress.clone(),
            ));
        }
        match running.join_next().await {
            Some(r) => r.map_err(|e| e.to_string())??,
            None => break,
        }
    }
    ctx.send(Event::Progress {
        done: done.load(std::sync::atomic::Ordering::Relaxed),
        total,
    });
    Ok(req.dest.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn codes_from_pasted_text() {
        let url = "https://embed.gog.com/on_login_success?origin=client&code=AbC-123_xyz.9";
        assert_eq!(code_from(url).unwrap().expose(), "AbC-123_xyz.9");
        assert_eq!(
            code_from("  AbCdEf123456  ").unwrap().expose(),
            "AbCdEf123456"
        );
        assert_eq!(
            code_from("https://x/?code=abcdefgh1234&state=1")
                .unwrap()
                .expose(),
            "abcdefgh1234"
        );
        for bad in [
            "",
            "short",
            "https://embed.gog.com/on_login_success",
            "code=a b c d e f g h",
            "code=<script>alert</script>",
        ] {
            assert!(code_from(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn login_url_is_gogs() {
        let u = login_url();
        assert!(u.starts_with("https://auth.gog.com/auth?client_id=46899977096215655&"));
        assert!(u.contains(
            "redirect_uri=https%3A%2F%2Fembed.gog.com%2Fon_login_success%3Forigin%3Dclient"
        ));
        assert!(thedas_launcher::store::allowed(&u));
    }

    #[test]
    fn depots_by_language() {
        let d = |m: &str, l: &[&str]| DepotRef {
            product_id: "1".into(),
            languages: l.iter().map(|s| s.to_string()).collect(),
            manifest: m.into(),
            size: 0,
        };
        let build = Build {
            base_product_id: "1".into(),
            depots: vec![d("a", &["en-US"]), d("b", &["*"]), d("c", &["fr-FR"])],
        };
        let names = |l| {
            pick_depots(&build, l)
                .into_iter()
                .map(|d| d.manifest)
                .collect::<Vec<_>>()
        };
        assert_eq!(names("en-us"), ["a", "b"]);
        assert_eq!(names("fr-fr"), ["b", "c"]);
        assert_eq!(names("pl-pl"), ["a", "b"]);
    }

    const H1: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn file_lists_are_checked() {
        let doc = format!(
            r#"{{"version":2,"depot":{{"items":[
                {{"type":"DepotFile","path":"bin_ship\\DAOrigins.exe","chunks":[{{"md5":"{H1}","size":10,"compressedMd5":"{H1}","compressedSize":5}}]}},
                {{"type":"DepotDirectory","path":"docs"}},
                {{"type":"DepotFile","path":"empty.txt","chunks":[]}}
            ]}}}}"#
        );
        let files = depot_files(doc.as_bytes(), "7").unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].rel, PathBuf::from("bin_ship/DAOrigins.exe"));
        assert_eq!(files[0].size, 10);
        assert_eq!(files[1].size, 0);
        let evil =
            r#"{"depot":{"items":[{"type":"DepotFile","path":"..\\..\\x.dll","chunks":[]}]}}"#;
        assert!(depot_files(evil.as_bytes(), "7").is_err());
        let bad_hash = r#"{"depot":{"items":[{"type":"DepotFile","path":"a","chunks":[{"md5":"../x","size":1,"compressedMd5":"y","compressedSize":1}]}]}}"#;
        assert!(depot_files(bad_hash.as_bytes(), "7").is_err());
        assert!(depot_files(b"not json", "7").is_err());
    }

    #[test]
    fn chunks_are_verified() {
        let data = b"hello dragon age hello dragon age";
        let packed = zlib(data);
        let c = Chunk {
            md5: md5_hex(data),
            size: data.len() as u64,
            compressed_md5: md5_hex(&packed),
            compressed_size: packed.len() as u64,
        };
        assert_eq!(unpack_chunk(&c, &packed).unwrap(), data);
        let mut flipped = packed.clone();
        flipped[3] ^= 1;
        assert!(unpack_chunk(&c, &flipped).is_err());
        // The right compressed bytes but a wrong unpacked checksum.
        let wrong = Chunk {
            md5: H1.into(),
            ..c.clone()
        };
        assert!(unpack_chunk(&wrong, &packed).is_err());
        // A stream that inflates past its stated size.
        let big = zlib(&[0u8; 4096]);
        let bomb = Chunk {
            md5: H1.into(),
            size: 16,
            compressed_md5: md5_hex(&big),
            compressed_size: big.len() as u64,
        };
        assert!(unpack_chunk(&bomb, &big).is_err());
    }

    /// GOG's public build list and file lists for Origins (no account needed). Network:
    /// `cargo test -p thedas-store -- --ignored live`.
    #[test]
    #[ignore]
    fn live_origins_file_list() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let http = reqwest::Client::new();
            let builds: Builds = http
                .get(format!("{CONTENT}/products/1949616134/os/windows/builds"))
                .query(&[("generation", "2")])
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let b = builds
                .items
                .iter()
                .find(|b| b.generation == 2 && b.branch.is_none())
                .unwrap();
            let build: Build =
                serde_json::from_slice(&get_meta(&http, &b.link).await.unwrap()).unwrap();
            assert_eq!(build.base_product_id, "1949616134");
            let mut files = Vec::new();
            for d in pick_depots(&build, "en-us") {
                let doc = get_meta(&http, &format!("{META}/{}", galaxy_path(&d.manifest)))
                    .await
                    .unwrap();
                files.extend(depot_files(&doc, &d.product_id).unwrap());
            }
            let total: u64 = files.iter().map(|f| f.size).sum();
            eprintln!("{} files, {}", files.len(), crate::size_text(total));
            assert!(files.len() > 1000);
            assert!(files.iter().any(|f| f.rel.starts_with("packages/core")));
        });
    }

    #[test]
    fn chunk_urls_fill_the_format() {
        let e: Endpoint = serde_json::from_str(
            r#"{"url_format":"{base_url}/token=nva={expires_at}~token={token}{path}",
                "parameters":{"base_url":"https://cdn.example","path":"/content-system/v2/store/1","token":"t","expires_at":123},
                "supports_generation":[2]}"#,
        )
        .unwrap();
        assert_eq!(
            chunk_url(&e, H1),
            format!(
                "https://cdn.example/token=nva=123~token=t/content-system/v2/store/1/01/23/{H1}"
            )
        );
        assert_eq!(galaxy_path(H1), format!("01/23/{H1}"));
        assert!(is_hash(H1) && !is_hash("xyz") && !is_hash(&H1.replace('0', "g")));
    }
}
