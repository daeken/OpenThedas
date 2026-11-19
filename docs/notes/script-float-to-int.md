# FloatToInt (SCR-031)

Static traces of Origins 1.05 and Dragon Age II at image base `0x400000`.
Neither original executable was run for these traces.

`FloatToInt` at `0x64f3f0` pops a single-precision float, loads it into the
floating-point unit, calls `0x8e34f0`, and pushes the returned 32-bit integer.
The conversion helper selects its SSE2 path using the CRT feature flag at
`0xc4dac4`. Initializer `0x9049e3` sets that flag from `0x904983`, which checks
CPUID feature bit 26 and operating-system SSE2 support through `0x904933`.

DA2's shipped LDF also declares `int FloatToInt(float f) = 49`.
Registration at `0x526155` assigns slot `0xc4` (`49 * 4`) to `0x4e3130`.
It pops a float via `0x9199f0`, calls the CRT helper `0xb2a680` and pushes
the returned integer via `0x90d510`. Initializer `0xb2b462` sets the SSE2
flag at `0xd86e4c` from CPUID bit 26 and an OS instruction probe
(`0xb2b400` and `0xb2b3b0`).

Both SSE2 paths (Origins `0x8e34f9`, DA2 `0xb2a689`) promote the input to
double and use `cvttsd2si` to truncate toward zero. Finite in-range values
discard their fractional part. An invalid conversion produces the integer
indefinite value, `-2147483648`. This includes either infinity, either sign of
NaN, and finite values outside the signed 32-bit range. Both signed zero and
single-precision subnormals convert to zero.

The largest accepted positive `f32` is `2147483520`. The next representable
value is `2147483648`, which produces integer indefinite. `-2147483648` is
valid and has the same integer result as an invalid conversion. The engine
checks a lower-inclusive, upper-exclusive range before its Rust cast, which
also sends every NaN to the native invalid result.

The implementation selects the native SSE2 behavior for both games on
every host architecture. Historical x87 fallbacks (Origins `0x8e3526`,
DA2 `0xb2a6b6`) have their own 64-bit intermediates and remain outside this
parity claim. Other numeric gaps in SCR-031, including legacy decimal
rounding, formatting and locale, remain tracked independently.

A synthetic NCS `CONST`/`ACTION`/`RETN` regression exercises both games
over signed zeros, subnormals, fractional values, adjacent signed-range
boundaries, finite extremes, infinities and signed quiet/signaling NaNs.
