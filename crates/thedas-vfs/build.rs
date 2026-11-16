use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/unrar_windows.cpp");

    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Cargo sets CARGO_CFG_TARGET_OS");
    let host = env::var("HOST").expect("Cargo sets HOST");
    let windows_host = host.split('-').any(|component| component == "windows");

    // unrar_sys 0.5.8 selects isnt.cpp and powrprof using the build host's cfg.
    // Supply the missing symbols only when cross-compiling from a non-Windows host.
    if target_os == "windows" && !windows_host {
        cc::Build::new()
            .cpp(true)
            .std("c++14")
            .cpp_link_stdlib(None)
            .file("src/unrar_windows.cpp")
            .compile("thedas_unrar_windows");
        println!("cargo:rustc-link-lib=powrprof");
    }
}
