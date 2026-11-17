//! Opening a simulation session from an install: the resource stack, the engine table (with
//! inferred additions), the module and an area. Shared by `thedas sim` and the MCP server.

use std::path::Path;
use std::sync::Arc;

use thedas_script::EngineTable;
use thedas_vfs::install::TextureQuality;
use thedas_vfs::{DaoInstall, Vfs};

use crate::sim::{Sim, SimError};

#[derive(Clone, Debug)]
pub struct SessionOptions {
    pub install: std::path::PathBuf,
    /// The run's unmounted play stack, when the caller already opened it. Sessions clone
    /// it before mounting levels, sharing archive handles and preserving mods/workspace.
    /// Standalone callers can leave this unset to open their own play stack.
    pub base_vfs: Option<Arc<Vfs>>,
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
    let mut vfs = match &o.base_vfs {
        Some(base) => base.as_ref().clone(),
        None => install.play_vfs(TextureQuality::High)?,
    };
    let (data, _) = vfs
        .read("script.ldf")
        .map_err(|_| SimError::NotFound("script.ldf".into()))?;
    let mut table = EngineTable::parse(&data)
        .map_err(|e| SimError::Parse("script.ldf".into(), e.to_string()))?
        .with_inferred();
    if install.game() == thedas_vfs::Game::DragonAge2 {
        add_dragon_age_2_constants(&mut table);
    }
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
    sim.game = install.game();
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

/// Opens the install with no module loaded: the start menu's simulation, which has the
/// game's resources (strings, tables) and nothing running.
pub fn open_bare(o: &SessionOptions) -> Result<Sim, SimError> {
    let install = DaoInstall::open(&o.install)?;
    let vfs = match &o.base_vfs {
        Some(base) => base.clone(),
        None => Arc::new(install.play_vfs(TextureQuality::High)?),
    };
    let (data, _) = vfs
        .read("script.ldf")
        .map_err(|_| SimError::NotFound("script.ldf".into()))?;
    let mut table = EngineTable::parse(&data)
        .map_err(|e| SimError::Parse("script.ldf".into(), e.to_string()))?
        .with_inferred();
    if install.game() == thedas_vfs::Game::DragonAge2 {
        add_dragon_age_2_constants(&mut table);
    }
    let mut sim = Sim::new(vfs, Arc::new(table), o.seed);
    sim.game = install.game();
    Ok(sim)
}

/// Constants the simulation reads by name that Dragon Age II's `script.ldf` does not
/// declare (its scripts have them compiled in): the creature property ids, which are the
/// rows of DA2's `properties` table (`Health` 7, `Mana` 10, `Experience` 35, `Level` 36;
/// DA2 keeps mana and stamina in one property). One the table declares is left as it is.
/// Provisional: read from the shipped table, not traced in DragonAge2.exe.
fn add_dragon_age_2_constants(table: &mut EngineTable) {
    use thedas_script::ldf::{ConstValue, Constant, Type};
    for (name, value) in [
        ("PROPERTY_DEPLETABLE_HEALTH", 7),
        ("PROPERTY_DEPLETABLE_MANA_STAMINA", 10),
        ("PROPERTY_SIMPLE_EXPERIENCE", 35),
        ("PROPERTY_SIMPLE_LEVEL", 36),
    ] {
        if !table.constants.iter().any(|c| c.name == name) {
            table.constants.push(Constant {
                ty: Type::Int,
                name: name.to_string(),
                value: ConstValue::Int(value),
            });
        }
    }
}

/// The install root from `THEDAS_INSTALL`, if set.
pub fn install_from_env() -> Option<std::path::PathBuf> {
    std::env::var_os("THEDAS_INSTALL").map(std::path::PathBuf::from)
}

pub fn exists(p: &Path) -> bool {
    thedas_vfs::join_ci(p, "packages/core").is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_sessions_reuse_the_supplied_play_stack() {
        let root = std::env::temp_dir().join(format!("thedas-session-base-{}", std::process::id()));
        std::fs::create_dir_all(root.join("packages/core")).unwrap();
        let resources = root.join("resources");
        std::fs::create_dir_all(&resources).unwrap();
        std::fs::write(resources.join("script.ldf"), b"int TEST_CONSTANT = 7;\n").unwrap();
        let mut base = thedas_vfs::Vfs::new();
        base.push_directory(&resources, false, "run stack").unwrap();
        let base = Arc::new(base);
        let opts = SessionOptions {
            install: root.clone(),
            base_vfs: Some(base.clone()),
            module: "unused".into(),
            area: None,
            waypoint: None,
            seed: 1,
            keep_trace: false,
            cutscenes: Default::default(),
            chargen: Default::default(),
        };
        // The install has no script.ldf: rebuilding its stack would fail.
        for _ in 0..4 {
            let sim = open_bare(&opts).unwrap();
            assert!(Arc::ptr_eq(&sim.vfs, &base));
            assert_eq!(crate::sim::constant(&sim.table, "TEST_CONSTANT"), 7);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dragon_age_2_gets_the_property_ids_its_script_ldf_lacks() {
        let mut table =
            EngineTable::parse(b"int PROPERTY_SIMPLE_LEVEL = 99;\nint PROPERTY_VALUE_TOTAL = 1;\n")
                .unwrap();
        add_dragon_age_2_constants(&mut table);
        let c = |n: &str| crate::sim::constant(&table, n);
        assert_eq!(c("PROPERTY_DEPLETABLE_HEALTH"), 7);
        assert_eq!(c("PROPERTY_DEPLETABLE_MANA_STAMINA"), 10);
        assert_eq!(c("PROPERTY_SIMPLE_EXPERIENCE"), 35);
        // One the table declares stays as declared.
        assert_eq!(c("PROPERTY_SIMPLE_LEVEL"), 99);
    }
}
