//! Animation metadata (GFF4): animation trees (`.anb`, `ANBT`), the general data beside
//! each animation (`.gad`, `GAD `) and its events (`.evt`, `EVT `). The keyframes
//! themselves (`.ani`) are not read here.
//!
//! Field labels, read from the files:
//! - Tree: 4021 the nodes, 4022 the tree's name (`Staff`, `Bow`, `humanoid`, ...).
//! - Node: 4024 the action (`fire`, `aim`, `attk_v1`, ...), 4025 the animation (`.ani`
//!   resource, absent for a node naming the tree itself), 4026 its playback speed; 4027 to
//!   4030 are not interpreted (provisional: 4027 looks like "plays once" and 4029/4030 like
//!   tree links).
//! - General data: 4007 the animation's name, 4009 its length in seconds, 4010 its speed.
//! - Events: 4020 the list; each 4016 its time in seconds and 4017 its kind (provisional:
//!   kind 10 sits where a ranged weapon lets go, kind 1 near a melee hit).

use crate::gff4::Gff4;
use crate::typed::{Doc, TypedResult};

/// One action of an animation tree.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimNode {
    pub action: String,
    /// The `.ani` resource, if the node plays one.
    pub animation: Option<String>,
    pub speed: f32,
}

/// An animation tree (`.anb`): which animation each action plays.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimTree {
    pub name: String,
    pub nodes: Vec<AnimNode>,
}

impl AnimTree {
    pub fn parse(data: &[u8]) -> TypedResult<Self> {
        Self::from_gff(&Gff4::parse(data)?)
    }

    pub fn from_gff(doc: &Gff4) -> TypedResult<Self> {
        let d = Doc::new(doc, "animation tree");
        d.expect_type("ANBT")?;
        let root = doc.root;
        let name = d.opt_str(root, 4022)?.unwrap_or_default();
        let mut nodes = Vec::new();
        for n in d.structs(root, 4021)? {
            nodes.push(AnimNode {
                action: d.opt_str(n, 4024)?.unwrap_or_default(),
                animation: d.opt_str(n, 4025)?.filter(|s| !s.is_empty()),
                speed: d.opt_f32(n, 4026)?.unwrap_or(1.0),
            });
        }
        Ok(Self { name, nodes })
    }

    /// The node for an action (names compare without case).
    pub fn action(&self, action: &str) -> Option<&AnimNode> {
        self.nodes
            .iter()
            .find(|n| n.action.eq_ignore_ascii_case(action))
    }
}

/// The general data beside an animation (`.gad`).
#[derive(Clone, Debug, PartialEq)]
pub struct AnimInfo {
    pub name: String,
    /// Length in seconds at speed 1.
    pub duration: f32,
    pub speed: f32,
}

impl AnimInfo {
    pub fn parse(data: &[u8]) -> TypedResult<Self> {
        Self::from_gff(&Gff4::parse(data)?)
    }

    pub fn from_gff(doc: &Gff4) -> TypedResult<Self> {
        let d = Doc::new(doc, "animation data");
        d.expect_type("GAD ")?;
        let root = doc.root;
        let duration = d.opt_f32(root, 4009)?.unwrap_or(0.0);
        if !duration.is_finite() || duration < 0.0 {
            return Err(crate::typed::TypedError::Invalid(format!(
                "animation data: length {duration} is not a duration"
            )));
        }
        Ok(Self {
            name: d.opt_str(root, 4007)?.unwrap_or_default(),
            duration,
            speed: d.opt_f32(root, 4010)?.unwrap_or(1.0),
        })
    }
}

/// One event of an animation (`.evt`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimEvent {
    /// Seconds from the start.
    pub time: f32,
    pub kind: i64,
}

/// The events of an animation (`.evt`), in file order.
pub fn parse_events(data: &[u8]) -> TypedResult<Vec<AnimEvent>> {
    events_from_gff(&Gff4::parse(data)?)
}

pub fn events_from_gff(doc: &Gff4) -> TypedResult<Vec<AnimEvent>> {
    let d = Doc::new(doc, "animation events");
    d.expect_type("EVT ")?;
    let mut out = Vec::new();
    for e in d.structs(doc.root, 4020)? {
        out.push(AnimEvent {
            time: d.opt_f32(e, 4016)?.unwrap_or(0.0),
            kind: d.opt_i64(e, 4017)?.unwrap_or(0),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gff4::Value;
    use crate::typed::testutil::{Builder, s, structs};

    #[test]
    fn a_tree_maps_actions_to_animations() {
        let mut b = Builder::new(b"ANBT");
        let fire = b.add(
            b"node",
            vec![
                (4024, s("fire")),
                (4025, s("mh_c.stf_spl_d_cst.ani")),
                (4026, Value::F32(1.0)),
            ],
        );
        let named = b.add(
            b"node",
            vec![(4024, s("Staff")), (4025, Value::String(None))],
        );
        let root = b.add(
            b"tree",
            vec![(4021, structs(&[fire, named])), (4022, s("Staff"))],
        );
        let t = AnimTree::from_gff(&b.finish(root)).unwrap();
        assert_eq!(t.name, "Staff");
        let n = t.action("FIRE").unwrap();
        assert_eq!(n.animation.as_deref(), Some("mh_c.stf_spl_d_cst.ani"));
        assert_eq!(n.speed, 1.0);
        assert_eq!(t.action("Staff").unwrap().animation, None);
        assert!(t.action("aim").is_none());
    }

    #[test]
    fn general_data_gives_the_length_and_rejects_nonsense() {
        let mut b = Builder::new(b"GAD ");
        let root = b.add(
            b"gad ",
            vec![
                (4007, s("fire")),
                (4009, Value::F32(0.433)),
                (4010, Value::F32(1.0)),
            ],
        );
        let g = AnimInfo::from_gff(&b.finish(root)).unwrap();
        assert_eq!((g.name.as_str(), g.duration, g.speed), ("fire", 0.433, 1.0));
        let mut b = Builder::new(b"GAD ");
        let root = b.add(b"gad ", vec![(4009, Value::F32(-1.0))]);
        assert!(AnimInfo::from_gff(&b.finish(root)).is_err());
        let mut b = Builder::new(b"EVT ");
        let root = b.add(b"evtl", vec![]);
        assert!(
            AnimInfo::from_gff(&b.finish(root)).is_err(),
            "wrong file type"
        );
    }

    #[test]
    fn events_keep_their_times_and_kinds_and_garbage_is_an_error() {
        let mut b = Builder::new(b"EVT ");
        let e1 = b.add(
            b"evt ",
            vec![(4016, Value::F32(0.03)), (4017, Value::I32(10))],
        );
        let e2 = b.add(
            b"evt ",
            vec![(4016, Value::F32(0.43)), (4017, Value::I32(13))],
        );
        let root = b.add(b"evtl", vec![(4020, structs(&[e1, e2]))]);
        let ev = events_from_gff(&b.finish(root)).unwrap();
        assert_eq!(
            ev,
            vec![
                AnimEvent {
                    time: 0.03,
                    kind: 10
                },
                AnimEvent {
                    time: 0.43,
                    kind: 13
                }
            ]
        );
        assert!(parse_events(b"not a gff").is_err());
        assert!(AnimTree::parse(&[]).is_err());
        assert!(AnimInfo::parse(&[0xff; 64]).is_err());
    }
}
