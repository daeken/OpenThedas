# Origins raw object formatting (SCR-031)

Static trace of Origins 1.05, image base `0x400000`; no original-game execution.
The registration at `0x675874` binds `ObjectToString` (54) to `0x64f890`.
That handler pops the raw 32-bit object value through `0xa2f6d0` and passes
it directly to the wide formatting helper `0x4c3030`, using signed decimal
`%d` at `0xaea900`. The helper counts the output through `0x8e7701` and
writes it through `0x8e78e7`. The handler then pushes the resulting string
through `0xa2d730`.

There is no object-manager lookup or validity test on this path. Existing,
destroyed, unknown and invalid IDs all undergo the same signed 32-bit
interpretation: `0x7fffffff` becomes `2147483647`, `0x80000000` becomes
`-2147483648`, and `0xffffffff` becomes `-1`. The engine's invalid sentinel
`0x7f000000` becomes `2130706432`, not an empty string. Formatting our IDs
does not make their allocation match the original engine's IDs.

`ToString(any)` is a separate handler (`0x668f30`); its object branch
resolves the object through `0x5d5840` and uses virtual member `+0x18`.
The text it reads at the result's `+0xbc` is inside the tag wrapper at
`+0xb8`, also read by `GetTag` (`0x6596a0`). It is distinct from the
template resource at `+0x1cc` used by `GetResRef` (`0x6594e0`). Objects
without that virtual member take a separate fallback. Those object-kind
and fallback semantics still need modelling; this fix changes only
`ObjectToString`. General numeric formatting remains under SCR-031.
DA2 has not been traced and retains its earlier unsigned formatting.
