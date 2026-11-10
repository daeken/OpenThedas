//! A set of movies sharing one engine bridge: loading from the game's resources, starting
//! them the way the game does, input, ticking and CPU captures. The application adds the GPU
//! composite on top.

use std::sync::Arc;

use thedas_vfs::Vfs;

use crate::protocol::{GameBridge, GameMessage, START_SCENE};
use crate::{Gpu, ImageSource, ResourceLookup, Rgba, UiError, UiMovie};

/// Movie sets by name (`menu`, `hud`); anything else is a comma-separated list of movie names.
pub fn movie_names(spec: &str) -> Vec<String> {
    let list: &[&str] = match spec {
        "menu" => &["mainmenu"],
        "hud" => &["portraits", "quickbar", "minimap"],
        _ => &[],
    };
    let names: Vec<String> = if list.is_empty() {
        spec.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        list.iter().map(|s| s.to_string()).collect()
    };
    names
        .into_iter()
        .map(|n| {
            if n.contains('.') {
                n.to_ascii_lowercase()
            } else {
                format!("{}.gfx", n.to_ascii_lowercase())
            }
        })
        .collect()
}

pub fn vfs_lookup(vfs: &Arc<Vfs>) -> ResourceLookup {
    let vfs = vfs.clone();
    Arc::new(move |name: &str| vfs.read(name).ok().map(|x| x.0))
}

/// English talk-table strings from every `*en-us.tlk` in the stack.
pub fn talk_table(vfs: &Vfs) -> crate::protocol::StringLookup {
    let mut map = std::collections::HashMap::new();
    for name in vfs.names().into_iter().filter(|n| n.ends_with("en-us.tlk")) {
        if let Ok((data, _)) = vfs.read(&name) {
            if let Ok(t) = thedas_formats::Tlk::parse(&data) {
                map.extend(
                    t.strings
                        .into_iter()
                        .map(|(k, v)| (k, v.trim_end_matches('\0').to_string())),
                );
            }
        }
    }
    Arc::new(move |id| map.get(&id).cloned())
}

/// `base = src + base * (1 - src.a)` for a premultiplied `src` of the same size.
pub fn blend_premultiplied(base: &mut Rgba, src: &Rgba) {
    if (base.width, base.height) != (src.width, src.height) {
        return;
    }
    for (b, s) in base
        .pixels
        .chunks_exact_mut(4)
        .zip(src.pixels.chunks_exact(4))
    {
        let inv = 255 - s[3] as u32;
        for c in 0..3 {
            b[c] = (s[c] as u32 + (b[c] as u32 * inv + 127) / 255).min(255) as u8;
        }
        b[3] = (s[3] as u32 + (b[3] as u32 * inv + 127) / 255).min(255) as u8;
    }
}

pub struct Stage {
    pub movies: Vec<UiMovie>,
    pub bridge: GameBridge,
    gpu: Gpu,
    lookup: ResourceLookup,
    images: ImageSource,
    size: (u32, u32),
}

impl Stage {
    pub fn new(gpu: Gpu, vfs: &Arc<Vfs>, size: (u32, u32)) -> Self {
        let lookup = vfs_lookup(vfs);
        let atlases = vfs
            .names()
            .into_iter()
            .filter(|n| n.starts_with("atl_") && n.ends_with("_dat.xml"));
        let images = ImageSource::new(lookup.clone(), atlases);
        log::info!("ui: {} atlas images indexed", images.len());
        Self {
            movies: Vec::new(),
            bridge: GameBridge::new(talk_table(vfs)),
            gpu,
            lookup,
            images,
            size,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Loads a movie (`mainmenu.gfx`) on top of the others and starts it.
    pub fn open(&mut self, name: &str) -> Result<(), UiError> {
        let (swf, rep) = crate::load_movie(name, &self.lookup, &mut self.images)?;
        log::info!(
            "ui: {name}: {} images inlined, {} missing, Scaleform tags dropped {:?}",
            rep.images_inlined.len(),
            rep.images_missing.len(),
            rep.dropped
        );
        for m in &rep.images_missing {
            log::warn!("ui: {name}: image {m} not found");
        }
        let mut m = UiMovie::new(
            &self.gpu,
            name,
            &swf,
            self.size,
            self.lookup.clone(),
            Some(self.bridge.responder()),
        )?;
        // Run until the injected frame-one action has registered its callback (class
        // definitions and InitMovie run first), then start the movie as the game does.
        for _ in 0..5 {
            if !m.callbacks().is_empty() {
                break;
            }
            m.run_frame();
        }
        m.call(START_SCENE, &[]);
        self.movies.push(m);
        Ok(())
    }

    /// Opens several movies, logging the ones that fail.
    pub fn open_all(&mut self, names: &[String]) {
        for n in names {
            if let Err(e) = self.open(n) {
                log::warn!("ui: {n}: {e}");
            }
        }
    }

    pub fn close(&mut self, name: &str) -> bool {
        let before = self.movies.len();
        self.movies.retain(|m| m.name != name);
        before != self.movies.len()
    }

    /// Advances every movie; returns the messages meant for the rest of the engine.
    pub fn tick(&mut self, dt_ms: f64) -> Vec<GameMessage> {
        let mut out = Vec::new();
        for m in &mut self.movies {
            m.tick(dt_ms);
            out.extend(
                m.drain_calls()
                    .into_iter()
                    .filter_map(|c| self.bridge.handle(c)),
            );
            for line in m.drain_log() {
                log::debug!("ui: {}: {line}", m.name);
            }
        }
        out
    }

    pub fn resize(&mut self, size: (u32, u32)) {
        self.size = size;
        for m in &mut self.movies {
            m.resize(size);
        }
    }

    pub fn mouse_move(&mut self, x: f64, y: f64) {
        for m in &mut self.movies {
            m.mouse_move(x, y);
        }
    }

    pub fn mouse_button(&mut self, x: f64, y: f64, down: bool) {
        for m in &mut self.movies {
            m.mouse_button(x, y, down);
        }
    }

    /// Loads that failed, by movie.
    pub fn failed_loads(&self) -> Vec<(String, String)> {
        self.movies
            .iter()
            .flat_map(|m| {
                m.fetches()
                    .into_iter()
                    .filter(|(_, ok)| !ok)
                    .map(|(u, _)| (m.name.clone(), u))
            })
            .collect()
    }

    /// Renders every movie and blends the frames over `base` (straight RGBA) on the CPU.
    pub fn capture_over(&mut self, base: &mut Rgba) -> Result<(), UiError> {
        for m in &mut self.movies {
            m.render();
            let frame = m
                .capture()
                .ok_or_else(|| UiError::Render(format!("capturing {}", m.name)))?;
            blend_premultiplied(base, &frame);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_movies() {
        assert_eq!(movie_names("menu"), vec!["mainmenu.gfx"]);
        assert_eq!(
            movie_names("Quickbar, x.swf,"),
            vec!["quickbar.gfx", "x.swf"]
        );
        assert_eq!(movie_names("hud").len(), 3);
    }

    #[test]
    fn blends_premultiplied() {
        let mut base = Rgba {
            width: 1,
            height: 1,
            pixels: vec![200, 100, 0, 255],
        };
        let src = Rgba {
            width: 1,
            height: 1,
            pixels: vec![50, 0, 0, 128],
        };
        blend_premultiplied(&mut base, &src);
        assert_eq!(base.pixels, vec![50 + 100, 50, 0, 255]);
    }
}
