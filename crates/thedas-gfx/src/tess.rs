//! Turns shapes, glyphs and drawings into triangles (CPU side, cached by the renderer).

use lyon::math::point;
use lyon::path::Path;
use lyon::tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, StrokeOptions,
    StrokeTessellator, StrokeVertex, VertexBuffers,
};

use crate::swf::{Fill, Line, Matrix, Record};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    /// Position in the object's space (twips).
    pub pos: [f32; 2],
    /// Texture coordinate (bitmap fills; 0..1 over the image).
    pub uv: [f32; 2],
    /// Straight RGBA (solid fills; white otherwise).
    pub color: [u8; 4],
}

/// What a group of triangles is painted with.
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    /// Colours are in the vertices.
    Solid,
    Bitmap {
        id: u16,
        repeat: bool,
        smooth: bool,
    },
    /// A gradient: kind (2 linear, 3 radial, 4 focal), the matrix from object space to
    /// gradient space, and its 256-entry ramp (straight RGBA).
    Gradient {
        kind: u32,
        to_gradient: Matrix,
        focal: f32,
        ramp: Vec<[u8; 4]>,
    },
}

#[derive(Clone, Debug)]
pub struct Mesh {
    pub paint: Paint,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

/// Tessellation tolerance in twips.
const TOLERANCE: f32 = 2.0;

type Edge = ((f32, f32), Option<(f32, f32)>, (f32, f32));

/// Builds closed contours from a fill style's directed edges.
fn contours(mut edges: Vec<Edge>) -> Path {
    let mut b = Path::builder();
    let key = |p: (f32, f32)| ((p.0 * 4.0).round() as i64, (p.1 * 4.0).round() as i64);
    let mut by_start: std::collections::HashMap<(i64, i64), Vec<usize>> =
        std::collections::HashMap::new();
    for (i, e) in edges.iter().enumerate() {
        by_start.entry(key(e.0)).or_default().push(i);
    }
    let mut used = vec![false; edges.len()];
    for i in 0..edges.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let start = edges[i].0;
        b.begin(point(start.0, start.1));
        let mut cur = i;
        let mut guard = 0;
        loop {
            let (_, c, to) = edges[cur];
            match c {
                Some(c) => {
                    b.quadratic_bezier_to(point(c.0, c.1), point(to.0, to.1));
                }
                None => {
                    b.line_to(point(to.0, to.1));
                }
            }
            if key(to) == key(start) {
                break;
            }
            let next = by_start
                .get(&key(to))
                .and_then(|l| l.iter().copied().find(|&j| !used[j]));
            match next {
                Some(j) => {
                    used[j] = true;
                    cur = j;
                }
                None => break,
            }
            guard += 1;
            if guard > 1_000_000 {
                break;
            }
        }
        b.close();
    }
    edges.clear();
    b.build()
}

fn ramp(stops: &[(u8, crate::swf::Color)]) -> Vec<[u8; 4]> {
    let mut out = vec![[0u8; 4]; 256];
    if stops.is_empty() {
        return out;
    }
    for (i, px) in out.iter_mut().enumerate() {
        let i = i as u8;
        let mut lo = stops[0];
        let mut hi = stops[stops.len() - 1];
        for w in stops.windows(2) {
            if i >= w[0].0 && i <= w[1].0 {
                lo = w[0];
                hi = w[1];
                break;
            }
        }
        let t = if i <= lo.0 {
            0.0
        } else if i >= hi.0 || hi.0 == lo.0 {
            1.0
        } else {
            (i - lo.0) as f32 / (hi.0 - lo.0) as f32
        };
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        *px = [
            m(lo.1.r, hi.1.r),
            m(lo.1.g, hi.1.g),
            m(lo.1.b, hi.1.b),
            m(lo.1.a, hi.1.a),
        ];
    }
    out
}

