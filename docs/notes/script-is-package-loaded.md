# Origins native 862: IsPackageLoaded

Own interpretation of the read-only GOG 1.05 executable trace.

The registration at `0x6793ac` selects `0x6657b0`. The handler reads one string
and calls the module manager's `0x5fbc10`. That method case-folds the query and
each name in its string vector (storage at `+0x88`, 28-byte strings), then tests
whole-string equality. It does not inspect add-in enablement, paths, resource
names, archive labels or the currently controlled creature.

The campaign-loading path reads CIF field `0x42e4` (17124) at `0x5fd7f3`. It
iterates that string list and appends each package name to the vector at
`0x5fd8ee`, before registering that package's resource folders. The query is
therefore about the campaign's additional package dependencies, not every
installed package or the implicitly present base core resources. The base
campaign has no additional package dependency; Awakening requests core_ep1.
An installed Awakening add-in's core files alone must not make this query true
while the base campaign is loaded.

The simulation now reads that list into ModuleInfo.packages during CIF loading.
Origins native 862 searches those names using ASCII case-insensitive equality.
Loading a JSON world already requires the same loaded module, so its campaign
dependency metadata remains available; it does not need a duplicate world-save
field or a save revision change. Loading a different campaign replaces the
metadata. Existing DA2 behavior is preserved pending a DA2 trace.

The existing resource loader's broader support for additional campaign packages
and standalone add-in campaign selection is separate from this query. This
change does not mount all installed expansion resources into the base campaign.

Three tests cover the inferred 862 declaration and dispatch, exact matching,
case folding, optional trailing NUL, absent dependencies, misleading installed
add-ins/layer labels, same-module save restoration, retained DA2 behavior, and
a synthetic CIF dependency-list write/parse round trip including absent/null
lists. Because the CIF metadata reader changes, validation includes the corpus
in addition to the sim regression gate and determinism checks.
