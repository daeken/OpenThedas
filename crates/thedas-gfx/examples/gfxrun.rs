//! Runs a movie headless the way the engine starts one and prints what it did: engine calls,
//! warnings, the display tree. Usage:
//! `gfxrun MOVIE.gfx [FONTS.gfx] [frames]`

use std::sync::{Arc, Mutex};

use thedas_gfx::avm1::HostValue;
use thedas_gfx::display::Kind;
use thedas_gfx::swf::{Character, Font, Movie};
use thedas_gfx::{Host, Player};

struct Log(Arc<Mutex<Vec<String>>>);

impl Host for Log {
    fn fscommand(&mut self, command: &str, args: &str) {
        if let Ok(mut l) = self.0.lock() {
            l.push(format!("fscommand {command} {args:?}"));
        }
    }
    fn external_call(&mut self, name: &str, args: &[HostValue]) -> HostValue {
        if let Ok(mut l) = self.0.lock() {
            l.push(format!("call {name} {args:?}"));
        }
        HostValue::Undefined
    }
    fn image_size(&mut self, _url: &str) -> Option<(u32, u32)> {
        Some((64, 64))
    }
}

struct FontLib(Vec<Arc<Font>>);

impl thedas_gfx::text::Fonts for FontLib {
    fn font(&self, name: &str, bold: bool, italic: bool) -> Option<Arc<Font>> {
        let name = match name {
            "$BodyFont" => "Dragon Text",
            "$TitleFont" => "Nogard",
            n => n,
        };
        self.0
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(name) && f.bold == bold && f.italic == italic)
            .or_else(|| self.0.iter().find(|f| f.name.eq_ignore_ascii_case(name)))
            .cloned()
    }
}

fn dump(p: &Player, d: thedas_gfx::display::DisplayId, indent: usize, out: &mut String) {
    let Some(o) = p.vm.display.get(d) else { return };
    let kind = match &o.kind {
        Kind::Clip(c) => format!(
            "clip#{} f{}/{}{}",
            c.character,
            c.frame.map(|f| f + 1).unwrap_or(0),
            c.timeline.frames.len(),
            if c.playing { " playing" } else { "" }
        ),
        Kind::Shape(_, id) => format!("shape#{id}"),
        Kind::Morph(_, id) => format!("morph#{id}"),
        Kind::Text(t) => format!("text {:?}", t.text().chars().take(40).collect::<String>()),
        Kind::Bitmap { url, .. } => format!("bitmap {url}"),
    };
    out.push_str(&format!(
        "{:indent$}{} [{}] {} at ({:.1},{:.1}){}\n",
        "",
        o.name,
        o.depth,
        kind,
        o.matrix.tx / 20.0,
        o.matrix.ty / 20.0,
        if o.visible {
            String::new()
        } else {
            " hidden".to_string()
        } + &if o.blend != thedas_gfx::swf::BlendMode::Normal {
            format!(" blend={:?}", o.blend)
        } else {
            String::new()
        } + &if o.color.is_identity() {
            String::new()
        } else {
            format!(" cx={:?}/{:?}", o.color.mul, o.color.add)
        },
        indent = indent
    ));
    for c in o.children.values() {
        dump(p, *c, indent + 2, out);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let movie = std::fs::read(&args[1]).expect("movie");
    let fonts = args
        .get(2)
        .and_then(|f| std::fs::read(f).ok())
        .and_then(|d| Movie::parse(&d).ok())
        .map(|m| {
            m.characters
                .values()
                .filter_map(|c| match c {
                    Character::Font(f) => Some(f.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let frames: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10);
    let log = Arc::new(Mutex::new(Vec::new()));
    let name = args[1].rsplit('/').next().unwrap_or("movie").to_string();
    let t0 = std::time::Instant::now();
    let mut p = Player::new(
        &movie,
        &name,
        Box::new(Log(log.clone())),
        Arc::new(FontLib(fonts)),
        None,
    )
    .expect("parse");
    let t_load = t0.elapsed();
    let (w, h) = p.vm.movie.stage_size();
    p.set_stage_size(w as f64 * 1280.0 / 720.0 * 0.75, h as f64);
    p.set_variable(
        "_global.GUISystem.OriginalStageWidth",
        &HostValue::Number(w as f64),
    );
    p.set_variable(
        "_global.GUISystem.OriginalStageHeight",
        &HostValue::Number(h as f64),
    );
    p.set_variable("_root.OriginalStageWidth", &HostValue::Number(w as f64));
    p.set_variable("_root.OriginalStageHeight", &HostValue::Number(h as f64));
    p.set_variable("_root.Filename", &HostValue::String(name.clone()));
    p.invoke("_global.GUISystem.StartSceneFromGame", &[]);
    p.invoke("_global.MovieUtils.UpdateAllLayouts", &[]);
    let t1 = std::time::Instant::now();
    for _ in 0..frames {
        p.run_frame();
    }
    let t_frames = t1.elapsed();
    if std::env::var_os("GFX_RENDER").is_some() {
        struct NoImages;
        impl thedas_gfx::render::Images for NoImages {
            fn image(&mut self, _: &str) -> Option<(u32, u32, Vec<u8>)> {
                Some((4, 4, vec![128; 64]))
            }
        }
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter =
            pollster::block_on(instance.request_adapter(&Default::default())).expect("adapter");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).expect("device");
        let mut r =
            thedas_gfx::render::Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        let size = (1280, 720);
        r.render(&mut p, &mut NoImages, size);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let n = 60;
        let (mut script, mut render, mut drawn) =
            (std::time::Duration::ZERO, std::time::Duration::ZERO, 0);
        for _ in 0..n {
            let a = std::time::Instant::now();
            p.tick(1000.0 / 30.0);
            let b = std::time::Instant::now();
            if r.render_if_dirty(&mut p, &mut NoImages, size) {
                drawn += 1;
            }
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            script += b - a;
            render += b.elapsed();
        }
        println!(
            "per tick: script {:?}, render {:?} ({drawn}/{n} ticks drew, {} draws)",
            script / n,
            render / n,
            r.draws
        );
    }
    for l in log
        .lock()
        .map(|l| l.clone())
        .unwrap_or_default()
        .iter()
        .take(60)
    {
        println!("{l}");
    }
    for l in p.drain_log().iter().take(
        std::env::var("GFX_LOG")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(80),
    ) {
        println!("{l}");
    }
    for path in std::env::var("GFX_SHOW")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
    {
        let v = p.get_value(path);
        let f = matches!(&v, thedas_gfx::avm1::value::Value::Object(o) if p.vm.is_function(*o));
        println!(
            "{path} = {:?}{}",
            p.get_variable(path),
            if f { " (function)" } else { "" }
        );
    }
    let mut out = String::new();
    dump(&p, p.vm.root, 0, &mut out);
    let lines: Vec<&str> = out.lines().collect();
    for l in lines
        .iter()
        .take(if std::env::var_os("GFX_TREE").is_some() {
            usize::MAX
        } else {
            120
        })
    {
        println!("{l}");
    }
    println!(
        "{} display lines; load {:?}; {frames} frames {:?}; heap {}",
        lines.len(),
        t_load,
        t_frames,
        p.vm.heap.live()
    );
}
