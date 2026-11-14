# DA2 encounter registry

The DA2 creature templates store `EncounterID`. The older getter read `EncounterId`, so it returned zero for authored members. The older setter wrote that alternate spelling, and `GetEncounter` was unimplemented.

The user's local analysis image establishes the command behavior:

- Command 425, `SetEncounterId`, at 0x4e7260 resolves a creature and its area. Without an area it does nothing. Otherwise it unregisters the creature's old id with 0x55a060, registers the new id with 0x55e050, then writes the stored id. It does this even if the id is unchanged.
- Unregistration finds/removes the matching id (0x65c140 -> 0x47a9e0 and 0x6f6ef0). Registration appends the creature's object id to the positive encounter's list. Nonpositive ids have no list.
- Command 426, `GetEncounterId`, at 0x4e7300 returns the stored creature field, zero for an invalid creature.
- Command 427, `GetEncounter`, at 0x518f00 asks the current area for its registry (0x5559f0). Nonpositive ids, missing areas and absent encounter lists yield an empty object array. The getter copies the member list without filtering active, dead, party or hostile actors.

The simulation retains registration order, filtering the current area and stored encounter id at query time. Creation, reassignment, cross-area movement and destruction keep it current. The order survives save/reload. The list is absent from Origins' saves and is only maintained for DA2. Queries change no state or RNG.

Older OpenThedas snapshots have no saved registry order. Recovery uses object creation order; this is provisional because prior script reassignment order cannot be reconstructed. An old setter's `EncounterId` value takes precedence over the template's `EncounterID` when both are present, then is normalized to the canonical spelling. Subsequent registrations retain their actual order.
