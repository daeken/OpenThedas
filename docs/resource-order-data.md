# Origins loose data subfolders

The Origins directory source scans child directories at the parent's priority. This includes
`data/abilities`, `data/movies`, `data/talktables`, `data/cursors`, and other subdirectories;
they are not four separately prioritized folders.

Static trace of the GOG executable:

- The resource-manager accessor is 0x4c7990; the seven registration arguments belong to
  0x4c7ea0, called immediately after it. Earlier notes attributed the arguments to the accessor.
- The `DATA:` registration at 0x40e17c uses source factory 1, priority 100005. The registered
  factory is 0x4d0150 (registered at 0x40e0be); it creates a directory with its scan byte
  +0xc0 set. Its rank is 1 (0x4cf1b0).
- The directory scan, 0x4cf690, recognizes a child directory and registers it through
  0x4c7ea0 at 0x4cf977 with the parent's priority and factory 1. The child performs the same
  scan. The final flag copied to +0xc1 controls filesystem change watching (0x4cfdd0),
  rather than child discovery.
- The general source comparator compares priority, rank, then path (0x4e6660), placing the
  greater UTF-16 path first at equal priority and rank. Directory paths include their
  trailing backslash.

**PROVISIONAL: duplicate-name tie-break among `data/` subfolders.** OpenThedas makes the
greater UTF-16 directory path win: a child beats its root, and `talktables` beats `cursors`,
independent of registration sequence. The static trace establishes the DATA factory,
recursive child registration, inherited priority/rank, and the general comparator. The
complete registration-to-winning-lookup path for a duplicate loose name across two `data/`
children has **not been traced end to end**. Applying the general comparator to that case
is an inference; no comparison with a running original establishes the winner. Synthetic
fixtures only check that OpenThedas implements the chosen rule.

`push_origins_package` now uses recursive loose-file discovery and the existing source-path
ordering. Audio remains below data; textures and patch remain above it. DA2 is unchanged.
This does not implement the original's `toolset`/`resmetrics` scan exclusions, nested ERF
registration, or delayed RIM loading. Those are separate resource-discovery behaviors.

The original also has direct loaders: cursors use the `CURSORS:` alias and Win32's image loader
(0x415791..0x415832), and language setup examines `packages/core/data/talktables`
(0x40f65e). These coexist with the resource directory scan; they are not evidence of another
priority for arbitrary loose files.

The formerly unnamed priority-100004 registrations at 0x402100 load `global.rim`,
`designerscripts.rim`, `guiglobal.rim`, and `globalvfx.rim`, rather than these subdirectories.
This is static evidence, not a comparison with a running original.

Binary identity: DAOrigins.exe SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`, image base 0x400000.

Validation on the GOG global stack: all 79,282 visible names had identical winning files,
archive entries, labels, resolved files, and MD5 content fingerprints before/after; no names
were added, removed, or moved. All 530 merged tables (42,374 row positions and cell sets)
were unchanged. The regression fixture additionally exercises duplicate loose names across
root, known children, arbitrary nested children, and case-sensitive source paths, with
textures still above data. Synthetic fixtures check the implementation, not the original.
