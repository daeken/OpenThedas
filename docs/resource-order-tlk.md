# Origins named talk-table loading

The current English talk-table merge is provisional: `Vfs::talk_tables` lists language
suffix matches by filename and its consumers let later tables replace earlier strings.
The original uses named registrations, loaded-table lifetimes and an explicitly reordered
lookup vector. Reversing the filename sort, sorting tables by VFS priority, or applying a
stable gender partition would not reproduce it.

Static trace of the GOG executable:

- 0x4e5a80 receives a table/package name, builds `<name>_<language>`, tests that exact TLK
  through 0x4c5e40, and falls back to `_en-us` if the localized base is absent. 0x4e5810
  appends `.tlk`, loads the table, and appends a loaded-table entry through 0x4e5570.
  A present but unparseable localized base does not trigger an English retry. The base
  must load before its optional `_f` companion is attempted, in the same chosen language.
  Named base and female entries carry explicit false/true flags, respectively; this is
  separate from DA2's `_p` patches.
- 0x4e4990 walks the vector in its current order, asks 0x4e47f0 for the string id, and
  stops on the first hit. This query does not itself filter by the entry's gender flag.
- Core registration creates CTlkTable and loads `Core` at 0x5d6562. Other registrations
  include module/campaign (0x5d51de), add-in module/core (0x5efc7b/0x5f4e65), offers
  (0x49a0cc/0x49b230), and extra campaign packages (0x5fde44).
- The `RimLoadedMessage` handler (0x4e5770) uses 0x4e46e0 to open
  `<message-name>.tlk` directly and appends the resulting table. This path does not apply
  the named loader's locale/fallback procedure. The paired `RimUnloadedMessage` handler,
  0x4e4e50, forwards the message name to 0x4e4bd0. The handler types are confirmed by
  their Microsoft RTTI metadata, rather than inferred from their call shapes alone.
- **Unload by table name is 0x4e4bd0.** It compares the name stored in each loaded table
  object, frees matches and erases their vector entries through 0x4e4b10, backing up the
  iteration after removal so all matches are removed. The named loader stores the
  requested base name separately from its localized filename, so base and female entries
  unload together. Name equality is case-sensitive UTF-16 comparison, through
  0x4c0de0 -> 0x414170 -> 0x413800; resource filename case folding does not establish
  registry-name equality. No matching entry is still a successful return.

## Gender reprioritization

**0x4e5cf0 is not unload.** It walks the vector from front to back, copies an entry whose
flag matches the requested boolean, erases that entry with 0x4e4b10, then inserts the copy
at index zero with 0x4e56e0. This reverses the selected group's relative order and places
it before the other entries. For example, matching entries A then B become B then A.
The function has no unchanged-gender early return, so calling it again can reverse that
group again. The lookup's first-hit rule applies to the reordered vector.

The caller at 0x6361b1..0x6361d5 selects true for a creature whose appearance gender is 2;
the saved-player path at 0x6c7b48..0x6c7b7c does the same for GFF gender field 0x3fc2.
The earlier note that assigned unload to 0x4e5cf0 was incorrect.

One flag input remains unresolved: unlike the named loader, the inspected RIM-loaded
callback does not explicitly initialize its local entry's gender flag before copying it
into the vector. Its resource-handle constructor initializes only that handle. No trace
here establishes a deterministic flag value for that path; assigning false would be an
additional assumption.

## Measured membership and implementation boundary

The measured GOG global stack exposes 84 readable English base tables and 205,843 unique
string ids. Of those ids, 1,596 occur in multiple tables; every overlapping value has
identical text. No English `_f` tables are present in this stack. This audits our current
membership, without claiming that the original loads all 84 tables at once, and supplies
no differing shipped collision with which to validate an ordering correction.

The merge implementation remains unchanged. The follow-up is a shared named TLK registry
for the simulation and UI, with differing-duplicate-string fixtures, first-hit and gender
reordering checks, locale/female membership, case-sensitive unload-all and RIM message
lifetimes. Exact startup ordering of all registration/event paths and the unresolved RIM
flag still need tracing; VFS priority alone does not establish them. All-resource and all-string winner
dumps, table row-position audits and the character-creation click-through are required
before changing that behavior. Origins does not establish DA2 `_p` precedence.

Binary identity: DAOrigins.exe SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`, image base 0x400000.
No original executable or VM was run. These are static observations; synthetic fixtures
would check the replacement's selected behavior, not execution of the original game.
