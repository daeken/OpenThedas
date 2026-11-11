//! `thedas`: command-line tools for inspecting and validating Dragon Age: Origins data.

mod autoplay;
mod corpus;
mod dump;
mod script;
mod sim;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use thedas_vfs::install::TextureQuality;
use thedas_vfs::{DaoInstall, Erf};

#[derive(Parser)]
#[command(
    name = "thedas",
    version,
    about = "Inspect and validate Dragon Age: Origins data"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List the entries of an ERF or RIM archive.
    Ls { archive: PathBuf },
    /// Extract entries from an archive (all of them if no names are given).
    Extract {
        archive: PathBuf,
        names: Vec<String>,
        #[arg(short, long, default_value = ".")]
        out: PathBuf,
    },
    /// Print a resource in readable form (GFF3 and GFF4 as JSON, GDA and TLK as tables). Use
    /// `archive.erf:entry.gda` for an archive entry.
    Dump {
        target: String,
        /// Print GDA and TLK files as their raw GFF4 tree.
        #[arg(long)]
        raw: bool,
        /// Look `target` up by resource name in this install's resource stack.
        #[arg(long)]
        install: Option<PathBuf>,
    },
    /// Show which layers of an install provide a resource, winner first.
    Resolve {
        install: PathBuf,
        name: String,
        /// Also mount this level's archives (e.g. `lgt101d`), as when its area is loaded.
        #[arg(long)]
        level: Option<String>,
    },
    /// Compiled scripts: disassemble, run, or rank engine functions by use.
    Script {
        #[command(subcommand)]
        command: ScriptCommand,
    },
    /// Run the headless simulation: load a module and area, tick, report.
    Sim {
        install: PathBuf,
        /// Module folder under `modules/`.
        #[arg(long, default_value = "single player")]
        module: String,
        /// Area resource (e.g. cir110ar_inn); defaults to the module's start area.
        #[arg(long)]
        area: Option<String>,
        /// Waypoint tag to place the hero at.
        #[arg(long)]
        waypoint: Option<String>,
        #[arg(long, default_value_t = 100)]
        ticks: u64,
        #[arg(long, default_value_t = 50)]
        tick_ms: u64,
        /// Write the trace as JSON lines here.
        #[arg(long)]
        trace: Option<PathBuf>,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Talk to the creature with this tag after ticking.
        #[arg(long)]
        talk: Option<String>,
        /// Replies to pick, by index, in order (default: always the first).
        #[arg(long, value_delimiter = ',')]
        replies: Vec<usize>,
    },
    /// Play a scenario from a save over several seeds (simple deterministic player: party AI,
    /// attack what is seen, seek the nearest hostile, take doors to new areas) and report hero
    /// deaths and who was alive at given times.
    Autoplay {
        install: PathBuf,
        /// A save made with the MCP's save_state.
        save: PathBuf,
        #[arg(long, default_value = "single player")]
        module: String,
        #[arg(long, default_value_t = 10)]
        seeds: u64,
        #[arg(long, default_value_t = 1)]
        first_seed: u64,
        /// Game minutes before a run gives up.
        #[arg(long, default_value_t = 30)]
        minutes: u64,
        /// Stop when this plot flag is set (`plot:flag`, plot by resource name or GUID).
        #[arg(long)]
        until_flag: Option<String>,
        /// Stop on reaching this area.
        #[arg(long)]
        until_area: Option<String>,
        /// Take doors only to new areas whose tag starts with this (e.g. `pre4`).
        #[arg(long)]
        exit_prefix: Option<String>,
        /// Time the samples from when this plot flag is first set.
        #[arg(long)]
        mark_flag: Option<String>,
        /// Creature tags whose survival to report.
        #[arg(long, value_delimiter = ',')]
        watch: Vec<String>,
        /// Seconds after the mark to sample survival at.
        #[arg(long, value_delimiter = ',', default_value = "30,60,120")]
        at: Vec<u64>,
        /// One line per run.
        #[arg(long)]
        verbose: bool,
        /// With --steer: leave the hero's fighting to the party AI (autopilot tactics) too.
        #[arg(long)]
        ai_hero: bool,
        /// The older harness player: order the hero's attacks and walk the party towards
        /// hostiles in view. Without it the party AI alone fights (the standard measurement).
        #[arg(long)]
        steer: bool,
        /// First walk the hero up to the creature with this tag (to start an encounter that
        /// begins on approach).
        #[arg(long)]
        approach: Option<String>,
        /// Report how the party and its foes spent the fighting time, and their attacks.
        #[arg(long)]
        activity: bool,
        /// Followers waiting in the party pool get the experience the hero gets (provisional:
        /// no script does it).
        #[arg(long)]
        pool_xp: bool,
        /// End a run ("won") once this many fights are won (every hostile seen in it is down).
        #[arg(long, default_value_t = 0)]
        fights: u32,
        /// Write the last run's full trace here (JSON lines).
        #[arg(long)]
        trace: Option<PathBuf>,
        /// Add the runs' coverage counters (unimplemented, stubbed and presentation-only
        /// engine calls, missing 2DA tables and columns) to this JSON file.
        #[arg(long)]
        coverage: Option<PathBuf>,
        /// Write the last run's end state here.
        #[arg(long)]
        save_to: Option<PathBuf>,
    },
    /// Summarise a layout's pathing grid and optionally write it as a PGM image.
    Navgrid {
        install: PathBuf,
        /// Layout name (`hrt201n`) or area resource (`bhn200ar_castle_after_siege`).
        layout: String,
        /// Write the grid's data bytes as a greyscale PGM here.
        #[arg(long)]
        pgm: Option<PathBuf>,
        /// Print the cells around these world positions (`x,y;x,y`).
        #[arg(long)]
        at: Option<String>,
        /// Route between two world positions (`x,y,z;x,y,z`), reporting how much of the
        /// grid is reachable from the first when there is no route.
        #[arg(long)]
        route: Option<String>,
    },
    /// Open every archive in an install and parse every GFF4-family resource.
    Corpus {
        /// Install roots or individual archives.
        paths: Vec<PathBuf>,
        /// Write a JSON report here.
        #[arg(long)]
        report: Option<PathBuf>,
        /// Worker threads.
        #[arg(short, long, default_value_t = default_jobs())]
        jobs: usize,
        /// Example failures to print per category.
        #[arg(long, default_value_t = 5)]
        examples: usize,
    },
}

