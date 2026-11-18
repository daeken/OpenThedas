# Toolset lightmapper compatibility

The script-facing GFF and Aergia bindings are in thedas-bake's toolset module.
The embedded interpreter loads the caller's Python source. The stock script uses
the GFF binding to open and decode the original MMH and MSH files, then passes
its position, UV, index, transform, light, and film data to Aergia.
The executable installs the native combine, atlas, and DXT5 stages as the subprocess
targets used by the Python driver. It accepts the script filename followed by the
arguments that EclipseRay would give that script.

Registered subprocess handlers run synchronously. Their `Popen` objects
expose the exit status through `wait()`, `poll()`, and `communicate()`.
`returncode` starts as `None`. The first status observation sets it to the
exit code. The Windows Python 2.5.4 probe checked repeated waits, polling
after completion, and retrieval of piped output after waiting. An authored
driver then used `wait()` to compress the three captured uncompressed
Spoiled Princess atlas pages through both hosts. The Windows original
reproduced all three captured compressed DDS files by SHA-256. The
macOS ARM64 run completed and reproduced its own earlier compression
output byte-for-byte. The private probes are
`probe-original-python-process.ps1` and
`probe-original-editor-compressor-wait.ps1`.

Run from the lightmapper working directory if the script uses relative paths.

~~~sh
cargo run -p thedas-bake --bin thedas-eclipseray -- /path/to/LightMapper.py --terse --cpus=1 --inputDir=/path/to/job
~~~

## Reference observations

The reference was EclipseRay 1.2 from the extracted Toolset installer, with
LightMapper.py 1.0.3, x86 Python 2.5.4, and pywin32 build 214. It ran under
Wine 11.21 and Rosetta on macOS ARM64. The Rust candidate ran natively on
the same Mac. Private capture data and replay scripts are under
DAODecomp's ignored local/research/toolset/reference directory.

The stock script produced identical position, index, UV, normal, bounds,
and transform arrays for the original Oth terrain model used in the probe.
The comparison used little-endian i32 indices and binary64 values for
Python floats on both sides. The GFF reader exposes the binary vertex
and index fields as Python buffers and removes the on-disk string
terminator at the COM boundary.

An authored 32×32 job used that model with an ambient and a directional light.
The original and candidate drivers each produced three top-origin 24-bit
TGAs. Header and footer bytes agreed. Per-pixel results follow.

| Pass | Differing pixels out of 1,024 | Maximum channel error |
| --- | ---: | ---: |
| Lightmap | 0 | 0 |
| Shadow map | 0 | 0 |
| Ambient occlusion | 1 | 1 |

EclipseRay's camera divides the UV chart into vertical strips and chooses
the first boundary above a sample. That boundary may reject the sample or
select a triangle whose projection puts the ray outside the mesh. A
camera ray that misses geometry contributes the scene background, blue
with linear RGB (0, 0, 0.8) by default. The pinned executable (SHA-256
6040dc050f5e81f93fb3b9e04a4265a521660f97b301610c3df528577e91b197)
builds and queries the strip table at RVAs 0xdd10, 0xc760 and 0xc8d0.
The native camera matched ten controlled TGA captures exactly, covering
touching, overlapping, duplicate, and out-of-range UVs, the default and
script-defined backgrounds, and a monochrome pass with film gamma 1.5.
The edge builder stores the horizontal span and the third vertex's
signed distance in binary32 at RVAs 0xa0cd and 0xa138. The camera query
stores the evaluated edge height in binary32 at RVA 0xc9bf.

The strip-edge comparator at RVA 0xa200 can regard two nearby lower
boundaries as equivalent, producing a nontransitive relation. The original
insertion sort at RVA 0xb0b0 first compares the new item to the first
boundary. The partition at RVA 0xb6c0 compares with the moving start of
the equivalent range. On macOS ARM64, Unicorn 2.1.4 executed the pinned
x86 sort and comparator on 128 authored edge sequences of 4–500
elements. The previous Rust order agreed on 58 sequences. The revised
routine agreed on all 128. The ten original camera films and the
Spoiled Princess replay produced identical bytes before and after these
rounding and sorting changes.

Before reproducing the camera projection, background and intersection
arithmetic, the terrain films differed at 29, 12 and 30 pixels. The last
two remaining lightmap differences disappeared when the native trace
matched the float-store points and wide barycentric boundary comparison
from yafraycore.dll (SHA-256
518465f2089b5106f63ddad04a99d6425bd37760a3f0cadbc82a18f8c6ef7f71,
RVA 0x13ce0). The remaining AO pixel is at (4, 28), with original 247
and candidate 248 in each channel.

Without the volume transmittance correction, all 1,024 lightmap pixels
differed, with a maximum channel error of 44. EclipseRay installs the
EmissionIntegrator for the scene. Its shipped implementation initializes each
optical-depth channel to 1 and evaluates exp(-depth) even for an empty volume
list (EmissionIntegrator.dll SHA-256
5c63c76bafed0efe2f496bb95666240443bada4551d9f5849231468cc3caf130,
RVA 0x1440). Directlight.dll applies this transmittance to direct light after
material and cosine contributions (RVA 0x4150). Ambient light has a separate
path. A flat directional fixture with unit red radiance produced a red byte
of 93 at gamma 1, with either 0 or 1.3 supplied for material IOR.

## Postprocessing

Wine 11.21 ran the original BakedMapProcessor. The native combiner's entire
DDS matched for four controlled inputs covering red-channel shadow/AO sampling,
unequal input sizes, near-constant 4×4 reduction, and the reduction threshold.
The original rounds source coordinates and indexes the image as a flat array.
A mismatched-dimension probe could read past the source allocation and returned
unrelated bytes. The native combiner reports that case as an error.
In the Wine captures, BakedMapProcessor also displaced the closing angle
bracket of each metadata Value tag into its CDATA opener. The local comparison
corrects that specific serialization error when reading the reference.

CreateAtlas failed under Wine. The original PE32 CreateAtlas and
TextureProcessor then ran under Windows 11 ARM64 build 26300 in Parallels,
using Windows x86 emulation. Given identical input DDS files, the native
atlas step matched five placement XML files and thirteen uncompressed DDS
pages byte for byte. The cases were the terrain probe, a ten-texture packing
probe, two equal-width cases with 32 and 40 textures, and an 80-texture case
with three widths and three heights. The larger fixtures exercise the
width-only introsort at RVA 0x6cf0 of CreateAtlas.exe (SHA-256
b20b67fa5c3184ad95ac0c232a3d9f1276fa570d8b2f35b9168379c4af3cc8a5).
The 32-texture fixture also exercises legacy midpoint rounding when writing
half-texel placement offsets at six decimal places. The native texture
metadata matched the reference source dimensions, bit depth, category, texture
type, and producer. It omits the original host and export-time fields.

TextureProcessor.exe (SHA-256
969c01ee0bf667993cf80573a0d2e7e9ebc0fbf10bec27ba7db6b9244d5d998a)
routes the stock `-dxt5 -nomipmap` call to its legacy NVIDIA DDS encoder
through RVA 0x8570. The native alpha encoder reproduces the ramp fitting
and packing observed at RVAs 0x74d40, 0x96f60 and 0x95b70. The RGB encoder
uses the fast principal-axis fit at RVA 0xa48a0 and the cluster fit at
RVA 0x94a90. The cluster quantizer at RVA 0x79ad0 scores rounded
interpolants, then RVA 0x944d0 assigns indices with truncated interpolants
and scores the two fits for the final selection at RVA 0x9c340.

An instrumented copy of the pinned executable under Wine reported x87
control word 0x027f during the endpoint search. A binary64 replay of the
captured inputs reproduced all 2,048 scores across the 32 packing blocks
using the observed channel-addition and sample-accumulation order.
The modified executable produced the same packing DDS files as the
unmodified reference run under Windows.

