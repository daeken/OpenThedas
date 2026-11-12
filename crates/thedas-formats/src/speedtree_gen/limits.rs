//! Limits that keep malformed or hostile `.spt` files (mods can ship their own) from making
//! the generator run away. Not part of the original computation: every game file is far
//! inside them (the largest grows 774 branches, 712 leaves and about 7,300 surface vertices),
//! and a file outside them is rejected with an error instead of being grown.

use crate::speedtree::SpeedTree;

/// Why a tree was not generated.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GenerateError {
    #[error("{what} is {value}, outside the supported 0..={max}")]
    OutOfRange { what: String, value: i64, max: i64 },
    #[error("the tree grows more than {max} {what}")]
    TooLarge { what: &'static str, max: usize },
}

pub const MAX_LEVELS: usize = 16;
pub const MAX_SEGMENTS: i64 = 1_000;
pub const MAX_CROSS_SECTIONS: i64 = 1_000;
pub const MAX_FLARES: i64 = 256;
pub const MAX_LOD_LEVELS: i64 = 32;
pub const MAX_BLADES: i64 = 64;
pub const MAX_PROFILE_SEGMENTS: i64 = 1_000;
pub const MAX_LEAF_TEXTURES: usize = 256;
pub const MAX_BRANCHES: usize = 200_000;
pub const MAX_SKELETON_VERTICES: usize = 2_000_000;
/// Children considered (each draws a few random values even when culled).
pub const MAX_CHILD_CANDIDATES: usize = 2_000_000;
pub const MAX_LEAVES: usize = 50_000;
/// Branch tube, frond and leaf-mesh vertices together.
pub const MAX_SURFACE_VERTICES: usize = 8_000_000;

fn range(what: impl Into<String>, value: i64, max: i64) -> Result<(), GenerateError> {
    if (0..=max).contains(&value) {
        Ok(())
    } else {
        Err(GenerateError::OutOfRange {
            what: what.into(),
            value,
            max,
        })
    }
}

