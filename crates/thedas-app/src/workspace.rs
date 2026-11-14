//! Mod mode's live edits: the workspace folder (`workspace/` in the player's data folder)
//! mounted above every mod while the player asks for it, and the textures of objects
//! exported for Blender (see [`crate::glb`]) written back into it as they are saved.
//!
//! - `workspace/override/`: loose files that beat every layer of the game's stack (see
//!   [`thedas_vfs::Vfs::set_workspace`]). It is an ordinary mod folder: what is in it can be
//!   shipped as a mod as it is.
//! - `workspace/blender/<tag>/`: an export: `<tag>.glb`, its textures as PNGs and
//!   `manifest.json`. A PNG saved after its export (or after the texture it last made) is
//!   turned into the texture it came from, under the original's name, in `override/`. The
//!   `.glb` saved after its export (or its last import) is imported (see [`crate::glb_in`]):
//!   its meshes become the `.msh` files they came from, in `override/`, and `.imported`
//!   in the export's folder says when and what came of it.
//!
//! Textures are written in the original's own format where it is block-compressed (DXT1,
//! DXT3, DXT5, by `texpresso`), else uncompressed (8-bit BGRA), with a full mip chain, so
//! the game reads them as it reads the originals (see [`encode_dds`]). Mounted only by mod mode's "Live
//! edits" switch or the `--mod-workspace` launch option, never because the folder has files;
//! not with mods switched off (`THEDAS_MODS=none`).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use thedas_vfs::{Vfs, WORKSPACE_LABEL};

/// The workspace folder.
pub fn dir() -> PathBuf {
    crate::saves::data_dir().join("workspace")
}

/// Where live edits go: the folder mounted above every mod.
pub fn override_dir() -> PathBuf {
    dir().join("override")
}

/// Where exports for Blender go, one folder per object.
pub fn blender_dir() -> PathBuf {
    dir().join("blender")
}

/// Mounts the workspace (`on`) or takes it down. Says what it did, or why not.
pub fn mount(on: bool) -> Result<String, String> {
    if !on {
        Vfs::set_workspace(None);
        log::info!("mods: live edits unmounted");
        return Ok("Live edits off".into());
    }
    if thedas_vfs::mods::default_list_file().is_none() {
        return Err("Live edits are off while mods are switched off (THEDAS_MODS)".into());
    }
    let d = override_dir();
    std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    let n = Vfs::set_workspace(Some(&d));
    log::info!(
        "mods: live edits mounted: {WORKSPACE_LABEL} ({}) with {n} file(s), above every mod",
        d.display()
    );
    Ok(format!("Live edits on: {n} file(s) in {}", d.display()))
}

/// Whether live edits are mounted.
pub fn mounted() -> bool {
    Vfs::workspace().is_some()
}

/// A PNG of an export that is newer than the texture it last made.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub png: PathBuf,
    /// The texture resource it stands for (`*.dds`).
    pub texture: String,
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// The exports' PNGs saved since their texture was last made (or, for one never made,
/// since the export). Reads each export's `manifest.json` for which texture each PNG is.
pub fn changes(blender: &Path, overrides: &Path) -> Vec<Change> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(blender) else {
        return out;
    };
    for export in rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let manifest = export.join("manifest.json");
        let Some(exported) = modified(&manifest) else {
            continue;
        };
        let Ok(m) = std::fs::read(&manifest)
            .map_err(|_| ())
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).map_err(|_| ()))
        else {
            continue;
        };
        for t in m["materials"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|mat| mat["textures"].as_array().into_iter().flatten())
        {
            let (Some(png), Some(texture)) = (t["png"].as_str(), t["texture"]["resource"].as_str())
            else {
                continue;
            };
            // Names from the manifest stay inside their folders.
            if png.contains(['/', '\\']) || texture.contains(['/', '\\']) || png.starts_with('.') {
                continue;
            }
            let path = export.join(png);
            let Some(saved) = modified(&path) else {
                continue;
            };
            let since = modified(&overrides.join(texture)).unwrap_or(exported);
            if saved > since && !out.iter().any(|c: &Change| c.texture == texture) {
                out.push(Change {
                    png: path,
                    texture: texture.to_ascii_lowercase(),
                });
            }
        }
    }
    out
}

