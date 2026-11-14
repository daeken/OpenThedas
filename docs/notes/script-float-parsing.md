# Origins float conversion grammar (SCR-031)

Static trace of Origins 1.05, image base `0x400000`; no original-game execution.
`StringToFloat` enters `0x64f7c0`, pops a string and calls `0x4c09d0`.
That member calls wide `strtod` at `0x8e84b5`/`0x8e83b2`, then stores the
double result as a single-precision float. The script handler also stores and
pushes a single-precision value.

The wide wrapper skips leading whitespace through `0x8efedf`, using the same
classification as the integer parser. Below U+0100, that means U+0009–000D,
U+0020 and U+00A0, excluding U+0085. Higher whitespace still needs the historical
Windows classification traced in the integer note.

`0x8efe22` calls the decimal state machine at `0x8effcf`. Its dispatch table
at `0x8f06c3` has twelve states: initial, leading zero, sign, integer digits,
fraction, initial decimal point, exponent marker, exponent sign, exponent
zero, exponent digits, end and implicit exponent. The caller disables the
implicit-exponent option (`0x8f028f`), so `12-3` converts to 12, not 0.012.
The active locale supplies the decimal point; the engine's default C locale
uses a period. Digits are ASCII 0–9, unlike the integer parser's wide-digit
helper. There is no hexadecimal, infinity or NaN branch.

Integer/fraction scanning at `0x8f0162`/`0x8f01c1` accepts a decimal prefix.
At least one digit must occur, on either side of the optional decimal point.
`D`, `d`, `E` and `e` start the exponent (`0x8f0104`); one optional sign and
ASCII decimal digits follow. An incomplete exponent restores the pointer to
its marker (`0x8f0220`/`0x8f015a`), retaining the mantissa. All other suffixes
end conversion. No conversion sets flag 4, which the wrapper turns into
positive zero; a valid negative zero retains its sign.

The implementation follows that grammar and converts the accepted prefix to
`f64` before narrowing to `f32`. This is deliberately a partial SCR-031 fix:
the old CRT's digit buffering and intermediate binary rounding in `0x8effcf`,
`0x904767` and `0x903ce3` have not been matched bit for bit. Long mantissas and
rounding boundaries remain unverified, as do higher Unicode whitespace and
non-default locales. Formatting is separate. DA2 is untraced and unchanged.
