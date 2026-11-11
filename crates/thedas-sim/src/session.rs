//! Opening a simulation session from an install: the resource stack, the engine table (with
//! inferred additions), the module and an area. Shared by `thedas sim` and the MCP server.

use std::path::Path;
use std::sync::Arc;

use thedas_script::EngineTable;
use thedas_vfs::DaoInstall;
use thedas_vfs::install::TextureQuality;

use crate::sim::{Sim, SimError};

#[derive(Clone, Debug)]
pub struct SessionOptions {
    pub install: std::path::PathBuf,
    pub module: String,
    /// Area resource; the module's start area when `None`.
    pub area: Option<String>,
    pub waypoint: Option<String>,
    pub seed: u64,
    /// Keep full trace entries (otherwise only counters).
    pub keep_trace: bool,
    /// What happens to cutscenes scripts load, from the module's first scripts on.
    pub cutscenes: crate::sim::CutscenePolicy,
    /// What happens when a script starts character creation, from the module's first
    /// scripts on (a new game starts it while the module loads).
    pub chargen: crate::sim::ChargenPolicy,
}

/// Opens the install and loads the module and area.
pub fn open(o: &SessionOptions) -> Result<Sim, SimError> {
    let install = DaoInstall::open(&o.install)?;
    let mut vfs = install.global_vfs(TextureQuality::High)?;
    let (data, _) = vfs
        .read("script.ldf")
        .map_err(|_| SimError::NotFound("script.ldf".into()))?;
    let table = EngineTable::parse(&data)
        .map_err(|e| SimError::Parse("script.ldf".into(), e.to_string()))?
        .with_inferred();
    // Mount the area's level archives so lookups match the renderer's.
    if let Some(area) = &o.area {
        if let Ok((data, _)) = vfs.read(&format!("{area}.are")) {
            if let Ok(Some(layout)) = thedas_formats::Area::parse(&data).map(|a| a.layout) {
                let _ = install.mount_level(&mut vfs, &layout);
            }
        }
    }
    let mut sim = Sim::new(Arc::new(vfs), Arc::new(table), o.seed);
    sim.world.trace.keep = o.keep_trace;
    sim.cutscene_policy = o.cutscenes;
    sim.chargen_policy = o.chargen.clone();
    sim.load_module(install.root(), &o.module)?;
    let area = o
        .area
        .clone()
        .or_else(|| sim.module_info.start_area.clone());
    if let Some(area) = area {
        sim.enter_area(&area, o.waypoint.as_deref())?;
    }
    Ok(sim)
}

/// The install root from `THEDAS_INSTALL`, if set.
pub fn install_from_env() -> Option<std::path::PathBuf> {
    std::env::var_os("THEDAS_INSTALL").map(std::path::PathBuf::from)
}

pub fn exists(p: &Path) -> bool {
    p.join("packages").join("core").is_dir()
}
