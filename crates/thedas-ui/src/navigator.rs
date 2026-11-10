//! A Ruffle navigator that serves movie loads from the game's resource system.
//!
//! Scaleform movies import shared libraries by `.swf` name (`gfxfontlib.swf`) and load
//! sub-movies by name; the game ships them as `.gfx`. Every fetch is resolved by the last path
//! component, trying the name as given and then with a `.gfx` extension. Nothing is read from
//! disk or the network.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use async_channel::{Receiver, Sender};
use ruffle_core::backend::navigator::{
    ErrorResponse, NavigationMethod, NavigatorBackend, NullExecutor, NullSpawner, OwnedFuture,
    Request, SuccessResponse,
};
use ruffle_core::indexmap::IndexMap;
use ruffle_core::loader::Error;
use ruffle_core::socket::{ConnectionState, SocketAction, SocketHandle};
use url::{ParseError, Url};

/// Looks a resource up by file name (`mainmenu.gfx`), returning its bytes.
pub type ResourceLookup = Arc<dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync>;

/// Base URL movies are given; relative loads resolve against it.
pub const BASE_URL: &str = "file:///gui/";

/// Every fetch, in order, with whether it was served. Shared with [`crate::Movie`].
pub type FetchLog = Arc<std::sync::Mutex<Vec<(String, bool)>>>;

pub struct ResourceNavigator {
    lookup: ResourceLookup,
    spawner: NullSpawner,
    log: FetchLog,
}

impl ResourceNavigator {
    pub fn new(lookup: ResourceLookup, executor: &NullExecutor, log: FetchLog) -> Self {
        Self {
            lookup,
            spawner: executor.spawner(),
            log,
        }
    }
}

/// The size an image URL asks for (`img://name.dds?Width=64&Height=64`, as the movies'
/// `MovieUtils.GetImageUrl` builds it): Scaleform scales the image to it.
pub fn requested_size(url: &str) -> Option<(u32, u32)> {
    let query = url.split_once('?')?.1;
    let mut w = None;
    let mut h = None;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        let v = v
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v| *v >= 1.0 && *v <= 4096.0)?;
        match k.to_ascii_lowercase().as_str() {
            "width" => w = Some(v.round() as u32),
            "height" => h = Some(v.round() as u32),
            _ => {}
        }
    }
    Some((w?, h?))
}

/// A PNG (RGBA8) resampled to `w` x `h` (bilinear), or `None` when it is not one.
pub fn resize_png(data: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let decoder = png::Decoder::new(std::io::Cursor::new(data));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    let (sw, sh) = (info.width as usize, info.height as usize);
    if (sw as u32, sh as u32) == (w, h) {
        return Some(data.to_vec());
    }
    let px =
        |x: usize, y: usize, c: usize| buf[(y.min(sh - 1) * sw + x.min(sw - 1)) * 4 + c] as f32;
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h as usize {
        let fy = ((y as f32 + 0.5) * sh as f32 / h as f32 - 0.5).max(0.0);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        for x in 0..w as usize {
            let fx = ((x as f32 + 0.5) * sw as f32 / w as f32 - 0.5).max(0.0);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            for c in 0..4 {
                let top = px(x0, y0, c) * (1.0 - tx) + px(x0 + 1, y0, c) * tx;
                let bottom = px(x0, y0 + 1, c) * (1.0 - tx) + px(x0 + 1, y0 + 1, c) * tx;
                out.push((top * (1.0 - ty) + bottom * ty).round() as u8);
            }
        }
    }
    crate::stage::encode_png(&crate::Rgba {
        width: w,
        height: h,
        pixels: out,
    })
}

/// Candidate resource names for a URL: its last path component, then the same with `.gfx`.
pub fn candidates(url: &str) -> Vec<String> {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if name.is_empty() {
        return Vec::new();
    }
    let mut out = vec![name.clone()];
    let stem = name.rsplit_once('.').map(|x| x.0).unwrap_or(&name);
    let gfx = format!("{stem}.gfx");
    if gfx != name {
        out.push(gfx);
    }
    out
}

struct Response {
    url: String,
    body: Option<Vec<u8>>,
}

