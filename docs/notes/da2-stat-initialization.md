# DA2 stat initialization and old snapshots

The default attack resolver uses the shipped `dae_basic_attacks` event-1006 handler. Its accuracy inputs must be initialized before judging its balance. This note records clean-room understanding of the shipped scripts and the user's local analysis image; no game resource or executable material belongs in the repository.

## NPC levels after spawn

`creature_core` calls `AS_ScaleCreature` at bytecode 0xe9d4 from SPAWN (0x199f), passing -1 for its optional package and level. The monster path clears property bases, establishes health, and grants package abilities. It preserves an existing positive level for summoned effect 332 or assigns an explicitly supplied positive level. Ordinary monsters can correctly leave SPAWN with level property 36 equal to zero.

The next assignment is in `rules_core`'s PERCEPTION_APPEAR handler, not a native property-getter fallback. Its out-of-combat hostile-perception path calls 0x699c at 0x2b0c. For a creature still at level zero, that helper computes a level at 0x6a2e -> 0x6b33 and writes property 36 at 0x6a1b:

1. Read the party leader's level and XP (properties 36 and 35).
2. Round the level down if XP is below halfway between that level's and the next level's thresholds in M2DA 164, column 0xa0f02e99. Ordinary perception requests this rounding.
3. Read the party leader's area's local row 37. This selects M2DA 225. Clamp to its lower bound (column 0x7443cc93, at least one) and upper bound (0xa43f9ab9).
4. Unless M2DA 225 column 0x75c44589 bypasses it, apply the positive appearance ceiling in APR/M2DA 2 column 0xd0c45555.

The real prologue area's row 20 clamps these NPCs to level 3. A read-only MCP trace of the shipped handler confirms the same table lookups and the resulting property write. Normal spawn/perception continues to run those scripts; repair only reproduces the computation for missing levels in already-perceived/in-combat old snapshots. An NPC that has not encountered a hostile retains zero until the normal script assigns it.

## Repair policy

Old snapshots can hold PC-race/class creatures with all six attribute bases zero. Earlier engine versions omitted their appearance-derived race, so their spawn/hire scripts took the monster path. Valid attributes and assigned levels must survive reload unchanged. Noncombatants and rank-zero ambient actors must remain outside this repair.

For that recognizable missing-stat state, reconstruct the defaults from merged property table 98 column 0xaacda048. Union the race, class and package starter abilities, using the same table selection as AS_InitProperties/AS_SetRace/AS_SetClass, and restore the class primary attribute. Reconstruct missing XP/level from the package's initial-XP or party-XP-offset settings and the level thresholds. Preserve learned abilities, explicit unrelated properties, locals, commands, plots and RNG. Preserve each resource's current fraction when its maximum changes. Do not replay SPAWN or PARTY_MEMBER_HIRED, whose other side effects are inappropriate during load.

Attack/defense and unconditional ability contributions remain derived on query. Never save their totals back into bases: that would double bonuses on the next load. The repair is idempotent and does not change Origins' save revision. It reconstructs data that was never initialized; it cannot infer arbitrary player stat allocations absent from a damaged snapshot.

## Expected accuracy and observed outcomes

With the shipped prologue PC defaults and race/class bonuses, Hawke, Carver and Bethany have primary attribute 13. The native attribute-rating interpolation yields scaled attack 65.6. At target level 3, the attack optimum is 68; the below-knee conversion is:

`attack_fraction = 0.6 * (65.6 / 68)^4 = 0.51967394`

Melee hurlocks have defense bonus 10 percentage points, ranged hurlocks zero; the rank adjustment for this encounter is zero. After the shipped 5%-95% accuracy bounds, party accuracy is 41.967394% against melee and 51.967394% against ranged. Failed party accuracy rolls return BLOCKED (5), so blocks remain common even when the engine is behaving correctly. Dodge is a separate roll and critical classification follows successful accuracy.

Bethany's cunning bonus yields scaled defense 60. At an attacking hurlock's level 3, its percentage is `0.2 * (60 / 68)^4 = 0.121226996`. A melee hurlock therefore has accuracy 0.878773. On normal difficulty the script reserves the five percentage points immediately below that accuracy for BLOCKED (`0.1 * difficulty coefficient 0.5`). Expected outcomes against her, before critical classification, are 82.8773% hit, 5% block and 12.1227% miss.

A 2,000-roll sample for each pairing produced 42.55% and 40.45% hits for Hawke/Bethany against melee, and 53.05% for Hawke against ranged. The hurlock-to-Bethany sample produced 83.05% hits, 4.9% blocks and 12.05% misses. All are consistent with the formulas. These are tests of the default-attack resolver, not a claim that animation-specific combo abilities or the newer damage pipeline have complete parity.
