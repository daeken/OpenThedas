# 06: Abilities

Gathered 2026-11-12 on main 1eb844a. Source: abi_base.gda as the game resolves it (patch002.erf wins), 551 rows. The DLC and Awakening
worksheets (abi_gxa, abi_drk, abi_str, abi_gib, abi_lel, abi_shale, abi_base_* variants) are not counted. Counts only.

## Families

Split by the `AbilityType` column: 1 talent, 2 spell, 3 skill (per thedas-sim quickbar.rs), and 4 item (inferred). Monster abilities are
taken to be ids 90000–99999 of type 1 or 2, which is inferred from the id ranges and labels.

| family | rows | mentioned in a test, gate log, QA log or note (upper bound) |
| --- | --- | --- |
| talents (type 1, not monster) | 138 | 62 |
| spells (type 2, not monster) | 93 | 22 |
| skills (type 3; 30 of them follower-specific) | 70 | 16 |
| monster abilities | 147 | 9 |
| item abilities (type 4, and ids 200000+) | 102 | 47 |
| total (excluding row 0) | 550 | 156 |

How "mentioned" was counted: a regex over OpenThedas-testdata (logs, gate runs, qa runs, notes, sim-work JSON and driver scripts) and over
crates/*.rs (tests and source; 37 ids there). It looks for an abi_base id next to "ability", "talent", "spell", "skill", "cooling down" or
"UseAbility", or in the `"id:` cooldown form. That makes it an upper bound on what was exercised: a mention is not proof the ability ran,
and short ids can collide with other numbers. No test or gate step asserts per-ability outcomes. The gate compares plot flags and end
areas, so abilities are exercised only incidentally by the party AI in chain fights (s06, s07, s09, s10, s12–s14) and autoplay scenarios.

## Known broken or untested families (from notes/gaps-sim.md, handoff-sim.md and source)

- Monster abilities: 138 of 147 never mentioned. Broodmother (16), archdemon (5), golem (5), dragon and werewolf rows are beyond any save the harness reaches.
- Summoning (Animate Dead, summon rows): not found in the sim (gaps-sim D2).
- Shapeshifting (Shapeshifter spells): appearance and ability swap unverified (D1).
- Ranged talents and projectile spells: projectiles land instantly, with no flight time (B2, handoff-sim §7.3).
- Ability timing: provisional cast-start-to-impact; the tree search order and the no-animation 1.5 s case are untraced (B3, handoff-sim §7.3).
- AoE spells: AoE flags (GetAOEFlags/SetAOEFlags unimplemented), shapes and friendly fire by difficulty unverified (B5).
- Control effects (stun, knockdown, grab, paralyze, sleep, petrify): engine-side movement and action locks only partly modelled; effect flags are not enforced by the sim (B7; see 02).
- Sustained and modal abilities: implemented, upkeep and fatigue only partly verified (D3).
- Specialisations: unlock via plot (trainers, manuals) not audited (D4).
- Skills:
  - Trap-making and trap detection: GetTrapDisarmDifficulty and GetTrapDetectionDifficulty are unimplemented (B13).
  - Stealing: IsItemStealable is unimplemented (B14).
  - Poison-making and Herbalism: the crafting flow is not built and ShowCraftingGUI is unimplemented (D8).
  - Coercion (persuade/intimidate): the dialog skill-check plumbing is unverified (F13).
  - Stealth: detection roll timing untraced (B12).
- Item abilities: health poultices are used by the chain driver. Runes and item upgrades are not built (D7); SetCanUseItem is unimplemented (E2).
- Injuries: injury on knockout and the injury kit flow are unverified (D9).
- UI: the skills (K) and spells/talents (P) screens leave 98 and 170 AbilityInstance cooldown/pending/status values unanswered (see 03).
