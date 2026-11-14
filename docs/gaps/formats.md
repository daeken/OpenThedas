# Known gaps: file formats and file system

[Back to the index](../KNOWN_GAPS.md)

Readers and writers for the game's resource types, archives, DLC, mods and saves.

## Tables and resource types measured against a real install

The inventory below is from 2026-11-12 at main `1eb844a`. That run counted 563,593 entries,
including 45,962 encrypted DLC entries it skipped. The later GOG key recovery check read
all 563,593 entries with zero skips and zero read errors. The inventory counts still
reflect the earlier run.

### Tables

The install has 3,519 distinct `.gda` names, 1,787 of them in the core packages.

| Core tables | Count |
| --- | --- |
| Named in our source | 50 |
| Read by table id in the simulation's source | 12 |
| Referenced by a constant table id in compiled scripts (served generically) | 42 |
| A worksheet with no static reference | 324 |
| Not a worksheet and no static reference | 1,359 |

"No static reference" overstates what is unread. Scripts reach tables by computed id at 3,059
call sites (ability tables, the `aip_*` AI packages, `ts_*` treasure, the approval tables), the
interface movies ask for tables by id, and the sound-set tables (`sse_*`, `sst_*`, 686 of them),
the `*_variation` tables and the `statecnt_*` tables are reached through names or ids stored in
other tables.

Tables with no known reader, static or dynamic (inferred, so each needs checking before work
starts on it):

- **Load hints**: `res_*` (351 per-area resource lists), `loadhints` and its four detail-level
  variants, `chargen_preload`. `SetLoadHint` does nothing. These are the natural input to area
  preloading.
- **Gestures and ambient animation**: `gst_*` (22), `gestures_core_settings`, `poses`,
  `pose_transitions`, `placeable_interactions`.
- **Synchronised animation**: `synccreatureanimations`, `syncdeathanimations`,
  `unsyncdeathanimations`, `synccombatanims`, `nosynccombatanims`, `syncplaceableanimations`,
  `syncabilityanimations`, `synclargecreatures1` to `5`, `halfroundcombatanims`.
- **Melee rings**: `meleering1` to `9`.
- **Crafting**: `crafting_recipes`, `crafting_recipe_types`.
- **Items**: `iprp_*` (15), `itempropdef`, `itemprops`, `armordata`.
- **Approval and party**: `gift_values`, `motivations`, `heroicpartystats`, `party_picker`.
- **World map**: `rand_*` (7 random-encounter tables), `enc_trips_sp`, `location_states`.
- **Sound**: `sound_settings`, `internal_soundsettings`, `sound_projects_to_load`,
  `playersoundsets`, `ss_probabilitymodif`, `sound_geometry_mat_types`.
- **Appearance and character creation**: the `*_heads`, `*_hair`, `*_tints` and
  `*_tint_masks` tables, `human_beard`, `raceicons`, `background_*` (the chargen movie may ask
  for these by id; unverified).
- **Interface, tutorials, plots**: `trainingsteps`, `tutorials`, `camera_selection`,
  `dialog_tokens`, `stringtokens`, `main_plot_screenshots`, `plotactionsets`,
  `plotactionstates`.
- **Rules**: `autobalance`, `diffsettings`, `resistances`, `creaturetypes`, `simplecreature`,
  `lock_difficulty`, `trap_difficulty`, `starting_points`, `var_*` (12), `ai_movetypes`,
  `personality`, `abilityusetype`, `crusts`.
- **Army battle**: `cli_army_*` (8).

### Resource types with no reader or no consumer

Counts are corpus entries, with each archive copy counted.

