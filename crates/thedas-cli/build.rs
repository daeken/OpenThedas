//! Keep replay's build identity in step with the app's journal header.
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
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/logs/HEAD");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
