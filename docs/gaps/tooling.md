# Known gaps: tooling, platforms and project

[Back to the index](../KNOWN_GAPS.md)

Building and running on each platform, tests, CI, the command-line tool and the MCP server.

## Code inventory

Found by reading the code at main on 2026-11-12: provisional markers, stubs, ignored inputs and features with no code. "Original: not traced" means nobody has yet checked what the original does. Line numbers drift; search for the quoted names if a line has moved.

### Documentation

- **TOOL-001** **README lists `thedas-audio` as planned.** Original: n/a. Ours: the crate exists and is in use. Evidence: `README.md:146`. Size: S. Depends: none. Kind: bug.
- **TOOL-002** **README layout table is out of date.** Original: n/a. Ours: no `thedas-audio` row; the `thedas-cli` row lists `ls`, `extract`, `dump`, `resolve`, `corpus` but not `script`, `sim`, `autoplay`, `navgrid`, `fsb`, `fev`, `play`, `audio-check`; `thedas-app` described as a free-fly viewer. Evidence: `README.md:131`, `README.md:144`, `crates/thedas-cli/src/main.rs:29`. Size: S. Depends: none. Kind: bug.
- **TOOL-003** **README MCP tool table is incomplete.** Original: n/a. Ours: the server also has `screenshot`, `coverage`, `set_head_morph`, `world_map`, `travel`, `store`, `choose_party`, `job_board`, `popup`, `autopilot`, `level_up`, `loot`, `watch_object`, `use_ability`, `attach`, `detach`, `live_status`, `live_input`, `live_screenshot`, `live_sync`, none in the table. Evidence: `README.md:249`, `crates/thedas-mcp/src/main.rs:763`. Size: S. Depends: none. Kind: bug.
- **TOOL-004** **No contributor documentation.** Original: n/a. Ours: no CONTRIBUTING file and no `docs/` folder; README refers to an "architecture doc" not in the repository. Evidence: `README.md:146`. Size: S. Depends: none. Kind: missing.

### Build, platforms, packaging

- **TOOL-005** **CI does not install Linux audio/input dev packages.** Original: n/a. Ours: `thedas-app` pulls cpal (ALSA) and gilrs (udev); `ci.yml` installs only Java and Rust. Whether ubuntu-latest builds is not known from the repository. Evidence: `.github/workflows/ci.yml:17`, `crates/thedas-app/Cargo.toml`. Size: S. Depends: none. Kind: unverified.
- **TOOL-006** **CI checks only fmt, clippy and debug tests.** Original: n/a. Ours: no release build, no artifacts, no MSRV (1.85) job, no corpus run (needs game data). Evidence: `.github/workflows/ci.yml:24`, `Cargo.toml:10`. Size: S. Depends: none. Kind: missing.
- **TOOL-007** **Packaging and installers.** Original: packaging/installers. Ours: no code found. Evidence: `.github/workflows/ci.yml:1`. Size: M. Depends: TOOL-006. Kind: missing.
- **TOOL-008** **iOS and Android.** Original: iOS and Android targets. Ours: no code found; no `cfg(target_os)` anywhere in the workspace. Evidence: `Cargo.toml:1`. Size: L. Depends: none. Kind: missing.
- **TOOL-010** **Resolved: no Java or git player dependency.** The runtime uses the workspace `thedas-gfx` crate; the previous Ruffle dependencies and Java setup are removed.
- **TOOL-011** **No third-party licence notices.** Original: n/a. Ours: dependency licences (Symphonia MPL-2.0, cpal, wgpu) are noted only in a doc comment; no NOTICE file or `cargo-deny`. Evidence: `crates/thedas-audio/src/lib.rs:13`. Size: S. Depends: TOOL-007. Kind: missing.

- **TOOL-019** **The install is never found automatically.** Original: n/a. Ours: only `--install <path>` or `THEDAS_INSTALL`; no registry lookup, no Steam, GOG or EA library detection and no picker. `DaoInstall::open` checks only that `packages/core` exists, and only the GOG 1.05 install has been tried. The original's settings file is not read. Evidence: `crates/thedas-vfs`, `crates/thedas-app/src/main.rs`. Size: M. Depends: none. Kind: missing.
- **TOOL-020** **Install folder names are joined with fixed casing.** Original: n/a. Ours: `packages/core`, `modules/<module>`, `Addins`, `Offers` and the folders under them are opened by exact name, so on a case-sensitive file system the install must keep its original casing (inferred; never run on Linux). Entries inside archives match without regard to case. Evidence: `crates/thedas-vfs`. Size: S. Depends: none. Kind: unverified.
- **TOOL-021** **No releases.** Original: n/a. Ours: no prebuilt binaries, app bundle or packaging scripts; building needs Rust 1.85, Java 17 and network access, and on Linux the ALSA and udev development packages (inferred). The launcher the maintainer uses is local and untracked. Size: M. Depends: TOOL-007. Kind: missing.
- **TOOL-022** **Only macOS on Metal has ever run.** Original: n/a. Ours: wgpu, cpal and gilrs are portable in principle and there is no OS-specific code, but nothing has been built or run on Windows or Linux, Vulkan or DX12, and continuous integration is switched off. Pads are tested only on macOS over Bluetooth. Size: M. Depends: TOOL-005. Kind: unverified.
- **TOOL-024** **Case-sensitive volumes: names not yet resolved case-insensitively.** Original: Windows matches every file name ignoring case. Ours: install folders, DLC manifests, level files (`.arl`, `.opf`, `env/<level>/`), module folders, the audio library's folders and the movies fallback go through `thedas_vfs::resolve_ci` (archive entries were already case-insensitive). Not yet: a DLC folder's `core`/`module` found by `addins()` only when it has a manifest (about 21 of 49 retail `Addins` folders have none, and `Offers/` is not read there while the audio library reads it), paths the user gives (`--save`, `--saves`, `--install`), and any future direct `std::fs` open of a name from data, which must use `resolve_ci`. Evidence: `crates/thedas-vfs/src/path.rs`, `crates/thedas-vfs/src/install.rs` (`addins`). Size: S. Depends: none. Kind: unverified.

