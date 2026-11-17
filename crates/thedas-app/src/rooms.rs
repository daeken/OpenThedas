//! Origins' room visibility (0x68afd0 -> 0x6886f0). The party's occupied rooms seed
//! the authored RML graph. A camera looking out over the level uses the same room set.

use std::collections::{HashMap, HashSet};

use thedas_formats::phy::mask;
use thedas_render::scene::RoomVisibility;
use thedas_sim::{collision::Collision, sim::Sim};

/// The room recorded by the first eligible static shape below the actor (0x695d70,
/// 0x67b3a0, 0x5d1e00). The creature's appearance height (0x6a1a30) and a 2 m
/// bias are added to its feet. The finite ray length is provisional.
fn room_below(collision: &Collision, [x, y, z]: [f32; 3], height: f32) -> Option<i64> {
    collision
        .ray_hits([x, y, z + height + 2.0], [x, y, z - 1000.0])
        .into_iter()
        .filter_map(|hit| collision.shape(hit.shape))
        .find_map(|shape| {
            if shape.mask & mask::STATIC != 0 {
                shape.room
            } else {
                None
            }
        })
}

pub fn visible_rooms(
    sim: &Sim,
    collision: &Collision,
    rooms: &HashMap<i64, RoomVisibility>,
) -> Option<HashSet<i64>> {
    // Cinematic playback shows the entire layout (0x7e3f76 -> 0x67c980).
    if rooms.len() <= 1 || sim.world.cutscene.is_some() {
        return None;
    }
    let area = sim.world.area?;
    let occupied: HashSet<_> = thedas_sim::engine::party_of(sim)
        .into_iter()
        .filter_map(|id| sim.world.get(id))
        .filter(|actor| actor.area == Some(area))
        .filter_map(|actor| {
            room_below(
                collision,
                actor.position,
                crate::live::appearance_height(sim, actor.id),
            )
        })
        .filter(|id| rooms.contains_key(id))
        .collect();
    // Keep the geometry if a custom level or missing PHY has no room under the party.
    if occupied.is_empty() {
        return None;
    }
    log::trace!("occupied layout rooms {occupied:?}");

    let mut closed = HashMap::new();
    for &id in sim.world.actor_ids().iter() {
        let Some(door) = sim.world.get(id).filter(|object| object.area == Some(area)) else {
            continue;
        };
        let Some((&a, &b)) = door.fields.get("RoomID1").zip(door.fields.get("RoomID2")) else {
            continue;
        };
        let (a, b) = (i64::from(a), i64::from(b));
        if a == b || !rooms.contains_key(&a) || !rooms.contains_key(&b) || !sim.is_door(id) {
            continue;
        }
        // 0x74bcc0 reads the current state controller. The runtime group reflects its
        // blocking flag. For older saves the stock door controller's closed states are 0/1.
        let blocks = door.active
            && door.placeable_group.map_or_else(
                || {
                    matches!(
                        door.fields
                            .get("State")
                            .or_else(|| door.fields.get("AnimContState")),
                        None | Some(0 | 1)
                    )
                },
                |group| group & mask::PLACEABLES != 0,
            );
        closed
            .entry(pair(a, b))
            .and_modify(|all_closed| *all_closed &= blocks)
            .or_insert(blocks);
    }
    Some(connected(rooms, occupied, &closed))
}

fn pair(a: i64, b: i64) -> (i64, i64) {
    (a.min(b), a.max(b))
}

/// The occupied room and its neighbours that pass the portal test (0x687ec0).
/// A neighbour with no outgoing portal list passes directly. Other neighbours must
/// be reachable through 3099 edges with no intervening closed doors (0x686a70).
fn connected(
    rooms: &HashMap<i64, RoomVisibility>,
    occupied: HashSet<i64>,
    closed: &HashMap<(i64, i64), bool>,
) -> HashSet<i64> {
    let mut visible = occupied.clone();
    for source in occupied {
        let adjacent = &rooms[&source].adjacent;
        let mut seen = HashSet::from([source]);
        let mut pending = vec![source];
        while let Some(room) = pending.pop() {
            for &next in &rooms[&room].portals {
                if adjacent.contains(&next)
                    && rooms.contains_key(&next)
                    && !closed.get(&pair(room, next)).copied().unwrap_or(false)
                    && seen.insert(next)
                {
                    pending.push(next);
                }
            }
        }
        visible.extend(seen);
        visible.extend(
            adjacent
                .iter()
                .copied()
                .filter(|id| rooms.get(id).is_some_and(|room| room.portals.is_empty())),
        );
    }
    visible
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_door_hides_a_room_reached_through_a_neighbour() {
        let rooms = HashMap::from([
            (
                1,
                RoomVisibility {
                    adjacent: vec![2, 3, 4],
                    portals: vec![2],
                },
            ),
            (
                2,
                RoomVisibility {
                    adjacent: vec![1, 3],
                    portals: vec![1, 3],
                },
            ),
            (
                3,
                RoomVisibility {
                    adjacent: vec![2],
                    portals: vec![2],
                },
            ),
            (4, RoomVisibility::default()),
        ]);
        let occupied = HashSet::from([1]);
        let door = HashMap::from([((2, 3), true)]);
        assert_eq!(
            connected(&rooms, occupied.clone(), &door),
            HashSet::from([1, 2, 4])
        );
        assert_eq!(
            connected(&rooms, occupied, &HashMap::new()),
            HashSet::from([1, 2, 3, 4])
        );
        assert_eq!(
            connected(&rooms, HashSet::from([1, 3]), &door),
            HashSet::from([1, 2, 3, 4])
        );
    }
}
