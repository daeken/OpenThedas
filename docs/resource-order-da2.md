# DA2 core folder priorities and package-order trace boundary

The install's DA2 core now groups its existing sources by the traced folder priorities:
audio, data, textures, patch, lowest first. Previously all loose sources came before every
archive, allowing a data archive to hide a loose patch or texture resource. The install core
override remains the higher-priority overlay. Origins and DA2's other packages are unchanged.

This is a bounded folder-priority correction, not a complete source/lifetime implementation.
Membership and internal order within each core group are preserved. In particular, data's
selected loose children still precede its eagerly loaded archives, as do loose textures and
patch files within their groups. The remaining xoreos-derived choices are not evidence of
the original's internal order.

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
| textures | 100003 | 0x7afa60 |
| env | 100002 | 0x7b5acf |
| locale | 100001 | 0x7b5b2e |
| patch | 100000 | 0x7b5be4 |
| install override | 1 | 0x7b5c67 |
| user override | 0 | 0x7b5ca4 |

User core data/locale/patch use 20007/20006/20005 respectively. Texture setup separately
registers `TEXTURES:` through the same manager, factory 1 and priority 100003, at
0x7afa3f..0x7afa73. Its alias builder (0x7ae8b0) selects `packages/core/textures/high` or
`medium`; patch textures have a separate alias and priority (50001). This correction does
not change the existing texture membership or implement that alias lifecycle.

Startup factories and their virtual rank methods are:

| Kind | Factory | Constructor / vtable | Rank method | Rank |
| --- | --- | --- | --- | ---: |
| plain directory | 0x909270 | 0x908cd0 / 0xc262a4 | 0x903b20 | 0 |
| scanned directory | 0x9092f0 | 0x908cd0 / 0xc262a4 | 0x903b20 | 1 |
| loaded RIM | 0x8ffab0 | 0x8ff6c0 / 0xc2625c | 0x896160 | 2 |
| ERF | 0x901400 | 0x901260 / 0xc261e4 | 0x81c1e0 | 3 |
| RIMP | 0x9018f0 | 0x901840 / 0xc26214 | 0x6eb540 | 4 |

The directory rank is its scan flag at +0xf4, not the separate change-notification flag
at +0xf5. Indexing at 0x9082d0 recursively registers children as factory 1 with the parent's
priority (0x90851e..0x90853f), registers ERFs as factory 3 (0x908579..0x908656), catalogues
RIMs for later loading (0x90869f..0x908709 -> 0x9076c0), and registers RIMPs as factory 4
(0x908718..0x9087a7). These branches are distinct.

Registration calls comparator 0x8d8ac0 at 0x8deb8b before insertion. The comparator uses
unsigned priority ascending (+0x80), then signed rank ascending (virtual slot +0x24), then
path descending (+8; comparison helper 0x404a40 returns `existing.path < new.path`). This
source list is the manager's +4 list. The path query at 0x8e5d50 starts at its front
(0x81dba0), skips unavailable sources, asks each source's +8 lookup and stops at the first
hit (0x8e5de6 -> 0x8e5e13). Iteration advances through 0x5023d0. The existence query at
0x8dc000 likewise stops after its first successful source lookup.

Do not translate the factory ranks into a blanket loose-over-all-archives reorder. Startup
0x7ae550 explicitly loads eleven global RIMs at priority 100004, above ordinary data at
100005. RIM discovery alone does not mount their contents, and our eager membership still
includes other RIMs. RIMP construction also records nonempty metadata-bearing sources with
the manager through 0x8f4300; its full use still needs tracing. A suffix sort cannot substitute
for those lifetimes and priorities.

The implemented correction therefore changes only relative core folder groups. Remaining
work includes named RIM discovery/load/unload, internal source ranks and path order, RIMP
metadata, selected texture aliases/patch textures, nested archive discovery, and the complete
campaign/add-in registration paths. These are static observations, not validation against a
running original. Synthetic fixtures check the replacement's selected behavior only.
