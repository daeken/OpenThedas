//! Placeable walkmeshes: one local pathing grid per state, in list order.
//! Origins loads `ModelName_Appearance.pwk` and root list 3016 (0x7500e0).

use crate::gff4::{Gff4, List, Value};
use crate::level::{NavGrid, field};
use crate::typed::{Doc, TypedError, TypedResult};

#[derive(Clone, Debug, PartialEq)]
pub struct Pwk {
    /// Missing grids retain their state index.
    pub states: Vec<Option<NavGrid>>,
}

impl Pwk {
    pub fn parse(data: &[u8]) -> TypedResult<Self> {
        Self::from_gff4(&Gff4::parse(data)?)
    }

    pub fn from_gff4(gff: &Gff4) -> TypedResult<Self> {
        let d = Doc::new(gff, "placeable walkmesh");
        d.expect_type("PWK ")?;
        let entries = match d.get(gff.root, 3016) {
            None | Some(Value::List(None)) => &[][..],
            Some(Value::List(Some(List::Struct(v)))) => v.as_slice(),
            _ => {
                return Err(TypedError::Invalid(
                    "pwk: state list is not a struct list".into(),
                ));
            }
        };
        let states = entries
            .iter()
            .map(|state| {
                let Some(state) = state else { return Ok(None) };
                let Some(g) = d.opt_struct(*state, 3110)? else {
                    return Ok(None);
                };
                let dimension = |label| {
                    d.opt_i64(g, label)?
                        .and_then(|v| u32::try_from(v).ok())
                        .ok_or_else(|| TypedError::Invalid("pwk: invalid grid dimensions".into()))
                };
                let columns = dimension(field::GRID_COLUMNS)?;
                let rows = dimension(field::GRID_ROWS)?;
                let data = d.bytes(g, field::GRID_DATA)?.to_vec();
                let count = u64::from(columns) * u64::from(rows);
                let cell_size = d.opt_f32(g, field::GRID_CELL_SIZE)?.unwrap_or(0.0);
                let base = d.opt_vec3(g, field::GRID_BASE)?.unwrap_or_default();
                if count != data.len() as u64
                    || !cell_size.is_finite()
                    || cell_size <= 0.0
                    || !base.iter().all(|v| v.is_finite())
                {
                    return Err(TypedError::Invalid("pwk: invalid pathing grid".into()));
                }
                Ok(Some(NavGrid {
                    columns,
                    rows,
                    cell_size,
                    clearance: d.opt_f32(g, field::GRID_CLEARANCE)?.unwrap_or(0.0),
                    base,
                    normal: d
                        .opt_vec3(g, field::GRID_NORMAL)?
                        .unwrap_or([0.0, 0.0, 1.0]),
                    data,
                    heights: d.numbers(g, field::GRID_HEIGHT)?,
                    surfaces: Vec::new(),
                }))
            })
            .collect::<TypedResult<_>>()?;
        Ok(Self { states })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gff4::{List, Value};
    use crate::typed::testutil::Builder;

    #[test]
    fn states_keep_list_order_and_absent_grids() {
        let mut b = Builder::new(b"PWK ");
        let g = b.add(
            b"NAVI",
            vec![
                (3086, Value::U32(2)),
                (3087, Value::U32(1)),
                (3088, Value::F32(0.25)),
                (3090, Value::Vec3([-1.0, 2.0, 0.0])),
                (3092, Value::List(Some(List::U8(vec![7, 8])))),
                (3093, Value::List(Some(List::I16(vec![0, 16])))),
            ],
        );
        let empty = b.add(b"    ", vec![]);
        let state = b.add(b"    ", vec![(3110, Value::Struct(Some(g)))]);
        let root = b.add(
            b"PPG ",
            vec![(
                3016,
                Value::List(Some(List::Struct(vec![None, Some(empty), Some(state)]))),
            )],
        );
        let mut doc = b.finish(root);
        let pwk = Pwk::from_gff4(&doc).unwrap();
        assert_eq!(pwk.states.len(), 3);
        assert!(pwk.states[0].is_none());
        assert!(pwk.states[1].is_none());
        let grid = pwk.states[2].as_ref().unwrap();
        assert_eq!(grid.data, [7, 8]);
        assert_eq!(grid.heights, [0.0, 16.0]);
        assert_eq!(grid.base, [-1.0, 2.0, 0.0]);
        // A bad size must not expose a partially indexed grid.
        doc.structs[g.0].values[0] = Value::U32(3);
        assert!(Pwk::from_gff4(&doc).is_err());
        doc.structs[g.0].values[0] = Value::U32(2);
        doc.structs[g.0].values[2] = Value::F32(f32::NAN);
        assert!(Pwk::from_gff4(&doc).is_err());
    }
}
