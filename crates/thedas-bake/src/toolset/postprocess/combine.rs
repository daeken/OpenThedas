use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};

use super::image::{Image, pixel_count};

pub(super) fn run(lm: &Path, sm: &Path, ao: &Path, out: &Path) -> Result<()> {
    fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    walk(lm, sm, ao, out)
}

fn walk(lm: &Path, sm: &Path, ao: &Path, out: &Path) -> Result<()> {
    for entry in fs::read_dir(lm).with_context(|| format!("reading {}", lm.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name();
        if file_type.is_dir() {
            walk(
                &entry.path(),
                &sm.join(&name),
                &ao.join(&name),
                &out.join(&name),
            )?;
        } else if file_type.is_file() {
            let Some(name) = name.to_str() else { continue };
            let Some((prefix, ext)) = name.rsplit_once('.') else {
                continue;
            };
            let Some((stem, tag)) = prefix.rsplit_once('.') else {
                continue;
            };
            if !tag.eq_ignore_ascii_case("StandardLightMap") || stem.is_empty() {
                continue;
            }
            let shadow = matching_file(sm, &format!("{stem}.ShadowMap.{ext}"))?;
            let occlusion = matching_file(ao, &format!("{stem}.AmbientOcclusionMap.{ext}"))?;
            let lightmap = Image::read_tga(&entry.path())?;
            let shadow = Image::read_tga(&shadow)?;
            let occlusion = Image::read_tga(&occlusion)?;
            let (width, height) = combined_size(&lightmap, &shadow, &occlusion);
            let image = merge(&lightmap, &shadow, &occlusion)
                .with_context(|| format!("combining {stem}"))?;
            fs::create_dir_all(out)?;
            let output = out.join(format!("{stem}.dds"));
            image.write_bgra_dds(&output, false)?;
            super::write_texture_meta(&output, width, height, "BakedMapProcessor")?;
        }
    }
    Ok(())
}

pub(super) fn matching_file(dir: &Path, name: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    if path.is_file() {
        return Ok(path);
    }
    let mut found = None;
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(name)
        {
            ensure!(
                found.is_none(),
                "ambiguous filename {name} in {}",
                dir.display()
            );
            found = Some(entry.path());
        }
    }
    found.with_context(|| format!("missing {name} in {}", dir.display()))
}

fn combined_size(light: &Image, shadow: &Image, occlusion: &Image) -> (u32, u32) {
    (
        4.max(light.width).max(shadow.width).max(occlusion.width),
        4.max(light.height).max(shadow.height).max(occlusion.height),
    )
}

fn merge(lm: &Image, sm: &Image, ao: &Image) -> Result<Image> {
    let (width, height) = combined_size(lm, sm, ao);
    let mut pixels = Vec::with_capacity(pixel_count(width, height)?);
    let sample = |image: &Image, x: u32, y: u32| -> Result<[u8; 4]> {
        let nearest = |at: u32, source: u32, dest: u32| {
            (2 * u64::from(at) * u64::from(source) + u64::from(dest)) / (2 * u64::from(dest))
        };
        let x = nearest(x, image.width, width);
        let y = nearest(y, image.height, height);
        // The original uses a flat index: rounding X to the width can
        // read into the next row, as long as it remains inside the image.
        let index = y * u64::from(image.width) + x;
        image
            .pixels
            .get(index as usize)
            .copied()
            .context("nearest sample lies beyond the source allocation")
    };
    for y in 0..height {
        for x in 0..width {
            let [r, g, b, _] = sample(lm, x, y)?;
            let [a, ..] = sample(sm, x, y)?;
            let [occlusion, ..] = sample(ao, x, y)?;
            // BakedMapProcessor divides AO before multiplying each channel and
            // truncates the product. Rounding the division can cross an integer boundary.
            let occlusion_scale = f64::from(occlusion) / 255.0;
            pixels.push([r, g, b, a].map(|value| (f64::from(value) * occlusion_scale) as u8));
        }
    }
    let first = pixels[0];
    let near_constant = pixels.iter().all(|pixel| {
        pixel
            .iter()
            .zip(first)
            .all(|(&value, original)| value.abs_diff(original) <= 2)
    });
    let image = Image::new(width, height, pixels)?;
    if near_constant {
        image.resize_box(4, 4)
    } else {
        Ok(image)
    }
}