/// Bitmap fills need the image's size to map texture coordinates: `size(id)` gives it in
/// pixels.
pub fn tessellate_shape(
    fills: &[Fill],
    lines: &[Line],
    records: &[Record],
    size: &dyn Fn(u16) -> Option<(f32, f32)>,
) -> Vec<Mesh> {
    // Edges per (style set, fill index); line segments per (style set, line index).
    let mut fill_edges: Vec<(Vec<Fill>, Vec<Vec<Edge>>)> = Vec::new();
    let mut line_paths: Vec<(Line, Vec<Edge>)> = Vec::new();
    let mut cur_fills = fills.to_vec();
    let mut cur_lines = lines.to_vec();
    fill_edges.push((cur_fills.clone(), vec![Vec::new(); cur_fills.len()]));
    let (mut f0, mut f1, mut ln) = (0u32, 0u32, 0u32);
    let (mut px, mut py) = (0.0f32, 0.0f32);
    let mut line_strip: Vec<Edge> = Vec::new();
    let mut line_style: Option<Line> = None;
    let flush_line =
        |strip: &mut Vec<Edge>, style: &Option<Line>, out: &mut Vec<(Line, Vec<Edge>)>| {
            if let Some(s) = style {
                if !strip.is_empty() {
                    out.push((s.clone(), std::mem::take(strip)));
                }
            }
            strip.clear();
        };
    for r in records {
        match r {
            Record::Style {
                move_to,
                fill0,
                fill1,
                line,
                new_styles,
            } => {
                if let Some((nf, nl)) = new_styles {
                    flush_line(&mut line_strip, &line_style, &mut line_paths);
                    cur_fills = nf.clone();
                    cur_lines = nl.clone();
                    fill_edges.push((cur_fills.clone(), vec![Vec::new(); cur_fills.len()]));
                    f0 = 0;
                    f1 = 0;
                    ln = 0;
                    line_style = None;
                }
                if let Some(f) = fill0 {
                    f0 = *f;
                }
                if let Some(f) = fill1 {
                    f1 = *f;
                }
                if line.is_some() || move_to.is_some() {
                    flush_line(&mut line_strip, &line_style, &mut line_paths);
                }
                if let Some(l) = line {
                    ln = *l;
                    line_style = if ln == 0 {
                        None
                    } else {
                        cur_lines.get(ln as usize - 1).cloned()
                    };
                }
                if let Some((x, y)) = move_to {
                    px = *x as f32;
                    py = *y as f32;
                }
            }
            Record::Line { to } | Record::Curve { to, .. } => {
                let c = match r {
                    Record::Curve { control, .. } => Some((control.0 as f32, control.1 as f32)),
                    _ => None,
                };
                let a = (px, py);
                let b = (to.0 as f32, to.1 as f32);
                let set = fill_edges.len() - 1;
                if f1 != 0 {
                    if let Some(v) = fill_edges[set].1.get_mut(f1 as usize - 1) {
                        v.push((a, c, b));
                    }
                }
                if f0 != 0 {
                    if let Some(v) = fill_edges[set].1.get_mut(f0 as usize - 1) {
                        v.push((b, c, a));
                    }
                }
                if ln != 0 {
                    line_strip.push((a, c, b));
                }
                px = b.0;
                py = b.1;
            }
        }
    }
    flush_line(&mut line_strip, &line_style, &mut line_paths);

    let mut solid = Mesh {
        paint: Paint::Solid,
        vertices: Vec::new(),
        indices: Vec::new(),
    };
    let mut out = Vec::new();
    let mut tess = FillTessellator::new();
    let opts = FillOptions::tolerance(TOLERANCE).with_fill_rule(FillRule::EvenOdd);
    for (styles, per_fill) in fill_edges {
        for (i, edges) in per_fill.into_iter().enumerate() {
            if edges.is_empty() {
                continue;
            }
            let Some(style) = styles.get(i) else { continue };
            let path = contours(edges);
            let mut buf: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
            if tess
                .tessellate_path(
                    &path,
                    &opts,
                    &mut BuffersBuilder::new(&mut buf, |v: FillVertex| v.position().to_array()),
                )
                .is_err()
            {
                continue;
            }
            push_fill(&mut solid, &mut out, style, &buf, size);
        }
    }
    let mut stroke = StrokeTessellator::new();
    for (style, edges) in line_paths {
        let mut b = Path::builder();
        let mut started = false;
        let mut last = (f32::NAN, f32::NAN);
        for (a, c, to) in edges {
            if !started || a != last {
                if started {
                    b.end(false);
                }
                b.begin(point(a.0, a.1));
                started = true;
            }
            match c {
                Some(c) => {
                    b.quadratic_bezier_to(point(c.0, c.1), point(to.0, to.1));
                }
                None => {
                    b.line_to(point(to.0, to.1));
                }
            }
            last = to;
        }
        if started {
            b.end(false);
        }
        let path = b.build();
        let width = (style.width as f32).max(20.0);
        let mut buf: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
        if stroke
            .tessellate_path(
                &path,
                &StrokeOptions::tolerance(TOLERANCE).with_line_width(width),
                &mut BuffersBuilder::new(&mut buf, |v: StrokeVertex| v.position().to_array()),
            )
            .is_err()
        {
            continue;
        }
        let fill = style.fill.clone().unwrap_or(Fill::Solid(style.color));
        push_fill(&mut solid, &mut out, &fill, &buf, size);
    }
    if !solid.indices.is_empty() {
        out.insert(0, solid);
    }
    out
}

