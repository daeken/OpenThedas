//! `thedas tree`: compute a SpeedTree (`.spt`) with the clean-room generator and export it as
//! OBJ or render flat-shaded side and top views to PNG, for comparing shapes and density with
//! the original's screenshots.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use thedas_formats::speedtree_gen::mesh::TreeMesh;

type V3 = [f32; 3];

/// Untextured colours per surface kind.
const BARK: [f32; 3] = [0.45, 0.30, 0.18];
const FROND: [f32; 3] = [0.30, 0.55, 0.22];
const LEAF: [f32; 3] = [0.42, 0.72, 0.28];
const BACKGROUND: [u8; 3] = [222, 230, 238];

/// One coloured triangle.
struct Tri {
    p: [V3; 3],
    normal: V3,
    color: [f32; 3],
}

pub fn run(
    target: &str,
    lod: usize,
    obj: Option<&Path>,
    png: Option<&Path>,
    size: u32,
    scale: f32,
) -> Result<()> {
    let data = crate::load_target(target)?;
    let spt =
        thedas_formats::speedtree::parse(&data).with_context(|| format!("parsing {target}"))?;
    let mesh = TreeMesh::new(&spt);
    let g = &mesh.generated;
    let (lo, hi) = mesh.bounds;
    println!(
        "{target}: {} branches ({} with tubes), {} leaves, {} frond guides; {} branch / {} frond / {} leaf LOD levels",
        g.skeleton.branches.len(),
        g.branches.tubes.iter().flatten().count(),
        g.skeleton.leaves.len(),
        g.skeleton.guides.len(),
        mesh.branches.lods.len(),
        mesh.fronds.lods.len(),
        mesh.leaves.len(),
    );
    println!(
        "size {:.2}; bounds x {:.2}..{:.2} y {:.2}..{:.2} z {:.2}..{:.2}; height {:.2} units = {:.2} m at placement scale {scale}",
        g.skeleton.size,
        lo[0],
        hi[0],
        lo[1],
        hi[1],
        lo[2],
        hi[2],
        hi[2] - lo[2],
        (hi[2] - lo[2]) * scale
    );
    println!("fingerprint 0x{:016x}", mesh.fingerprint());
    let tris = triangles(&mesh, lod);
    println!("LOD {lod}: {} triangles", tris.len());
    if let Some(path) = obj {
        write_obj(path, &tris)?;
        println!("wrote {}", path.display());
    }
    if let Some(path) = png {
        write_png(path, &tris, size)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

/// The surfaces drawn at `lod` (levels past a part's last use its last), with leaf cards as
/// upright quads facing −Y (the side view's camera).
fn triangles(mesh: &TreeMesh, lod: usize) -> Vec<Tri> {
    let mut out = Vec::new();
    for (part, color) in [(&mesh.branches, BARK), (&mesh.fronds, FROND)] {
        let Some(list) = part.lods.get(lod.min(part.lods.len().saturating_sub(1))) else {
            continue;
        };
        for t in list.chunks_exact(3) {
            let v = [t[0], t[1], t[2]].map(|i| part.vertices[i as usize]);
            out.push(Tri {
                p: v.map(|v| v.position),
                normal: v[0].normal,
                color,
            });
        }
    }
    if let Some(cards) = mesh
        .leaves
        .get(lod.min(mesh.leaves.len().saturating_sub(1)))
    {
        for card in cards {
            let c = [0, 1, 2, 3].map(|i| card.corner(i, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]));
            let shade = card.color_scale.clamp(0.3, 1.0);
            let color = LEAF.map(|x| x * shade);
            for p in [[c[0], c[1], c[2]], [c[0], c[2], c[3]]] {
                out.push(Tri {
                    p,
                    normal: card.normals[0],
                    color,
                });
            }
        }
    }
    out
}

fn write_obj(path: &Path, tris: &[Tri]) -> Result<()> {
    let mut s = String::from("# thedas tree export (Z up)\n");
    for (name, color) in [("bark", BARK), ("frond", FROND), ("leaf", LEAF)] {
        let _ = writeln!(s, "# material {name}: {color:?}");
    }
    for t in tris {
        for p in t.p {
            let _ = writeln!(s, "v {} {} {}", p[0], p[1], p[2]);
        }
        let _ = writeln!(s, "vn {} {} {}", t.normal[0], t.normal[1], t.normal[2]);
    }
    for i in 0..tris.len() {
        let (v, n) = (3 * i + 1, i + 1);
        let _ = writeln!(s, "f {v}//{n} {}//{n} {}//{n}", v + 1, v + 2);
    }
    std::fs::write(path, s).with_context(|| format!("writing {}", path.display()))
}

/// Renders a side view (looking along +Y, X right, Z up) and a top view (looking down, X right,
/// Y up) next to each other at the same scale, with a depth buffer and two-sided flat shading.
fn write_png(path: &Path, tris: &[Tri], size: u32) -> Result<()> {
    let (w, h) = (size as usize * 2, size as usize);
    let mut image = vec![BACKGROUND; w * h];
    let mut depth = vec![f32::INFINITY; w * h];
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for t in tris {
        for p in t.p {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    if tris.is_empty() {
        (lo, hi) = ([0.0; 3], [1.0; 3]);
    }
    let span = (hi[0] - lo[0])
        .max(hi[1] - lo[1])
        .max(hi[2] - lo[2])
        .max(1e-6);
    let margin = size as f32 * 0.05;
    let k = (size as f32 - 2.0 * margin) / span;
    let light = {
        let l: V3 = [0.4, -0.6, 0.7];
        let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
        l.map(|x| x / n)
    };
    // View 0: screen x = X, y = Z, depth = Y. View 1: screen x = X, y = Y, depth = −Z.
    for view in 0..2 {
        let ox = (view * size as usize) as f32;
        let project = |p: V3| -> V3 {
            let x = ox + margin + (p[0] - lo[0]) * k;
            if view == 0 {
                [x, h as f32 - margin - (p[2] - lo[2]) * k, p[1]]
            } else {
                [x, h as f32 - margin - (p[1] - lo[1]) * k, -p[2]]
            }
        };
        for t in tris {
            let n = t.normal;
            let lambert = (n[0] * light[0] + n[1] * light[1] + n[2] * light[2]).abs();
            let shade = 0.45 + 0.55 * lambert;
            let rgb = t.color.map(|c| (c * shade * 255.0).clamp(0.0, 255.0) as u8);
            raster(
                t.p.map(project),
                rgb,
                &mut image,
                &mut depth,
                w,
                h,
                view * size as usize,
                size as usize,
            );
        }
    }
    let file =
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&image.concat())?;
    Ok(())
}

/// Fills one screen-space triangle within the columns `x0..x0 + width`.
#[allow(clippy::too_many_arguments)]
fn raster(
    p: [V3; 3],
    rgb: [u8; 3],
    image: &mut [[u8; 3]],
    depth: &mut [f32],
    w: usize,
    h: usize,
    x0: usize,
    width: usize,
) {
    let [a, b, c] = p;
    let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    if area.abs() < 1e-9 || !area.is_finite() {
        return;
    }
    let min_x = a[0].min(b[0]).min(c[0]).floor().max(x0 as f32) as usize;
    let max_x = a[0].max(b[0]).max(c[0]).ceil().min((x0 + width - 1) as f32);
    let min_y = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
    let max_y = a[1].max(b[1]).max(c[1]).ceil().min((h - 1) as f32);
    if max_x < 0.0 || max_y < 0.0 {
        return;
    }
    let (max_x, max_y) = (max_x as usize, max_y as usize);
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let q = [x as f32 + 0.5, y as f32 + 0.5];
            let w0 = ((b[0] - q[0]) * (c[1] - q[1]) - (b[1] - q[1]) * (c[0] - q[0])) / area;
            let w1 = ((c[0] - q[0]) * (a[1] - q[1]) - (c[1] - q[1]) * (a[0] - q[0])) / area;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let z = w0 * a[2] + w1 * b[2] + w2 * c[2];
            let i = y * w + x;
            if z < depth[i] {
                depth[i] = z;
                image[i] = rgb;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_fills_inside_and_respects_depth() {
        let (w, h) = (8, 8);
        let mut image = vec![[0u8; 3]; w * h];
        let mut depth = vec![f32::INFINITY; w * h];
        let tri = [[0.0, 0.0, 1.0], [8.0, 0.0, 1.0], [0.0, 8.0, 1.0]];
        raster(tri, [9, 9, 9], &mut image, &mut depth, w, h, 0, w);
        assert_eq!(image[w + 1], [9, 9, 9]);
        assert_eq!(image[w * 7 + 7], [0, 0, 0]);
        // A farther triangle does not overwrite.
        let far = tri.map(|p| [p[0], p[1], p[2] + 1.0]);
        raster(far, [1, 1, 1], &mut image, &mut depth, w, h, 0, w);
        assert_eq!(image[w + 1], [9, 9, 9]);
    }
}
