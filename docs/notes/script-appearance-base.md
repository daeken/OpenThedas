# Origins script appearance changes (SCR-029)

Static Origins 1.05 trace; no original-game execution. DA2 is untraced and
retains its earlier behavior.

`SetAppearanceType` (`0x674c80`) accepts a creature (`0x5d58c0`) and either a
positive appearance or -1. Zero and other negative inputs return unchanged.
The appearance block keeps unsigned 16-bit current (`+0x58`) and base
(`+0x5a`) values. Positive arguments narrow to 16 bits after validation;
there is no table-row validity check before assignment.

When the third argument is nonzero and the requested appearance is positive,
the handler updates the base through `0x747b40` **before** comparing the
current value with the low 16 bits of the request. An unchanged current
appearance can therefore still change its base. -1 never updates the base.
The comparison also means current 65535 makes a -1 request return unchanged.
For differing values the handler copies the appearance block (`0x74a610`),
then sets its current appearance through `0x748a40`, selecting the base for
-1. Both current and base survive the copy.

OpenThedas records this mutable value in the optional saved
`Object::base_appearance`. On the first positive script change it captures
the previous template fallback, or the current appearance when no template
can be read. Subsequent temporary changes retain that base. Old saves omit
the optional field and continue to use the template fallback until a script
change captures or assigns it. Existing saved JSON without the field loads
without a revision migration.

Native initial loading remains a separate SCR-029 gap. The structured reader
at `0x748b80` initializes base from current, but `0x74a8fc..0x74a96a` can use
a table-defined base and an explicit GFF field 16327. OpenThedas does not yet
initialize its base from those native table/save inputs. This change covers
the script setter's validation, narrowing and mutable-base behavior; it does
not claim native template/save initialization or model/animation attachment
parity.
