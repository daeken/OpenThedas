# OpenThedas

An open source replacement engine for Dragon Age: Origins (and later Dragon Age II), written in
Rust, that runs on the original game data. Parity with the original comes first; modernization
(mod management, post-processing, new rendering paths) comes after and must never require breaking
parity.

You need your own copy of the game. Nothing from the original game is, or will ever be, in this
repository.

## Status

Milestone M0, "read everything", is done: archive readers, layered resource lookup, and
GFF3/GFF4/GDA/TLK parsers, checked against every file in a DA:O install by the corpus tool.

Milestone M1, "walk through one area", is in progress. Typed readers cover the whole chain from
an area file to the GPU (areas, layouts, room layouts, models, meshes, materials, DDS), and
`thedas-app` loads an area (the Spoiled Princess, `lgt101d`, by default) and lets you fly
through it. World geometry is drawn with the game's own shaders: `thedas-shader` translates the
D3D9 bytecode in `shaders.erf` to naga IR (all 532 shaders translate and validate), and the
renderer binds constants, samplers and render states the way the material files describe.
Materials outside the Static1 family fall back to a hand-written stand-in unless you pass
`--shaders all`.

## Layout

| Crate | What it does |
| --- | --- |
| `thedas-vfs` | ERF V2.0/V2.1/V2.2/V3.0 and RIM archives, DLC decryption, layered lookup with provenance, install discovery |
| `thedas-formats` | GFF3 and GFF4 readers and writers, GDA tables, talk tables; typed readers for areas (`.are`), layouts (`.arl`, `.rml`), models (`.mmh`), meshes (`.msh`), materials (`.mao`, `.mat`) and DDS textures |
| `thedas-shader` | D3D9 shader bytecode and constant-table parsing; translation to validated naga IR |
| `thedas-render` | wgpu renderer: loads an area's static geometry, draws each material with its translated original shaders (or a stand-in), captures |
| `thedas-app` | The `thedas-app` binary: a window with a free-fly camera, or a headless capture |
| `thedas-cli` | The `thedas` tool: `ls`, `extract`, `dump`, `resolve`, `corpus` |

Planned crates follow the architecture doc: `thedas-script`, `thedas-sim`, `thedas-ui`,
`thedas-audio`, `thedas-video`.

## Walking through an area

```sh
./target/release/thedas-app --install "$GAME"                      # the Spoiled Princess
./target/release/thedas-app --install "$GAME" --area lot120ar_danes_refuge
```

WASD moves, Q/E (or C/Space) go down/up, Shift is faster, hold the right mouse button or use the
arrow keys to look, P prints the camera, F12 saves a PNG, Esc quits. `THEDAS_INSTALL` can stand in
for `--install`. `--shaders original` (the default) uses the game's Static1 and Black shaders,
`--shaders all` every original shader that translates, `--shaders standin` none.

For side-by-side comparisons, render a fixed view with no window:

```sh
./target/release/thedas-app --install "$GAME" --capture out.png \
    --camera=-18,6,2.2,-20,-10 --size 1280x720
```

The camera is `x,y,z,yaw,pitch` in world units and degrees; Z is up and yaw 0 looks along +X.

## Using the CLI

```sh
cargo build --release
GAME=~/path/to/Dragon\ Age\ Origins   # the folder holding packages/ and modules/

./target/release/thedas ls "$GAME/packages/core/data/2da.erf"
./target/release/thedas dump "$GAME/packages/core/data/2da.erf:abi_base.gda"
./target/release/thedas resolve "$GAME" abi_base.gda
./target/release/thedas resolve "$GAME" lgt101d_room000.dds --level lgt101d
./target/release/thedas corpus "$GAME" --report corpus-report.json
```

`corpus` opens every archive in the install, reads every entry, parses every GFF3/GFF4 resource
(including loose files such as talk tables), writes each one back out and checks that it reads
back to the same document, and parses GDA, TLK, area, layout, model, mesh, material (XML) and DDS
files into their typed forms. It exits non-zero
on any failure. DLC archives encrypted with a key we do not have are counted, not failed.

## Development

```sh
cargo test
```

Unit tests use synthetic fixtures only. Tests never need, and must never commit, game data.

xoreos is a useful reference for file formats. It is GPLv3 and this project is MIT, so read it
to understand a format, then write our own implementation; never copy its code. The same applies
to any proprietary SDK source.
