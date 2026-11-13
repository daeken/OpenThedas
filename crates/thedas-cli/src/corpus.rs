//! The corpus check: open every archive, read every entry, and parse and round-trip every
//! GFF4-family resource, including loose files outside archives. Formats with a typed reader
//! (GDA, TLK, areas, layouts, models, meshes, materials, DDS) are also parsed into their typed
//! form; the `typed` column counts those.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use anyhow::Result;
use serde_json::json;
use thedas_formats::gff3::{Gff3, is_gff3};
use thedas_formats::gff4::{Gff4, is_gff4};
use thedas_formats::{
    Area, Arl, Dds, Gda, MaterialDef, MaterialObject, MaterialState, Mmh, Msh, Rml, Tlk,
};
use thedas_script::{EngineTable, Ncs};
use thedas_vfs::erf::sniff_version;
use thedas_vfs::{DaoInstall, Erf, extension};

#[derive(Default, Clone, Debug)]
struct ExtStats {
    entries: u64,
    bytes: u64,
    encrypted: u64,
    read_failed: u64,
    gff3: u64,
    gff4: u64,
    parsed: u64,
    round_trip: u64,
    byte_exact: u64,
    typed_ok: u64,
    typed_failed: u64,
}

#[derive(Default)]
struct Stats {
    archives: u64,
    archives_failed: u64,
    loose_files: u64,
    by_ext: BTreeMap<String, ExtStats>,
    /// Category -> examples (location, message).
    failures: BTreeMap<String, Vec<(String, String)>>,
    failure_counts: BTreeMap<String, u64>,
    archive_versions: BTreeMap<String, u64>,
    /// Shader model counts (`vs_3_0`, `ps_2_1`, ...).
    shader_models: BTreeMap<String, u64>,
    /// Scaleform tags in GFx movies, by name, with the movies' external images.
    gfx_tags: BTreeMap<String, u64>,
}

impl Stats {
    fn fail(&mut self, category: &str, location: String, message: String, keep: usize) {
        *self.failure_counts.entry(category.to_string()).or_default() += 1;
        let v = self.failures.entry(category.to_string()).or_default();
        if v.len() < keep {
            v.push((location, message));
        }
    }
}

enum Item {
    Archive(PathBuf),
    Loose(PathBuf),
}

fn gather(paths: &[PathBuf]) -> Result<(Vec<Item>, Option<PathBuf>)> {
    let mut items = Vec::new();
    let mut root = None;
    for p in paths {
        if p.is_dir() {
            // Label paths relative to the install root when the directory is inside one.
            let base = p
                .ancestors()
                .find(|a| DaoInstall::open(a).is_ok())
                .unwrap_or(p);
            root.get_or_insert_with(|| base.to_path_buf());
            let mut files = Vec::new();
            walk(p, &mut files)?;
            files.sort();
            for f in files {
                if thedas_vfs::vfs::is_archive_path(&f) {
                    items.push(Item::Archive(f));
                } else {
                    items.push(Item::Loose(f));
                }
            }
        } else if thedas_vfs::vfs::is_archive_path(p) {
            items.push(Item::Archive(p.clone()));
        } else {
            items.push(Item::Loose(p.clone()));
        }
    }
    Ok((items, root))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk(&entry.path(), out)?;
        } else if ft.is_file() {
            out.push(entry.path());
        }
    }
    Ok(())
}

/// The engine-function table for checking scripts, loaded once from the install being
/// checked (if the paths include one).
static ENGINE_TABLE: std::sync::OnceLock<Option<EngineTable>> = std::sync::OnceLock::new();

/// Failure categories that are reported but do not fail the run: an archive with an unusual
/// extension, and engine calls the shipped `script.ldf` does not declare. In the GOG 1.05
/// install those are engine functions #859-#862 and a two-argument `CommandFly`, used by the
/// DLC and Awakening scripts (their engine extended the table; no newer `script.ldf` ships),
/// plus a few 2006 iteration-test scripts compiled against an older table.
const NON_FATAL: &[&str] = &["unexpected-archive", "script/undeclared-action"];

