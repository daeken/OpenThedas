//! Records the commit the app is built from, for issue reports (`report::build_info`).
//! Without git it is not set.
//!
//! On Windows it also gives the executable the OpenThedas emblem as its icon
//! (`assets/icon/openthedas.rc`). Best effort: a machine with no resource compiler (`windres`,
//! `llvm-rc`) builds the game without the icon and says so; the window still shows the emblem
//! at run time.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    if let Some(commit) = git(&["rev-parse", "HEAD"]) {
        println!("cargo:rustc-env=THEDAS_GIT_COMMIT={commit}");
    }
    // Built again when HEAD moves (a commit, a checkout, a merge); in a worktree the
    // reflog is the worktree's own.
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/logs/HEAD");
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon/openthedas.rc");
    println!("cargo:rerun-if-changed=assets/icon/openthedas.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        if let Err(e) = embed_resource::compile("assets/icon/openthedas.rc", embed_resource::NONE)
            .manifest_optional()
        {
            println!("cargo:warning=no executable icon: {e}");
        }
    }
}
