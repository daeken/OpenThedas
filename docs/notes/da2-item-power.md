# DA2 item property types and scaled power

GetItemProperties (316) is registered at 0x52709d, wrapper 0x51b360, with
enumeration at 0x56f210. A desired type of -1 includes every property; other
values match column 0x12a02374 of item_properties (M2DA 12001). The result has
unique property IDs in item order. The old shared implementation ignored this
filter. DA2's basic damage script requests type 5 and takes the first result;
returning all properties let it take a combat-style property instead.

GetItemPropertyPower (784), registered at 0x5270aa, wraps 0x4defb0 through
0x4ebcf0. It returns a float and delegates to 0x54dbd0. An unscaled query returns
the first matching local base power. Scaling reads the same property row:

- Mode 0 (and values other than 1 or 2) uses a quadratic in the item's level:
  (A * level + B) * level + C, then multiplies by base power.
- Mode 1 uses 0x697b90: property_functions 371 selects one of four columns in
  optimal_values 405, at the item's level row.
- Mode 2 finds the first damage-type property through 0x545aa0, evaluates its
  scaled value through 0x54d7d0 and multiplies the row's coefficient and base power.

Unless the row bypasses item-type scaling, type 3 uses base-item column
0x42f209a3 and material column 0xea246395 (tables 12000 and 12002); other types
use base-item column 0xdafb8bab. Missing factors default to one. The selected integer or
float effect parameter then multiplies the result (selection 0x36001b03).
Float parameters may use 0x52fb40's signed rounding: magnitude below one becomes
one; otherwise floor(abs(value) + 0.5), with the original sign. The API's integer
parameter cases return their multiplied float without the separate effect-field
integer conversion. The helper used by mode 2 returns that same float.

The simulation's existing item model stores integer base powers and has no
socketed sub-items. Native rune recursion and fractional/modified power storage
therefore remain unsupported; the include-sub-items argument has no extra
objects to enumerate. A malformed metadata cycle returns zero rather than
recursing indefinitely. Origins keeps its existing APIs and behavior.

This connects the shipped damage script to the requested property and its native
table scaling. It does not replace the game's damage formula, change difficulty,
or resolve DA2 animation timing and projectile flight.
