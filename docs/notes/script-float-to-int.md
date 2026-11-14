# Origins FloatToInt (SCR-031)

Static trace of Origins 1.05 at image base `0x400000`. The original game was
never executed for this trace.

`FloatToInt` at `0x64f3f0` pops a single-precision float, loads it into the
floating-point unit, calls `0x8e34f0`, and pushes the returned 32-bit integer.
The conversion helper selects its SSE2 path using the CRT feature flag at
`0xc4dac4`. Initializer `0x9049e3` sets that flag from `0x904983`, which checks
CPUID feature bit 26 and operating-system SSE2 support through `0x904933`.

The SSE2 path at `0x8e34f9` promotes the input to double and converts it to a
signed 32-bit integer with truncation toward zero. Finite in-range values
discard their fractional part. An invalid conversion produces the integer
indefinite value, `-2147483648`. This includes either infinity, either sign of
NaN, and finite values outside the signed 32-bit range. Both signed zero and
single-precision subnormals convert to zero.

The largest accepted positive `f32` is `2147483520`. The next representable
value is `2147483648`, which produces integer indefinite. `-2147483648` is
valid and has the same integer result as an invalid conversion. The engine
checks a lower-inclusive, upper-exclusive range before its Rust cast, which
also sends every NaN to the native invalid result.

The implementation selects the native SSE2 behavior for Origins on every
host architecture. The CRT's historical x87 fallback at `0x8e3526` has its own
64-bit intermediate and remains outside this parity claim. DA2 retains its
previous conversion until its executable is traced. The other numeric gaps
in SCR-031, including legacy decimal rounding, formatting and locale, remain
tracked independently.

The engine regression exercises both games over signed zeros, subnormals,
fractional values, adjacent signed-range boundaries, finite extremes,
infinities, and signed quiet/signaling NaN bit patterns. Private compiled
script probes also exercise the normal VM ACTION path.
