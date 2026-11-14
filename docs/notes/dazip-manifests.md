# DAZIP manifest runtime handling (FMT-012)

Static trace of the GOG DAOrigins.exe (image base 0x400000, SHA-256
2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3).
Only observations and our implementation are recorded here.

The game consumes installed AddInItem records from AddIns.xml; unpacking a ZIP and
updating the original's registry belong to its external installer. OpenThedas keeps
packages in its own cache and uses mods.txt as the explicit enable operation, leaving
both the original install and its registry alone.

- The registry enumerates AddInItem records at 0x5fab11..0x5fb085. UID identifies a
  record; campaign aliases fall back from ExtendedModuleUID to ExtendedModuleName,
  then ModuleName (0x5faba3..0x5fac96). A folder without a corresponding record has no
  other record's campaign or enable settings to inherit.
- Metadata readers are 0x5f65c0 (format 0) and 0x5f6c30 (format 1). They read Title,
  Description, Version, GameVersion, Publisher, Type and PrereqList. Version is a
  release label; it is not the game's minimum version. OpenThedas reports the
  English text (falling back to DefaultText), release label and prerequisites in
  `thedas mods`.
- GameVersion is copied into the installed record at +0x118. The check at
  0x5edde0..0x5eded7 reads the executable's FileVersion, truncates at the second
  dot, numerically compares the two strings, and sets +0x177. The initialization
  loop calls it at 0x5fb6f5 and clears Enabled if it fails (0x5fb765). Packaged
  resources and level directories now use the same gate. With no readable game
  executable OpenThedas retains its existing policy of skipping this comparison.
- The two prerequisite readers, 0x5f3690 and 0x5f38c0, recognize ProductID,
  microContentID and Title (also strref in format 1). Complete records enter a
  vector; unknown or incomplete attribute sets do not. These are metadata and
  add-in identities, not paths or minimum-version expressions.
- Enabling an add-in at 0x5f5f40 compares the requested UID with installed records
  (0x5f6007..0x5f6054), respects the version flag (0x5f605f), and, when recursion
  was requested, calls itself for each prerequisite (0x5f6084..0x5f60c2). The
  recursive call receives microContentID, not ProductID or the display title.
  A missing record does not disable the parent: the search ends without a match.
  OpenThedas follows that operation for explicitly listed packages, reports a
  missing dependency, and visits each UID once so cycles terminate.
- Core registration accepts Type 1 or 2 (0x5f4610); explicit enablement cannot
  make another Type mount. Online authorization remains the separate, deliberately
  unapplied policy documented under FMT-011.

The package reader accepts UTF-8 and BOM-marked UTF-16 XML, decoded entities and
CDATA, and matches each item to its own Contents/AddIns child case insensitively.
Malformed XML and duplicate/path-like UIDs fail visibly; unmatched manifest items
and unmatched folders are reported. Override resources retain their mods.txt
position. No package content is executed or installed over game files.

Validation uses synthetic ZIP fixtures and real installed registry metadata. A
publisher-distributed DAZIP has not yet been supplied locally; this is a remaining
integration-validation limitation, not evidence of a running-original comparison.