pub fn run(paths: &[PathBuf], report: Option<&Path>, jobs: usize, keep: usize) -> Result<bool> {
    let started = Instant::now();
    let (items, root) = gather(paths)?;
    ENGINE_TABLE.get_or_init(|| {
        let install = DaoInstall::open(root.as_ref()?).ok()?;
        let vfs = install
            .global_vfs(thedas_vfs::install::TextureQuality::High)
            .ok()?;
        let (data, _) = vfs.read("script.ldf").ok()?;
        EngineTable::parse(&data).ok()
    });
    let rel = |p: &Path| -> String {
        match &root {
            Some(r) => p.strip_prefix(r).unwrap_or(p).display().to_string(),
            None => p.display().to_string(),
        }
    };
    let total = items.len();
    eprintln!("corpus: {total} files to check with {jobs} threads");

    let stats = Mutex::new(Stats::default());
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..jobs {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    let mut local = Stats::default();
                    match item {
                        Item::Archive(p) => check_archive(p, &rel(p), &mut local, keep),
                        Item::Loose(p) => check_loose(p, &rel(p), &mut local, keep),
                    }
                    merge(&mut stats.lock().unwrap(), local, keep);
                    let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if d % 500 == 0 || d == total {
                        eprintln!(
                            "corpus: {d}/{total} files ({:.0}s)",
                            started.elapsed().as_secs_f64()
                        );
                    }
                }
            });
        }
    });
    let stats = stats.into_inner().unwrap();
    let ok = print_summary(&stats, started.elapsed().as_secs_f64());
    if let Some(path) = report {
        std::fs::write(path, serde_json::to_vec_pretty(&to_report(&stats, ok))?)?;
        eprintln!("corpus: report written to {}", path.display());
    }
    Ok(ok)
}

fn merge(into: &mut Stats, from: Stats, keep: usize) {
    into.archives += from.archives;
    into.archives_failed += from.archives_failed;
    into.loose_files += from.loose_files;
    for (k, v) in from.archive_versions {
        *into.archive_versions.entry(k).or_default() += v;
    }
    for (k, v) in from.gfx_tags {
        *into.gfx_tags.entry(k).or_default() += v;
    }
    for (k, v) in from.shader_models {
        *into.shader_models.entry(k).or_default() += v;
    }
    for (ext, s) in from.by_ext {
        let t = into.by_ext.entry(ext).or_default();
        t.entries += s.entries;
        t.bytes += s.bytes;
        t.encrypted += s.encrypted;
        t.read_failed += s.read_failed;
        t.gff3 += s.gff3;
        t.gff4 += s.gff4;
        t.parsed += s.parsed;
        t.round_trip += s.round_trip;
        t.byte_exact += s.byte_exact;
        t.typed_ok += s.typed_ok;
        t.typed_failed += s.typed_failed;
    }
    for (cat, n) in from.failure_counts {
        *into.failure_counts.entry(cat.clone()).or_default() += n;
    }
    for (cat, examples) in from.failures {
        let v = into.failures.entry(cat).or_default();
        for e in examples {
            if v.len() < keep {
                v.push(e);
            }
        }
    }
}

fn check_archive(path: &Path, label: &str, stats: &mut Stats, keep: usize) {
    stats.archives += 1;
    let erf = match Erf::open(path) {
        Ok(e) => e,
        Err(e) => {
            stats.archives_failed += 1;
            stats.fail("archive-open", label.to_string(), e.to_string(), keep);
            return;
        }
    };
    *stats
        .archive_versions
        .entry(erf.version.as_str().to_string())
        .or_default() += 1;
    for entry in &erf.entries {
        let name = entry.display_name();
        let ext = extension(&name);
        let location = format!("{label}:{name}");
        let es = stats.by_ext.entry(ext.clone()).or_default();
        es.entries += 1;
        es.bytes += entry.size as u64;
        if erf.is_encrypted() && erf.installed_key().is_none() {
            es.encrypted += 1;
            continue;
        }
        match erf.read(entry, None) {
            Ok(data) => check_data(&data, &name, &ext, location, stats, keep),
            Err(e) => {
                stats.by_ext.get_mut(&ext).unwrap().read_failed += 1;
                stats.fail("entry-read", location, e.to_string(), keep);
            }
        }
    }
}