/// Exports whose `.glb` was saved since the export or since its last import.
pub fn glb_changes(blender: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(blender) else {
        return out;
    };
    for export in rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let Some(exported) = modified(&export.join("manifest.json")) else {
            continue;
        };
        let since = modified(&export.join(".imported"))
            .unwrap_or(exported)
            .max(exported);
        let Ok(files) = std::fs::read_dir(&export) else {
            continue;
        };
        for f in files.flatten().map(|e| e.path()) {
            let glb = f.extension().is_some_and(|e| e.eq_ignore_ascii_case("glb"));
            if glb && modified(&f).is_some_and(|m| m > since) {
                out.push(f);
                break;
            }
        }
    }
    out
}

/// What importing a saved `.glb` did.
pub struct GlbImport {
    /// The `.msh` files written.
    pub written: Vec<String>,
    /// What came of each, and the warnings.
    pub notes: Vec<String>,
    /// The object moved in the file: its id and tag as exported, and its new place.
    pub moved: Option<(u32, String, crate::glb_in::Placed)>,
}

/// Imports a saved `.glb` beside its `manifest.json`, writing the meshes that changed into
/// `overrides` and a note in `.imported` (or the error).
pub fn import_glb(
    glb: &Path,
    overrides: &Path,
    read: &dyn Fn(&str) -> Option<crate::glb::Read>,
    format_of: &dyn Fn(&str) -> Option<thedas_formats::dds::DdsFormat>,
) -> Result<GlbImport, String> {
    let dir = glb.parent().ok_or("no folder")?;
    let result = (|| {
        let manifest: serde_json::Value = std::fs::read(dir.join("manifest.json"))
            .map_err(|e| format!("manifest.json: {e}"))
            .and_then(|b| serde_json::from_slice(&b).map_err(|e| format!("manifest.json: {e}")))?;
        let bytes = std::fs::read(glb).map_err(|e| format!("{}: {e}", glb.display()))?;
        // Files beside the export: its PNGs, and images an editor saved beside it.
        let files = |name: &str| -> Option<Vec<u8>> {
            if name.contains(['/', '\\']) || name.starts_with('.') {
                return None;
            }
            std::fs::read(dir.join(name)).ok()
        };
        let imp = crate::glb_in::import(&bytes, &manifest, read, &files, false)?;
        std::fs::create_dir_all(overrides).map_err(|e| e.to_string())?;
        let mut written = Vec::new();
        let mut warnings = imp.warnings;
        let safe = |n: &str| !n.contains(['/', '\\']) && !n.starts_with('.') && !n.is_empty();
        for t in imp.textures {
            if !safe(&t.resource) {
                continue;
            }
            let encoding = Encoding::like(format_of(&t.like));
            let dds = encode_dds(t.width, t.height, t.rgba, encoding);
            std::fs::write(overrides.join(&t.resource), dds).map_err(|e| e.to_string())?;
            warnings.push(format!("{}: painted ({encoding:?})", t.resource));
            written.push(t.resource);
        }
        for (file, data) in &imp.materials {
            if !safe(file) {
                continue;
            }
            std::fs::write(overrides.join(file), data).map_err(|e| e.to_string())?;
            warnings.push(format!("{file}: a new material"));
            written.push(file.clone());
        }
        for m in &imp.models {
            if m.mmh.contains(['/', '\\']) || m.mmh.starts_with('.') {
                continue;
            }
            std::fs::write(overrides.join(&m.mmh), &m.data).map_err(|e| e.to_string())?;
            written.push(m.mmh.clone());
            warnings.push(format!("{}: {}", m.mmh, m.changes.join("; ")));
        }
        for m in &imp.meshes {
            if m.msh.contains(['/', '\\']) || m.msh.starts_with('.') {
                continue;
            }
            std::fs::write(overrides.join(&m.msh), &m.data).map_err(|e| e.to_string())?;
            written.push(m.msh.clone());
            warnings.push(format!(
                "{}: {} chunk(s) rewritten{}",
                m.msh,
                m.chunks.len(),
                if m.emptied.is_empty() {
                    String::new()
                } else {
                    format!(", {} emptied ({})", m.emptied.len(), m.emptied.join(", "))
                }
            ));
        }
        let object = &manifest["object"];
        let moved = imp.moved.map(|p| {
            (
                object["id"].as_u64().unwrap_or(0) as u32,
                object["tag"].as_str().unwrap_or("").to_string(),
                p,
            )
        });
        Ok(GlbImport {
            written,
            notes: warnings,
            moved,
        })
    })();
    // Noted whatever came of it, so a file that fails is not tried again until saved again.
    let note = match &result {
        Ok(r) => serde_json::json!({
            "written": r.written,
            "warnings": r.notes,
            "moved": r.moved.as_ref().map(|(_, _, p)| serde_json::json!({
                "position": p.position, "facing": p.facing,
            })),
        }),
        Err(e) => serde_json::json!({"error": e}),
    };
    let _ = std::fs::write(
        dir.join(".imported"),
        serde_json::to_vec_pretty(&note).unwrap_or_default(),
    );
    result
}

