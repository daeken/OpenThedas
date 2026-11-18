# DA2 threat table maintenance

The creature update at `0x5439f0` reads the total of property 41,
`Threat_Decay_Rate`, and invokes `0x6c4f10` with the rate multiplied
by elapsed milliseconds in seconds. A missing or zero rate skips the
table update. The Origins path reads property 40, which DA2's
`properties.gda` assigns to specialization points. At the entrance
to encounter 10008, the emissary and hurlock archers have zero in
property 40 and `-0.5` in property 41.

The table update resolves each enemy as a creature. It adds the
delta to accumulated threat with a floor of zero and recomputes
proximity on the ground plane with the same 60 m and 100-point
constants at `0xd37130` and `0xd3712c`. This keeps the existing
simulation update schedule and its provisional tie order.

`ClearThreatTable` 202 registers at `0x5269b5` to `0x4e5cc0`. The
wrapper defaults the optional boolean to true. At `0x6c4e80`, true
empties the table and false zeros the accumulated component while
retaining enemy IDs and proximity, then reorders entries. The
shipped rules script issues the false call for the emissary and a
hurlock in the 10008 baseline. Previously both calls deleted the
target history. The candidate applies these semantics only in DA2,
preserving the old Origins path and save representation.