/// FMOD event files parse whole; sound banks' headers parse and their samples' data lie
/// within the file (decoding every sample is `thedas fsb --check`'s job).
fn check_audio(path: &Path, ext: &str, label: &str, stats: &mut Stats, keep: usize) {
    use std::io::Read;
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let result = if ext == "fev" {
        std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|d| {
                thedas_formats::fev::Fev::parse(&d)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            })
    } else {
        let read = || -> std::result::Result<(), String> {
            let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
            let mut head = vec![0u8; 48];
            f.read_exact(&mut head).map_err(|e| e.to_string())?;
            let len = thedas_formats::fsb::Fsb::header_len(&head).map_err(|e| e.to_string())?;
            if len as u64 > size {
                return Err(format!(
                    "headers end at {len}, past the file's {size} bytes"
                ));
            }
            head.resize(len, 0);
            f.read_exact(&mut head[48..]).map_err(|e| e.to_string())?;
            let fsb = thedas_formats::fsb::Fsb::parse(&head).map_err(|e| e.to_string())?;
            match fsb.samples.iter().find(|s| s.data.end as u64 > size) {
                Some(s) => Err(format!("{}: data past the end of the file", s.name)),
                None => Ok(()),
            }
        };
        read()
    };
    let es = stats.by_ext.entry(ext.to_string()).or_default();
    es.entries += 1;
    es.bytes += size;
    typed_result(stats, ext, label.to_string(), result, keep);
}

fn check_loose(path: &Path, label: &str, stats: &mut Stats, keep: usize) {
    use std::io::Read;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if ext == "fev" || ext == "fsb" {
        return check_audio(path, &ext, label, stats, keep);
    }
    let mut head = [0u8; 16];
    let n = match std::fs::File::open(path).and_then(|mut f| f.read(&mut head)) {
        Ok(n) => n,
        Err(e) => {
            stats.fail("loose-read", label.to_string(), e.to_string(), keep);
            return;
        }
    };
    if !is_gff4(&head[..n])
        && !(n >= 8 && &head[4..6] == b"V3" && head[..4].iter().all(u8::is_ascii_graphic))
    {
        // An archive with an unusual extension would be worth knowing about.
        if sniff_version(&head[..n]).is_some() {
            stats.fail(
                "unexpected-archive",
                label.to_string(),
                "ERF magic in a file without an archive extension".into(),
                keep,
            );
        }
        return;
    }
    stats.loose_files += 1;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match std::fs::read(path) {
        Ok(data) => {
            let es = stats.by_ext.entry(ext.clone()).or_default();
            es.entries += 1;
            es.bytes += data.len() as u64;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            check_data(&data, &name, &ext, label.to_string(), stats, keep);
        }
        Err(e) => stats.fail("loose-read", label.to_string(), e.to_string(), keep),
    }
}

/// Records the outcome of a typed parse for an extension.
fn typed_result(
    stats: &mut Stats,
    ext: &str,
    location: String,
    result: std::result::Result<(), String>,
    keep: usize,
) {
    let es = stats.by_ext.entry(ext.to_string()).or_default();
    match result {
        Ok(()) => es.typed_ok += 1,
        Err(e) => {
            es.typed_failed += 1;
            stats.fail(&format!("typed/{ext}"), location, e, keep);
        }
    }
}

