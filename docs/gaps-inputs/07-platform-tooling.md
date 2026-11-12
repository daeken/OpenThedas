# 07: Platform and tooling

Gathered 2026-11-12 on main 1eb844a from the source, the Cargo manifests and the GitHub API (read only). It has only been run on macOS
(Apple M4, Metal). Nothing here was tried on Windows or Linux, so every portability statement is inferred from the code.

## What is macOS-specific or untested elsewhere

- **Movies (Bink .bik) need an external ffmpeg.** `crates/thedas-app/src/intro.rs` `find_ffmpeg()` searches PATH, then `/opt/homebrew/bin` and
  `/usr/local/bin`, for a file named exactly `ffmpeg`. It spawns `ffmpeg -i <movie> -f rawvideo -pix_fmt rgba -`. Movie sound is a second
  ffmpeg process (`crates/thedas-app/src/audio.rs`, decoding to f32 stereo).
  - Windows: the search joins `ffmpeg` without `.exe`, so `ffmpeg.exe` on PATH is probably never found and every boot or cutscene movie is
    skipped with a warning (inferred).
  - Linux: works if ffmpeg is on PATH and its build includes the Bink decoder (inferred).
  - No in-tree Bink decoder exists (intro.rs: "until there is a Bink decoder of our own").
  - Docs: the intro.rs module comment still says the movies play silent, but audio.rs now decodes their sound. README "Playing" still says the
    intro movies are not shown (no Bink playback), while its Status section says they play.
- **Audio** uses `cpal` 0.16 (thedas-audio feature `device`) and `symphonia` (mp3). cpal covers CoreAudio, WASAPI and ALSA, so it is portable
  in principle. Linux builds need the ALSA development package (inferred from cpal's requirements). FMOD .fev/.fsb playback is our own code,
  with no FMOD library needed.
- **Gamepad**: `gilrs` 0.11. The paddev.rs comment documents the macOS path (IOKit HID). Linux needs libudev and evdev access; Windows uses
  XInput/WGI. Pads have only been tested on macOS: an Elite 2 over Bluetooth LE, whose paddles gilrs doesn't report.
- **Graphics**: wgpu 30 (Metal, Vulkan, DX12). BC texture compression and clamp-to-border are requested only if the adapter has them
  (thedas-render/src/gpu.rs). Only Metal has been exercised.
- **Interface**: Ruffle is a git dependency at a pinned revision (ruffle_core and ruffle_render_wgpu, rev c8a03cc…). Building it needs Java 17
  for asc.jar (the CI file's comment). Network access to GitHub is needed at build time.
- **Paths and casing**: the install is opened by joining fixed names: `packages/core`, `modules/<module>`, `Addins`, `Offers`, and under each
  `data`, `patch`, `textures`, `audio`, `movies`. On a case-sensitive Linux file system the install must keep the original casing (inferred).
  Entries inside archives are matched case-insensitively.
- **Saves**: `$HOME/.openthedas/saves` (thedas-app saves.rs). With no HOME (usual on Windows) it falls back to `./openthedas-saves` relative
  to the working directory. No USERPROFILE or APPDATA handling.
- **No OS-specific code** otherwise: no `cfg(target_os)`, no `std::os::*`, no Objective-C or Win32 calls. The only process spawns are ffmpeg.

## Finding the install

- Only `--install <path>` or `THEDAS_INSTALL`. thedas-app fails with "give --install or set THEDAS_INSTALL", and the MCP and CLI need it
  likewise. No registry lookup, no Steam, GOG, EA App or Origin library detection, no picker.
- `DaoInstall::open` only checks that `packages/core` exists. Tested against the GOG 1.05 install only.
- Encrypted DLC (45,962 corpus entries) is skipped. There is no way to supply DLC keys.
- The original's settings (DragonAge.ini, Documents/BioWare/Dragon Age/Settings) and its save games (.das) are not read.

## Build, CI, distribution

- **GitHub Actions**: `.github/workflows/ci.yml` (fmt, clippy -D warnings, test, on macos, ubuntu and windows) exists. The GitHub API reports the "CI"
  workflow as `disabled_manually`. That is house policy: PRs merge after local checks on the Mac. So the Linux and Windows builds are never checked.
- **No releases**: the GitHub API lists 0. There are no prebuilt binaries, no installer, no app bundle, no packaging scripts in the repo
  (no .iss, .nsi, .wxs, .desktop, Info.plist or Dockerfile).
- **Building** needs Rust 1.85+ (edition 2024), Java 17 (Ruffle), network access for the Ruffle git dependency, and (Linux) the ALSA and udev
  dev packages (inferred). It also needs ffmpeg at run time for movies.
- `play.command` (the launcher with the install path built in) is local only and untracked.
- The game data must be the player's own copy. Nothing from the game is in the repo.

## Other things that block a stranger

- The test harness (sweep chain saves, gate.sh, QA saves, autoplay scenarios) lives outside the repo in OpenThedas-testdata, with absolute
  /Users/daeken paths in scripts (`run_chain.sh`, `drive.py`, gate.sh). No one else can reproduce the regression gate.
- Tests that need the game are `#[ignore]`d and read `THEDAS_INSTALL` (some also need ffmpeg). So `cargo test` without the game only covers the unit-level tests.
- The default `thedas-app` boots to the start menu. New Game works, but much content past the openings is untested (see 05), and some
  screens are unwired (see 03).
- Only English (en-us) fonts and voice-over folders are wired: `--language` exists, but only fonts_en-us.gfx is used.
- No Dragon Age II support.
