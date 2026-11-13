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
| `.bik` | 85 loose | header only | decoded by an external `ffmpeg` |

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
| `.bik` | header only | no | via external ffmpeg | |
| `.xml` (`chargenmorphcfg.xml`, Manifest.xml) | ad hoc | no | yes | Manifest by attribute scraping |
| `.fxe`, `.fxa`, `.lip` | no | no | no | no code found |
| `.lvl`, `.pth` | no | no | no | no code names them |
| `.vfx` | no (by extension) | no | no | effects come from 2DA `ModelName` and `.mmh` emitters |
| cursors (`data/cursors`) | no | no | no | folder mounted only |
| `.das` | no | no | no | own JSON saves instead |
| `.dac` | no | no | no | |
| `.dazip` | no | no | no | |

### Archives and encryption

- **FMT-001** **DLC passwords only from the GOG Ultimate Edition executable.** Original: decrypts DLC ERF V2.2 with an offline password per ERF module ID, chosen by the routine at RVA `0x87840` (GOG build) and decoded as `(character - 'A') / 2` per digit. Ours: `gog_dlc` reads the encoded strings from the player's own `bin_ship/DAOrigins.exe` at run time (nothing is stored in the repository) and accepts a password only when its MD5 matches the archive's digest. Verified on the GOG Ultimate Edition build (SHA-256 `2346caee…bfb3`): all 377 encrypted archives, 24 module IDs, and `thedas corpus` read the 45,962 formerly skipped entries with no errors. The table is found at fixed file offsets (provisional), so other builds (Steam, EA, retail 1.04/1.05, other languages) are not recognised and their DLC stays unread, logged once; a pattern search for the routine would cover them. The online entitlement path is not traced. Evidence: `crates/thedas-vfs/src/gog_dlc.rs`. Size: M. Depends: none. Kind: unverified.
- **FMT-002** **XOR (type 1) and DA2 Blowfish (type 3) checked only synthetically.** Original: ERF encryption types 1 and 3. Ours: both read. Type 1 takes the DA:O decimal password; its nine-byte XOR cycle matches the original DA:O instructions at RVA `0xeefe0` run with a synthetic key. Type 3 takes raw key bytes and checks the MD5 of the Blowfish ECB encryption of bytes 0–255, after xoreos's description (provisional), against a synthetic vector. No shipped type 1 or type 3 archive has been read, and DA2 keys are not discovered. Reported by the maintainer's outside agent and not re-checked here (unverified): the official DA2 demo (SHA-256 `46398e363fb105c481142d7b759253be96d04e3d8a7de9a1cd05df39de8a55fc`) requests the Blaze config `DA2_DATA` and reads `DLC_<n>_moduleId` and `DLC_<n>_key` from the response; entitlement processing supplies a key indexed by module ID; its ERF reader decodes 32 hex digits into 16 bytes; the demo's validator at RVA `0x4bfbe0` accepted the synthetic vector and matched our reader's plaintext; and the inspected PC install has 191 type 3 archives across 19 module IDs. Shipped DA2 passwords and resource decryption remain unverified. Evidence: `crates/thedas-vfs/src/erf.rs`, `crates/thedas-vfs/src/crypto.rs`. Size: S. Depends: none. Kind: unverified.
- **FMT-003** **LZMA and Xbox LZX compression unsupported.** Original: compression codes 2 and 3. Ours: `UnsupportedCompression`. Evidence: `crates/thedas-vfs/src/erf.rs:490`. Size: M. Depends: none. Kind: missing.
- **FMT-005** **No ERF/RIM writer.** Original: n/a (mod packing). Ours: archives are read-only; the only writer is a test fixture. Evidence: `crates/thedas-vfs/src/erf.rs:584`. Size: M. Depends: none. Kind: missing.

### Layering, DLC, mods

