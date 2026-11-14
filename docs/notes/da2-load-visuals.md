# DA2 environmental visuals during snapshot reload

The shipped real-prologue area script applies permanent VFX 600001 to its
firewall both during the initial area setup and in its event-43
`AREALOADSAVE_POSTLOADEXIT` handler. A read-only script disassembly and an engine
call trace confirm that this reapplication has no preceding removal. OpenThedas
snapshots already retain the original visual, so each reload used to add a copy.

Snapshot compatibility now reconciles exact reapplications from the three area
save-load events. It applies only to a DA2 permanent, non-expiring visual with
ability zero, created by the target placeable's own area. `APPLY_EFFECT` still
runs. Afterwards one matching copy is retained; older accumulated copies are
removed when that visual is reissued. Other effects remain in order. Normal
gameplay applications, Origins, temporary effects, creature/item targets,
different creators and ability visuals retain their existing stacking behavior.

This is a deliberately narrow policy for replaying OpenThedas snapshots, not a
claim that native VFX generally cannot stack. Native application dispatches visual
type 4 through 0x6caf90, and 0x6c3820 selects presentation by visual group and
priority. The native effect-list writer at 0x8b6b10 does not establish a blanket
uniqueness rule. Native save/presentation lifetime parity remains separate from
this repair; no global visual deduplication or save-format revision is introduced.
