# RIM discovery in Origins scanned folders

A `.rim` found in an override folder is recorded for a later load by name. Its contents do
not become a searchable resource source during the directory scan. OpenThedas therefore
keeps excluding RIM contents from `Vfs::push_folder`, which also mounts mod folders.

Static trace of the GOG executable (the original was not run):

- 0x4cf9b1..0x4cfa10 identifies `.rim`, then calls 0x4c7b00 with its containing source,
  filename, priority, and scan metadata. Unlike the ERF branch, this does not call the
  source insertion function 0x4c7ea0.
- 0x4c7b00 lowercases the filename and records the containing directory and priority in the
  manager's map at +0x90, protected by its +0xe8 lock. Duplicate names compare containing
  sources with 0x4e6660; records in loading/loaded states are not replaced. This chooses a
  RIM file for a future request, rather than exposing its entries.
- Explicit loading through 0x4c8550 consults that map, builds the path, creates factory 2,
  then calls 0x4c8190. Parsing succeeds before 0x4c8259 inserts the RIM into the searchable
  sources. Only then does its rank 2 take part in resource precedence (ERFs rank 3).
- Actual callers explicitly load the four global RIMs (0x402100), and the module's
  `moduleglobal.rim` and `extradesignerscripts.rim` (0x5d51e3, 0x5d5220).

Thus “RIM before ERF” describes already mounted sources at equal priority; it does not mean
“mount every RIM found by a directory scan.” No automatic RIM mounting is added to mod or
override folders, and their current winners are unchanged.

OpenThedas still lacks the general named-RIM registry/load lifecycle. Installed package
RIMs are mounted eagerly by `archives_in`; that separate difference, including selecting a
same-named RIM from override at load time, remains open. A regression fixture checks that a
scanned RIM cannot introduce or override an entry while a neighboring ERF can.
