# DA2 package-order trace boundary

DA2 retains the existing xoreos-derived order in `push_package`: loose data and selected data
subfolders, loose textures/audio/patch, then archives in data, abilities, selected textures,
and patch. This is an implementation choice pending a complete DA2 trace, not evidence that
the original puts loose files below archives.

A bounded static inspection used the existing captured-code analysis PE under DAODecomp,
without starting a VM or executing either game. The installed retail build's SHA-256 is
478357e492859402618e2225df25d6d78a8105ea6f30f548c19602483474ed0a. Its disk `.text` is encrypted;
the analysis PE replaces that section with previously captured mapped code. The capture
manifest identifies the analysis PE as SHA-256
c40556fa6fa05abde376987d332afd6ed964b9094b18710ad762e890450fbae2. Addresses below are VAs at
image base 0x400000 and refer to that captured retail build, not the separately available demo.

The inspected startup function registers factories at 0x7b594a..0x7b59a3 and these core folders
through manager accessor 0x8fc9e0 and registration function 0x8de9c0:

| Folder | Priority | Anchor |
| --- | ---: | --- |
| audio sound | 100008 | 0x7b59c6 |
| audio VO | 100006 | 0x7b5a09 |
| data | 100005 | 0x7b5a4c |
| env | 100002 | 0x7b5acf |
| locale | 100001 | 0x7b5b2e |
| patch | 100000 | 0x7b5be4 |
| install override | 1 | 0x7b5c67 |
| user override | 0 | 0x7b5ca4 |

User core data/locale/patch use 20007/20006/20005 respectively. The directory factory is
0x9092f0 (registered as kind 1), which sets its scan flag before calling 0x908cd0; its rank
method, 0x903b20, returns 1 when the flag is set, 0 otherwise.

Registration calls comparator 0x8d8ac0 at 0x8deb8b before insertion. The comparator uses
unsigned priority ascending (+0x80), then signed rank ascending (virtual slot +0x24), then
path descending (+8; comparison helper 0x404a40 returns `existing.path < new.path`). This
structure agrees with Origins, but matching structure alone does not establish all factory
ranks, RIM/ERF discovery and patch behavior, or campaign/add-in registration in DA2.

No DA2 reorder is proposed. Remaining work is the full factory/scan and package lifecycle
trace, followed by all-name winner dumps on a DA2 install and explanation of every move.
Do not copy Origins constants or its loading policy into DA2 on the basis of this partial
trace. These are static observations, not validation against a running original.