The unmodified compressor's final DDS headers and all 3,424 BC3 blocks
matched byte for byte on aligned inputs. The comparison includes 96 blocks
from the terrain and packing atlas jobs, 1,536 controlled blocks with wide
and narrow ranges, ramps, sparse values, and explicit alpha endpoints,
768 blocks with gray values, two distinct colors, and clustered colors,
and 1,024 blocks with random colors and colors near the gamut limits.

For direct compression, RVA 0x8f70 rounds each source dimension up to a
multiple of four with a minimum of four. A capture at RVA 0x72c40
matched nearest-neighbor sampling at destination texel centers on 16
controlled inputs from 1×1 to 35×34, including 61-pixel strips on each
axis. Those 16 files and an earlier 7×9 probe produced 196 BC3 blocks.
All 17 native DDS files matched the unmodified original byte for byte.

The unchanged stock driver completed the authored terrain job with the
native executable on macOS ARM64 and generated the final DDS and metadata.
The final placement XML matched the reference combining Wine rendering and
combination with the Windows atlas and compression tools. The combined
uncompressed DDS differed by one level in three channels of the remaining
AO pixel. The final compressed DDS matched the original byte for byte.

An earlier x64 `x86_64-pc-windows-gnullvm` build with static runtime linking
completed the same job under Windows 11 ARM64 x64 emulation. The current
Windows build was replayed under Wine 11.21 and Rosetta. All ten outputs
matched the macOS ARM64 build byte for byte, including all three films,
the final DDS, placement XML, and metadata.

Remaining comparison work includes the AO pixel, a complete editor bake and
import, and non-stock script and postprocessor options.

BioWare's earlier LightMapper.py, from the EclipseRay 1.1 download
(script SHA-256 5b6aa13e17f38cb59455659812166fb60d24514fd23ca8c4585189fe5089d779),
completed an authored ambient-only job in both the native runtime and pinned
EclipseRay 1.2 under Wine. The placement XML matched exactly. The lightmap,
shadow map, and ambient occlusion films differed at 0, 0, and 1 of the 1,024
pixels, with a peak difference of one level. Both hosts rejected the older
script's three-argument directional light call. Its companion EclipseRay 1.1
executable faulted during startup under Wine 11.21. This script is a separate
official release.

The editor probe used the original Toolset v1.0.1008.0 on Windows 11 ARM64
with x86 emulation. The portable SQL Server 2005 engine faulted before
opening the database. The original Toolset database backup
(SHA-256 66dbce4e029ecc094ff878c2390103aff31e8d5e45bb54f6a9ab201a9fda292d)
restored successfully to SQL Server 2022 in a local x64 Linux container.
The Toolset's original x86 database client connected after registering
the bundled Engine.dll and PluginManager.dll and pointing the editor at
the restored database. Python 2.5.4 with pywin32 also instantiated the
original Engine.dll GFF reader in the same guest.