/// Checks the counts a file sets directly before anything is grown.
pub fn validate(tree: &SpeedTree) -> Result<(), GenerateError> {
    range(
        "the branch level count",
        tree.branch_levels.len() as i64,
        MAX_LEVELS as i64,
    )?;
    let roots = tree.roots.as_ref().map(|r| &r.roots);
    for (i, level) in tree.branch_levels.iter().chain(roots).enumerate() {
        let name = |field: &str| format!("branch level {i} {field}");
        range(name("segments"), level.segments.into(), MAX_SEGMENTS)?;
        range(
            name("cross-section segments"),
            level.cross_section_segments.into(),
            MAX_CROSS_SECTIONS,
        )?;
        range(name("flare count"), level.flares.count.into(), MAX_FLARES)?;
    }
    if let Some(f) = &tree.fronds {
        range("the frond blade count", f.blades.into(), MAX_BLADES)?;
        range(
            "the frond profile segments",
            f.profile_segments.into(),
            MAX_PROFILE_SEGMENTS,
        )?;
        range("the frond LOD count", f.lod_count.into(), MAX_LOD_LEVELS)?;
        range(
            "the frond segment override",
            f.segment_override.into(),
            MAX_SEGMENTS,
        )?;
        range(
            "the frond minimum length segments",
            f.min_length_segments.into(),
            MAX_SEGMENTS,
        )?;
        range(
            "the frond minimum cross segments",
            f.min_cross_segments.into(),
            MAX_PROFILE_SEGMENTS,
        )?;
    }
    if let Some(lod) = &tree.lod {
        if let Some(n) = lod.branch_lod_count {
            range("the branch LOD count", n.into(), MAX_LOD_LEVELS)?;
        }
        if let Some(n) = lod.leaf_lod_count {
            range("the leaf LOD count", n.into(), MAX_LOD_LEVELS)?;
        }
    }
    range(
        "the leaf texture count",
        tree.leaves.textures.len() as i64,
        MAX_LEAF_TEXTURES as i64,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speedtree_gen::mesh::TreeMesh;
    use crate::speedtree_gen::skeleton::tests::{level, tree};

    fn three_levels() -> SpeedTree {
        tree(vec![level(-90.0, 2.0), level(0.0, 3.0), level(0.0, 0.0)])
    }

    #[test]
    fn out_of_range_counts_are_rejected_before_growing() {
        let mut t = three_levels();
        t.branch_levels[1].segments = i32::MAX;
        assert!(matches!(
            TreeMesh::try_new(&t),
            Err(GenerateError::OutOfRange { .. })
        ));
        let mut t = three_levels();
        t.branch_levels[0].cross_section_segments = -1;
        assert!(TreeMesh::try_new(&t).is_err());
        let mut t = three_levels();
        t.branch_levels = vec![level(0.0, 0.0); MAX_LEVELS + 1];
        assert!(TreeMesh::try_new(&t).is_err());
        // `new` gives an empty tree instead of an error.
        assert!(TreeMesh::new(&t).branches.vertices.is_empty());
    }

    #[test]
    fn runaway_growth_stops_at_a_limit() {
        // An infinite frequency asks for i32::MAX children of the trunk.
        let mut t = three_levels();
        t.branch_levels[0].frequency = f32::INFINITY;
        assert!(matches!(
            TreeMesh::try_new(&t),
            Err(GenerateError::TooLarge { .. })
        ));
    }

    #[test]
    fn non_finite_values_do_not_panic_or_hang() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30, -1e30] {
            let mut t = three_levels();
            t.tree.size = value;
            let _ = TreeMesh::try_new(&t);
            let mut t = three_levels();
            let layer = &mut t.branch_levels[0].layers[0];
            layer.clamp_s = true;
            layer.s_offset = value;
            t.cluster_first_branch_level = Some(0);
            let _ = TreeMesh::try_new(&t);
            let mut t = three_levels();
            t.branch_levels[1].first_branch = value;
            t.branch_levels[0].last_branch = value;
            let _ = TreeMesh::try_new(&t);
        }
    }

    /// A small xorshift for repeatable mutations.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }
    }

    /// Mutation test over the game's files: truncated, byte-flipped and value-poisoned copies
    /// of every `.spt` in `$THEDAS_SPT_DIR` must parse to an error or generate (or be rejected
    /// by the limits) without panicking. Run with
    /// `THEDAS_SPT_DIR=<extracted .spt dir> cargo test -p thedas-formats mutated -- --ignored`.
    #[test]
    #[ignore = "needs game files: set THEDAS_SPT_DIR and run with --ignored"]
    fn mutated_game_files_never_panic() {
        const WORDS: [u32; 8] = [
            0x7f80_0000, // +inf
            0x7fc0_0000, // NaN
            0xffff_ffff,
            0x7fff_ffff,
            0x8000_0000,
            0x7149_f2ca, // 1e30
            0xf149_f2ca, // -1e30
            0,
        ];
        let dir = std::env::var("THEDAS_SPT_DIR").expect("THEDAS_SPT_DIR");
        let mut paths: Vec<_> = std::fs::read_dir(&dir)
            .expect("THEDAS_SPT_DIR")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("spt")))
            .collect();
        paths.sort();
        assert!(!paths.is_empty());
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for path in &paths {
            let data = std::fs::read(path).unwrap();
            for case in 0..8 {
                let mut d = data.clone();
                match case % 4 {
                    0 => d.truncate(rng.below(d.len())),
                    1 => {
                        for _ in 0..1 + rng.below(8) {
                            let i = rng.below(d.len());
                            d[i] = rng.next() as u8;
                        }
                    }
                    _ => {
                        for _ in 0..1 + rng.below(3) {
                            if d.len() >= 4 {
                                let i = rng.below(d.len() - 3);
                                let w = WORDS[rng.below(WORDS.len())];
                                d[i..i + 4].copy_from_slice(&w.to_le_bytes());
                            }
                        }
                    }
                }
                let outcome = std::panic::catch_unwind(|| {
                    if let Ok(t) = crate::speedtree::parse(&d) {
                        let _ = TreeMesh::try_new(&t);
                    }
                });
                assert!(outcome.is_ok(), "{} case {case} panicked", path.display());
            }
        }
    }
}