/// Writes each change's texture into `overrides`, in the format `format_of` gives for the
/// texture it replaces (the original's). Returns the textures written, and what failed.
pub fn apply(
    changes: &[Change],
    overrides: &Path,
    format_of: &dyn Fn(&str) -> Option<thedas_formats::dds::DdsFormat>,
) -> (Vec<String>, Vec<String>) {
    let (mut done, mut failed) = (Vec::new(), Vec::new());
    for c in changes {
        let target = Encoding::like(format_of(&c.texture));
        let made = std::fs::read(&c.png)
            .map_err(|e| e.to_string())
            .and_then(|b| png_to_dds(&b, target))
            .and_then(|dds| {
                std::fs::create_dir_all(overrides).map_err(|e| e.to_string())?;
                std::fs::write(overrides.join(&c.texture), dds).map_err(|e| e.to_string())
            });
        match made {
            Ok(()) => done.push(c.texture.clone()),
            Err(e) => failed.push(format!("{}: {e}", c.png.display())),
        }
    }
    (done, failed)
}

/// How a texture is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// Uncompressed 8-bit BGRA.
    Bgra8,
    /// Block-compressed: DXT1, DXT3, DXT5.
    Bc1,
    Bc2,
    Bc3,
}

impl Encoding {
    /// The encoding for a texture that replaces one of `format` (uncompressed for formats
    /// not block-compressed, or none known).
    pub fn like(format: Option<thedas_formats::dds::DdsFormat>) -> Encoding {
        use thedas_formats::dds::DdsFormat as F;
        match format {
            Some(F::Bc1) => Encoding::Bc1,
            Some(F::Bc2) => Encoding::Bc2,
            Some(F::Bc3) => Encoding::Bc3,
            _ => Encoding::Bgra8,
        }
    }
}

/// A PNG as a DDS of `encoding`, with every mip down to 1x1 (each a box filter of the one
/// above).
pub fn png_to_dds(png: &[u8], encoding: Encoding) -> Result<Vec<u8>, String> {
    let (w, h, rgba) = decode_png(png)?;
    Ok(encode_dds(w, h, rgba, encoding))
}

/// A PNG's pixels as 8-bit RGBA: (width, height, pixels).
pub fn decode_png(png: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let size = reader
        .output_buffer_size()
        .ok_or("the image is too large")?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width, info.height);
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return Err(format!("{w}x{h} is not a texture size"));
    }
    let px = (w * h) as usize;
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf[..px * 4].to_vec(),
        png::ColorType::Rgb => buf[..px * 3]
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf[..px * 2]
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => buf[..px].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("an indexed PNG was not expanded".into()),
    };
    Ok((w, h, rgba))
}