#[derive(Subcommand)]
enum ScriptCommand {
    /// Print a script's instructions with engine function names.
    Disasm {
        install: PathBuf,
        /// A script resource (`name` or `name.ncs`) or a file.
        script: String,
    },
    /// Run a script against an engine that implements nothing, printing every engine call.
    Run {
        install: PathBuf,
        script: String,
        /// Object id for OBJECT_SELF.
        #[arg(long, default_value_t = 0)]
        caller: u32,
    },
    /// Rank engine functions by static call count across every script in the install.
    Stats {
        install: PathBuf,
        /// Write a JSON report here (and Markdown beside it).
        #[arg(long)]
        report: Option<PathBuf>,
    },
}

fn default_jobs() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

fn main() {
    if let Err(e) = run() {
        // A closed pipe (`thedas ls x.erf | head`) is not an error worth reporting.
        let broken_pipe = e
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe);
        if !broken_pipe {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Ls { archive } => ls(&archive),
        Command::Extract {
            archive,
            names,
            out,
        } => extract(&archive, &names, &out),
        Command::Dump {
            target,
            raw,
            install,
        } => {
            let data = match install {
                Some(root) => {
                    let vfs = DaoInstall::open(&root)?.global_vfs(TextureQuality::High)?;
                    vfs.read(&target)
                        .with_context(|| format!("{target}: not in the resource stack"))?
                        .0
                }
                None => load_target(&target)?,
            };
            dump::dump(&data, raw, &mut std::io::stdout().lock())
        }
        Command::Resolve {
            install,
            name,
            level,
        } => resolve(&install, &name, level.as_deref()),
        Command::Script { command } => match command {
            ScriptCommand::Disasm { install, script } => script::disasm(&install, &script),
            ScriptCommand::Run {
                install,
                script,
                caller,
            } => script::run(&install, &script, caller),
            ScriptCommand::Stats { install, report } => script::stats(&install, report.as_deref()),
        },
        Command::Sim {
            install,
            module,
            area,
            waypoint,
            ticks,
            tick_ms,
            trace,
            seed,
            talk,
            replies,
        } => sim::run(&sim::Options {
            install: &install,
            module: &module,
            area: area.as_deref(),
            waypoint: waypoint.as_deref(),
            ticks,
            tick_ms,
            trace: trace.as_deref(),
            seed,
            talk: talk.as_deref(),
            replies: &replies,
        }),
        Command::Autoplay {
            install,
            save,
            module,
            seeds,
            first_seed,
            minutes,
            until_flag,
            until_area,
            exit_prefix,
            mark_flag,
            watch,
            at,
            verbose,
            ai_hero,
            steer,
            approach,
            activity,
            pool_xp,
            fights,
            trace,
            coverage,
            save_to,
        } => autoplay::run(&autoplay::Options {
            install: &install,
            module: &module,
            save: &save,
            seeds,
            first_seed,
            minutes,
            until_flag: until_flag.as_deref(),
            until_area: until_area.as_deref(),
            exit_prefix: exit_prefix.as_deref(),
            mark_flag: mark_flag.as_deref(),
            watch: &watch,
            at: &at,
            verbose,
            ai_hero,
            steer,
            approach: approach.as_deref(),
            activity,
            pool_xp,
            fights,
            trace_to: trace.as_deref(),
            coverage_to: coverage.as_deref(),
            save_to: save_to.as_deref(),
        }),
        Command::Navgrid {
            install,
            layout,
            pgm,
            at,
            route,
        } => navgrid(
            &install,
            &layout,
            pgm.as_deref(),
            at.as_deref(),
            route.as_deref(),
        ),
        Command::Corpus {
            paths,
            report,
            jobs,
            examples,
        } => {
            if paths.is_empty() {
                bail!("give at least one install root or archive");
            }
            let ok = corpus::run(&paths, report.as_deref(), jobs.max(1), examples)?;
            if !ok {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

fn ls(path: &Path) -> Result<()> {
    use std::io::Write;
    let erf = Erf::open(path)?;
    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "{}: ERF {} | {} entries | encryption {:?} | compression {:?}",
        path.display(),
        erf.version.as_str(),
        erf.entries.len(),
        erf.encryption,
        erf.compression
    )?;
    for e in &erf.entries {
        let packed = if e.packed_size != e.size {
            e.packed_size.to_string()
        } else {
            String::new()
        };
        writeln!(out, "{:>10} {:>10}  {}", e.size, packed, e.display_name())?;
    }
    Ok(())
}

fn extract(path: &Path, names: &[String], out: &Path) -> Result<()> {
    let erf = Erf::open(path)?;
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let mut count = 0;
    for e in &erf.entries {
        let name = e.display_name();
        if !names.is_empty() && !names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            continue;
        }
        if name.contains(['/', '\\']) || name.starts_with('.') {
            bail!("refusing to extract suspicious entry name {name:?}");
        }
        let data = erf.read(e, None)?;
        std::fs::write(out.join(&name), data)?;
        count += 1;
    }
    eprintln!("extracted {count} entries to {}", out.display());
    Ok(())
}

/// Loads `file` or `archive:entry`.
fn load_target(target: &str) -> Result<Vec<u8>> {
    if Path::new(target).is_file() {
        return std::fs::read(target).with_context(|| format!("reading {target}"));
    }
    let Some((archive, entry)) = target.rsplit_once(':') else {
        bail!("{target}: no such file");
    };
    let erf = Erf::open(archive)?;
    let e = erf
        .find(entry)
        .with_context(|| format!("{archive} has no entry {entry}"))?;
    Ok(erf.read(e, None)?)
}

fn resolve(install: &Path, name: &str, level: Option<&str>) -> Result<()> {
    let install = DaoInstall::open(install)?;
    let mut vfs = install.global_vfs(TextureQuality::High)?;
    if let Some(level) = level {
        install.mount_level(&mut vfs, level)?;
    }
    let hits = vfs.hits(name);
    if hits.is_empty() {
        bail!("{name}: not found in the global resource stack");
    }
    for (i, h) in hits.iter().enumerate() {
        let marker = if i == 0 { "wins" } else { "shadowed" };
        match &h.entry {
            Some(entry) => println!(
                "{marker:>8}  layer {:>3}  {}  ({})",
                h.layer,
                install.relative(&h.file),
                entry
            ),
            None => println!(
                "{marker:>8}  layer {:>3}  {}",
                h.layer,
                install.relative(&h.file)
            ),
        }
    }
    Ok(())
}

fn navgrid(
    install: &std::path::Path,
    layout: &str,
    pgm: Option<&std::path::Path>,
    at: Option<&str>,
    route: Option<&str>,
) -> anyhow::Result<()> {
    use anyhow::Context;
    let install = thedas_vfs::DaoInstall::open(install)?;
    let mut vfs = install.global_vfs(thedas_vfs::install::TextureQuality::High)?;
    let layout = match vfs.read(&format!("{layout}.are")) {
        Ok((data, _)) => thedas_formats::Area::parse(&data)?
            .layout
            .context("area names no layout")?,
        Err(_) => layout.to_string(),
    };
    install.mount_level(&mut vfs, &layout)?;
    let (data, _) = vfs
        .read(&format!("{layout}.arl"))
        .with_context(|| format!("{layout}.arl"))?;
    let arl = thedas_formats::Arl::parse(&data)?;
    let g = arl.grid.context("the layout has no pathing grid")?;
    let mut hist = std::collections::BTreeMap::<u8, usize>::new();
    for &b in &g.data {
        *hist.entry(b).or_default() += 1;
    }
    let (lo, hi) = g
        .heights
        .iter()
        .fold((f32::MAX, f32::MIN), |(l, h), &x| (l.min(x), h.max(x)));
    println!(
        "{layout}: {}x{} cells of {} m from {:?}, clearance {}, heights {lo}..{hi}",
        g.columns, g.rows, g.cell_size, g.base, g.clearance
    );
    println!("data byte histogram: {hist:?}");
    for p in at.unwrap_or("").split(';').filter(|s| !s.is_empty()) {
        let v: Vec<f32> = p.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        if v.len() < 2 {
            continue;
        }
        match g.cell([v[0], v[1], 0.0]) {
            Some((cx, cy)) => {
                for dy in -1i32..=1 {
                    let row: Vec<String> = (-1i32..=1)
                        .map(|dx| {
                            let c = (
                                (cx as i32 + dx).max(0) as u32,
                                (cy as i32 - dy).max(0) as u32,
                            );
                            format!("{}@{:.2}", g.value(c), g.center(c)[2])
                        })
                        .collect();
                    println!("  ({},{}) {}", v[0], v[1], row.join("  "));
                }
            }
            None => println!("  ({},{}) outside the grid", v[0], v[1]),
        }
    }
    if let Some(r) = route {
        let pts: Vec<[f32; 3]> = r
            .split(';')
            .filter_map(|p| {
                let v: Vec<f32> = p.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                (v.len() == 3).then(|| [v[0], v[1], v[2]])
            })
            .collect();
        anyhow::ensure!(pts.len() == 2, "--route wants x,y,z;x,y,z");
        let (from, to) = (pts[0], pts[1]);
        match thedas_sim::nav::route(&g, from, to) {
            Some(r) => println!("route: {} points", r.len()),
            None => {
                let reach = thedas_sim::nav::region(&g, from);
                let n = reach.iter().filter(|&&b| b).count();
                let closest = (0..g.rows)
                    .flat_map(|y| (0..g.columns).map(move |x| (x, y)))
                    .filter(|&(x, y)| reach.get((y * g.columns + x) as usize) == Some(&true))
                    .map(|c| {
                        let m = g.center(c);
                        let d = ((m[0] - to[0]).powi(2) + (m[1] - to[1]).powi(2)).sqrt();
                        (d, c, thedas_sim::nav::height(&g, c))
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                println!("no route; {n} cells reachable from the start");
                if let Some((d, c, h)) = closest {
                    let m = g.center(c);
                    println!(
                        "closest reachable point to the goal: ({:.1},{:.1}) at {h:.2} m, {d:.1} m away",
                        m[0], m[1]
                    );
                }
            }
        }
    }
    if let Some(path) = pgm {
        let mut out = format!("P5\n{} {}\n255\n", g.columns, g.rows).into_bytes();
        // Rows top to bottom with +Y up.
        for y in (0..g.rows).rev() {
            for x in 0..g.columns {
                out.push(g.value((x, y)));
            }
        }
        std::fs::write(path, out)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}
