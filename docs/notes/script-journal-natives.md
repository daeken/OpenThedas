# Origins journal closure and removal

Own interpretation of the read-only GOG 1.05 `DAOrigins.exe` trace. No executable
bytes or extracted code are included here. DA2 dispatch is unchanged.

## Native paths

* Native 846, `CloseQuestGroup`, enters at `0x663f00`. It reads a resource name,
  gets the journal manager through `0x5d5810`, and tests whether the name is a
  registered quest group with `0x6ebac0`. A group goes to `0x6f5da0`; another
  name goes to the individual closer at `0x6f5380`. Both calls disable the
  completion notification argument.
* Group closure snapshots the active list, selects entries whose PlotGroup
  matches the name, and calls the individual closer for each. It does not
  establish a persistent rule closing quests added to the group later.
* Individual closure searches only the active list. It sets the selected
  entry's Completed attribute (`+0x138`). An entry with a ParentPlot returns
  at this point: completing a subquest does not archive its parent or siblings.
* For a root quest, the closer follows the active entries' ParentPlot links
  and collects the root and its descendants. It clears their Updated attribute
  (`+0x170`), moves them from the active list to the completed list, and clears
  the active-quest selector only when that selector names the selected root.
  It does not set every descendant's Completed attribute. Consequently the
  list an entry belongs to and its Completed bit are separate state.
* Native 850, `RemoveQuest`, enters at `0x663fe0`. A group name is ignored.
  Otherwise `0x6ef5e0` searches the active, completed, and pending entry lists
  and erases the named entry. There is no plot-flag write, descendant traversal,
  or active-quest selector write in that path.
* Resource comparisons use the entry's resource name, not its plot GUID.
  The implementation accepts the simulator's existing optional `plt_` prefix.

## Representation and limits

The existing journal is reconstructed from plot flags, entry marks, and plot
metadata. Additive `World.quest_states` records the explicit Completed,
archived-list membership, and removal overrides by plot GUID. Empty overrides
are omitted from JSON and older snapshots default to no overrides. Flags remain
available to scripts after removal. A subsequent journal-text update can recreate
the removed entry. Explicit closure raises no HUD completion alert.

The executable's flag update (`0x75ff6b` onward) can defer a child in a pending
list (`0x6f1cf0`) until its parent becomes active; active-entry creation at
`0x6f5040` subsequently promotes pending children. The simulator's existing
parent-heading reconstruction does not model that pending list. These natives
operate on the reconstructed visible lists; removing a pending-only entry and
the precise activation order of later children remain limitations. Parent-first
group traversal follows the native invariant that a child becomes active after
its parent. No asset parsing changes are made.

## Tests

The `journal::tests::native_` tests invoke the script dispatch with synthetic
plots and distinguish root archival, a subquest's Completed bit, and group
closure. They cover resource names versus GUIDs, case folding, repeated calls,
missing and unseen entries, headings without their own text, notification and
plot-flag preservation, single-entry removal, later text recreating an entry,
JSON compatibility, and unchanged DA2 dispatch and rendering state.
