//! `thedas`: command-line tools for inspecting and validating Dragon Age: Origins data.

mod corpus;
mod dump;
mod script;

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
        Command::Dump { target, raw } => {
            let data = load_target(&target)?;
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
