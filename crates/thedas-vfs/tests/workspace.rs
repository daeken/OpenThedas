//! The workspace (mod mode's live edits) is the process's, so its tests live here, in a
//! process of their own, and take turns through one lock.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use thedas_vfs::{Vfs, WORKSPACE_LABEL};

static TURN: Mutex<()> = Mutex::new(());

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("thedas-vfs-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A play stack: a game folder, a mod above it in the overlay.
fn play_stack(base: &std::path::Path) -> Vfs {
    let (game, mods) = (base.join("game"), base.join("mods"));
    std::fs::create_dir_all(&game).unwrap();
    std::fs::create_dir_all(&mods).unwrap();
    std::fs::write(game.join("a.dds"), b"game").unwrap();
    std::fs::write(mods.join("a.dds"), b"mod").unwrap();
    let mut vfs = Vfs::new();
    vfs.push_directory(&game, false, "game").unwrap();
    let mut overlay = Vfs::new();
    overlay.push_directory(&mods, false, "mod:a").unwrap();
    vfs.set_overlay(overlay);
    vfs.play_from();
    vfs
}

#[test]
fn the_workspace_beats_every_layer_and_rescans() {
    let _turn = TURN.lock().unwrap_or_else(|p| p.into_inner());
    let base = scratch("workspace");
    let ws = base.join("ws");
    std::fs::create_dir_all(ws.join("sub")).unwrap();
    let vfs = Arc::new(play_stack(&base));
    assert_eq!(vfs.read("a.dds").unwrap().0, b"mod");
    assert!(Vfs::workspace().is_none());

    assert_eq!(Vfs::set_workspace(Some(&ws)), 0);
    assert_eq!(vfs.read("a.dds").unwrap().0, b"mod");
    std::fs::write(ws.join("sub").join("A.DDS"), b"live").unwrap();
    // A new name is seen once the folder is scanned again.
    assert_eq!(Vfs::rescan_workspace(), 1);
    let (data, p) = vfs.read("a.dds").unwrap();
    assert_eq!(data, b"live");
    assert_eq!(p.layer_label, WORKSPACE_LABEL);
    assert_eq!(p.layer, 2);
    assert_eq!(vfs.hits("a.dds").len(), 3);
    assert_eq!(vfs.lookup("a.dds").unwrap().layer_label, WORKSPACE_LABEL);
    // Beneath it, the layers' own.
    assert_eq!(vfs.read_beneath_workspace("a.dds").unwrap().0, b"mod");
    // A changed file is read afresh without a scan.
    std::fs::write(ws.join("sub").join("A.DDS"), b"live 2").unwrap();
    assert_eq!(vfs.read("a.dds").unwrap().0, b"live 2");
    assert_eq!(Vfs::workspace().unwrap().1, 1);
    // The player's layers (report.json's `mods`) name it while it is mounted.
    let layers = thedas_vfs::mods::player_layers(&vfs);
    assert!(
        layers
            .iter()
            .any(|l| l.label == WORKSPACE_LABEL && l.files == 1)
    );
    // A removed file falls through to the layers.
    std::fs::remove_file(ws.join("sub").join("A.DDS")).unwrap();
    assert_eq!(vfs.read("a.dds").unwrap().0, b"mod");
    std::fs::write(ws.join("b.gda"), b"new").unwrap();
    Vfs::rescan_workspace();
    assert!(vfs.names().contains(&"b.gda".to_string()));
    Vfs::set_workspace(None);
    assert!(Vfs::workspace().is_none());
    assert!(vfs.read("b.gda").is_err());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn stacks_the_game_does_not_play_from_ignore_it() {
    let _turn = TURN.lock().unwrap_or_else(|p| p.into_inner());
    let base = scratch("workspace-other");
    let ws = base.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("a.dds"), b"live").unwrap();
    let game = base.join("game");
    std::fs::create_dir_all(&game).unwrap();
    std::fs::write(game.join("a.dds"), b"game").unwrap();
    let mut tool = Vfs::new();
    tool.push_directory(&game, false, "game").unwrap();
    Vfs::set_workspace(Some(&ws));
    assert_eq!(tool.read("a.dds").unwrap().0, b"game");
    assert_eq!(tool.hits("a.dds").len(), 1);
    assert!(!tool.names().contains(&"b.gda".to_string()));
    assert_eq!(play_stack(&base).read("a.dds").unwrap().0, b"live");
    Vfs::set_workspace(None);
    let _ = std::fs::remove_dir_all(&base);
}