| Type | Entries | Reader | Consumer |
| --- | --- | --- | --- |
| `.fxe` | 308,332 | none | none (facial animation and lip sync, inferred) |
| `.fxa` | 37 | none | none (face actors, inferred) |
| `.met` | 2,364 | none (almost all encrypted) | none |
| `.pwk` | 774 | generic GFF only | none (placeable walk shapes, inferred) |
| `.dlb` | 731 | generic GFF only | none |
| `.mal` | 347 | none | named only as the emitters' default material library |
| `.lst` | 320 | generic GFF3 only | none |
| `.cre` | 135 | none | none |
| `.wnd` | 116 | generic GFF only | none found |
| `.cur` | 70 | none | none (the original's mouse cursors) |
| `.cns`, `.ncc`, `.nsc`, `.nss` | 26, 24, 19, 30 | none | none (console and script debug files) |
| `.tbi` | 11 | generic GFF only | none |
| `.fxs`, `.fxr` | 7, 7 | none (encrypted) | none |
| `.fdl` | 4 | generic GFF | none |
| `.bic` | 3 | generic GFF3 only | none (character files) |
| `.ttf` | 2 | none | none |
| `.spt` | 688 | typed | parsed, not drawn; billboards stand in |
| `.bik` | 85 loose | typed (`thedas-video`) | decoded natively, video and sound |

The corpus run's only failures are 5,058 non-fatal "undeclared action" reports in scripts, all
from four function ids missing from `script.ldf` or the two-argument `CommandFly` in DLC and
Awakening scripts.

## Code inventory

Found by reading the code at main on 2026-11-12: provisional markers, stubs, ignored inputs and features with no code. "Original: not traced" means nobody has yet checked what the original does. Line numbers drift; search for the quoted names if a line has moved.

### Resource types

| Type | Reader | Writer | Consumer | Notes |
| --- | --- | --- | --- | --- |
| `.erf` V2.0/V2.1/V2.2/V3.0 | yes | no (test helper only) | yes (vfs) | GOG DLC passwords recovered locally. XOR and DA2 Blowfish accept explicit passwords. LZMA and LZX unsupported |
| `.rim` | yes (ERF V2.0) | no | yes | |
| GFF4 (generic) | yes (V4.0, V4.1) | yes | yes | PC platform only |
| GFF3 (generic) | yes (V3.2) | yes | yes | strings raw bytes |
| `.gda` | yes | no typed writer | yes | via GFF4 |
| `.tlk` | yes (V0.2, V0.4, V0.5) | no typed writer | yes | only `*en-us.tlk` loaded |
| `.are` | yes (GFF3 + typed layout) | GFF3 | yes (sim, audio, render) | |
| `.arl`, `.rml` | yes | no | yes | labels provisional |
| `.opf` | no | no | no | mounted as a loose file only |
| `.mmh` | yes | no | yes | includes emitter `nemt` nodes |
| `.msh` | yes | no | yes | |
| `.mao`, `.mat.xml` | yes | no | yes | |
| `.sta.xml` | yes | no | yes | |
| `.dds` | yes (DXT1/3/5, BGRA8, BGRX8, BGR8, BGRA4, L8) | no | yes | no DX10, BC4/5 |
| `.tga` | no | no | no | no code found |
| `.mor` | yes | no | yes | |
| `.mop` | yes (`MorphPreset`) | no | yes | |
| `.tnt` | yes | no | yes | |
| `.mtx` (probes) | yes | no | yes | |
| `.stg` | yes | no | yes | |
| `.anb`, `.cub` | yes | no | yes | |
| `.gad`, `.evt`, `.ani` | yes | no | yes | |
| `.phy` | yes | no | yes (sight, camera fade) | cooked mesh partly read |
| `.spt` | yes | no | partial | render uses first billboard and size only |
| `.fev` | yes | no | yes | music chunk raw |
| `.fsb` | yes (FSB4) | no | yes | IMA ADPCM, MP3, PCM |
| `.wav` | no | yes (offline renders) | CLI/app recording | |
| `.gfx` | yes (scan, rewrite to SWF) | SWF rewrite | yes (Ruffle) | LZMA SWF unsupported |
| `.fxo` / `.psh` / `.vsh` | yes (thedas-shader) | no | yes | |
| `.ncs` | yes (thedas-script) | no | yes | |
| `.nss` | no | no | name only | no compiler |
| `script.ldf` | yes | no | yes | |
| `.dlg` | yes (sim, GFF4) | no | yes | |
| `.plo` | yes (sim) | no | yes | |
| `.cut` | yes (sim) | no | yes | |
| `.cif` | yes (sim) | no | yes | |
| `.utc`, `.uti`, `.utp`, `.utm` | yes (GFF3) | GFF3 | yes (sim) | |
| `.utt`, `.uts`, `.ute`, `.utw`, `.uta` | no | no | no | no code names them |
| `<layout>_mmi.gff` | yes (ui minimap) | no | yes | |
| `.bik` | yes (`thedas-video`) | yes | yes | Bink 1 revision `i`, Bink audio DCT |
| `.xml` (`chargenmorphcfg.xml`, Manifest.xml) | ad hoc | no | yes | Manifest by attribute scraping |
| `.fxe`, `.fxa`, `.lip` | no | no | no | no code found |
| `.lvl`, `.pth` | no | no | no | no code names them |
| `.vfx` | no (by extension) | no | no | effects come from 2DA `ModelName` and `.mmh` emitters |
| cursors (`data/cursors`) | no | no | no | folder mounted only |
| `.das` | no | no | no | own JSON saves instead |
| `.dac` | no | no | no | |
| `.dazip` | no | no | no | |

### Archives and encryption

- **FMT-001** **DLC passwords on builds other than GOG are found by search, not read.** Original: decrypts DLC ERF V2.2 with an offline password per ERF module ID, chosen by the routine at RVA `0x87840` (GOG build) and decoded as `(character - 'A') / 2` per digit. Ours: a password from the player's keys file (`keys_file`) is used first. Otherwise `gog_dlc` reads the encoded strings from the player's own `bin_ship/DAOrigins.exe` when it is the GOG Ultimate Edition build (fixed file offsets, provisional; verified on SHA-256 `2346caee…bfb3`: all 377 encrypted archives, 24 module IDs, `thedas corpus` reads the 45,962 entries with no errors). For any other build `dlc_search` tries every eight-digit password against the archive's MD5 digest on all cores (about 3.5 s worst case on four cores in a release build), once per password, and keeps what it finds in `dlc-keys.txt` in the player's data folder, checked against the digest again on every use. Still open: that every DA:O password is exactly eight digits is known only from the GOG table (provisional); the search has not been run against a real non-GOG install; it runs on whichever thread first reads the archive, so a first start can pause. The online entitlement path is not traced. Evidence: `crates/thedas-vfs/src/gog_dlc.rs`, `crates/thedas-vfs/src/dlc_search.rs`, `crates/thedas-vfs/src/keys_file.rs`. Size: S. Depends: none. Kind: unverified.
- **FMT-002** **XOR (type 1) checked only synthetically; DA2 DLC needs the player's keys file.** Original: ERF encryption types 1 and 3. Ours: both read. Type 1 takes the DA:O decimal password; its nine-byte XOR cycle matches the original DA:O instructions at RVA `0xeefe0` run with a synthetic key, and no shipped type 1 archive has been read. Type 3 (DA2) takes the 16 key bytes and checks the MD5 of the Blowfish ECB encryption of bytes 0–255. DA2's passwords came from EA's servers (the demo requests the Blaze config `DA2_DATA` and reads `DLC_<n>_key`, as reported by the maintainer's outside agent), so they cannot be found offline: the player supplies a keys file in the format of the commonly shared `erfpasswords.txt` (`keys_file`: `--keys`, else `THEDAS_KEYS`, else `keys.txt` in the data folder; every password is checked against each archive's digest). With such a file, `thedas corpus` on the EA app install (2026-11-14, macOS) read all 13,918 encrypted entries of its DLC with no new failures. Without one, DA2 DLC stays unread and the error and the log say where the file goes. Evidence: `crates/thedas-vfs/src/keys_file.rs`, `crates/thedas-vfs/src/erf.rs`, `crates/thedas-vfs/src/crypto.rs`. Size: S. Depends: none. Kind: unverified.
- **FMT-003** **LZMA and Xbox LZX compression unsupported.** Original: compression codes 2 and 3. Ours: `UnsupportedCompression`. Evidence: `crates/thedas-vfs/src/erf.rs:490`. Size: M. Depends: none. Kind: missing.
- **FMT-005** **No ERF/RIM writer.** Original: n/a (mod packing). Ours: archives are read-only; the only writer is a test fixture. Evidence: `crates/thedas-vfs/src/erf.rs:584`. Size: M. Depends: none. Kind: missing.

### Layering, DLC, mods

- **FMT-006** **Order inside a package and between add-ins is xoreos's.** Original: every resource folder is registered with a priority number and the lowest wins: core override (user 0, install 1), add-ins (9000 and below), the player's campaign and core folders (20000s), the campaign (50000s), core (100000s). Ours: the same order between those groups, with the override folders as the stack's overlay. Still provisional: inside one package we put loose files under archives, then patch, then override, where the original ranks override, patch, locale, env, data by number; add-ins follow the registry's order as traced (FMT-011). Evidence: `crates/thedas-vfs/src/install.rs`. Size: M. Depends: none. Kind: provisional.
- **FMT-007** **Level env-folder search order guessed.** Original: not traced. Ours: core env, then DLC by priority highest first. Evidence: `crates/thedas-vfs/src/install.rs:201`. Size: S. Depends: FMT-006. Kind: provisional.
- **FMT-008** **Which add-in folders join the base campaign.** Original: two registration functions. 0x5f4610 (start-up and the enable/disable paths) registers every enabled add-in's `core` folders with no comparison against a campaign; 0x5ef460 (end of a campaign load) registers `module` folders and skips an add-in whose campaign match fails (flag test at 0x5ef6f0). Ours: every enabled add-in's `core` folder is mounted; its `module` folder only when the manifest's `ExtendedModuleUID` is `Single Player`, so the stand-alone campaigns (empty value), Awakening's add-ins (`DAO_PRC_EP_1`) and an add-in with no such attribute contribute `core` only. Add-ins in the player's documents folder follow the same rule. Traced since: the matched string is the registry item's `ExtendedModuleUID` (falling back to `ExtendedModuleName`, then `ModuleName`; parser at 0x5faba3), and the byte at +0x16c is a "pending" flag set at start and when an add-in is switched on or off (0x5f6068, 0x5f6306) and cleared by 0x5f4610, not a per-campaign decision. The level search (`env_dirs`) lists each enabled add-in's `core/env` and `module/env` whatever campaign it extends, and the audio library takes every `Addins/*/core` and `Offers/*/core` folder without reading a manifest (FMT-007, FMT-009). Evidence: `crates/thedas-vfs/src/install.rs:63`, `crates/thedas-vfs/src/install.rs:229`, `crates/thedas-audio/src/library.rs:75`. Size: M. Depends: FMT-014. Kind: provisional.
- **FMT-047** **DLC content does not appear.** Original: with the DLC installed a new game starts with its bonus items, the world map gains the DLC pins after Lothering, and the party camp gains its additions. Ours: measured with and without the add-ins on a new game, Lothering and the camp, nothing differs; add-in table rows are not merged into the base tables (inferred, not traced). The add-ins' `PRCSCR_*.gda` scripts now run when an area list loads (`crates/thedas-sim/src/prcscr.rs`), so what is left is the tables and whatever the scripts' ownership checks need. Evidence: `crates/thedas-vfs/src/install.rs:147`, `crates/thedas-sim/src/m2da.rs:3`. Size: M. Depends: FMT-006, SIM-158. Kind: missing.
- **FMT-009** **`Offers/` is not mounted.** Original: the offer manager registers an offer's `core` and `module` folders like an add-in's (same aliases, code around 0x499afb), but only when the campaign matches, record byte +0x1a8 is set and record byte +0x1a6 is zero. Those two bytes are not identified; the likely reading is that +0x1a6 marks content the player already owns, so an offer's stub content (the in-game seller and purchase prompt) is mounted only for DLC the player lacks. Ours: `global_vfs` and `env_dirs` read only `Addins/`; audio reads both `Addins/` and `Offers/`. On an install with every DLC present (GOG) that reading means no offer would be registered, so the stacks agree; on an install missing a DLC the original would mount that offer and we do not. Evidence: `crates/thedas-vfs/src/install.rs`, `crates/thedas-audio/src/library.rs:79`. Size: S. Depends: FMT-011. Kind: unverified.
- **FMT-010** **Manifest.xml is scraped, not parsed.** Original: not traced. Ours: first `AddInItem`'s `UID`, `Priority`, `Enabled` attributes by text search; other items and elements ignored. Evidence: `crates/thedas-vfs/src/install.rs:275`. Size: S. Depends: none. Kind: provisional.
- **FMT-011** **AddIns.xml is read for discovery only; Offers.xml not at all.** Original: `AddIns.xml` in the player's `Settings` folder is the run-time registry of add-ins (parser at 0x5faa34); many shipped add-ins (the promotional items) have no manifest in their own folder. Ours: an add-in is known from the registry (the player's documents copy first in a game session, then the copy GOG's installer keeps at `__support/userdocs/AddIns.xml`), and from its folder's `Manifest.xml` when no registry names it. Applied as traced: `Enabled` must be non-zero and the `<Type>` element 1 or 2; `State` is ignored, as the original's registration ignores it. Deliberately not applied: `RequiresAuthorization`, which in the original needs an "authorized" byte set by its online entitlement check (GOG's registry carries no such attribute; other stores' may, and their add-ins are mounted here all the same). Also applied as traced: an item whose `<GameVersion>` is above the game's is not mounted (record byte +0x177); the game's version is read as the original reads it, from the `FileVersion` string of the install's `DAOrigins.exe` cut at its second dot (1.05 for `1.05.13263.0`), and the gate is skipped when that cannot be read. An add-in left out by any of these gates is named in the log with the reason. Add-ins a registry names are ordered as traced: the parser appends each item to the record vector (0x5f8b70 from 0x5fa6da), nothing sorts it, and both registration functions walk it from the last record to the first handing out falling priority numbers, so the registry's first item beats every later one; `Priority` does not order records. Provisional: add-ins no registry names (manifest only, the player's `AddIns` folder, an unpacked `.dazip`) sit below all of those, by manifest `Priority`; and the checks that decide whether an item joins the record vector at all (0x5f0a90) are not read. `Offers.xml` and the `Offers/` folder are not read (FMT-009). Evidence: `crates/thedas-vfs/src/install.rs`. Size: S. Depends: none. Kind: partial.
- **FMT-012** **DAZIP packages: one manifest item, no installer steps.** Original: installs a `.dazip` into the player's documents folder (its add-ins under `AddIns`, its override files under `packages/core/override`) and records it in `AddIns.xml`. Ours: a `.dazip` named in the mod list is unpacked once into a cache folder of our own; its add-ins join the stack as installed ones do (manifest priority, `core` always, `module` for the campaign it extends) and its override files are a mod layer at the list entry's place. Each add-in takes its own manifest `AddInItem`, matched by UID to its folder (a folder no item names takes the first item's settings), and levels (`env`) inside packaged add-ins are searched with the install's. Not done: prerequisites, versions and the manifest's text are ignored. Not tried with a real package yet (synthetic fixtures only). Evidence: `crates/thedas-vfs/src/mods.rs`. Size: S. Depends: FMT-011. Kind: partial.
- **FMT-013** **User-directory folders are read only in part.** Original: the player's documents folder holds copies of the core and campaign `override`, `patch`, `data`, `locale` and `toolset` folders (priorities 0 and 20000–20007) and installed add-ins. Ours: a game session mounts the user's `packages/core` and campaign folders and the add-ins under the user's `AddIns` in the original's places, and `packages/core/override` on top; levels (`env`) in a user add-in are searched with the install's add-ins (by the same order), `AddIns.xml` enablement is not read for them (FMT-011), and off Windows the folder is looked for only at `HOME/Documents/BioWare/Dragon Age`. Evidence: `crates/thedas-vfs/src/install.rs`, `crates/thedas-vfs/src/mods.rs`. Size: S. Depends: FMT-011. Kind: partial.
- **FMT-048** **Mod layer rules inside one mod are guesses.** Original: not traced. Ours: a mod's folders are flattened to file names, `.erf`/`.rim` files in a mod folder sit below its loose files, and of two loose files with one name the later path wins. Zip archives are read in place; a `.7z` is unpacked once into the mod cache in the player's data folder and mounted from there (LZMA and LZMA2 only, no encrypted archives); rar mods must be unpacked to a folder by hand (anywhere but the install). The cache is never pruned. The explicit load order (`mods.txt`, later lines win) is our addition. Evidence: `crates/thedas-vfs/src/mods.rs`. Size: S. Depends: none. Kind: provisional.
- **FMT-014** **Only `modules/single player` can be a campaign.** Original: Awakening expansion layout, DLC and toolset-exported modules as campaigns. Ours: the sim loads modules from `modules/<dir>` only and the global stack hard-codes `modules/single player`. Evidence: `crates/thedas-sim/src/sim.rs:2680`, `crates/thedas-vfs/src/install.rs:131`. Size: L. Depends: FMT-008. Kind: missing.
- **FMT-015** **Install discovery.** Original: install discovery per platform and storefront. Ours: no code found; `--install` or `THEDAS_INSTALL` required. Evidence: `crates/thedas-app/src/main.rs:2950`. Size: M. Depends: none. Kind: missing.
- **FMT-016** **Install folder case is fixed.** Original: case-insensitive Windows file system. Ours: resource names are lowercased, but install paths (`packages/core`, `Addins`, `modules/single player`, `textures/high`, `env`) are joined in fixed case; only `Manifest.xml`/`manifest.xml` is tried both ways. Evidence: `crates/thedas-vfs/src/install.rs:47`, `crates/thedas-vfs/src/install.rs:100`. Size: S. Depends: none. Kind: unverified.

### Localisation and saves

- **FMT-017** **Talk tables are English only.** Original: talk tables per language. Ours: the sim and UI load only `*en-us.tlk`; female-variant tables have no code; the font library is `fonts_en-us.gfx`. Evidence: `crates/thedas-sim/src/sim.rs:431`, `crates/thedas-ui/src/stage.rs:80`, `crates/thedas-ui/src/lib.rs:409`. Size: M. Depends: none. Kind: missing.
- **FMT-018** **No typed writers for GDA and TLK.** Original: n/a. Ours: both are writable only as generic GFF4 trees. Evidence: `crates/thedas-formats/src/gda.rs:1`, `crates/thedas-formats/src/tlk.rs:1`. Size: S. Depends: none. Kind: missing.
- **FMT-019** **Original `.das` saves are not read or written.** Original: save game formats (.das). Ours: our own JSON save; no code for `.das`. Evidence: `crates/thedas-sim/src/save.rs:3`. Size: L. Depends: none. Kind: missing.
- **FMT-020** **Save screenshots.** Original: screenshots embedded in saves. Ours: no code found. Evidence: `crates/thedas-app/src/saves.rs:7`. Size: S. Depends: FMT-019. Kind: missing.
- **FMT-021** **Character export/import (.dac).** Original: character export/import. Ours: no code found. Evidence: `crates/thedas-sim/src/save.rs:1`. Size: M. Depends: FMT-019. Kind: missing.

### Textures, models, animation

- **FMT-022** **DDS variants missing; DXT2/4 treated as DXT3/5.** Original: texture formats (DDS variants). Ours: DX10 headers, ATI1/ATI2 (BC4/BC5), R5G6B5, A1R5G5B5, A8 are rejected; premultiplied DXT2/DXT4 decode as non-premultiplied DXT3/DXT5. Evidence: `crates/thedas-formats/src/dds.rs:124`, `crates/thedas-formats/src/dds.rs:141`. Size: S. Depends: none. Kind: missing.
- **FMT-023** **TGA.** Original: TGA textures. Ours: no code found. Evidence: `crates/thedas-formats/src/dds.rs:1`. Size: S. Depends: none. Kind: missing.
- **FMT-024** **SpeedTree trees are parsed but not built.** Original: trees grown from `.spt` at load (per module doc). Ours: render takes only the first billboard's corners and the tree size; branches, fronds and leaves are unused; token layout from the SDK. Evidence: `crates/thedas-render/src/scene.rs:725`, `crates/thedas-formats/src/speedtree.rs:19`. Size: L. Depends: none. Kind: missing.
- **FMT-025** **SpeedTree placements' fields from xoreos.** Original: not traced. Ours: `SPT ` struct field meanings from xoreos. Evidence: `crates/thedas-formats/src/level.rs:606`. Size: S. Depends: FMT-024. Kind: provisional.
- **FMT-026** **Level and room layout labels provisional.** Original: not traced except where noted. Ours: labels identified from data or xoreos; frame-effect entries with no id or non-numeric values are skipped. Evidence: `crates/thedas-formats/src/level.rs:8`, `crates/thedas-formats/src/level.rs:186`, `crates/thedas-formats/src/level.rs:231`, `crates/thedas-formats/src/level.rs:274`. Size: M. Depends: none. Kind: provisional.
- **FMT-027** **FaceFX.** Original: FaceFX (.fxe/.fxa). Ours: no code found. Evidence: `crates/thedas-formats/src/lib.rs:36`. Size: L. Depends: none. Kind: missing.
- **FMT-028** **Mesh labels and primitive type provisional.** Original: not traced. Ours: labels from data; primitive type 0 taken as a triangle list. Evidence: `crates/thedas-formats/src/mesh.rs:6`, `crates/thedas-formats/src/mesh.rs:239`. Size: S. Depends: none. Kind: provisional.
- **FMT-029** **Model labels and transform rule provisional.** Original: not traced. Ours: `parent * T * R` from data. Evidence: `crates/thedas-formats/src/model.rs:9`. Size: S. Depends: none. Kind: provisional.
- **FMT-030** **Material `Logic` selection provisional.** Original: not traced. Ours: inferred ordering of detail levels and terrain UV scales. Evidence: `crates/thedas-formats/src/material.rs:13`, `crates/thedas-formats/src/material.rs:198`, `crates/thedas-formats/src/material.rs:437`. Size: S. Depends: none. Kind: provisional.
- **FMT-031** **Render-state loader not traced.** Original: engine state loader. Ours: D3D9 defaults for unset values; unknown blend factor guessed. Evidence: `crates/thedas-formats/src/state.rs:18`, `crates/thedas-formats/src/state.rs:310`. Size: S. Depends: none. Kind: provisional.
- **FMT-032** **Morph fields, target blending and tint composition provisional.** Original: morph rebuild job (not traced). Ours: slot names from data, blend median, composition guessed; slots past a dozen tints ignored. Evidence: `crates/thedas-formats/src/morph.rs:7`, `crates/thedas-formats/src/morph.rs:317`, `crates/thedas-formats/src/morph.rs:479`, `crates/thedas-formats/src/morph.rs:575`. Size: M. Depends: none. Kind: provisional.
- **FMT-033** **Animation tree fields partly uninterpreted.** Original: not traced. Ours: 4027 (plays once) from data; 4028-4030 not interpreted; event kinds from data. Evidence: `crates/thedas-formats/src/anim.rs:9`, `crates/thedas-formats/src/anim.rs:136`. Size: S. Depends: none. Kind: provisional.
- **FMT-034** **PHY capsule axis guessed; cooked mesh partly read.** Original: not traced. Ours: capsule axis +Z from pose origin; cooked PhysX structures beyond the triangles not read. Evidence: `crates/thedas-formats/src/phy.rs:17`, `crates/thedas-formats/src/phy.rs:253`. Size: S. Depends: none. Kind: provisional.
- **FMT-035** **Stage field names from xoreos; camera actor ignored.** Original: not traced. Ours: stage camera alone frames a line; the camera actor's own transform and FOV track are not used. Evidence: `crates/thedas-formats/src/stage.rs:21`. Size: S. Depends: none. Kind: provisional.
- **FMT-036** **Area `Layout` field inferred.** Original: not traced. Ours: inferred from data. Evidence: `crates/thedas-formats/src/area.rs:5`. Size: S. Depends: none. Kind: provisional.
- **FMT-037** **`.opf` and cursors are mounted with no consumer.** Original: not traced. Ours: `.opf` beside a level and `data/cursors` are pushed into the stack; nothing reads them (system cursor is used). Evidence: `crates/thedas-vfs/src/install.rs:213`, `crates/thedas-vfs/src/install.rs:150`. Size: S. Depends: none. Kind: missing.
- **FMT-038** **VFX files.** Original: VFX (.vfx). Ours: no reader by extension; effects come from `ModelName` in a 2DA and `.mmh` emitter nodes. Evidence: `crates/thedas-ui/src/models.rs:993`, `crates/thedas-formats/src/emitter.rs:1`. Size: M. Depends: none. Kind: unverified.
- **FMT-039** **Path and level files.** Original: path/navigation (.pth), levels (.lvl). Ours: no code names them; pathing comes from the `.arl` grid (3020/3110). Evidence: `crates/thedas-formats/src/level.rs:21`. Size: S. Depends: none. Kind: unverified.

### Interface, scripts, sound

- **FMT-040** **GFx tag numbers inferred; other Scaleform tags dropped.** Original: Scaleform runtime. Ours: tag numbers and `DefineExternalImage` layout inferred; non-image Scaleform tags dropped in the rewrite; LZMA SWF refused. Evidence: `crates/thedas-formats/src/gfx.rs:9`, `crates/thedas-formats/src/gfx.rs:33`. Size: M. Depends: none. Kind: provisional.
- **FMT-041** **Font library matched by name only; outlines forced.** Original: GFxFontLib. Ours: name match only (not bold/italic); device-font fields switched to outlines. Evidence: `crates/thedas-formats/src/gfx.rs:339`, `crates/thedas-formats/src/gfx.rs:395`. Size: S. Depends: none. Kind: provisional.
- **FMT-042** **Scale-9 below-corner rule guessed.** Original: Flash's rule. Ours: not checked. Evidence: `crates/thedas-formats/src/gfx/scale9.rs:14`. Size: S. Depends: none. Kind: provisional.
- **FMT-043** **No NSS compiler.** Original: scripts (.nss) are compiled by the toolset. Ours: `.ncs` read and run; `.nss` only resolved by name. Evidence: `crates/thedas-sim/src/sim.rs:477`. Size: M. Depends: none. Kind: missing.
- **FMT-044** **FEV field meanings partly by offset.** Original: `fmod_event.dll` loader (traced for order). Ours: fields the loader only stores are named by offset (`f28`) and provisional. Evidence: `crates/thedas-formats/src/fev.rs:18`. Size: S. Depends: none. Kind: provisional.

### Console and Dragon Age II

- **FMT-045** **Console data rejected.** Original: console versions' data. Ours: GFF4 non-PC platforms error; LZX unsupported (FMT-003); FSB console codecs refused (AUD-059). Evidence: `crates/thedas-formats/src/gff4.rs:383`. Size: L. Depends: FMT-003, AUD-059. Kind: missing.
- **FMT-046** **Dragon Age II formats.** Original: Dragon Age II archive and format differences. Ours: ERF V3.0 and GFF4 V4.1 read and write; DA2 encryption accepts an explicit key (FMT-002). DA2-specific typed formats remain unsupported. Evidence: `crates/thedas-vfs/src/erf.rs:8`, `crates/thedas-formats/src/gff4.rs:1`. Size: L. Depends: FMT-002. Kind: missing.

### Checked and looks complete

- ERF V2.0, V2.1, V2.2 (with DAO Blowfish given a key), V3.0 reading; BioWare, headerless and standard zlib.
- Layered lookup with provenance, mark/truncate level mounts, fall-through past unreadable encrypted entries.
- Level mounting: loose `.arl`, level rim, room rims, `.gpu.rim`s.
- GFF4 V4.0/V4.1 read and canonical write (corpus round-trips byte-exactly).
- GFF3 V3.2 read and write.
- GDA (column hashes) and TLK V0.2/V0.4/V0.5 (Huffman) reading.
- Tint field order, as traced (0x749600, 0xa604a0).
- Emitter fields, as traced in the toolset converter and blueprint loader.
- FEV parse of every event file in the install; FSB4 parse.
- Probe (`.mtx`) irradiance matrices.
- Terrain, water fog, lightmap atlas fields in layouts (labels provisional, see FMT-026).
