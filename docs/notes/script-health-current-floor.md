# Origins creature health setter and current floor (SCR-026)

Read-only static Origins 1.05 analysis. This covers the creature path of
`SetMaxHealth`, the health floor written by the immortality setters, and
the depletable current clamp. Other object health virtuals and DA2 remain
outside this slice.

Despite its name, `SetMaxHealth` at `0x65a330` does not change a creature's
maximum. It resolves a CCObject, sign-extends the low 16 argument bits and
calls virtual `+0x12c`. The creature vtable `0xb10484` selects `0x6ab5b0`.
That method requires an existing property 7, converts the narrowed integer
to float and calls the property's **current** setter at `+0x2c`. It marks
the container dirty. The base, modifiers and total are not assigned. For
example, a creature with base 40, modifier 13.5 and current 12 receives
current 30 from an argument of 30; its total remains 53.5. Argument 65535
becomes -1 before clamping. Missing health is not created.

The current setter `0x79c8d0` reads the property's floor at `+0x78` and its
computed total, then calls `0x79c2d0`. It selects the request when greater
than the floor, otherwise the floor; it then selects the total unless the
total is greater than that result. Thus a total of zero wins over a floor
of one, and an unordered total is returned. Base and modifier changes at
`0x79c860` and `0x79c890` also call this current setter. Script property
writes and reclamps now share that selection with the mana setter.

`SetImmortal` (`0x659c50`) and `Engine_SetImmortal` (`0x6d7b10`) call virtual
`+0x16c`. The creature implementation `0x6ad130` stores the flag and, only
when health exists, writes its floor through property virtual `+0x30`:
one for any nonzero argument, zero otherwise. The floor setter `0x79c490`
does not reclamp current. Mana is unaffected. The base-object implementation
`0x699fe0` only stores the flag.

The floor is separate state, not a value derived from the flag on every
write. Property construction `0x79ca30` initializes it to zero; clone
`0x79c7d0` copies it. The base-object constructor initializes its flag at
`0x699b59`. The base-object reader `0x69aad0` assigns the loaded flag directly
at `0x69aea7`; the creature reader calls that reader at `0x6c0745`.
The property reader `0x79ca80` has no floor field. These flag-only operations
must not be conflated with the creature's floor-setting virtual. This trace
does not establish all later initialization, death or resurrection paths.

OpenThedas stores `health_floor_one` separately, defaults it to false and
omits that default from JSON. Old worlds therefore retain the initialized
zero floor, including worlds with a true immortality flag; they contain no
evidence that the creature virtual previously ran. New worlds preserve the
floor through our own save/reload and object cloning. Native save-format
support and a complete native load lifecycle are not claimed. Other object
health virtuals, DA2, death scheduling and status answers remain SCR-026
work. No table contents, merge precedence or row positions are changed.

The full s01–s13 comparison against main `125f5b43` has identical logs,
ordered plot writes and ending areas. Its only saved-state changes are
77 added true floor members across 14 of 35 saves, on 11 creature IDs.
Each belongs to an immortal creature with existing health; removing just
those members makes all 35 complete states equal. No health, base,
modifier, timing or other saved field differs in that sweep.

## Combat and developer-heal review

The independent property floor does not replace the combat scripts' protection.
`Sim::apply_effect` in `combat.rs` dispatches `APPLY_EFFECT` to the creature's
script, which forwards damage to `rules_core`. Its damage handler tests
`IsImmortal` and explicitly writes current health 1 for a lethal hit. That
`SetCreatureProperty` call reaches `Sim::set_creature_property` in `engine.rs`;
both its set and update branches call `clamped_current`. A requested 1 remains
1 with a normal positive total even when `health_floor_one` is false.
`Sim::check_death` separately refuses to kill immortal or plot creatures.
Plot targets reject damage before the health write: they keep their existing
health, including 1 when starting at 1, as on main.

The review probe applies 33 lethal hits per binary, including repeated hits
after our save/reload, old saves lacking the floor member, all immortal/plot
flag combinations, and an actual immortal template created through
`CreateObject`. It retains a false floor and survives at 1 after becoming a
hostile combat target. Mortal controls reach zero and die. Health/death results
agree with main; only explicitly assigned floor metadata differs.

A fresh extended s01–s16 comparison against main `d0a0e51a` records every
existing health inspection and party health before/after each `healplayer`
invocation. All 9,599 observations and 44 heal pairs per binary are
byte-identical. The s12/s13 health audits contain 1,244/3,333 observations
and 5/16 heal pairs, respectively; their story logs also match. The shipped
developer heal uses current-property updates, not `SetMaxHealth`. All 32
scenario exits succeed. Extending the chain retains 116 true floor members
on 12 creatures across 18 of 35 saves; removing only those members makes
the complete worlds equal. No current health, total, modifier or other field
differs. Native load/death lifecycle questions outside this combat path
remain open.