### Tests, benchmarks, fuzzing

- **TOOL-012** **Thin test coverage in several crates.** Original: n/a. Ours: `#[test]` counts per crate: thedas-sim 199, thedas-formats 81, thedas-ui 71, thedas-app 66, thedas-render 65, thedas-audio 21, thedas-vfs 12, thedas-script 8, thedas-shader 6, thedas-mcp 2, thedas-cli 1. Files with none: `thedas-audio/src/library.rs`, `device.rs`; `thedas-app/src/cutactors.rs`, `live.rs`; `thedas-cli/src/{corpus,autoplay,audio,script,sim,main}.rs`; `thedas-mcp/src/{main,ui}.rs`; `thedas-formats/src/typed.rs`. Evidence: `crates/thedas-cli/src/dump.rs`, `crates/thedas-mcp/src/live.rs`. Size: M. Depends: none. Kind: missing.
- **TOOL-013** **Tests needing an install are ignored.** Original: n/a. Ours: three `#[ignore]` tests need `THEDAS_INSTALL`; nothing runs them. Evidence: `crates/thedas-app/src/main.rs:3575`, `crates/thedas-app/src/intro.rs:682`, `crates/thedas-ui/src/atlas.rs:207`. Size: S. Depends: TOOL-006. Kind: missing.
- **TOOL-014** **Benchmarks.** Original: benchmarks. Ours: no code found. Evidence: `Cargo.toml:1`. Size: S. Depends: none. Kind: missing.
- **TOOL-015** **Fuzzing of parsers.** Original: fuzzing of parsers. Ours: no code found, though CLAUDE.md requires parsers never to panic. Evidence: `CLAUDE.md:11`. Size: M. Depends: none. Kind: missing.
- **TOOL-016** **Corpus does not type-check every reader.** Original: n/a. Ours: corpus typed checks cover GDA, TLK, ARE, ARL, RML, MMH, MSH, MAO, MAT, STA, DDS, MTX, GFX, FXO, NCS, SPT, LDF, ANB, ANI, CUT, DLG, EVT, GAD, MOP, MOR, PLO, TNT, FEV, FSB; not `.phy`, `.stg`, emitters or Bink headers. Evidence: `crates/thedas-cli/src/corpus.rs:369`, `crates/thedas-cli/src/corpus.rs:585`. Size: S. Depends: none. Kind: missing.

- **TOOL-023** **The regression harness is not in the repository.** Original: n/a. Ours: the sweep chain saves, the gate script, QA saves and autoplay scenarios live in a private test-data folder, with absolute paths to one machine in the scripts, so nobody else can reproduce the regression gate. `cargo test` without the game covers only unit-level tests. Size: M. Depends: none. Kind: missing.

### Mod and MCP tooling

- **TOOL-017** **Mod tooling is read-only.** Original: mod tooling. Ours: `ls`, `extract`, `dump` (one way, to JSON), `resolve` with provenance; no archive packer, no JSON-to-GFF, no DAZIP, no script compiler. Evidence: `crates/thedas-cli/src/main.rs:29`. Size: M. Depends: FMT-005, FMT-012, FMT-043. Kind: missing.
- **TOOL-018** **MCP cannot drive several player actions directly.** Original: what a player can do. Ours: no dedicated tool found for equipping or unequipping from inventory, editing tactics, setting the quickbar, switching the leader, pausing, reading the journal or codex, saving to the player's save folder (`save_state` writes simulation JSON), changing options or inspecting audio; `call_engine_function`, `run_script` and `live_input` (attached game) are the fallbacks. Evidence: `crates/thedas-mcp/src/main.rs:903`. Size: M. Depends: none. Kind: missing.

### Checked and looks complete

- Workspace builds the CLI without the audio device feature; only the app enables cpal.
- CI matrix runs fmt, clippy with `-D warnings` and tests on macOS, Linux and Windows on push to main and on pull requests.
- `.gitignore` excludes game-data extensions and report folders.
- CLI: archive listing/extraction, dump, resolve with provenance, script disasm/run/stats, headless sim, autoplay, navgrid, FSB/FEV listing and checks, offline event playback, audio-name check, corpus.
- MCP: load, step, trace, objects, dialog, movement, travel, store, party picker, level-up, abilities, attack, loot, plots and locals, save/load state, UI movies with input and screenshots, attach to a running game.
