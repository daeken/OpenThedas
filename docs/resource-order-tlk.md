# Origins named talk-table loading

The current English talk-table merge is provisional: `Vfs::talk_tables` lists language
suffix matches by filename and its consumers let later tables replace earlier strings.
The original uses named registrations and loaded-table lifetimes. Reversing the filename
sort, or sorting tables by VFS priority, would not reproduce it.

Static trace of the GOG executable:

- 0x4e5a80 receives a table/package name, builds `<name>_<language>`, tests that exact TLK
  through 0x4c5e40, and falls back to `_en-us` if the localized base is absent. 0x4e5810
  appends `.tlk`, loads the table, and appends a loaded-table entry through 0x4e5570.
  It can also load the `_f` companion; this is separate from DA2's `_p` patches.
- 0x4e4990 walks the loaded vector from front to back, asks 0x4e47f0 for the string id,
  and stops on the first hit. A later loaded table does not overwrite the earlier hit.
- Core registration creates CTlkTable and loads `Core` at 0x5d6562. Other registrations
  include module/campaign (0x5d51de), add-in module/core (0x5efc7b/0x5f4e65), offers
  (0x49a0cc/0x49b230), and extra campaign packages (0x5fde44).
- RIM-load notifications also open and append the named table (0x4e5770 -> 0x4e46e0).
  Unload handling removes entries (0x4e5cf0). Which tables exist for a lookup therefore
  depends on named resource load/unload state, not a wildcard filename listing.

The measured GOG global stack exposes 84 readable English tables and 205,843 unique string
ids. Of those ids, 1,596 occur in multiple tables; every overlapping value has identical
text. This audits our current membership, without claiming that the original loads all 84
at once. It supplies no differing shipped string with which to validate a collision fix.

The merge implementation remains unchanged. The follow-up is a named TLK registry tied to
RIM and campaign lifetimes, with a differing-duplicate-string fixture and before/after
winner dumps for every string id. The runtime ordering of all event paths and female-table
selection still need tracing; Origins does not establish DA2 `_p` precedence. Character
creation also needs the click-through and row-position checks whenever that behavior changes.
No original executable was run for this investigation.
