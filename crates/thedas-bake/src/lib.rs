//! Our own lightmap baker, modelled on the Dragon Age: Origins toolset's lightmapper, to learn
//! by comparison exactly how the shipped lightmaps were made. The formulas and their sources
//! are in `docs/lightmaps/baker-spec.md`; what is not yet traced is marked provisional there and
//! here.

pub mod bake;
pub mod bvh;
pub mod compare;
pub mod fit;
pub mod gather;
