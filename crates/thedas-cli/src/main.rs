//! `thedas`: command-line tools for inspecting and validating Dragon Age: Origins data.

mod corpus;
mod dump;

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
    /// Print a resource in readable form. Use `archive.erf:entry.gda` for an archive entry.
    Dump {
        target: String,
        /// Print GDA and TLK files as their raw GFF4 tree.
        #[arg(long)]
        raw: bool,
    },
    /// Show which layers of an install provide a resource, winner first.
    Resolve { install: PathBuf, name: String },
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
        Command::Resolve { install, name } => resolve(&install, &name),
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

fn resolve(install: &Path, name: &str) -> Result<()> {
    let install = DaoInstall::open(install)?;
    let vfs = install.global_vfs(TextureQuality::High)?;
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
