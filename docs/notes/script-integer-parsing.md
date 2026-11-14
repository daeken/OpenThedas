# Script integer parsing (SCR-031)

Read-only static trace of the GOG Origins executable, image base 0x400000.

`StringToInt` (51) and `HexStringToInt` (58) register the same handler,
0x64f510. It reads a string, calls 0x4c0950 and returns the integer; it does
not distinguish the function ids. The string conversion passes base zero to
signed wide `strtol` at 0x8e835f, implemented by 0x8e8168.

After leading whitespace and one optional sign, the first decimal digit selects
the base: nonzero means decimal, zero means octal, and zero followed by ASCII
x/X means hexadecimal. Conversion stops at the first invalid digit. No digits
give zero; trailing text is ignored. Both names therefore turn `077tail` into
63 and `0x2a` into 42, but bare `ff` into zero. Signed overflow saturates at the
32-bit limits, including hexadecimal input; `0xffffffff` becomes 2147483647.

The wide decimal-digit helper 0x8fc637 recognizes 18 ten-digit families starting
at U+0030, U+0660, U+06F0, U+0966, U+09E6, U+0A66, U+0AE6, U+0B66,
U+0C66, U+0CE6, U+0D66, U+0E50, U+0ED0, U+0F20, U+1040, U+17E0,
U+1810 and U+FF10. These also participate in the leading-zero test. Hexadecimal
letters are ASCII. Other decimal families are rejected, including supplementary
characters represented by surrogate pairs in the original.

Whitespace below U+0100 uses the CRT table referenced at 0xc15514. Its space
class contains U+0009 through U+000D, U+0020 and U+00A0, but not U+0085.
Higher code units go through the OS character-type wrapper 0x8ec839. Our
existing Unicode whitespace approximation is retained for that higher range;
parity with historical Windows character classification remains unverified.

This completes the traced integer prefix/radix/overflow behavior, including
the previously unanswered Origins `HexStringToInt`. It does not close SCR-031:
float parsing, formatting and the higher Unicode space classification remain
tracked there. DA2 has not been traced and keeps its previous behavior.
