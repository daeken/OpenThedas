# OpenThedas

An open source replacement engine for Dragon Age: Origins (and later Dragon Age II), written in
Rust, that runs on the original game data. Parity with the original comes first; modernization
(mod management, post-processing, new rendering paths) comes after and must never require breaking
parity.

You need your own copy of the game. Nothing from the original game is, or will ever be, in this
repository.

## Status

Milestone M0, "read everything": archive readers, layered resource lookup, and GFF3/GFF4/GDA/TLK
parsers, checked against every file in a DA:O install by the corpus tool.

## Layout

| Crate | What it does |
| --- | --- |
| `thedas-vfs` | ERF V2.0/V2.1/V2.2/V3.0 and RIM archives, DLC decryption, layered lookup with provenance, install discovery |
| `thedas-formats` | GFF3 and GFF4 readers and writers, GDA tables, talk tables |
| `thedas-cli` | The `thedas` tool: `ls`, `extract`, `dump`, `resolve`, `corpus` |

Planned crates follow the architecture doc: `thedas-script`, `thedas-sim`, `thedas-render`,
`thedas-ui`, `thedas-audio`, `thedas-video`, `thedas-app`.

## Using the CLI

```sh
cargo build --release
GAME=~/path/to/Dragon\ Age\ Origins   # the folder holding packages/ and modules/

./target/release/thedas ls "$GAME/packages/core/data/2da.erf"
./target/release/thedas dump "$GAME/packages/core/data/2da.erf:abi_base.gda"
./target/release/thedas resolve "$GAME" abi_base.gda
./target/release/thedas corpus "$GAME" --report corpus-report.json
```

`corpus` opens every archive in the install, reads every entry, parses every GFF3/GFF4 resource
(including loose files such as talk tables), writes each one back out and checks that it reads
back to the same document, and parses GDA and TLK files into their typed forms. It exits non-zero
on any failure. DLC archives encrypted with a key we do not have are counted, not failed.

## Development

```sh
cargo test
```

Unit tests use synthetic fixtures only. Tests never need, and must never commit, game data.

xoreos is a useful reference for file formats. It is GPLv3 and this project is MIT, so read it
to understand a format, then write our own implementation; never copy its code. The same applies
to any proprietary SDK source.
