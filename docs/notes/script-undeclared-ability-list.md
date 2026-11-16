# Origins undeclared native 859 and historical ID 10

Own prose from the read-only GOG 1.05 executable trace.

The registration at `0x6764c6` associates 859 with `GetAbilityList` and handler
`0x673080`. It reads `(object, int, int)` and returns an integer array. It
requires a resolved creature and its ability component, otherwise returning an
empty array. `0x71d920` performs the enumeration:

* Type 4 is excluded; valid explicit types are 1, 2 and 3. Zero concatenates
  those three owned-ability lists in that order. Other types return empty.
* A nonzero final argument filters the already-owned list by positive remaining
  cooldown. It does not enumerate cooldown records, add unowned abilities, or
  duplicate an ability for separate source-item cooldown records.

The inferred 859 declaration now carries the traced name and dispatches to the
Origins implementation. The simulator uses its existing AbilityType metadata
and generic empty-source cooldown key for these owned types. Its owned set is
ordered by ID, so within each type group it retains deterministic ID order;
the original's insertion order is not recoverable from existing snapshots.
DA2's separately traced GetAbilityList remains unchanged, and inferred 859 stays
unimplemented for DA2.

Two native tests exercise the undeclared numeric signature and typed result,
type grouping, invalid types and objects, ownership versus cooldown membership,
source-specific cooldowns, boundary expiry and read-only behavior. Existing DA2
list tests remain in place.

## ID 10

The Origins registration sequence goes from IDs 6, 7, 8 and 9 to 13 at
`0x6753e4`–`0x67543e`; it subsequently registers 11 and 12. The complete
registration scan contains no entry for 10. The shipped script declaration
also has none. The audit identifies its callers as historical 2006 iteration
test scripts. Therefore this executable supplies no handler to implement for
10. The simulator keeps the unknown-native error rather than inventing a
signature or a successful no-op for those obsolete callers.