- **FMT-006** **Order inside a package and between add-ins is xoreos's.** Original: every resource folder is registered with a priority number and the lowest wins: core override (user 0, install 1), add-ins (9000 and below), the player's campaign and core folders (20000s), the campaign (50000s), core (100000s). Ours: the same order between those groups, with the override folders as the stack's overlay. Still provisional: inside one package we put loose files under archives, then patch, then override, where the original ranks override, patch, locale, env, data by number; add-ins are ordered by manifest `Priority`, where the original's record order is untraced, as is its second registration group. Evidence: `crates/thedas-vfs/src/install.rs`. Size: M. Depends: none. Kind: provisional.
- **FMT-007** **Level env-folder search order guessed.** Original: not traced. Ours: core env, then DLC by priority highest first. Evidence: `crates/thedas-vfs/src/install.rs:201`. Size: S. Depends: FMT-006. Kind: provisional.
- **FMT-008** **Which add-in folders join the base campaign.** Original: two registration functions. 0x5f4610 (start-up and the enable/disable paths) registers every enabled add-in's `core` folders with no comparison against a campaign; 0x5ef460 (end of a campaign load) registers `module` folders and skips an add-in whose campaign match fails (flag test at 0x5ef6f0). Ours: every enabled add-in's `core` folder is mounted; its `module` folder only when the manifest's `ExtendedModuleUID` is `Single Player`, so the stand-alone campaigns (empty value), Awakening's add-ins (`DAO_PRC_EP_1`) and an add-in with no such attribute contribute `core` only. Add-ins in the player's documents folder follow the same rule. Provisional: that the matched string is `ExtendedModuleUID` is inferred from the shipped manifests, and one of the flag bytes that gate 0x5f4610 (+0x16c) is not identified. The level search (`env_dirs`) lists each enabled add-in's `core/env` and `module/env` whatever campaign it extends, and the audio library takes every `Addins/*/core` and `Offers/*/core` folder without reading a manifest (FMT-007, FMT-009). Evidence: `crates/thedas-vfs/src/install.rs:63`, `crates/thedas-vfs/src/install.rs:229`, `crates/thedas-audio/src/library.rs:75`. Size: M. Depends: FMT-014. Kind: provisional.
- **FMT-047** **DLC content does not appear.** Original: with the DLC installed a new game starts with its bonus items, the world map gains the DLC pins after Lothering, and the party camp gains its additions. Ours: measured with and without the add-ins on a new game, Lothering and the camp, nothing differs; add-in table rows are not merged into the base tables and the DLC scripts named by the `prcscr` table are never run (inferred, not traced). Evidence: `crates/thedas-vfs/src/install.rs:147`, `crates/thedas-sim/src/m2da.rs:3`. Size: M. Depends: FMT-006, SIM-158. Kind: missing.
- **FMT-009** **`Offers/` is not mounted.** Original: not traced. Ours: `global_vfs` and `env_dirs` read only `Addins/`; audio reads both `Addins/` and `Offers/`. Evidence: `crates/thedas-vfs/src/install.rs:85`, `crates/thedas-audio/src/library.rs:79`. Size: S. Depends: none. Kind: bug.
- **FMT-010** **Manifest.xml is scraped, not parsed.** Original: not traced. Ours: first `AddInItem`'s `UID`, `Priority`, `Enabled` attributes by text search; other items and elements ignored. Evidence: `crates/thedas-vfs/src/install.rs:275`. Size: S. Depends: none. Kind: provisional.
- **FMT-011** **AddIns.xml and Offers.xml.** Original: AddIns.xml, Offers.xml (installed and enabled content lists). Ours: no code found; enablement comes from each manifest. Evidence: `crates/thedas-vfs/src/install.rs:83`. Size: M. Depends: none. Kind: missing.
- **FMT-012** **DAZIP packages: one manifest item, no installer steps.** Original: installs a `.dazip` into the player's documents folder (its add-ins under `AddIns`, its override files under `packages/core/override`) and records it in `AddIns.xml`. Ours: a `.dazip` named in the mod list is unpacked once into a cache folder of our own; its add-ins join the stack as installed ones do (manifest priority, `core` always, `module` for the campaign it extends) and its override files are a mod layer at the list entry's place. Not done: only the manifest's first `AddInItem` is read, so a package with several add-ins gives the others the first one's settings; prerequisites, versions and the manifest's text are ignored; levels (`env`) inside a packaged add-in are not searched; the cache is never pruned. Not tried with a real package yet (synthetic fixtures only). Evidence: `crates/thedas-vfs/src/mods.rs`. Size: S. Depends: FMT-011. Kind: partial.
- **FMT-013** **User-directory folders are read only in part.** Original: the player's documents folder holds copies of the core and campaign `override`, `patch`, `data`, `locale` and `toolset` folders (priorities 0 and 20000–20007) and installed add-ins. Ours: a game session mounts the user's `packages/core` and campaign folders and the add-ins under the user's `AddIns` in the original's places, and `packages/core/override` on top; levels (`env`) in a user add-in are not searched, `AddIns.xml` enablement is not read (FMT-011), and off Windows the folder is looked for only at `HOME/Documents/BioWare/Dragon Age`. Evidence: `crates/thedas-vfs/src/install.rs`, `crates/thedas-vfs/src/mods.rs`. Size: S. Depends: FMT-011. Kind: partial.
- **FMT-048** **Mod layer rules inside one mod are guesses.** Original: not traced. Ours: a mod's folders are flattened to file names, `.erf`/`.rim` files in a mod folder sit below its loose files, and of two loose files with one name the later path wins. Only zip archives are read in place; 7z and rar mods must be unpacked to a folder (anywhere but the install). The explicit load order (`mods.txt`, later lines win) is our addition. Evidence: `crates/thedas-vfs/src/mods.rs`. Size: S. Depends: none. Kind: provisional.
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