impl SuccessResponse for Response {
    fn url(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.url)
    }

    fn set_url(&mut self, url: String) {
        self.url = url;
    }

    fn body(self: Box<Self>) -> OwnedFuture<Vec<u8>, Error> {
        let body = self.body.unwrap_or_default();
        Box::pin(async move { Ok(body) })
    }

    fn text_encoding(&self) -> Option<&'static encoding_rs::Encoding> {
        None
    }

    fn status(&self) -> u16 {
        0
    }

    fn redirected(&self) -> bool {
        false
    }

    fn next_chunk(&mut self) -> OwnedFuture<Option<Vec<u8>>, Error> {
        let chunk = self.body.take();
        Box::pin(async move { Ok(chunk) })
    }

    fn expected_length(&self) -> Result<Option<u64>, Error> {
        Ok(self.body.as_ref().map(|b| b.len() as u64))
    }
}

impl NavigatorBackend for ResourceNavigator {
    fn navigate_to_url(
        &self,
        url: &str,
        target: &str,
        _vars_method: Option<(NavigationMethod, IndexMap<String, String>)>,
    ) {
        log::info!("ui: getURL({url}, {target}) ignored");
    }

    fn fetch(&self, request: Request) -> OwnedFuture<Box<dyn SuccessResponse>, ErrorResponse> {
        let url = request.url().to_string();
        let found = candidates(&url)
            .into_iter()
            .find_map(|name| (self.lookup)(&name))
            .map(|body| match requested_size(&url) {
                Some((w, h)) => resize_png(&body, w, h).unwrap_or(body),
                None => body,
            });
        if let Ok(mut log) = self.log.lock() {
            log.push((url.clone(), found.is_some()));
        }
        Box::pin(async move {
            match found {
                Some(body) => Ok(Box::new(Response {
                    url,
                    body: Some(body),
                }) as Box<dyn SuccessResponse>),
                None => Err(ErrorResponse {
                    url: url.clone(),
                    error: Error::FetchError(format!("no resource for {url}")),
                }),
            }
        })
    }

    fn resolve_url(&self, url: &str) -> Result<Url, ParseError> {
        match Url::parse(url) {
            Ok(u) => Ok(u),
            Err(ParseError::RelativeUrlWithoutBase) => Url::parse(BASE_URL)?.join(url),
            Err(e) => Err(e),
        }
    }

    fn spawn_future(&mut self, future: OwnedFuture<(), Error>) {
        self.spawner.spawn_local(future);
    }

    fn pre_process_url(&self, url: Url) -> Url {
        url
    }

    fn connect_socket(
        &mut self,
        _host: String,
        _port: u16,
        _timeout: Duration,
        handle: SocketHandle,
        _receiver: Receiver<Vec<u8>>,
        sender: Sender<SocketAction>,
    ) {
        let _ = sender.try_send(SocketAction::Connect(handle, ConnectionState::Failed));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swf_names_fall_back_to_gfx() {
        assert_eq!(
            candidates("file:///gui/gfxfontlib.swf"),
            vec!["gfxfontlib.swf".to_string(), "gfxfontlib.gfx".to_string()]
        );
        assert_eq!(candidates("Quickbar.GFX?x=1"), vec!["quickbar.gfx"]);
        assert_eq!(
            requested_size("img://a.dds?Width=64&Height=32"),
            Some((64, 32))
        );
        assert_eq!(requested_size("img://a.dds?Async=1"), None);
        assert_eq!(requested_size("img://a.dds"), None);
        let png = crate::stage::encode_png(&crate::Rgba {
            width: 2,
            height: 2,
            pixels: [255, 0, 0, 255].repeat(4),
        })
        .unwrap();
        let big = resize_png(&png, 8, 8).unwrap();
        let d = png::Decoder::new(std::io::Cursor::new(&big))
            .read_info()
            .unwrap();
        assert_eq!((d.info().width, d.info().height), (8, 8));
        assert!(resize_png(b"not a png", 4, 4).is_none());
        assert_eq!(candidates("noext"), vec!["noext", "noext.gfx"]);
        assert!(candidates("file:///gui/").is_empty());
    }
}
