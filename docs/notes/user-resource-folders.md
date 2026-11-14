# Origins writable resource folders (FMT-013)

Static trace of the GOG DAOrigins.exe, image base 0x400000; SHA-256
2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3.

The registration calls use the same directory factory for writable resources as
for install resources. Lower priority numbers win:

| User resource source | Priority | Registration |
| --- | ---: | --- |
| core override | 0 | 0x40e385 |
| campaign override | 20000 | 0x5fd6a5 |
| campaign patch | 20001 | 0x5fd629 |
| campaign locale, selected language | 20002 | 0x5fd588 |
| campaign data | 20003 | 0x5fd4d4 |
| campaign toolset | 20004 | 0x5fd458 |
| core patch | 20005 | 0x40e2f9 |
| core locale, selected language | 20006 | 0x40e485 |
| core data | 20007 | 0x40e1f8 |

The locale registrations append the language directory and a separator; they do
not recursively mount all languages. The engine currently reads English resources,
so writable locale sources use `locale/en-us`. The core has no writable toolset
registration. The campaign aliases and their folder suffixes are set up around
0x410658..0x410a40; core `WRITE_*` aliases point under the user data directory.
Directory scanning and archive precedence retain the existing folder loader's
rules and the limits recorded under FMT-006.

Add-in records come from the user's Settings/AddIns.xml (0x5faa34), rather than
requiring a Manifest.xml in every AddIns child. The parser copies Enabled to
record +0x15d, Type to +0x150, and GameVersion to +0x118. Both registration and
level discovery must use the same enabled records: being physically present in
the user AddIns folder does not bypass those gates. Registry item position is
retained even for absent folders, so a user's and the install's add-ins share one
ordering. A fallback manifest cannot turn a registry-disabled add-in back on.
The offline authorization and manifest-only discovery policies remain FMT-011.

OpenThedas keeps the standard Documents/BioWare/Dragon Age discovery and adds
`THEDAS_USER_DIR` for importing a user folder from another machine or a Wine
prefix. This override is our cross-platform configuration, not a newly inferred
original-game setting. It names the Dragon Age folder itself. A nonexistent
explicit folder is reported instead of falling through to another user's files;
`THEDAS_MODS=none` still suppresses all user resource folders.

Synthetic fixtures check all eight non-overlay source positions, recursive patch
files, language isolation, the absent core toolset source, registry-only user
add-ins, disabled/newer/wrong-Type items, registry ordering and explicit folder
selection. Existing fixtures cover the priority-0 user override and install-only
stacks. This work does not establish new claims about duplicate source names in
the original beyond the existing FMT-006 trace.
