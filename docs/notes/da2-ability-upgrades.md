# DA2 upgraded ability numbers

The native handlers for `GetUpgradableAbilityHashedM2DAInt` and `Float` are
0x4e1700 and 0x4e17a0. They pass a zero fallback to 0x6a1300 and 0x6a1b50.
Both require a creature and its ability manager, and read M2DA 1 (merged ABI).

The requested column's value on the base ability is always the starting value.
The same column on row zero is metadata: only a nonzero marker enables upgrade
additions. Then, in order, the base ability's four upgrade-id columns
0xd300f4d9, 0xf82da71a, 0xe136965b and 0xae77009c are checked against the
creature's learned abilities (0x551e50). Each learned upgrade contributes its
own value from the requested column. Negative contributions are valid. The base
ability need not itself be learned. There is no recursion into upgrades of an
upgrade, and no uniqueness filter across the four authored slots.

Integer additions use 32-bit wrapping arithmetic. Float additions round to
single precision after each addition, as the native helper stores its running
sum. Invalid targets, missing tables/rows/columns yield the zero fallback.
OpenThedas computes the result from the current learned set on every query;
granting, removing and reloading abilities cannot leave cached upgrade totals.

These script-query additions are separate from conditional passive stat modifiers
in M2DA 117. Their native state selector (0x52f7c0) still needs its owner-component
enum and state-change callers mapped before using it for conditional bonuses.
