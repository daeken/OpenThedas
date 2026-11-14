# DA2 delayed ability impacts

`ConstructAbilityImpactandDelayEvent` (script command 890) was previously unimplemented.
The shipped `ability_core` calls it when distributing an ability's impacts. Dropping that
call drops the later ability-script event, including the damage, combat result and target.

Read-only tracing of the player's DragonAge2 analysis image established registration at
0x526613 and handler 0x4f00d0. The handler constructs an event with the supplied type,
calling object as creator, four object parameters (caster, target, item, secondary target),
one float (damage), five integers (ability, combat result, hit, hand, damage type), and
one vector (target position). These are separate indexed parameter lists. It verifies the
recipient object, optionally stores the supplied script override, and schedules delivery.
It does not run the ability script synchronously or calculate replacement damage.

The implementation uses the existing deterministic event queue and snapshot serialization.
An empty script selects the recipient's ordinary event handler. Invalid recipients schedule
nothing. Millisecond rounding and nonpositive-delay normalization follow the existing
`DelayEvent` implementation; native scheduler sub-millisecond behavior remains provisional.
No Origins command or ability executor is changed.

A synthetic regression verifies every payload slot, distinct caller/caster/recipient,
explicit and empty script selection, deferred zero-delay delivery, invalid recipients,
and exactly one delivery at the original deadline after world serialization. Local
integration uses the player's installed scripts, separately from repository fixtures.
