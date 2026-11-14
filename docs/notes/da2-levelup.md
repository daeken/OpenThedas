# DA2 level-up input

DA2's sys_chargen has no cases for Origins' manual attribute/ability assignment
events 50 and 51. Sending those events opened and closed a level-up screen but
spent no points. The native input uses creature messages instead:

- BuyAttributeMessage: registered handler 0x578c90 adds the signed amount to the
  attribute base and subtracts it from property 38.
- BuyAbilityMessage: registered handler 0x598ce0 reads the ability registry,
  subtracts its authored cost for a purchase, adds the learned ability and places
  a usable ability on the quickbar. A negative id removes it and refunds its cost.
  The registry loader at 0x5cb490 maps type column 0x100b492a values 1, 2 and 5
  to point pool 39, and type 3 to pool 40. Cost is column 0x2058ed0b.
- AutoLevelUpMessage: 0x45ef20 sends module event 56 with the creature in object
  slot 0. sys_chargen owns the table-driven attribute and talent choices.
- SetCanLevelUp: 0x4eaf30 ignores its flag and calls 0x544580, which sums
  properties 38 and 39 and updates the creature's level-up mark. Specialization
  points alone do not set that mark. GetCanLevelUp reads the mark (0x4f7750).

The shared simulation input helpers route manual choices through the correct
handler for each game. The MCP uses the same helpers and reports DA2's level
(property 36) and its actual unspent pools; property 35 is XP, not skill points.
DA2 attribute order is strength, dexterity, magic, cunning, willpower, constitution.
The portrait and character-sheet level-up availability also use the DA2 pools.

Manual input rejects invalid attributes, absent ability metadata, unsupported
ability types, duplicate changes and insufficient points. As in the native
creature handler, this does not reconstruct all eligibility checks performed by
the ability-tree movie. Quickbar slot selection retains the existing provisional
policy. The native optional quickbar-overflow tutorial notification is not added
here; it does not execute the learned ability's damage or passive bonuses.

IsAbilityAvailable (0x4e6280 -> 0x556600) is eligibility to learn: truncated
level 36 must meet column 0x38681c66; each nonzero prerequisite ability in
0x0ca45f1d, 0xb2de6b6b, 0x99f338a8 and 0x80e809e9 must be learned; both
attribute-id/value pairs (0x535458ed/0x2150237a and 0x78790b2e/0x0a7d70b9)
must be satisfied. It does not require the queried ability itself or available
purchase points. Treating it as HasAbility made sys_chargen's auto allocator
incapable of learning: it specifically requires unlearned AND available.

DA2's GoToLevelUpMessage (0x647fe0) identifies the requested creature in event
24's object slot 0. The module handles tutorials rather than calling StartCharGen
as Origins does. The simulation opens the requested member's input scope itself;
a stale scope for Hawke must not prevent opening a companion's screen after the
first level-up. Input remains limited to living party members out of combat with
attribute or talent points. Character creation's existing start/end path is unchanged.
