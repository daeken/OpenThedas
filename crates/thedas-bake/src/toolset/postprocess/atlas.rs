use std::{fmt::Write, fs, path::Path};

use anyhow::{Context, Result, ensure};

use super::{
    image::{Image, pixel_count},
    write_texture_meta, xml_escape,
};

pub(super) struct Options {
    pub width: u32,
    pub height: u32,
    pub gutter: u32,
    pub half_texel: bool,
    pub input_width: u32,
    pub input_height: u32,
}

pub(super) fn run(input: &Path, output: &Path, job: &Path, options: &Options) -> Result<()> {
    pixel_count(options.width, options.height)?;
    pixel_count(options.input_width, options.input_height)?;
    ensure!(
        options.gutter <= 16384,
        "atlas gutter outside the supported range"
    );
    let document = fs::read_to_string(job).with_context(|| format!("reading {}", job.display()))?;
    let document = roxmltree::Document::parse(&document)?;
    ensure!(
        document.root_element().has_tag_name("Root"),
        "expected an atlas job Root"
    );
    fs::create_dir_all(output)?;
    for group in document
        .root_element()
        .children()
        .filter(roxmltree::Node::is_element)
    {
        ensure!(
            group.has_tag_name("Atlas"),
            "unknown atlas job element {}",
            group.tag_name().name()
        );
        let prefix = group
            .attribute("Output")
            .context("atlas has no Output attribute")?;
        ensure!(!prefix.is_empty(), "atlas has an empty Output attribute");
        let mut inputs = Vec::new();
        for entry in group.children().filter(roxmltree::Node::is_element) {
            ensure!(
                entry.has_tag_name("Input"),
                "unknown atlas entry {}",
                entry.tag_name().name()
            );
            let name = entry
                .attribute("Texture")
                .context("atlas entry has no Texture attribute")?;
            let source = Image::read_dds(&super::combine::matching_file(input, name)?)?;
            let (width, height) = (
                source.width.min(options.input_width),
                source.height.min(options.input_height),
            );
            let image = if (width, height) == (source.width, source.height) {
                source
            } else {
                source.resize_box(width, height)?
            };
            let gutter = 2 * options.gutter;
            pixel_count(
                (width + gutter).next_power_of_two(),
                (height + gutter).next_power_of_two(),
            )?;
            inputs.push(Input {
                name,
                image,
                width: width + gutter,
                height: height + gutter,
            });
        }
        let order = (0..inputs.len()).collect();
        let pages = pack(&inputs, order, options.width, options.height, 0);
        write_pages(output, prefix, &inputs, &pages, options)?;
    }
    Ok(())
}

struct Input<'a> {
    name: &'a str,
    image: Image,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy)]
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl Rect {
    fn fits(self, input: &Input<'_>) -> bool {
        input.width <= self.width && input.height <= self.height
    }

    fn inside(self, width: u32, height: u32) -> bool {
        self.x + self.width <= width && self.y + self.height <= height
    }
}

struct Page {
    width: u32,
    height: u32,
    fitting: Vec<Rect>,
    growing: Vec<Rect>,
    placed: Vec<(usize, Rect)>,
}

impl Page {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width: 1,
            height: 1,
            fitting: Vec::new(),
            growing: vec![Rect {
                width,
                height,
                x: 0,
                y: 0,
            }],
            placed: Vec::new(),
        }
    }

    fn insert(&mut self, id: usize, input: &Input<'_>) -> bool {
        let area = |free: &Rect| {
            u64::from(self.width.max((free.x + input.width).next_power_of_two()))
                * u64::from(self.height.max((free.y + input.height).next_power_of_two()))
        };
        let free = if let Some(index) = self.fitting.iter().position(|free| free.fits(input)) {
            self.fitting.remove(index)
        } else if let Some((index, _)) = self
            .growing
            .iter()
            .enumerate()
            .filter(|(_, free)| free.fits(input))
            .min_by_key(|(_, free)| area(free))
        {
            self.growing.remove(index)
        } else {
            return false;
        };
        let rect = Rect {
            x: free.x,
            y: free.y,
            width: input.width,
            height: input.height,
        };
        self.placed.push((id, rect));
        self.split(free, rect);
        let width = self.width.max((rect.x + rect.width).next_power_of_two());
        let height = self.height.max((rect.y + rect.height).next_power_of_two());
        if (width, height) != (self.width, self.height) {
            self.width = width;
            self.height = height;
            self.growing.retain(|free| {
                if free.inside(width, height) {
                    self.fitting.push(*free);
                    false
                } else {
                    true
                }
            });
        }
        true
    }

    fn split(&mut self, free: Rect, placed: Rect) {
        let dw = free.width - placed.width;
        let dh = free.height - placed.height;
        if dw <= 3 && dh <= 3 {
            return;
        }
        let right = Rect {
            x: placed.x + placed.width,
            y: free.y,
            width: dw,
            height: free.height,
        };
        let bottom = Rect {
            x: free.x,
            y: placed.y + placed.height,
            width: free.width,
            height: dh,
        };
        if u64::from(free.width) * u64::from(dh) <= u64::from(free.height) * u64::from(dw) {
            self.growing.push(right);
            if dw > 4 && dh > 4 {
                self.growing.push(Rect {
                    width: placed.width,
                    ..bottom
                });
            }
        } else {
            self.growing.push(bottom);
            if dw > 4 && dh > 4 {
                self.growing.push(Rect {
                    height: placed.height,
                    ..right
                });
            }
        }
    }

    fn area(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }
}