/// An RGBA image as a DDS of `encoding`, with its mip chain: the one place textures are
/// encoded (block compression by `texpresso`, its cluster fit).
pub fn encode_dds(w: u32, h: u32, rgba: Vec<u8>, encoding: Encoding) -> Vec<u8> {
    let mut mips = vec![(w, h, rgba)];
    while let Some((pw, ph, p)) = mips.last().filter(|(pw, ph, _)| *pw > 1 || *ph > 1) {
        let (nw, nh) = ((pw / 2).max(1), (ph / 2).max(1));
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let mut sum = 0u32;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let sx = (x * 2 + dx).min(pw - 1);
                        let sy = (y * 2 + dy).min(ph - 1);
                        sum += p[((sy * pw + sx) * 4 + c) as usize] as u32;
                    }
                    next[((y * nw + x) * 4 + c) as usize] = (sum / 4) as u8;
                }
            }
        }
        mips.push((nw, nh, next));
    }
    let block = match encoding {
        Encoding::Bgra8 => None,
        Encoding::Bc1 => Some((texpresso::Format::Bc1, *b"DXT1")),
        Encoding::Bc2 => Some((texpresso::Format::Bc2, *b"DXT3")),
        Encoding::Bc3 => Some((texpresso::Format::Bc3, *b"DXT5")),
    };
    let levels: Vec<Vec<u8>> = mips
        .into_iter()
        .map(|(mw, mh, p)| match block {
            Some((f, _)) => {
                let mut o = vec![0u8; f.compressed_size(mw as usize, mh as usize)];
                f.compress(
                    &p,
                    mw as usize,
                    mh as usize,
                    texpresso::Params::default(),
                    &mut o,
                );
                o
            }
            None => p
                .chunks_exact(4)
                .flat_map(|c| [c[2], c[1], c[0], c[3]])
                .collect(),
        })
        .collect();
    let mut out = Vec::with_capacity(128 + levels.iter().map(Vec::len).sum::<usize>());
    let mut u = |v: u32| out.extend_from_slice(&v.to_le_bytes());
    // Header: caps, height, width, pitch (or the top level's size), pixel format, mips.
    u(u32::from_le_bytes(*b"DDS "));
    u(124);
    let size_flag = if block.is_some() { 0x8_0000 } else { 0x8 };
    u(0x1 | 0x2 | 0x4 | 0x1000 | 0x2_0000 | size_flag);
    u(h);
    u(w);
    u(match block {
        Some(_) => levels[0].len() as u32,
        None => w * 4,
    });
    u(0);
    u(levels.len() as u32);
    for _ in 0..11 {
        u(0);
    }
    u(32);
    match block {
        Some((_, fourcc)) => {
            u(0x4); // DDPF_FOURCC
            u(u32::from_le_bytes(fourcc));
            for _ in 0..5 {
                u(0);
            }
        }
        None => {
            u(0x40 | 0x1); // DDPF_RGB | DDPF_ALPHAPIXELS
            u(0);
            u(32);
            u(0x00ff_0000);
            u(0x0000_ff00);
            u(0x0000_00ff);
            u(0xff00_0000);
        }
    }
    u(0x1000 | 0x8 | 0x40_0000); // texture, complex, mipmap
    for _ in 0..4 {
        u(0);
    }
    for l in levels {
        out.extend(l);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
        thedas_ui::stage::encode_png(&thedas_ui::Rgba {
            width: w,
            height: h,
            pixels: rgba.to_vec(),
        })
        .unwrap()
    }

    #[test]
    fn block_compressed_textures_keep_their_look() {
        // A gradient with an alpha ramp, 8x8.
        let rgba: Vec<u8> = (0..64u32)
            .flat_map(|i| [(i * 4) as u8, (255 - i * 4) as u8, 128, (i * 4) as u8])
            .collect();
        for (enc, fourcc, alpha) in [
            (Encoding::Bc1, b"DXT1", false),
            (Encoding::Bc2, b"DXT3", true),
            (Encoding::Bc3, b"DXT5", true),
        ] {
            // DXT1 keeps one bit of alpha: given opaque pixels.
            let rgba: Vec<u8> = if alpha {
                rgba.clone()
            } else {
                rgba.chunks_exact(4)
                    .flat_map(|p| [p[0], p[1], p[2], 255])
                    .collect()
            };
            let dds = encode_dds(8, 8, rgba.clone(), enc);
            assert_eq!(&dds[84..88], fourcc);
            let d = thedas_formats::dds::Dds::parse(&dds).unwrap();
            assert_eq!((d.width, d.height, d.mip_count), (8, 8, 4));
            let back = d.decode_rgba8(0, 0).unwrap();
            let worst = back
                .chunks_exact(4)
                .zip(rgba.chunks_exact(4))
                .flat_map(|(a, b)| (0..if alpha { 4 } else { 3 }).map(move |k| a[k].abs_diff(b[k])))
                .max()
                .unwrap();
            assert!(worst <= 24, "{enc:?}: off by {worst}");
            // Every mip decodes.
            for m in 0..d.mip_count {
                d.decode_rgba8(0, m).unwrap();
            }
        }
        assert_eq!(
            Encoding::like(Some(thedas_formats::dds::DdsFormat::Bc3)),
            Encoding::Bc3
        );
        assert_eq!(
            Encoding::like(Some(thedas_formats::dds::DdsFormat::L8)),
            Encoding::Bgra8
        );
    }

    #[test]
    fn a_png_becomes_a_dds_the_game_reads() {
        let rgba: Vec<u8> = (0..4 * 2 * 4).map(|i| (i * 7) as u8).collect();
        let dds = png_to_dds(&png(4, 2, &rgba), Encoding::Bgra8).unwrap();
        let d = thedas_formats::dds::Dds::parse(&dds).unwrap();
        assert_eq!((d.width, d.height, d.mip_count), (4, 2, 3));
        assert_eq!(d.decode_rgba8(0, 0).unwrap(), rgba);
        // The last mip is the average of the whole image, channel by channel.
        assert_eq!(d.decode_rgba8(0, 2).unwrap().len(), 4);
    }

    #[test]
    fn only_pngs_saved_after_their_export_or_texture_count() {
        let base = std::env::temp_dir().join(format!("thedas-ws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (blender, overrides) = (base.join("blender"), base.join("override"));
        let export = blender.join("x");
        std::fs::create_dir_all(&export).unwrap();
        std::fs::create_dir_all(&overrides).unwrap();
        let p = png(1, 1, &[1, 2, 3, 4]);
        std::fs::write(export.join("t_0d.png"), &p).unwrap();
        std::fs::write(export.join("evil.png"), &p).unwrap();
        let manifest = serde_json::json!({"materials": [{"textures": [
            {"png": "t_0d.png", "texture": {"resource": "t_0d.dds"}},
            {"png": "../evil.png", "texture": {"resource": "x.dds"}},
        ]}]});
        let set = |p: &Path, secs: u64| {
            let f = std::fs::File::options().write(true).open(p).unwrap();
            f.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs))
                .unwrap();
        };
        std::fs::write(export.join("manifest.json"), manifest.to_string()).unwrap();
        set(&export.join("manifest.json"), 1000);
        set(&export.join("t_0d.png"), 1000);
        assert!(changes(&blender, &overrides).is_empty());
        set(&export.join("t_0d.png"), 2000);
        let c = changes(&blender, &overrides);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].texture, "t_0d.dds");
        let (done, failed) = apply(&c, &overrides, &|_| None);
        assert_eq!((done, failed.len()), (vec!["t_0d.dds".to_string()], 0));
        set(&overrides.join("t_0d.dds"), 3000);
        assert!(changes(&blender, &overrides).is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }
}
