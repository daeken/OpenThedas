# SpeedTree `.spt` files in Dragon Age: Origins

## Where they are

- Each outdoor level ships its trees, bushes and grass as `.spt` files in the level's main
  archive (`packages/core/env/<level>/<level>.rim`), named `<level>_tre_*` (trees),
  `<level>_bsh_*` (bushes) and `<level>_gra_*` (grass), plus two texture atlases in
  `<level>.gpu.rim`:
  - `<level>_diffuse.dds`, the composite atlas of bark, frond and leaf images;
  - `<level>_diffuse_billboards.dds` (and `_normal_billboards.dds`), the far-distance
    billboard images of the level's trees.
- The level layout (`.arl`) lists the trees the level uses: field 3355, a list of structs whose
  field 3353 is the `.spt` name without extension (any case).
- Room layouts (`.rml`), including the level-wide `<level>.rml`, place them: field 3354, a list
  of structs with 4 position, 5 rotation (quaternion x, y, z, w), 3356 scale (about 0.28-0.34 in
  every level checked, consistent with feet to metres times a small variance), 3357 the index
  into the level's tree list, 3375 an RGBA colour, 3379 a flag. A placement is relative to its
  room's transform.

## The file

An `.spt` file is a little-endian stream of 32-bit tokens, each followed by a payload whose
type the token implies (float, int, one-byte bool or byte, length-prefixed string, float
vector, or an "unsigned int" of four value bytes plus four padding bytes); some tokens open
sections that run to a closing token. Curves are text strings
(`BezierSpline min max variance { count x y tx ty len ... }`). The full token table and the
section structure are in `crates/thedas-formats/src/speedtree.rs`, which parses every `.spt`
in the game.

What the generator needs from a file is described where it is used, in
[generation.md](generation.md).

## Texture coordinates in the file

The texture-coordinate section gives, per leaf image, per billboard image and per frond
image, four (s, t) corners into an atlas, with `t` measured upward (the image's top row has
the larger `t`; a texture's v is `1 - t`). Leaf and frond images index `<level>_diffuse.dds`;
billboard images index `<level>_diffuse_billboards.dds`. Bushes and grass have leaf and frond
images but no billboard image.