fn fit_quality(pages: &[Page]) -> f32 {
    let area: u64 = pages.iter().map(Page::area).sum();
    let taken: u64 = pages
        .iter()
        .flat_map(|page| &page.placed)
        .map(|(_, rect)| u64::from(rect.width) * u64::from(rect.height))
        .sum();
    if area == 0 {
        0.0
    } else {
        1.0 - taken as f32 / area as f32
    }
}

fn pack(
    inputs: &[Input<'_>],
    mut order: Vec<usize>,
    width: u32,
    height: u32,
    depth: u8,
) -> Vec<Page> {
    crate::legacy_sort::sort_by(&mut order, |left, right| {
        inputs[left].width > inputs[right].width
    });
    let mut pages = Vec::<Page>::new();
    for id in order {
        let input = &inputs[id];
        if pages.iter_mut().any(|page| page.insert(id, input)) {
            continue;
        }
        let mut page = Page::new(
            width.max(input.width.next_power_of_two()),
            height.max(input.height.next_power_of_two()),
        );
        let inserted = page.insert(id, input);
        debug_assert!(inserted);
        pages.push(page);
    }
    if depth == 2 {
        return pages;
    }
    let mut retained = Vec::new();
    let mut replacements = Vec::new();
    for page in pages {
        let old = fit_quality(std::slice::from_ref(&page));
        if old >= 0.1 {
            let order = page.placed.iter().map(|&(id, _)| id).collect();
            let other = pack(inputs, order, page.width / 2, page.height / 2, depth + 1);
            if other.len() < 4 && fit_quality(&other) < old {
                replacements.extend(other);
                continue;
            }
        }
        retained.push(page);
    }
    retained.extend(replacements);
    retained
}

fn write_pages(
    out: &Path,
    prefix: &str,
    inputs: &[Input<'_>],
    pages: &[Page],
    options: &Options,
) -> Result<()> {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<AtlasData>\r\n",
    );
    for (index, page) in pages.iter().enumerate() {
        let name = format!("{prefix}{index:02}.dds");
        let filename = out.join(&name);
        let mut rgba = vec![[0; 4]; pixel_count(page.width, page.height)?];
        write!(xml, "\t<AtlasTexture Name=\"{}\">\r\n", xml_escape(&name))?;
        for &(id, rect) in &page.placed {
            let input = &inputs[id];
            copy_rect(&mut rgba, page.width, rect, &input.image, options.gutter);
            let scale = |size: u32, page_size: u32| {
                let size = f64::from(size);
                let full = size / f64::from(page_size);
                if options.half_texel {
                    full / (1.0 + 1.0 / size)
                } else {
                    full
                }
            };
            let name = xml_escape(input.name);
            let u = atlas_offset(rect.x + options.gutter, page.width, options.half_texel);
            let v = atlas_offset(rect.y + options.gutter, page.height, options.half_texel);
            let w = scale(input.image.width, page.width);
            let h = scale(input.image.height, page.height);
            write!(
                xml,
                "\t\t<SourceTexture Name=\"{name}\" OffsetAndScale=\"{u} {v} {w:.6} {h:.6}\"/>\r\n"
            )?;
        }
        xml.push_str("\t</AtlasTexture>\r\n");
        let image = Image::new(page.width, page.height, rgba)?;
        image.write_bgra_dds(&filename, true)?;
        write_texture_meta(&filename, page.width, page.height, "CreateAtlas")?;
    }
    xml.push_str("</AtlasData>");
    fs::write(out.join(format!("{prefix}dat.xml")), xml)?;
    Ok(())
}

fn atlas_offset(at: u32, page_size: u32, half_texel: bool) -> String {
    // The original CRT rounds a decimal midpoint away from zero.
    let page_size = u64::from(page_size);
    let half_pixels = 2 * u64::from(at) + u64::from(half_texel);
    let millionths = (half_pixels * 500_000 + page_size / 2) / page_size;
    format!("{}.{:06}", millionths / 1_000_000, millionths % 1_000_000)
}

fn copy_rect(destination: &mut [[u8; 4]], stride: u32, rect: Rect, source: &Image, gutter: u32) {
    for dy in 0..rect.height {
        let sy = dy.saturating_sub(gutter).min(source.height - 1);
        let dest_start = ((rect.y + dy) * stride + rect.x) as usize;
        for dx in 0..rect.width {
            let sx = dx.saturating_sub(gutter).min(source.width - 1);
            destination[dest_start + dx as usize] =
                source.pixels[(sy * source.width + sx) as usize];
        }
    }
}
