# DA2 recovery between fights

DA2 does not use Origins' regeneration amounts and properties table. Its native
creature-stat update at 0x56d7a0 adds elapsed milliseconds to one saved clock,
consuming passes while the clock is strictly greater than 2000 ms. Both combat
states use that period. A pass runs only while current health is positive.

The property-vector pass at 0x5bd660 invokes each property's regeneration method.
For depletables, 0x5bd8a0 adds a fraction of the nominal total through 0x5aa7f0:

- Outside combat: 0.4 of the nominal maximum per pass.
- In combat: 0.02 times the total of the regeneration property per pass.

The loader at 0x5d7e2c wires health 7 to combat regeneration 9 and stamina 10 to
combat regeneration 12. Health reservation is property 8 and stamina reservation
is property 11. These are percentages, clamped between 0 and 100. The current
setter at 0x5bd800 caps the usable value at nominal total times the unreserved
fraction; a maximum/total query still reports the nominal value.

This explains a concrete route failure: after an encounter the old simulation
carried Carver at 1/165 health and Aveline at about 29/165 into the next group,
despite a long exploration walk. Both had left combat. Three native recovery
passes would restore an unreserved pool from nearly empty to full. Warrior
stamina likewise stayed empty without the DA2 recovery path.

The simulation now uses that percentage recovery and clock for DA2, preserving
the remainder in saves. Origins retains its existing table-driven amount per
pass and separate combat/exploration periods. DA2's native per-property flag
that disables regeneration is not represented yet. This change does not select
basic-attack combos or provide projectile motion.
