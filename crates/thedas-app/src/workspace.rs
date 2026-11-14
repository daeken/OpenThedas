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
//! The textures are written uncompressed (8-bit BGRA with a full mip chain), which the game
//! reads as it reads the originals' compressed ones. Mounted only by mod mode's "Live
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

/// Imports a saved `.glb` beside its `manifest.json`, writing the meshes into `overrides`
/// and a note in `.imported`. Returns the `.msh` files written, and what came of each with
/// the warnings (or the error).
pub fn import_glb(
    glb: &Path,
    overrides: &Path,
    read: &dyn Fn(&str) -> Option<crate::glb::Read>,
) -> Result<(Vec<String>, Vec<String>), String> {
    let dir = glb.parent().ok_or("no folder")?;
    let result = (|| {
        let manifest: serde_json::Value = std::fs::read(dir.join("manifest.json"))
            .map_err(|e| format!("manifest.json: {e}"))
            .and_then(|b| serde_json::from_slice(&b).map_err(|e| format!("manifest.json: {e}")))?;
        let bytes = std::fs::read(glb).map_err(|e| format!("{}: {e}", glb.display()))?;
        let imp = crate::glb_in::import(&bytes, &manifest, read)?;
        std::fs::create_dir_all(overrides).map_err(|e| e.to_string())?;
        let mut written = Vec::new();
        let mut warnings = imp.warnings;
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
        Ok((written, warnings))
    })();
    // Noted whatever came of it, so a file that fails is not tried again until saved again.
    let note = match &result {
        Ok((w, warn)) => serde_json::json!({"written": w, "warnings": warn}),
        Err(e) => serde_json::json!({"error": e}),
    };
    let _ = std::fs::write(
        dir.join(".imported"),
        serde_json::to_vec_pretty(&note).unwrap_or_default(),
    );
    result
}

/// Writes each change's texture into `overrides`. Returns the textures written, and what
/// failed.
pub fn apply(changes: &[Change], overrides: &Path) -> (Vec<String>, Vec<String>) {
    let (mut done, mut failed) = (Vec::new(), Vec::new());
    for c in changes {
        let made = std::fs::read(&c.png)
            .map_err(|e| e.to_string())
            .and_then(|b| png_to_dds(&b))
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

/// A PNG as an uncompressed DDS: 8-bit BGRA, with every mip down to 1x1 (each a box filter
/// of the one above).
pub fn png_to_dds(png: &[u8]) -> Result<Vec<u8>, String> {
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
    Ok(dds_bgra(w, h, rgba))
}

/// An uncompressed DDS (A8R8G8B8) of an RGBA image, with its mip chain.
fn dds_bgra(w: u32, h: u32, rgba: Vec<u8>) -> Vec<u8> {
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
    let mut out = Vec::with_capacity(128 + mips.iter().map(|m| m.2.len()).sum::<usize>());
    let mut u = |v: u32| out.extend_from_slice(&v.to_le_bytes());
    // Header: caps, height, width, pitch, pixel format, mip count.
    u(u32::from_le_bytes(*b"DDS "));
    u(124);
    u(0x1 | 0x2 | 0x4 | 0x8 | 0x1000 | 0x2_0000);
    u(h);
    u(w);
    u(w * 4);
    u(0);
    u(mips.len() as u32);
    for _ in 0..11 {
        u(0);
    }
    u(32);
    u(0x40 | 0x1); // DDPF_RGB | DDPF_ALPHAPIXELS
    u(0);
    u(32);
    u(0x00ff_0000);
    u(0x0000_ff00);
    u(0x0000_00ff);
    u(0xff00_0000);
    u(0x1000 | 0x8 | 0x40_0000); // texture, complex, mipmap
    for _ in 0..4 {
        u(0);
    }
    for (_, _, p) in mips {
        out.extend(p.chunks_exact(4).flat_map(|c| [c[2], c[1], c[0], c[3]]));
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
    fn a_png_becomes_a_dds_the_game_reads() {
        let rgba: Vec<u8> = (0..4 * 2 * 4).map(|i| (i * 7) as u8).collect();
        let dds = png_to_dds(&png(4, 2, &rgba)).unwrap();
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
        let (done, failed) = apply(&c, &overrides);
        assert_eq!((done, failed.len()), (vec!["t_0d.dds".to_string()], 0));
        set(&overrides.join("t_0d.dds"), 3000);
        assert!(changes(&blender, &overrides).is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }
}