fn push_fill(
    solid: &mut Mesh,
    out: &mut Vec<Mesh>,
    style: &Fill,
    buf: &VertexBuffers<[f32; 2], u32>,
    size: &dyn Fn(u16) -> Option<(f32, f32)>,
) {
    match style {
        Fill::Solid(c) => {
            let base = solid.vertices.len() as u32;
            let color = [c.r, c.g, c.b, c.a];
            solid.vertices.extend(buf.vertices.iter().map(|p| Vertex {
                pos: *p,
                uv: [0.0; 2],
                color,
            }));
            solid.indices.extend(buf.indices.iter().map(|i| i + base));
        }
        Fill::Bitmap {
            id,
            matrix,
            repeat,
            smooth,
        } => {
            let (w, h) = size(*id).unwrap_or((1.0, 1.0));
            let inv = matrix.invert().unwrap_or_default();
            out.push(Mesh {
                paint: Paint::Bitmap {
                    id: *id,
                    repeat: *repeat,
                    smooth: *smooth,
                },
                vertices: buf
                    .vertices
                    .iter()
                    .map(|p| {
                        // The bitmap matrix maps image pixels to shape space (twips).
                        let (u, v) = inv.apply(p[0], p[1]);
                        Vertex {
                            pos: *p,
                            uv: [u / w, v / h],
                            color: [255; 4],
                        }
                    })
                    .collect(),
                indices: buf.indices.clone(),
            });
        }
        Fill::Linear(g) | Fill::Radial(g) | Fill::Focal(g) => {
            let kind = match style {
                Fill::Linear(_) => 2,
                Fill::Radial(_) => 3,
                _ => 4,
            };
            out.push(Mesh {
                paint: Paint::Gradient {
                    kind,
                    to_gradient: g.matrix.invert().unwrap_or_default(),
                    focal: g.focal,
                    ramp: ramp(&g.stops),
                },
                vertices: buf
                    .vertices
                    .iter()
                    .map(|p| Vertex {
                        pos: *p,
                        uv: [0.0; 2],
                        color: [255; 4],
                    })
                    .collect(),
                indices: buf.indices.clone(),
            });
        }
    }
}

/// A glyph outline (one implicit fill) as triangles in EM units.
pub fn tessellate_glyph(records: &[Record]) -> (Vec<[f32; 2]>, Vec<u32>) {
    let fills = [Fill::Solid(crate::swf::Color {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    })];
    // Glyph shapes use fill 1 on one side; make sure a fill is set.
    let meshes = tessellate_shape(&fills, &[], records, &|_| None);
    match meshes.into_iter().next() {
        Some(m) => (m.vertices.iter().map(|v| v.pos).collect(), m.indices),
        None => (Vec::new(), Vec::new()),
    }
}

/// A polygon fill from the drawing API.
pub fn tessellate_polygon(points: &[(f32, f32)], color: [u8; 4]) -> Option<Mesh> {
    if points.len() < 3 {
        return None;
    }
    let mut b = Path::builder();
    b.begin(point(points[0].0, points[0].1));
    for p in &points[1..] {
        b.line_to(point(p.0, p.1));
    }
    b.close();
    let path = b.build();
    let mut buf: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
    FillTessellator::new()
        .tessellate_path(
            &path,
            &FillOptions::tolerance(TOLERANCE).with_fill_rule(FillRule::EvenOdd),
            &mut BuffersBuilder::new(&mut buf, |v: FillVertex| v.position().to_array()),
        )
        .ok()?;
    Some(Mesh {
        paint: Paint::Solid,
        vertices: buf
            .vertices
            .iter()
            .map(|p| Vertex {
                pos: *p,
                uv: [0.0; 2],
                color,
            })
            .collect(),
        indices: buf.indices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swf::Color;

    #[test]
    fn square_fills() {
        let red = Fill::Solid(Color {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        });
        let recs = vec![
            Record::Style {
                move_to: Some((0, 0)),
                fill0: None,
                fill1: Some(1),
                line: None,
                new_styles: None,
            },
            Record::Line { to: (100, 0) },
            Record::Line { to: (100, 100) },
            Record::Line { to: (0, 100) },
            Record::Line { to: (0, 0) },
        ];
        let m = tessellate_shape(&[red], &[], &recs, &|_| None);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].indices.len(), 6);
        assert_eq!(m[0].vertices[0].color, [255, 0, 0, 255]);
    }
}
