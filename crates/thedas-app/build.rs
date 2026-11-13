//! Records the commit the app is built from (and whether the tree had changes), for issue
//! reports (`report::build_info`). Without git, neither is set.

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
    if let Some(status) = git(&["status", "--porcelain", "--untracked-files=no"]) {
        let dirty = if status.is_empty() { "0" } else { "1" };
        println!("cargo:rustc-env=THEDAS_GIT_DIRTY={dirty}");
    }
    // Built again when HEAD moves (a commit, a checkout, a merge); in a worktree the
    // reflog is the worktree's own.
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/logs/HEAD");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
