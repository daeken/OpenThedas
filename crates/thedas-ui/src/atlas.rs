//! GUI texture atlases (`atl_*_dat.xml` with `atl_*_NN.dds` pages) that hold the images
//! Scaleform movies reference by file name through `DefineExternalImage`.
//!
//! Each `SourceTexture` names an original image and gives `OffsetAndScale` (u, v, width, height
//! as fractions of the atlas page) and `OriginalImageSize`. The crop uses the page offset and the
//! original size in pixels. Provisional: inferred from the shipped atlas files.

use std::collections::HashMap;
use std::sync::Arc;

use thedas_formats::dds::Dds;

use crate::gfx::Rgba;
use crate::navigator::ResourceLookup;

#[derive(Clone, Debug, PartialEq)]
pub struct AtlasEntry {
    pub page: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

fn pair(s: &str) -> Option<(f64, f64)> {
    let mut it = s.split_whitespace().map(|x| x.parse::<f64>().ok());
    Some((it.next()??, it.next()??))
}

/// Parses one atlas description, returning image name (lower-case) to entry.
pub fn parse_atlas_xml(text: &str) -> Vec<(String, AtlasEntry)> {
    let Ok(doc) = roxmltree::Document::parse(text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for page in doc.descendants().filter(|n| n.has_tag_name("AtlasTexture")) {
        let Some(page_name) = page.attribute("Name") else {
            continue;
        };
        for src in page.children().filter(|n| n.has_tag_name("SourceTexture")) {
            let Some(name) = src.attribute("Name") else {
                continue;
            };
            let off = src.attribute("OffsetAndScale").and_then(|s| {
                let v: Vec<f64> = s
                    .split_whitespace()
                    .filter_map(|x| x.parse().ok())
                    .collect();
                (v.len() == 4).then(|| (v[0], v[1]))
            });
            let (Some((u, v)), Some((aw, ah)), Some((w, h))) = (
                off,
                src.attribute("AtlasSize").and_then(pair),
                src.attribute("OriginalImageSize").and_then(pair),
            ) else {
                continue;
            };
            out.push((
                name.to_ascii_lowercase(),
                AtlasEntry {
                    page: page_name.to_ascii_lowercase(),
                    x: (u * aw).round().max(0.0) as u32,
                    y: (v * ah).round().max(0.0) as u32,
                    width: w.max(0.0) as u32,
                    height: h.max(0.0) as u32,
                },
            ));
        }
    }
    out
}

/// Resolves GUI images by name from atlases, falling back to a standalone DDS.
pub struct ImageSource {
    lookup: ResourceLookup,
    entries: HashMap<String, AtlasEntry>,
    pages: HashMap<String, Option<Arc<Rgba>>>,
}

impl ImageSource {
    /// `atlas_names` are the `atl_*_dat.xml` resources to index.
    pub fn new(lookup: ResourceLookup, atlas_names: impl IntoIterator<Item = String>) -> Self {
        let mut entries = HashMap::new();
        for name in atlas_names {
            let Some(bytes) = lookup(&name) else { continue };
            let text = String::from_utf8_lossy(&bytes);
            for (k, e) in parse_atlas_xml(&text) {
                // DXT5 pages carry alpha; prefer them when an image is in both.
                let better = e.page.contains("dxt5")
                    || !entries
                        .get(&k)
                        .is_some_and(|o: &AtlasEntry| o.page.contains("dxt5"));
                if better {
                    entries.insert(k, e);
                }
            }
        }
        Self {
            lookup,
            entries,
            pages: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn decode(&self, name: &str) -> Option<Rgba> {
        let bytes = (self.lookup)(name)?;
        let dds = Dds::parse(&bytes).ok()?;
        let pixels = dds.decode_rgba8(0, 0).ok()?;
        Some(Rgba {
            width: dds.width,
            height: dds.height,
            pixels,
        })
    }

    fn page(&mut self, page: &str) -> Option<Arc<Rgba>> {
        if !self.pages.contains_key(page) {
            let img = self.decode(page).map(Arc::new);
            self.pages.insert(page.to_string(), img);
        }
        self.pages.get(page).cloned().flatten()
    }

    pub fn image(&mut self, name: &str) -> Option<Rgba> {
        let key = name.to_ascii_lowercase();
        let Some(e) = self.entries.get(&key).cloned() else {
            return self.decode(&key);
        };
        let page = self.page(&e.page)?;
        crop(&page, e.x, e.y, e.width, e.height)
    }
}

pub fn crop(img: &Rgba, x: u32, y: u32, w: u32, h: u32) -> Option<Rgba> {
    if x.checked_add(w)? > img.width || y.checked_add(h)? > img.height {
        return None;
    }
    let mut pixels = Vec::with_capacity(w as usize * h as usize * 4);
    for row in y..y + h {
        let at = (row as usize * img.width as usize + x as usize) * 4;
        pixels.extend_from_slice(img.pixels.get(at..at + w as usize * 4)?);
    }
    Some(Rgba {
        width: w,
        height: h,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_atlas_entries() {
        let xml = r#"<?xml version="1.0"?><AtlasData>
            <AtlasTexture Name="atl_x_dxt5_00.dds">
              <SourceTexture Name="Menu_I1.dds" OffsetAndScale="0.000488 0.25 0.5 0.1"
                 AtlasSize="2048 2048" OriginalImageSize="294 57"/>
              <SourceTexture Name="broken.dds" OffsetAndScale="x"/>
            </AtlasTexture></AtlasData>"#;
        let e = parse_atlas_xml(xml);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].0, "menu_i1.dds");
        assert_eq!(
            e[0].1,
            AtlasEntry {
                page: "atl_x_dxt5_00.dds".into(),
                x: 1,
                y: 512,
                width: 294,
                height: 57
            }
        );
        assert!(parse_atlas_xml("<not xml").is_empty());
    }

    #[test]
    fn crops_in_bounds_only() {
        let img = Rgba {
            width: 2,
            height: 2,
            pixels: (0..16).collect(),
        };
        let c = crop(&img, 1, 1, 1, 1).unwrap();
        assert_eq!(c.pixels, vec![12, 13, 14, 15]);
        assert!(crop(&img, 1, 1, 2, 1).is_none());
        assert!(crop(&img, u32::MAX, 0, 2, 1).is_none());
    }
}