/// Typed checks for formats that are not GFF: XML materials, DDS textures, SpeedTree files, ...
fn check_other(
    data: &[u8],
    name: &str,
    ext: &str,
    location: String,
    stats: &mut Stats,
    keep: usize,
) {
    let lower = name.to_ascii_lowercase();
    let result = if ext == "mao" {
        MaterialObject::parse(data)
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else if lower.ends_with(".mat.xml") {
        MaterialDef::parse(data)
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else if lower.ends_with(".sta.xml") {
        MaterialState::parse(data)
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else if ext == "vlm" {
        thedas_formats::vlm::Vlm::parse(data)
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else if ext == "mat" && thedas_formats::bmat::is_bmat(data) {
        thedas_formats::bmat::Bmat::parse(data)
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else if ext == "dds" {
        Dds::parse(data).map(|_| ()).map_err(|e| e.to_string())
    } else if ext == "mtx" {
        thedas_formats::probe::IrradianceProbe::parse(data)
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else if ext == "gfx" {
        return check_gfx(data, location, stats, keep);
    } else if ext == "fxo" {
        return check_shader(data, location, stats, keep);
    } else if ext == "ncs" {
        return check_script(data, location, stats, keep);
    } else if ext == "spt" {
        thedas_formats::speedtree::parse(data)
            .map_err(|e| e.to_string())
            .and_then(|tree| thedas_formats::speedtree_gen::check(&tree))
    } else if ext == "ldf" {
        EngineTable::parse(data)
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else {
        return;
    };
    typed_result(stats, ext, location, result, keep);
}

/// GFx movies: inventory the Scaleform tags and rewrite to plain SWF (without images).
fn check_gfx(data: &[u8], location: String, stats: &mut Stats, keep: usize) {
    let result = thedas_formats::gfx::scan(data).and_then(|inv| {
        for (code, n) in inv.scaleform_tags() {
            let name = thedas_formats::gfx::scaleform_tag_name(code)
                .map_or_else(|| code.to_string(), str::to_string);
            *stats.gfx_tags.entry(name).or_default() += n as u64;
        }
        thedas_formats::gfx::rewrite(data, &mut |_| None, None, None).map(|_| ())
    });
    typed_result(
        stats,
        "gfx",
        location,
        result.map_err(|e| e.to_string()),
        keep,
    );
}

/// Scripts: parse and disassemble, and check every engine call against the engine table
/// (known id, argument count within the declared parameters).
fn check_script(data: &[u8], location: String, stats: &mut Stats, keep: usize) {
    let table = ENGINE_TABLE.get().and_then(|t| t.as_ref());
    let result = Ncs::parse(data).map_err(|e| e.to_string()).inspect(|ncs| {
        let _ = ncs.disassemble(table);
    });
    let es = stats.by_ext.entry("ncs".into()).or_default();
    let ncs = match result {
        Ok(n) => {
            es.typed_ok += 1;
            n
        }
        Err(e) => {
            es.typed_failed += 1;
            stats.fail("typed/ncs", location, e, keep);
            return;
        }
    };
    let Some(table) = table else { return };
    for (id, argc) in ncs.actions() {
        match table.function_for_call(id, argc as usize) {
            None => {
                stats.fail(
                    "script/undeclared-action",
                    location.clone(),
                    format!("engine function #{id} is not in script.ldf"),
                    keep,
                );
            }
            Some(f) if (argc as usize) > f.params.len() || (argc as usize) < f.required() => {
                stats.fail(
                    "script/undeclared-action",
                    location.clone(),
                    format!(
                        "{} called with {argc} arguments, declared with {}..={}",
                        f.name,
                        f.required(),
                        f.params.len()
                    ),
                    keep,
                );
            }
            Some(_) => {}
        }
    }
}

/// Shaders: parse the D3D9 bytecode, translate it to naga IR, validate it and translate
/// that for each GPU backend. Failures are grouped by the missing opcode or feature, or by
/// the backend that refused, so coverage gaps show up as counts.
fn check_shader(data: &[u8], location: String, stats: &mut Stats, keep: usize) {
    let result = match thedas_shader::Shader::parse(data) {
        Err(e) => Err(("parse".to_string(), e.to_string())),
        Ok(shader) => {
            *stats
                .shader_models
                .entry(format!(
                    "{}_{}_{}",
                    if shader.stage == thedas_shader::Stage::Vertex {
                        "vs"
                    } else {
                        "ps"
                    },
                    shader.major,
                    shader.minor
                ))
                .or_default() += 1;
            thedas_shader::translate(&shader, &Default::default())
                .map_err(|e| (e.key(), e.to_string()))
                .and_then(|t| {
                    // What each GPU backend makes of the translation (Direct3D 12's HLSL,
                    // Vulkan's SPIR-V, OpenGL's GLSL): a shader one of them refuses stops
                    // the game on that backend only.
                    match thedas_shader::backends::failures(&t.module, &t.info).first() {
                        None => Ok(()),
                        Some((backend, e)) => Err((format!("backend/{backend:?}"), e.clone())),
                    }
                })
        }
    };
    let es = stats.by_ext.entry("fxo".into()).or_default();
    match result {
        Ok(()) => es.typed_ok += 1,
        Err((key, msg)) => {
            es.typed_failed += 1;
            stats.fail(&format!("shader/{key}"), location, msg, keep);
        }
    }
}

fn check_data(
    data: &[u8],
    name: &str,
    ext: &str,
    location: String,
    stats: &mut Stats,
    keep: usize,
) {
    if is_gff3(data) {
        return check_gff3(data, ext, location, stats, keep);
    }
    if !is_gff4(data) {
        return check_other(data, name, ext, location, stats, keep);
    }
    let es = stats
        .by_ext
        .get_mut(ext)
        .expect("entry counted before check");
    es.gff4 += 1;
    let doc = match Gff4::parse(data) {
        Ok(d) => d,
        Err(e) => {
            stats.fail(&format!("gff4-parse/{ext}"), location, e.to_string(), keep);
            return;
        }
    };
    es.parsed += 1;

    // Round trip: write, read back, and require the same graph.
    match doc.write().map_err(|e| e.to_string()).and_then(|bytes| {
        let back = Gff4::parse(&bytes).map_err(|e| format!("re-parse: {e}"))?;
        Ok((bytes, back))
    }) {
        Ok((bytes, back)) => {
            if bytes == data {
                es.byte_exact += 1;
            }
            if same_graph(&doc, &back) {
                es.round_trip += 1;
            } else {
                stats.fail(
                    &format!("gff4-roundtrip/{ext}"),
                    location.clone(),
                    "re-read document differs".into(),
                    keep,
                );
            }
        }
        Err(e) => stats.fail(&format!("gff4-write/{ext}"), location.clone(), e, keep),
    }

    let s = |r: std::result::Result<(), thedas_formats::TypedError>| r.map_err(|e| e.to_string());
    let typed = match (&doc.file_type.0, ext) {
        (b"G2DA", _) => Some(Gda::from_gff4(&doc).map(|_| ()).map_err(|e| e.to_string())),
        (b"TLK ", _) => Some(Tlk::from_gff4(&doc).map(|_| ()).map_err(|e| e.to_string())),
        (_, "arl") => Some(s(Arl::from_gff4(&doc).map(|_| ()))),
        (_, "rml") => Some(s(Rml::from_gff4(&doc).map(|_| ()))),
        (_, "mmh") => Some(s(Mmh::from_gff4(&doc).map(|_| ()))),
        (_, "tnt") => Some(s(thedas_formats::tint::Tint::from_gff4(&doc).map(|_| ()))),
        (_, "mor") => Some(s(thedas_formats::morph::Morph::from_gff4(&doc).map(|_| ()))),
        (_, "mop") => Some(s(
            thedas_formats::morph::MorphPreset::from_gff4(&doc).map(|_| ())
        )),
        (_, "msh") => Some(s(Msh::from_gff4(&doc).map(|_| ()))),
        (b"MAO ", _) => Some(
            MaterialObject::from_gff4(&doc)
                .map(|_| ())
                .map_err(|e| e.to_string()),
        ),
        (_, "ani") => Some(s(
            thedas_formats::anim::Animation::from_gff(&doc).map(|_| ())
        )),
        (_, "anb") => Some(s(thedas_formats::anim::AnimTree::from_gff(&doc).map(|_| ()))),
        (_, "gad") => Some(s(thedas_formats::anim::AnimInfo::from_gff(&doc).map(|_| ()))),
        (_, "evt") => Some(s(thedas_formats::anim::events_from_gff(&doc).map(|_| ()))),
        (_, "cnv") => Some(
            thedas_sim::cnv::dlg_from_cnv(&doc, "")
                .map(|_| ())
                .ok_or_else(|| format!("not a conversation (file type {})", doc.file_type)),
        ),
        (_, "dlg") => Some(
            thedas_sim::dialog::Dlg::from_gff4(&doc, "")
                .map(|_| ())
                .ok_or_else(|| format!("not a conversation (file type {})", doc.file_type)),
        ),
        (_, "cut" | "cl") => Some({
            // Every cutscene's tracks read, and the camera placed at each cut.
            let tracks = thedas_sim::cuttracks::Tracks::from_gff4(&doc);
            for &(at, _) in &tracks.cuts {
                let _ = tracks.camera_at(
                    at,
                    thedas_sim::cuttracks::Placement::NONE,
                    &thedas_sim::cuttracks::Finders::NONE,
                );
            }
            thedas_sim::cutscene::Cutscene::from_gff4(&doc, "")
                .map(|_| ())
                .ok_or_else(|| format!("not a cutscene (file type {})", doc.file_type))
        }),
        (_, "plo") => Some(if &doc.file_type.0 == b"PLTS" {
            let _ = thedas_sim::plot::plots_from_gff4(&doc, "");
            Ok(())
        } else if &doc.file_type.0 == b"PLOT" {
            // Dragon Age II: one plot, and every composite flag's expression well formed.
            match thedas_sim::plot::plots_from_gff4(&doc, "").first() {
                None => Err("plot without a GUID".to_string()),
                Some(p) => match p.composites.iter().find(|(_, c)| !c.well_formed()) {
                    Some((id, _)) => Err(format!("composite flag {id}: malformed expression")),
                    None => Ok(()),
                },
            }
        } else {
            Err(format!("not a plot file (file type {})", doc.file_type))
        }),
        _ => None,
    };
    if let Some(result) = typed {
        typed_result(stats, ext, location, result, keep);
    }
}

fn check_gff3(data: &[u8], ext: &str, location: String, stats: &mut Stats, keep: usize) {
    if ext == "are" {
        let result = Area::parse(data).map(|_| ()).map_err(|e| e.to_string());
        typed_result(stats, ext, location.clone(), result, keep);
    }
    let es = stats
        .by_ext
        .get_mut(ext)
        .expect("entry counted before check");
    es.gff3 += 1;
    let doc = match Gff3::parse(data) {
        Ok(d) => d,
        Err(e) => {
            stats.fail(&format!("gff3-parse/{ext}"), location, e.to_string(), keep);
            return;
        }
    };
    es.parsed += 1;
    match doc.write().map_err(|e| e.to_string()).and_then(|bytes| {
        let back = Gff3::parse(&bytes).map_err(|e| format!("re-parse: {e}"))?;
        Ok((bytes, back))
    }) {
        Ok((bytes, back)) => {
            if bytes == data {
                es.byte_exact += 1;
            }
            if format!("{doc:?}") == format!("{back:?}") {
                es.round_trip += 1;
            } else {
                stats.fail(
                    &format!("gff3-roundtrip/{ext}"),
                    location,
                    "re-read document differs".into(),
                    keep,
                );
            }
        }
        Err(e) => stats.fail(&format!("gff3-write/{ext}"), location, e, keep),
    }
}

/// Structural equality. Debug formatting prints floats exactly and treats NaN as equal to
/// NaN, which is what a round-trip check wants.
fn same_graph(a: &Gff4, b: &Gff4) -> bool {
    a.root == b.root
        && a.file_type == b.file_type
        && a.file_version == b.file_version
        && a.templates == b.templates
        && a.structs.len() == b.structs.len()
        && a.structs.iter().zip(&b.structs).all(|(x, y)| {
            x.template == y.template && format!("{:?}", x.values) == format!("{:?}", y.values)
        })
}

fn print_summary(stats: &Stats, secs: f64) -> bool {
    println!(
        "\nArchives: {} opened, {} failed. Loose GFF4 files: {}. Versions: {:?}",
        stats.archives - stats.archives_failed,
        stats.archives_failed,
        stats.loose_files,
        stats.archive_versions
    );
    if !stats.gfx_tags.is_empty() {
        println!("Scaleform tags: {:?}", stats.gfx_tags);
    }
    if !stats.shader_models.is_empty() {
        println!("Shader models: {:?}", stats.shader_models);
    }
    println!(
        "\n{:<8} {:>9} {:>10} {:>9} {:>8} {:>7} {:>7} {:>9} {:>10} {:>10} {:>8}",
        "ext",
        "entries",
        "MB",
        "encrypted",
        "readerr",
        "gff3",
        "gff4",
        "parsed",
        "roundtrip",
        "byteexact",
        "typed"
    );
    let mut t = ExtStats::default();
    for (ext, s) in &stats.by_ext {
        println!(
            "{:<8} {:>9} {:>10.1} {:>9} {:>8} {:>7} {:>7} {:>9} {:>10} {:>10} {:>8}",
            if ext.is_empty() { "(none)" } else { ext },
            s.entries,
            s.bytes as f64 / 1e6,
            s.encrypted,
            s.read_failed,
            s.gff3,
            s.gff4,
            s.parsed,
            s.round_trip,
            s.byte_exact,
            if s.typed_ok + s.typed_failed > 0 {
                format!("{}/{}", s.typed_ok, s.typed_ok + s.typed_failed)
            } else {
                String::new()
            }
        );
        t.entries += s.entries;
        t.bytes += s.bytes;
        t.encrypted += s.encrypted;
        t.read_failed += s.read_failed;
        t.gff3 += s.gff3;
        t.gff4 += s.gff4;
        t.parsed += s.parsed;
        t.round_trip += s.round_trip;
        t.byte_exact += s.byte_exact;
    }
    println!(
        "{:<8} {:>9} {:>10.1} {:>9} {:>8} {:>7} {:>7} {:>9} {:>10} {:>10}",
        "TOTAL",
        t.entries,
        t.bytes as f64 / 1e6,
        t.encrypted,
        t.read_failed,
        t.gff3,
        t.gff4,
        t.parsed,
        t.round_trip,
        t.byte_exact
    );
    if !stats.failure_counts.is_empty() {
        println!("\nFailures:");
        for (cat, n) in &stats.failure_counts {
            println!("  {cat}: {n}");
            for (loc, msg) in stats.failures.get(cat).into_iter().flatten() {
                println!("    {loc}: {msg}");
            }
        }
    }
    let ok = stats
        .failure_counts
        .keys()
        .all(|k| NON_FATAL.contains(&k.as_str()));
    println!("\n{} in {secs:.0}s", if ok { "PASS" } else { "FAIL" });
    ok
}

fn to_report(stats: &Stats, ok: bool) -> serde_json::Value {
    let by_ext: serde_json::Map<String, serde_json::Value> = stats
        .by_ext
        .iter()
        .map(|(k, s)| {
            (
                k.clone(),
                json!({
                    "entries": s.entries, "bytes": s.bytes, "encrypted": s.encrypted,
                    "read_failed": s.read_failed, "gff3": s.gff3, "gff4": s.gff4, "parsed": s.parsed,
                    "round_trip": s.round_trip, "byte_exact": s.byte_exact,
                    "typed_ok": s.typed_ok, "typed_failed": s.typed_failed,
                }),
            )
        })
        .collect();
    json!({
        "pass": ok,
        "archives": stats.archives,
        "archives_failed": stats.archives_failed,
        "archive_versions": stats.archive_versions,
        "shader_models": stats.shader_models,
        "gfx_tags": stats.gfx_tags,
        "loose_gff4_files": stats.loose_files,
        "by_extension": by_ext,
        "failure_counts": stats.failure_counts,
        "failure_examples": stats.failures,
    })
}