With this setup, the editor displayed the original Spoiled Princess layout
lgt101d.lvl (SHA-256 d5ea184c367f2543892b45c30298d9292cc78ada2fa3e7f92dc3947c40ee0884)
from BioWare's Light_Content archive
(SHA-256 d170c4cce63bbc29d958071ccf6f8c2e43a9c94fa6c6aa3c525e2e32c03243cb).
Tools → Render → Render Lightmaps wrote a 241-byte job_scene.xml and
reported missing model metadata and zero render targets. The first
job_scene.xml has SHA-256 51451328b130a14b40c43fec17144ca25db1af7d1c017c9479cf165c614b4e41.
The installer-provided arttools.erf
(SHA-256 b2dab66bc6230ab57b583695989c4ebad5b55c159bbc16542c46e0e28bd1013d)
contains original metadata for affected models such as fhi_upperbeam_0 and
prp_cupwood_01_0. Earlier attempts to make it available reused the editor
process that started before the archive was staged. That process cached
its resource paths and produced the empty scene captured in
editor-bake-no-targets.zip
(SHA-256 8c0925fc1a4539f96bae1df54edbbdb849fb9845d00626228808b617bd0d6c93).
After closing it, a fresh editor process with arttools.erf under
packages/core/data_tools in the test toolset and registered game roots
exported a complete job without a loose metadata override. The editor ran
the pinned EclipseRay v1.2 with the stock driver and produced 190 lightmaps,
190 shadow maps, 190 AO maps, 190 combined textures, and three final BC3
atlases plus placement XML and texture metadata. [Original Toolset lighting workflow](https://www.datoolset.net/wiki/Lighting)

The private editor-live-original-bake.log records the original command.
The lgt101d archives preserve five job XML files, 58 MMH/MSH pairs
exported by the editor, and the stage outputs.

| Archive | SHA-256 |
| --- | --- |
| editor-bake-spoiled-princess-inputs.zip | 046b58173a40383f5973454623654d1d4889ff53a37a2bdf5d34bc63c7b5eb48 |
| editor-bake-spoiled-princess-assets.zip | 08d5af1d7ba358371c4a923e5f0f2a2e763b940b5fe40743f4ff041b6b707863 |
| editor-bake-spoiled-princess-original-output.zip | a9421a82549f3161d18675181368dcf531900552d18c2c5dd38fe9952ff84bf9 |

The stock driver replay on macOS ARM64 used the original model bytes and
atlas job XML. It remapped file paths in the other four XML files. Two
MMH mesh references used different filename case from the editor's
exported MSH names. The native GFF binding and Python file checks now
resolve a unique case-insensitive ASCII match when the supplied path is
absent. That completed the replay and produced all 570 expected films.
Using the stock driver, the original GFF bridge under Wine 11.21 and the
native GFF bridge returned identical part names, positions, indices, UVs,
normals, bounding spheres, and part transforms for all 74 parts from the
export.

The lgt101d run exposed two postprocessor differences with identical inputs.
In BakedMapProcessor.exe (SHA-256
6fb7a13c78621e06b586711d293f7c36b5c440958c81100375a6c419747a5c76),
the x87 divide, multiply and truncation at RVAs 0x8623–0x863c produced
1,853 channels one level below exact integer division. Dividing in
binary64 before multiplying matches all 190 combined DDS files. At the
left boundary of its sort partition, CreateAtlas.exe RVAs 0x608f–0x6091
skips a swap when the two right-side cursors coincide. Matching that
condition reproduces the original three uncompressed pages and placement
XML byte for byte from the original films. The six stable texture metadata
fields match for every combined map and atlas.

The earlier isolated compressor replay differed in nine of 147,456 BC3
blocks across the three original uncompressed atlas pages. The sole
alpha difference was at block 12,372 of `lgt101d_Room002.dds`
at pixel (336, 384). For the seven nonzero alpha values, moving 113
from fit bin 3 to bin 2 gives the same exact squared residual using the
normalized binary32 inputs. A private Unicorn 2.1.4 trace of the pinned
`TextureProcessor.exe` (SHA-256
969c01ee0bf667993cf80573a0d2e7e9ebc0fbf10bec27ba7db6b9244d5d998a)
recorded the move in the six-level fit at RVA 0x95b70. That fit quantized
to endpoints 102 and 125, as captured from the original Windows bake.
The earlier native fit stopped at endpoints 101 and 124.
`probe-original-alpha-fit.py` reproduces the trace and exact-residual
comparison.

The native encoder now updates the centered bin moments for each adjacent
move and compares the scaled residuals in the order observed at original
RVA 0x96769. Recompressing the same three pages on macOS ARM64 matches
all 147,456 alpha blocks. The earlier replay after that change still had
eight RGB differences. On a separate
16,384-block authored page compressed by the pinned Windows tool, the
change removed all 723 alpha differences and the entire DDS matched byte
for byte. The fixture contains reflected alpha groups, narrow and wide
ramps, and samples at 0 and 255. The private generator and Windows
replay are `generate-alpha-heldout.py` and
`run-alpha-heldout-windows.py`. The reference DDS has SHA-256
be00d64e2711dae4e57e611b9c60fcd5a5749960856bf1e6aebc236f21fa02c5.

An eight-block fixture copied those RGB mismatches from the original
uncompressed atlas pages. The pinned compressor under Wine reproduced
their final BC3 bytes. Instrumented copies traced the original's fast
and ordered cluster candidates. The ordered cluster candidate matched
Rust on all eight blocks. The original chose the fast candidate on six
and its cluster candidate on two. In each case the prior Rust fast fit
produced different endpoint codes and changed the final BC3 block.

The fast fit uses the selected covariance diagonal to scale its axis at
RVA 0xa4c08. It stores each partial projection as binary32, scales by
the reciprocal of the squared axis length, and adds 128 before the
final store at RVA 0xa4cf9. The subtraction at RVAs 0xa4d27 and 0xa4d34
removes the bias from the extrema. It stores the unweighted endpoint
components before scaling them to RGB565. Applying these operations
made all three original atlas pages compress byte-for-byte on macOS
ARM64. The ignored `editor_job_combination_and_atlas_match_original`
test now checks those three final DDS files as well as the combined
maps, uncompressed atlas pages and placement XML.

A separate authored RGB page still differs in one of 4,096 blocks.
The original chose a cluster candidate with different continuous
endpoints for that block. The fast candidate bytes match. The private
`rgb-precision-heldout-differential.log` and
`rgb-heldout-native-stages.log` record the comparison.

The original fhi_pillarstone_0 mesh uses a triangle with three collinear
UVs in its MshMain_FHI_pillarstone_0 part. EclipseRay divides by its zero
UV determinant at RVA 0xa710. The ray builder at RVA 0xd040 still sets the
sample weight when the strip selects the triangle, so the film receives
the background when that ray misses. The replacement now preserves that
sample and rejects its nonfinite origin before tracing the BVH. A separate
authored camera chart with three collinear UVs matched the original 16×16
TGA byte for byte under Wine 11.21 and Rosetta (SHA-256
097b62ea1dfd7dec6e3b85eb11f6f52f8044eef905ac7c5f5e2ab9bb39f56334).

The full native replay produced byte-identical placement XML. Matching
EclipseRay's degenerate chart behavior eliminated the 131 shared locations
where the replacement rendered black in all three passes and the editor
capture had nonzero values. The remaining film differences are measured
against the original six-core editor bake.

| Pass | Differing pixels before | Differing pixels after | Pixels per pass | Peak channel difference |
| --- | ---: | ---: | ---: | ---: |
| Lightmap | 3,543 | 3,251 | 2,316,352 | 219 |
| Shadow map | 3,581 | 3,311 | 2,316,352 | 255 |
| Ambient occlusion | 3,776 | 3,472 | 2,316,352 | 255 |

Across all three output pages, 4,217 of 2,359,296 uncompressed atlas
pixels and 2,101 of 147,456 BC3 blocks still differed.

On the same Windows 11 ARM64 guest, two six-core EclipseRay runs of
eight selected lgt101d targets disagreed at 806 lightmap, 1,076 shadow,
and 829 AO pixels out of 266,240 per pass. The replays used the same
exported scene and stock render functions, with the target list reduced
and output paths remapped. The camera query writes ray coordinates to
process globals at RVAs 0xc931 and 0xc93e and reloads them before
scanning the selected strip at RVAs 0xc9aa and 0xc9b0. This unsynchronized
access provides a mechanism for intermittent coverage changes. The
repeat runs establish that the six-core reference varies.

Against a one-core original run of the same eight targets, the native
macOS ARM64 replay differed at 18 lightmap, 16 shadow, and five AO
pixels. The earlier native build differed at 263, 248, and 263 pixels,
respectively. A one-core replay of the pinned executable under Wine
11.21 and Rosetta reproduced all 24 original Windows films byte for
byte. The private editor-camera-race-windows-original.zip
contains the three Windows reference runs (SHA-256
2441343aed581c75d5fa905a8e98477855643ed05b34c0d7f6b9492f93bd97e6).
A full Windows one-core reference bake completed from the same export
using the unchanged stock driver and all 190 targets.

The previous Rust build produced the same 24 films byte for byte on
Windows x64 emulation and macOS ARM64 for the eight-target replay.
The Rust release build used for the Windows comparison (x64, SHA-256
8a299daa1f9f14e3887273e06cc76764c1545b08827136a92c5b9405b21f2946)
completed the full stock-script replay on Windows 11 ARM64 with its
embedded Python runtime and six worker threads. The original x86 baker
ran in the same guest with one worker, using `--cpus=1` to avoid the
observed race. The original DLLs and tools stayed in the original run.
Both runs used the same editor export, remapping the file paths to
separate output directories.

| Stage | Units compared | Different | Peak channel error |
| --- | ---: | ---: | ---: |
| Lightmap pixels | 2,316,352 | 580 | 55 |
| Shadow-map pixels | 2,316,352 | 343 | 255 |
| Ambient-occlusion pixels | 2,316,352 | 525 | 181 |
| Combined pixels | 2,316,352 | 738 | 181 |
| Uncompressed atlas pixels | 2,359,296 | 738 | 181 |
| Final BC3 blocks | 147,456 | 301 | |

Atlas placement XML was byte-identical. The metadata schema version and
six stable fields matched in all 193 records. Export timestamp and user
fields vary. Compared with a macOS ARM64 replay of the same Rust source,
the Windows replay differed by one AO pixel and one level, which
propagated to one final BC3 block.

| Private Windows replay archive | SHA-256 |
| --- | --- |
| editor-full-editor-original-single-core.zip | ee05976a385fc554be02bf9b4d74f7811e6c7348b422f6339b136a86c876241c |
| editor-full-editor-native-release.zip | 332d939f91afa569e1577db1ba5addc5deb58985149ba6dcb55358504a53dc3a |

EclipseRay RVA 0x4680 receives a row-major binary32 matrix and a
binary32 point, accumulates each affine row in x87 registers and
stores each world coordinate once. Aergia uses it for the script's
vector transform and for vertices submitted to the ray tracer. The
Rust binding now accumulates in binary64 before storing the result as
binary32. A private instruction probe executed the pinned helper in
Unicorn 2.1.4 on macOS ARM64. The binary64 calculation matched all 512
authored transforms in that probe. Binary32 accumulation differed on
441. The camera projection also retains the binary32 delta stores
observed at RVAs 0xa710–0xa769 and 0xca86–0xca98. Those stores alone
produced the same films on the full macOS replay.

With the transform change, the full lgt101d replays on Windows 11 ARM64
(Rust x64 release, six workers) and macOS ARM64 differed from the
one-core original Windows replay as follows.

| Stage | Units compared | Different on Windows | Different on macOS |
| --- | ---: | ---: | ---: |
| Lightmap pixels | 2,316,352 | 477 | 477 |
| Shadow-map pixels | 2,316,352 | 281 | 281 |
| Ambient-occlusion pixels | 2,316,352 | 418 | 419 |
| Uncompressed atlas pixels | 2,359,296 | 596 | 597 |
| Final BC3 blocks | 147,456 | 247 | 248 |

Across the two Rust runs, a single AO value again differed by one
level, affecting one final BC3 block. Placement XML and the stable
metadata still matched the original. Tall, narrow groups of differing
samples still occur on the `bedroom07` cupboard part and several
instances of `TableLow`. The private
`editor-full-windows-transform-differential.log` records the per-stage
counts and peak errors. The Windows binary has SHA-256
746dbd377291e39c5c0c17c4c6950621027df664774ae0ec8d3f016af7988860.
The Windows capture is `editor-full-editor-native-transform.zip`
(SHA-256
b4977d9562156ab527f06546a49e1e32eb94667050b5ca4747cafe83305320c3).

Full stock-script replays with the alpha fit change now differ from the
one-core original at 246 BC3 blocks on Windows and 247 on macOS. The
Windows 11 ARM64 run used the x64 Rust release with six workers and
produced all 570 films. The macOS ARM64 release used two workers. Each
host's films and uncompressed atlas pages matched its previous replay.
Placement XML was byte-identical to the original and the stable
metadata fields and schema matched for all 193 textures on both hosts.
The private `editor-full-windows-alpha-boundary-differential.log` and
`editor-alpha-boundary-full-differential.log` contain the stage
comparisons. The Windows binary has SHA-256
792c6796a9845e187b41b78d687c056a4ebbe0b1e3e814d129640c88005faef2.
The Windows capture is `editor-full-editor-native-alpha-boundary.zip`
(SHA-256
f387c684d48ae8e2a7eb82b779e47c931ab943f7d64312c9db923d80217a6f63).

Full stock-script replays with the RGB fit change differ in 238 BC3
blocks on Windows 11 ARM64 with x64 emulation and 239 on macOS ARM64.
Both hosts reproduced all their earlier films and uncompressed atlas
bytes. The original placement XML and the stable metadata fields and
schema for all 193 textures still match. The private
`editor-full-windows-rgb-precision-differential.log` and
`editor-rgb-precision-full-differential.log` contain the stage counts.
The Windows binary has SHA-256
40d541a4c6322b78d5ff5e8afe17f1ec2fc64853f0dd2920641e3f1631eeb0b7.
The Windows capture is `editor-full-editor-native-rgb-precision.zip`
(SHA-256
caa75414490879f8170d0218b9902169e40d0c8e5622d81dc88faec16ecda64b).

The Toolset camera and surface shading now use the geometric normal
arithmetic from yafraycore.dll RVAs 0x14180, 0xeac0 and 0x1320. The
original stores each edge, cross product component and the squared
length in binary32. It evaluates the products and sums for the cross
product and squared length in registers before storing the results.
The square root and its reciprocal each round to binary32. The camera
copies the cached normal through the getter at RVA 0x12d70. Surface
shading reads it at RVA 0x30040.

The private `probe-original-furniture-normals.py` runs the pinned routine
in Unicorn 2.1.4 on macOS ARM64, supplying the imported square root
with x87 `fsqrt`. Its inputs are triangles from the original stock
driver's cupboard and table mesh dumps, transformed with the frozen
editor's four instance matrices and the original transform routine.
The revised calculation matches all 1,826 normals bit-for-bit. The
previous calculation differs on 453. The 53-bit and 64-bit x87
precision settings agree on these inputs. The private
`editor-furniture-normal-oracle.log` has the per-instance counts.

The macOS ARM64 stock-driver terrain comparison now matches all three
original 32×32 TGAs byte-for-byte, including the previously differing
AO pixel. All eleven authored camera captures also match byte-for-byte.

Full stock-script runs with both camera and shading normals changed
produced all 570 films. Against the one-core Windows original, the
Windows x64 release with six workers and the macOS ARM64 release with
two workers measured as follows.

| Stage | Units compared | Different on Windows | Different on macOS |
| --- | ---: | ---: | ---: |
| Lightmap pixels | 2,316,352 | 449 | 449 |
| Shadow-map pixels | 2,316,352 | 269 | 269 |
| Ambient-occlusion pixels | 2,316,352 | 403 | 404 |
| Uncompressed atlas pixels | 2,359,296 | 572 | 573 |
| Final BC3 blocks | 147,456 | 230 | 231 |

The camera-only change on macOS measured 448 lightmap and 402 AO
differences, with the same 231 BC3 block differences as the combined
change. The shading path uses the measured normal arithmetic even
though the combined run has those three additional differing pixels.
The most different cupboard and table films retain their main groups
of errors. Windows and macOS differ by one AO level at one pixel.
Placement XML matches the original exactly and all 193 texture
metadata records match after excluding time and user fields.

The private `editor-full-windows-surface-normal-differential.log`,
`editor-surface-normal-full-differential.log` and
`editor-surface-normal-metadata-validation.log` record the comparisons.
The Windows binary has SHA-256
05c91f4ffedab97cf22a35530397e6788cc7c9ad272ba6b23d75c7b21082cf7f.
The capture is `editor-full-editor-native-surface-normal.zip`
(SHA-256
67f90bdabc4b6860a81b24f0de3f84068538572f2dba4235da8aa50c0573be99).

EclipseRay stores the reciprocal of the UV determinant in binary32
at RVA 0xa795 before scaling the four inverse matrix coefficients.
The previous implementation retained binary64 for this intermediate.
The private `furniture-trace/probe-inverse.py` runs the original builder
at RVA 0xa710 with the stock driver UVs for the 1,511 invertible faces
of the cupboard and table parts. On macOS ARM64 with Unicorn 2.1.4,
the previous coefficients differed on 853 faces. The revised
coefficients match for every face at both tested x87 precision settings.

The private `furniture-trace/camera-capture.s` logs the selected flag,
origin and direction after the pinned camera routine at RVA 0xd040.
Single-worker Wine runs of the instrumented executable using the
original assets emitted lightmap TGAs identical to the frozen Windows
reference for `3h_0_1zi` and `4e_0_ck`. Excluding differential rays,
the captures contain 131,072 and 32,768 primary samples respectively.
The previous macOS Rust build agreed on coverage and direction but
differed in 4,649 and 8,432 ray origins. With the reciprocal rounding
the coverage flag and all six ray components match across both targets.
The cupboard lightmap improved from 57 differing pixels with a peak
channel error of 51 to 30 pixels with a peak error of 8. The table
lightmap improved from 47 to 34 differing pixels with peak error 6.

Full stock-script runs processed all 570 films. The Windows 11 ARM64
replay used the x64 Rust release with six workers and the macOS ARM64
replay used two workers.

| Stage | Units compared | Different on Windows | Different on macOS |
| --- | ---: | ---: | ---: |
| Lightmap pixels | 2,316,352 | 158 | 158 |
| Shadow-map pixels | 2,316,352 | 149 | 149 |
| Ambient-occlusion pixels | 2,316,352 | 133 | 134 |
| Uncompressed atlas pixels | 2,359,296 | 201 | 202 |
| Final BC3 blocks | 147,456 | 80 | 81 |

The Windows binary has SHA-256
2dc9a7563936a346ec4bd05aca9870cc77c48229108cd74c0b8dfb4b0d814582.
The capture is `editor-full-editor-native-inverse-rounding.zip`
(SHA-256
1f2646fdc471eb5cba4d7c991114f008014a34cfeade2362d5600a98e5788c53).
The private `editor-full-windows-inverse-rounding-differential.log`
and `editor-inverse-rounding-full-differential.log` record the comparisons.

The ray-triangle routine passes the distance dot product through a
binary32 store in yafraycore.dll at RVA 0x35ac, before multiplying by
the reciprocal determinant at RVA 0x13ebc. The private
`furniture-trace/probe-intersection-distance.py` ran the pinned routine
in Unicorn 2.1.4 on 1,024 authored rays. The prior arithmetic differed
on 186. The revised arithmetic matches all hit flags, distances and
barycentrics for the corpus.

The private `furniture-trace/scene-capture.s` records the original
scene intersection result in the same two single-worker Wine jobs.
Both instrumented TGAs again match the frozen Windows reference. The
replacement agrees on which rays hit geometry. The cupboard has
49,284 rays and the table has 23,896. After adding the rounding step,
differing hit distances drop from 12,282 to 22 for the cupboard
and from 2,991 to 39 for the table. The five cupboard distance differences
within four ULPs round to the same surface point. The other 56 rays
land on a farther surface in the original. For every divergent ray,
the isolated original triangle routine reports the same distance as
Rust when given the closer triangle and that ray. This isolates the
remaining discrepancy to scene intersection behavior on these rays.
The private `furniture-trace/furniture-nearer-triangle-oracle.log` and
`furniture-trace/furniture-hit-point-differential.log` record the comparison.

An instrumented copy of the table replay also captured the original
scene's 402,739 k-d tree nodes and all 48,664 triangles. The rendered
TGA again matches the frozen original. The closer triangle is present
in the captured tree for every one of the table's 39 divergent rays.
A geometric traversal with closed intervals finds at least one leaf
containing that triangle before or at the original hit distance in
all 39 cases. The private
`furniture-trace/inspect-table-tree.py` and
`furniture-trace/original-table-tree-differential.log` record the check.

The private `furniture-trace/replay-table-tree.py` reconstructs this
captured tree in guest memory and runs the pinned yafraycore.dll
traversal at RVA 0x19160 in Unicorn 2.1.4. Each of the 39 queries
reproduces the original scene hit distance at both tested x87
precision settings. The triangle intersection routine is never
called for the nearer triangle. Tracing RVA 0x193c0 shows that the
rounded entry point equals the split plane and the exit point lies on
the left for the nearer triangle's first skipped branch. The original
descends into the left child alone. The nearer triangle is in the
right child, which the geometric ray intersects over a short interval.
The private `furniture-trace/trace-table-splits.py` records the
endpoint and split coordinates for those rays.

All 48,664 world-space triangles in the captured original tree match
the transformed triangles obtained from the stock script's mesh
dumps. For comparison, a private build of the archival YafaRay source
at commit `f754cdf037cebe8a4fc081123f74c6e1b9d19d89` used those asset
triangles and the Toolset's constructor parameters of automatic
depth, one triangle per leaf, 0.8 traversal cost and 0.33 empty bonus.
It reproduced the six root bounds exactly. The macOS ARM64 build made
402,305 nodes with the first split or leaf differences at depth nine.
The x86 build using x87 under Wine made 402,715 nodes with the first
differences at depth seven. The upstream scene intersection also
differs substantially on these rays. These builds compared the
source's algorithm with the actual Toolset DLL and remained outside
the baker. [Archival YafaRay k-d tree source](https://github.com/YafaRay/libYafaRay/blob/f754cdf037cebe8a4fc081123f74c6e1b9d19d89/src/yafraycore/kdtree.cc)

The Windows x64 and macOS ARM64 stock-script reruns completed all 570
films against the one-core Windows original.

| Stage | Units compared | Different on Windows | Different on macOS |
| --- | ---: | ---: | ---: |
| Lightmap pixels | 2,316,352 | 148 | 148 |
| Shadow-map pixels | 2,316,352 | 134 | 134 |
| Ambient-occlusion pixels | 2,316,352 | 127 | 128 |
| Uncompressed atlas pixels | 2,359,296 | 188 | 189 |
| Final BC3 blocks | 147,456 | 76 | 77 |

On both hosts, the placement XML matches byte-for-byte and all 193
metadata records match after excluding time and user fields. The
Windows binary has SHA-256
aae79d383516df7860902624500a5cfe30baef4f6abf0e55cd8d29b50b8cfef5.
The capture is `editor-full-editor-native-distance-rounding.zip`
(SHA-256
213c049b4507c3e95ec3b9376c54e71f3f628cdc506e66b765d7d359511e0d47).
The private `editor-full-windows-distance-rounding-differential.log`,
`editor-distance-rounding-full-differential.log` and
`editor-distance-rounding-metadata-validation.log` record the results.

The Toolset film path now evaluates gamma in binary64 and stores the
result in binary32. In the pinned yafraycore.dll, the call at RVA
0x29faf goes through the CRT's `_CIpow` and stores a float at RVA
0x29fb4. The x86 MSVCR90 DLL used in the Wine probes
(SHA-256 1dd407a6e84c1aeba2486d5c216fa79338427cbaefb0a6139fe72471c0dc3314)
stores both arguments as binary64 at RVAs 0x2cad2 and 0x2cad7 and
calls `pow` at RVA 0x2caf9.

For the AO film of `1r_0_38d`, the two Rust hosts produced the same
filter weights, accumulated color and weight, and normalized gamma
input at pixel (209, 174). The single-precision power result on macOS
was 0x3f5adadb. Windows produced 0x3f5adada. Both hosts round the
binary64 result to 0x3f5adada, which the private
`furniture-trace/probe-original-pow.py` also obtained from the x86 CRT
under both tested x87 precision settings. The resulting byte is 217
in the original and both Rust builds with this change.

The six-worker Windows x64 and two-worker macOS ARM64 replays produced
byte-identical files across all 570 TGAs, 190 combined DDS textures,
three uncompressed atlases and three final BC3 atlases. They match
the prior Windows run at each of those stages. Against the original,
both hosts differ at 148 lightmap, 134 shadow and 127 AO pixels,
188 uncompressed atlas pixels and 76 of 147,456 final BC3 blocks.
Placement XML still matches byte-for-byte and all 193 metadata records
match after excluding time and user fields.

The Windows binary has SHA-256
06d84a58b588f7dc164eab68bac934096486bb01a039ddb42587ca431132ae43.
The capture is `editor-full-editor-native-film-gamma.zip`
(SHA-256
aaf71be892254167f6e49b1d1813426226de41e7fff8c0f3f52925c93befcb60).
The private `editor-full-windows-film-gamma-differential.log`,
`editor-film-gamma-full-differential.log` and
`editor-film-gamma-metadata-validation.log` record the comparisons.

The Toolset path now builds and walks the original style of spatial
k-d tree. The existing BVH remains the accelerator used by the other
bake path. The pinned yafraycore.dll constructor at RVA 0x1b3e0
sets the root bounds and depth. The builder at RVA 0x1a210 uses
the ordered edge sweep at RVA 0x1ec90 and the bucket scorer at RVA
0x1c360. It clips triangles to expanded node bounds at RVA 0x2eb50.
The primary and shadow queries at RVAs 0x19160 and 0x19770 share
the traversal over rounded entry and exit coordinates. The leaf's
triangle order determines which hit wins an equal-distance comparison.

The instrumented single-worker table job produced the same TGA as
the frozen original. Its 48,664 ordered primitive inputs match the
transformed stock-script mesh arrays byte-for-byte. The Rust scorer
prototype matches all 580 captured edge sweeps and all 602 captured
bucket scores across the first eleven tree levels, including the
first split where the archived builder diverges. The captures and
scorer replay are in the private `furniture-trace` directory.

The Rust release passed the local integration test
`stock_toolset_tree_and_primary_hits_match` on macOS ARM64. It
reproduced all 402,739 nodes, including split axes, plane bits, child
indices and the primitive IDs in leaf order. The test also requires
the distance bits and chosen primitive to match the original for
all 73,180 captured primary hits on the cupboard and table targets.
These are scene-specific checks against captured original data.

The six-worker Windows x64 and ten-worker macOS ARM64 stock-script
replays produce byte-identical films and DDS outputs. Against the
one-core original, both hosts measure as follows.

| Stage | Units compared | Different |
| --- | ---: | ---: |
| Lightmap pixels | 2,316,352 | 16 |
| Shadow-map pixels | 2,316,352 | 8 |
| Ambient-occlusion pixels | 2,316,352 | 6 |
| Uncompressed atlas pixels | 2,359,296 | 23 |
| Final BC3 blocks | 147,456 | 11 |

The largest remaining lightmap channel difference is nine levels on
the single-shelf part `57_0_3c`. Eight of the 16 lightmap differences
are on `TableLow` target `4c_0_ck`. The private
`furniture-trace/legacy-tree-residual-pixels.log` records the coordinates
and RGB values. Placement XML matches byte-for-byte and all 193
texture metadata records match after excluding time and user fields.

The private Rust tree and traversal prototype reproduced all 55,008
captured primary hit distances and triangle IDs for the shelf and
all 23,896 for table target `4c_0_ck` using the original rays. These
single-worker Wine probes used the stock job with one target at a
time. The private
`furniture-trace/rust-kd-tree-shelf-hits.log` and
`furniture-trace/rust-kd-tree-table156-hits.log` record the counts.

The Windows release has SHA-256
f10480dd05464b6c3b391ce1a07dfa5704dace9d828d179c61ade3da4f019e55.
The Windows capture is `editor-full-editor-native-legacy-tree.zip`
(SHA-256
0255e80ddfc93aea6ae7490f1e95cc7b7e3fca3c8f842d90284d0358a23edb3e).
The private `editor-full-windows-legacy-tree-differential.log`,
`editor-legacy-tree-final-full-differential.log` and
`editor-legacy-tree-metadata-validation.log` record the comparisons.

### Mesh light culling and point-light rays

On macOS ARM64, the candidate matched the original's camera ray validity
and every primary ray component across 131,072 shelf and 32,768 table
samples. Feeding 497,320 shadow rays captured at yafraycore.dll RVA
0x22f94 into the candidate's spatial tree reproduced every occlusion
decision. The instrumented originals ran under Wine with one worker.
Their two TGAs matched the frozen original Windows captures.

Tracing the direct integrator at directlight.dll RVA 0x4e9c (SHA-256
8007992121d081f7cd022ed50aa93614455854d9a387d6da1cfbc8be62d812ce)
isolated bright samples on the shelf and table to lights excluded by
the original. EclipseRay writes the authored mesh sphere at RVAs
0x950d–0x9540, transforming the center and copying the radius. Its
camera render path at RVA 0x65a6 retains a finite light when its
distance from the center is strictly smaller than the sum of the
radii. That distance is stored as binary32 at RVA 0x65d0 before
the wide comparison at RVA 0x65f8. The Aergia binding now applies
the same test to point and spot lights for each render target.
After the correction both focused TGAs match the original byte
for byte.

The original point-light plugin (SHA-256
6522ea30f95e9579770cc9c4390b1e9ddbf748321f1712009c6036919684b606)
at RVA 0x1250 stores the squared displacement length and inverse
distance as binary32 before multiplying the direction components.
Replaying the captured surface points and the job's light positions
with those stores reproduced all 497,320 original ray distances and
directions. The stock-script baker uses the same arithmetic.

The macOS ARM64 replay and Windows x64 build under Wine produced
identical bytes across all 570 films, 190 combined DDS textures, and
three uncompressed and three compressed atlas pages. Both replays used
the same Mac and the stock script detected ten cores. The Windows
executable has SHA-256
9d0b7b0532d4b433005e852a3a559ea260bfe2d3f69a6c947d7562334fabf44f.
The Wine run used the LLVM-MinGW `libunwind.dll` beside the executable.
The results against the one-core original Windows capture follow.

| Stage | Units compared | Different | Maximum channel error |
| --- | ---: | ---: | ---: |
| Lightmap pixels | 2,316,352 | 2 | 1 |
| Shadow-map pixels | 2,316,352 | 8 | 1 |
| Ambient-occlusion pixels | 2,316,352 | 6 | 4 |
| Uncompressed atlas pixels | 2,359,296 | 14 | 4 |
| Final BC3 blocks | 147,456 | 6 | |

Placement XML remains byte-identical. All 193 texture metadata records
match after excluding the export timestamp and username. The private
`editor-point-rays-full-differential.log`,
`editor-point-rays-cross-platform.log` and
`furniture-trace/original-point-normalization-differential.log` record
these comparisons.

### Spotlights on original terrain

The pinned spotlight.dll (SHA-256
974458dede7b51ec69cfe97ab7bff342b09ebe35772c2d2466a7c747ce876c38)
normalizes the light axis at RVA 0x1570 through yafraycore.dll's
`vector3d_t::normalize` at RVA 0x1320. The spotlight constructor keeps
the degree-to-radian conversion wide through the cosine calls at
RVAs 0x168e–0x16ce. Its illumination routine at RVA 0x18a0 rounds
the squared length, inverse distance, cosine, cone interpolation
coordinate and cone factor at separate stores. The native Toolset
path mirrors those stores.

Two instrumented single-worker runs under Wine captured 82,809
accepted rays for a cone with a penumbra and 46,383 for an inclined
cone. The captured films matched the uninstrumented original.
Replaying the arithmetic independently reproduced the direction,
distance and RGB color bits for every captured ray. The old arithmetic
differed on 86,356 RGB results. A Rust regression test covers a
captured sample inside the penumbra of the first cone.

Five authored 128×128 jobs used the unchanged stock driver, the original
`oth000d_26_0` terrain asset from Oth, and a broad, soft-edged, hard-edged,
radius-limited or inclined spotlight. All 15 TGAs, 15 DDS files and five
placement XML files match byte-for-byte between original EclipseRay
under Wine and the native macOS ARM64 replacement. The ten texture
metadata records match after repairing the Wine serializer's displaced
bracket and excluding the export timestamp and username. The frozen
Spoiled Princess replay retained the differences in the table above.

The private `spotlight-probes/prepare.py` defines the authored inputs.
`spotlight-probes/point-samples-differential.log` and
`spotlight-probes/full-output-verification.log` record the comparisons.
The terrain MMH and MSH have SHA-256
be4f714bba4785c57a8d75e2491b321395f2acbc9a54c91a45961400cea165eb and
3b507726fc3742bd01abe2c18d392dd8d3ac1767bcf25b7484bdaba041b3613f.

### Film tile order

The original film initializer requests 32×32 tiles at yafraycore.dll
RVA 0x29de5. The tile builder at RVA 0x2c110 visits tiles left to
right, then top to bottom. Within a tile, the renderer visits
pixels in row order. The replacement now accumulates filter
contributions in that sequence for Toolset films.

Focused shadow replays on the floor `1e_0_38d` and bar
`3d_0_1n8` reproduced the frozen original Windows TGAs under
one-worker Wine. Tracing the original `addSample` at RVA 0x2a0d0
and final film `flush` at RVA 0x2b144 matched all 7,512 captured
sample coordinates and shadow values against the Rust trace. All
256 Gaussian table entries matched. The original tile order
changed the accumulated weight by one binary32 ULP at each of
the three investigated pixels. Normalization by the rounded
reciprocal of that weight accounted for the 254 versus 255 output.
The private `shadow-film/film-differential.log` records the
accumulators and their independent replay.

Against the one-core original Windows capture, the full macOS
ARM64 Spoiled Princess replay now matches all 190 shadow films
byte for byte. Two lightmap pixels and six ambient-occlusion
pixels still differ. Six of 2,359,296 uncompressed atlas pixels
and three of 147,456 final BC3 blocks differ. Placement XML is
byte-identical and the stable fields of the 193 texture metadata
records match. The private `editor-film-tiles-full-differential.log`
and `editor-film-tiles-metadata-validation.log` record the results.

### Ambient-occlusion ray rounding

The Toolset tangent frame at yafraycore.dll RVA 0x11280 stores
the reciprocal of the tangent length before scaling the two
components. The frame cross product at RVA 0xeac0 rounds each
component after the two products are subtracted. The cosine
sampler in shinydiffuse.dll (SHA-256
6aacf76742b98bc0288203c6c866a5adbf7217c9a9701b82e82b979bc565590d)
at RVA 0x1320 forms the azimuth with the binary64 value of pi,
stores the angle as binary32 and then calls the CRT sine and
cosine functions. It keeps the radial complement wide through
the square root. The native Toolset path reproduces those
individual stores.

Focused Wine runs on the wall `38_0_1k5`, chair `4m_0_3t` and
beam `1j_0_i` produced the frozen one-core original Windows
AO films. Their selected 900 shadow rays had matching shader
points. The old calculation matched the directions of 432
rays and changed three visibility results. Independently
replaying the corrected arithmetic matched the direction bits
of all 900 rays. The native replays then reproduced all three
complete AO films. The private `ao-film/rounded-direction-differential.log`
records the original ray decisions and the arithmetic replay.

The full macOS ARM64 replay now matches all 190 shadow and all
190 AO films byte-for-byte. Two RGB pixels differ by one channel
level in the 190 lightmap films. One channel level differs in the
uncompressed atlases. All three final DXT5 DDS atlases and the
placement XML are byte-identical. The 193 stable metadata records
match. The private `editor-ao-direction-full-differential.log` and
`editor-ao-direction-metadata-validation.log` record the comparison.

### Direct-light cosine and complete stock replay

The remaining two RGB pixels had matching sample coordinates and
accumulated Gaussian weights. The traced inputs to the film showed
a one-ULP difference in the accumulated blue channel for each texel.
The original direct light integrator at directlight.dll RVA 0x4441
stores the complete normal-dot-direction result as binary32 after
the x87 products and sums. The replacement now uses the same store
point. Both focused TGAs then matched their original counterparts.
The private `rgb-film/ceiling-differential.log` and
`rgb-film/floor-differential.log` record the earlier sample differences.

The full Spoiled Princess macOS ARM64 and Windows x64 replacement
runs match the one-worker original Windows bake byte-for-byte for
all 570 TGAs, 190 combined DDS textures, six uncompressed and
compressed atlas files, and the placement XML. All 193 texture
metadata records match after excluding the export timestamp and
username. The Windows replacement ran under Wine. See the private
`editor-direct-cosine-full-differential.log` and
`editor-direct-cosine-cross-platform.log`.

### Editor installation and preview

Place the Windows x64 `thedas-eclipseray.exe` and its accompanying
`libunwind.dll` in the Toolset's `Lightmapper` directory. Keep the
Lightmapper Command Working Folder set to that directory. Under
Tools → Options → Level Editor → LightMapper Command, use the stock
script with the replacement executable.

~~~text
$(TOOLSET)\Lightmapper\thedas-eclipseray.exe LightMapper.py --terse --cpus=4
~~~

After **Render Lightmaps** finishes, enable **Display Lightmaps** and
save the layout. The editor imports the bake output at that step and
writes the lightmap ERF next to the LVL. The Windows replacement has
an embedded interpreter and uses the installed BioWare resources.

For Nexus mod 6431, put the supplied script in the same directory as
`LightMapper-6431.py` and use the mod's half-resolution options.

~~~text
$(TOOLSET)\Lightmapper\thedas-eclipseray.exe LightMapper-6431.py --resMult=0.5 --progress --terse --cpus=4
~~~

Tools → Render → Render Lightmaps launched the earlier Windows Rust build
(SHA-256 8a299daa1f9f14e3887273e06cc76764c1545b08827136a92c5b9405b21f2946)
from the original editor. The unchanged stock script processed a new
lgt101d export and wrote 190 maps per pass, 190 combined DDS files,
and three final atlas pages with placement XML. The editor reported
script success and a 121-second submission. The configured command
retained the installer's `-cpus=2` spelling. The script detected six
cores, as its explicit CPU option is `--cpus=2`.
[Toolset lighting settings](https://www.datoolset.net/wiki/Lighting)

The live export's 116 MMH/MSH files matched the first editor export
by SHA-256. The five job XMLs contained the same scene, lights, target
and atlas entries after comparing their content independent of absolute
paths, element order and generated light GUIDs. The first and live
exports order entries differently. The fixed-input bake comparisons
above preserve the original order. The private
`editor-live-editor-native-release.zip` contains the new inputs and
outputs (SHA-256
5920af75bef0c40bd47d58e7e9032b8635a03c5bf31375dc73b5d7c578a50d15).

The editor's first save to a separate test layout reported an error
writing the lightmap ERF. A Windows file trace showed the editor looking
in `LvlWorkspace\lightmaps\lgt101d` when reopening the layout. The bake
had populated its configured `CombinedMaps` directory. These are
separate directories in the test setup. After staging all seven output
files in that lightmap cache, the editor copied them into the test
layout's resource-manager directory. SHA-256 matched the three original
Rust outputs, their metadata and the placement XML in
`editor-native-preview-validation.log`. The later Wine test below
verified automatic transfer and a new saved ERF.

For a separate archive test, BioWare's `ErfGenerator.exe` (SHA-256
09fcdba1f46e87dce0bc03304471c8023f6bd4c242037f02a124d4130904308d)
packed the three atlases, their three metadata files, and placement XML
into V2.0 sidecars next to copies of the untouched tutorial LVL. The
original one-core bake and current Windows Rust bake each round-tripped
all seven files by SHA-256. The editor opened both archives and emitted
`Unable to read the lightmap header gff` for each.

The pinned `EditorLvl.dll` (SHA-256
ac09d01b6c7ac9b7e8cec9cbe295cfa6eb8dc04bd8463a8541b85b020c90dbd1)
loads the archive member `lpak_manifest.gff` at RVA 0x178b0. Its
manifest writer at RVA 0xb0f0 declares an LMM V1.0 GFF with a list at
label 3331 and string and 64-bit timestamp fields at labels 3333 and
3332. Adding a manifest generated with `thedas-formats` removed the
header error from both control and replacement sidecars. The manifest
was supplied by the test harness. The baker still emits the files used
as archive payloads.

An isolated reopen temporarily moved aside the seven previously staged
test files. The Toolset opened each LVL and read the matching sidecar
directory and manifest. Windows File I/O events from the Toolset
process showed the manifest reads at offsets 2,382,768 and 2,381,784,
respectively. The trace recorded no requests for the placement XML or
dedicated reads for the atlas textures. The two resource-manager
directories also lacked the seven files. The test restored the earlier
cache after both opens. This test established manifest loading. The
later Wine test below verified atlas display from a saved sidecar.

In the private capture directory, `package-editor-lightmap-file-manifest.ps1`
records the manifest bytes and round-trips the sidecars. It copies the
payload files directly from the two frozen bake captures. The verification
script `verify-editor-lightmap-archive-trace.ps1` records the member
offsets and layout opens. The corresponding reads matched by both file
object and file key are in
`editor-lightmap-archive-direct-reads-by-handle.log`. The sidecars tested here
have SHA-256
934934244cef30be1723619f124211a2f63bd3322cc09f85c4ebb98c51805faa
and 3481c1fe9cecfdedd85aa832c9ef99a0cfc7f614b9fb595de7b5509415cd005b.

A second control used the original `EngGFFReader` to query the manifest
values. With the initial field order (3333, 3332), the reader found the
timestamp by index and reported it missing by label. Without explicit
UTF-16 terminators it sometimes included extra characters when returning
filenames. Ascending labels and terminated strings made both lookups
return all seven filenames and 64-bit timestamps correctly for each
bake. The five isolated variants and the results are in
`package-editor-lightmap-manifest-variants.ps1` and
`editor-lightmap-manifest-original-gff-variants.log`.

The editor reopened both corrected sidecars and successfully read each
534-byte manifest. The independent kernel trace
`editor-lightmap-archive-canonical-results.log` again recorded no
placement XML read or dedicated atlas reads. The open Rust-sidecar
document's Display Lightmaps state was zero. The read-only probe
`inspect-editor-lightmap-display-state-by-address.ps1` used the active
document's workspace directory suffix as the object address and verified
its vtable's mapping to `EditorLvl.dll`. The instructions at RVA
0x1368bf read that state before toggling the preview.
[Toolset level tutorial](https://www.datoolset.net/wiki/Level_tutorial)

The pinned ErfGenerator.exe under Wine packed and extracted fresh V2.0
sidecars for the byte-identical stock bake from each baker. Both sidecars
used the same validated LMM manifest from the earlier editor inspection.
Each round-trip recovered all eight supplied members byte-for-byte. The
three DDS files, placement XML and manifest matched across the two
archives. Their texture metadata matched after excluding the export
timestamp and username. The archive test used copies of the original
`lgt101d.lvl`. The private `editor-final-sidecars/verification.log`
records member sizes and SHA-256 hashes.

### Live bake, save and reopen under Wine

The original Toolset 1.0.1008 opened the tutorial layout interactively
under Wine 11.21 and Rosetta on macOS ARM64. The private profile used
the restored Toolset SQL database, the installer art archive, the
shipped DirectX x86 library, and NVIDIA's PhysX 2.8.1 runtime from the
game's redistributable. A private copy of Microsoft's MDAC network
library selected TLS 1.2 so its original SQLOLEDB provider could
connect. Wine's synchronous Direct3D command handling allowed the
editor to render the loaded models.

The editor launched the Windows x64 replacement
(SHA-256 c52e5d2581019ee6090ee0988bb0d1db0331b9a490445d09b62dce46f749561e)
from its lightmapper working directory with the stock script and
`--terse --cpus=4`. The fresh Spoiled Princess export contained 58 MMH
and 58 MSH files. The editor reported success after 48 seconds. The
replacement wrote 570 TGAs, 190 combined DDS files, and three atlas
pages before and after compression.

Enabling **Display Lightmaps** after the bake transferred all seven
files from `CombinedMaps` into the editor's lightmap cache and the
document's resource directory without a manual copy. Saving then
created a new LVL and ERF sidecar. BioWare's ErfGenerator extracted
the seven bake files byte-for-byte plus the 562-byte LMM manifest
written by the editor. The saved ERF has SHA-256
63917027a3c7a99bc1ffc6edf110d04e6dd83874434a74248ad792676871ff4b.
The Toolset also logged a wind-property serialization error during
the LVL save. The reopened layout and lightmap comparisons below
passed. The wind-property difference was outside this comparison.

After closing the document, the test moved the bake directory and
both editor caches aside. A fresh Wine editor process reopened the
saved LVL. Enabling Display Lightmaps changed 991,692 of 993,300
viewport pixels compared with the same scene with lightmaps disabled.
Every pixel in that region matched the pre-close lit view. The
private `editor-wine-live` directory contains the file trace, capture
scripts, saved layout, viewport captures,
`cold-reopen-viewport-verification.log` and
`live-bake-sidecar-verification.log`.

### Nexus Mods community driver

The supplied Nexus Mods archive for Dragon Age: Origins mod 6431,
file uploaded August 14, 2024, includes a LightMapper.py dated
February 11, 2010. The unmodified script has SHA-256
a49a69398b3182eee2ed875772ebe9fbc34c55ec6c703cdded25923776581712.
It adds resolution scaling and counts render targets for its progress
messages. Its progress timer uses `datetime.datetime.now()`, subtracts
local timestamps, and compares the elapsed time with a five-second
`datetime.timedelta`. The interpreter now supports those operations and
Python 2.5's timedelta argument normalization.
[Community script release](https://www.nexusmods.com/dragonage/mods/6431)

On the original Oth terrain job, EclipseRay 1.2 under Wine and the
native macOS ARM64 build each ran the unmodified script with
`--progress --cpus=1`. At `--resMult=0.5`, all three 32×32 targets
produced 16×16 TGAs. At `--resMult=0.0625`, all three produced 4×4
TGAs, exercising the script's minimum. For each scale, the original
and replacement produced byte-identical TGAs, combined and atlased
DDS files, compressed DDS and placement XML. The stable texture
metadata fields match after repairing the Wine serializer's displaced
bracket. Both runs reported all three targets complete.

A reduced Spoiled Princess replay used the original exported scene,
lights and mesh inputs. Its three selected table, shelf and floor targets
ran through every pass at half resolution. The original and native
outputs matched byte for byte across nine TGAs, three combined DDS
textures, three uncompressed atlas pages, three final compressed DDS
pages and the placement XML. All six stable texture metadata records
matched after the Wine serialization repair and excluding time and user
fields. Both runs reported nine targets complete.

The full Spoiled Princess export completed on macOS ARM64 and on the
Windows x64 replacement under Wine, using the same unmodified script
at `--resMult=0.5 --progress --cpus=1`. Their 570 TGAs, 190 combined
DDS textures, six uncompressed and compressed atlas DDS files, 193
texture metadata records, and placement XML were byte-identical. The
Windows binary has SHA-256
c52e5d2581019ee6090ee0988bb0d1db0331b9a490445d09b62dce46f749561e.

Original EclipseRay under Wine ran the same unmodified script to
completion of all 570 films and 190 combined textures. Its CreateAtlas
subprocess failed on the first invocation. Replaying the two remaining
original executables with the same combined textures, atlas XML and
options through a short Wine drive path completed both stages. Moving
the placement XML out of the compression input and copying the metadata
followed the script's file operations.

Across the full original and replacement runs, all 570 films, 190
combined textures, three uncompressed atlas pages, three final DXT5
DDS files and placement XML were byte-identical. All 193 texture
metadata records match after the Wine serialization repair and excluding
time and user fields.

The mod README's `--cpus=4` setting also completed the full half-resolution
bake. On native macOS ARM64, every output matched the replacement's
one-worker run byte-for-byte. Original EclipseRay under Wine completed
the four-worker run, including atlas and compression, using a short
drive path from the start. Compared with the four-worker replacement,
it differed at 2,325 lightmap, 2,243 shadow and 2,409 AO pixels out of
579,088 per pass. Seventy-nine combined textures and all three atlas
pages differed before and after compression. Placement XML and all 193
stable metadata records matched. The six-worker repeat tests above
establish that original EclipseRay's coverage can vary between runs.
Use `--cpus=1` when capturing a reference for byte comparisons.

The unmodified Nexus script also ran in a fresh editor bake under
Wine with the README's `--resMult=0.5 --progress --terse --cpus=4`.
The editor reported 570 of 570 targets complete. After enabling
Display Lightmaps and saving, BioWare's ErfGenerator recovered all
seven output files byte-for-byte from the new ERF, plus the manifest.
The Nexus ERF has SHA-256
ad3d76fdac62c709be6c3763cdbd3523006d377ccfaeccec41e2ddd1bcef2131.
A new editor process reopened the saved layout with the bake output
and editor caches set aside. The displayed lightmaps reproduced
all 993,300 measured viewport pixels. The private
`editor-wine-live/live-nexus-sidecar-verification.log` and
`editor-wine-live/cold-nexus-viewport-verification.log` record
those checks.

The private fixture and runner scripts are under
`community-6431` in the reference directory. The reports are
`small-differential.log`, `reduced-differential.log`,
`full-final-cross-differential.log`,
`full-original-render-combine-differential.log`,
`full-original-atlas-differential.log`,
`full-four-workers-differential.log`, and
`full-four-workers-original-differential.log`. The trace includes
separate output trees for the original and both replacement builds.

## Local checks

The ignored integration test in tests/toolset_mesh.rs reads the installed
driver through THEDAS_LIGHTMAPPER_SCRIPT, an original model through
THEDAS_GFF_REFERENCE_MODEL, and the original array dumps through
THEDAS_GFF_REFERENCE_CASE.

The ignored test in tests/toolset_films.rs uses THEDAS_BAKE_NATIVE_JOB and
THEDAS_BAKE_REFERENCE_JOB for the two job directories. It stops the candidate
when the driver requests BakedMapProcessor.exe, then checks film structure
and reports numeric differences. Set THEDAS_BAKE_EXACT_FILMS to require
identical pixels. These checks exercise one fixture.
Set THEDAS_BAKE_CAMERA_REFERENCE to the camera-probes directory to run the
other ignored test in that file. It executes the same authored Python
fixture that produced the eleven original camera TGAs and requires exact bytes.

Set THEDAS_BAKE_TOOLSET_TREE_REFERENCE to the private furniture-trace
directory to check the rebuilt spatial tree and selected primary hits.
This fixture expects `original-builder-ordered.bin`, the normalized
`original-builder-tree-normalized.bin`, and the cupboard and table
`original-*-ordered-hits.bin` files. Tree normalization changes guest
triangle addresses to the zero-based indices of the captured ordered
input and preserves split coordinates and leaf order. The private
`furniture-trace/normalize-original-builder-tree.py` produces that
file from the captured tree. Its SHA-256 is
949767c01c31bc963980710055cd9f32e9c9f65a35e5888da66ac208d41437e1.

The ignored tests in tests/toolset_postprocess.rs read
THEDAS_BAKE_POSTPROCESS_REFERENCE, the directory containing combine-probes,
windows-atlas-probes, windows-atlas-order, and compression-probes. They
compare isolated stages with identical inputs, require byte-identical BC3 DDS
files, and compare texture metadata fields. THEDAS_BAKE_FULL_RESULT can
additionally point to a native CombinedMaps directory for the terrain job.
Set THEDAS_BAKE_EDITOR_REFERENCE to the extracted lgt101d original output
directory and THEDAS_BAKE_EDITOR_JOB to its job XML and TGA subdirectory
to run the original editor's complete combination and atlas fixture.
