//! Origins placeable state initialization, transitions and walkmesh writes.
//! Animation completion/removal is a separate path (0x74d200).

use std::collections::BTreeMap;
use std::sync::Arc;

use thedas_formats::gda::Cell;
use thedas_formats::level::NavGrid;
use thedas_script::vm::ObjectId;

use crate::fmath::DetMath;
use crate::sim::Sim;
use crate::world::{ObjectKind, PlaceableClip};

impl Sim {
    /// The ASCII instance reader writes InitialState directly to the state word
    /// (0x7506ba..0x7506df); model setup then forces that state (0x752a88).
    /// Saved/script-assigned State takes precedence, including on legacy saves.
    pub(crate) fn init_placeable_state(&mut self, id: ObjectId) {
        if self.game != thedas_vfs::Game::Origins {
            return;
        }
        let Some(o) = self
            .world
            .get_mut(id)
            .filter(|o| o.kind == ObjectKind::Placeable && !o.fields.contains_key("State"))
        else {
            return;
        };
        let Some(&state) = o.fields.get("InitialState") else {
            return;
        };
        let state = i32::from(state as u16);
        o.fields.insert("State".into(), state);
        // Area registration also installs the state grid without requiring a
        // controller (0x750caa), subject to the same walkmesh gates.
        if !self.change_placeable_state(id, state, true) {
            self.write_placeable_walkmesh(id, state as u16, false);
        }
        // Model setup subsequently applies the loaded active flag through
        // vfunc +0x140 (0x752096 / 0x752aa3 -> 0x74f200).
        let o = self.world.get(id).unwrap();
        if !o.parked.unwrap_or(o.active) {
            let state = o.fields["State"] as u16;
            self.write_placeable_walkmesh(id, state, true);
        }
    }

    /// CCPlaceable's active setter (0x74f200) also installs/removes its PWK.
    pub(crate) fn set_object_active(&mut self, id: ObjectId, active: bool) {
        let Some(o) = self.world.get_mut(id) else {
            return;
        };
        // The script handler returns before vfunc +0x140 when activity already
        // matches (0x66198c / 0x661a8b). Replaying a removal could clear a later
        // footprint owner's cells. Model-load initialization calls PWK directly.
        if self.game == thedas_vfs::Game::Origins
            && o.kind == ObjectKind::Placeable
            && o.active == active
        {
            return;
        }
        o.active = active;
        if self.game == thedas_vfs::Game::Origins && o.kind == ObjectKind::Placeable {
            let state = o
                .fields
                .get("State")
                .or_else(|| o.fields.get("AnimContState"))
                .copied()
                .unwrap_or(0) as u16;
            self.write_placeable_walkmesh(id, state, !active);
        }
    }

    /// The ordinary state path (0x74f9d0, force=0), shared by the script setter
    /// and successful actions. Forced initialization shows the destination pose
    /// immediately; the renderer already chooses it on an object's first sighting.
    pub(crate) fn set_placeable_state(&mut self, id: ObjectId, state: i32) {
        if self.game == thedas_vfs::Game::DragonAge2 {
            if let Some(o) = self.world.get_mut(id) {
                o.fields.insert("State".into(), state);
            }
            return;
        }
        self.change_placeable_state(id, state, false);
    }

    fn change_placeable_state(&mut self, id: ObjectId, state: i32, force: bool) -> bool {
        let Some(o) = self
            .world
            .get(id)
            .filter(|o| o.kind == ObjectKind::Placeable)
        else {
            return false;
        };
        let tables = self.world.tables();
        let Some(types) = tables.get(93, "") else {
            return false;
        };
        let Some(Cell::String(controller)) =
            types.cell_by_hash(0x9a32_0a51, i64::from(o.appearance as u16))
        else {
            return false;
        };
        let controller = crate::engine::clean(controller);
        if controller.is_empty() {
            return false;
        }
        let Some(controller) = tables.get(0, controller).filter(|t| !t.is_empty()) else {
            return false;
        };
        let current = o
            .fields
            .get("State")
            .or_else(|| o.fields.get("AnimContState"))
            .copied()
            .unwrap_or(0) as u16;
        let state = state as u16;
        if current == state && !force {
            return false;
        }
        // Queries precede destination normalization (0x74fa14/0x74fa27).
        let transition = controller_animation(&controller, current, state);
        // The controller loader stores one record per row position (0xa3bc73).
        // The setter bounds the unsigned-short request by that record count.
        let state = if usize::from(state) < controller.order.len() {
            i32::from(state)
        } else {
            0
        };
        self.world
            .get_mut(id)
            .unwrap()
            .fields
            .insert("State".into(), state);
        // 0x74fb27 removes the old footprint, then 0x74fb4e installs the new
        // state's grid. A later transition must not leave its initial PWK behind.
        self.write_placeable_walkmesh(id, current, true);
        // Door physics restores its original group in blocking controller states
        // and uses group 1 otherwise (0x74e850 -> 0x74bcc0 / 0x74d350).
        let row = controller.order[state as usize];
        let appearance_row = i64::from(self.world.get(id).unwrap().appearance as u16);
        if matches!(types.cell_by_hash(0x5010_edc3, appearance_row), Some(Cell::Int(v)) if *v as u8 == 1)
            && let Some(Cell::String(model)) = types.cell_by_hash(0xfd84_03d5, appearance_row)
            && let Ok((data, _)) = self
                .vfs
                .read(&format!("{}.phy", crate::engine::clean(model)))
            && let Ok(physics) = thedas_formats::phy::Phy::parse(&data)
        {
            let o = self.world.get_mut(id).unwrap();
            let blocking = o.parked.unwrap_or(o.active)
                && matches!(controller.cell_by_hash(0x86a2_7132, row), Some(Cell::Int(v)) if *v != 0);
            o.placeable_group = Some(if blocking {
                physics
                    .actor_masks
                    .iter()
                    .copied()
                    .find(|&m| m != 0)
                    .unwrap_or(0)
            } else {
                1
            });
        }
        self.write_placeable_walkmesh(id, state as u16, false);
        // The forced path, like an ordinary transition, disables interaction if
        // neither controller action exists (0x74fb61..0x74fba1). It never enables
        // a Useable=0 object. These are action IDs, not controller events.
        let cells = &controller.rows[&row];
        let first_action = controller.order.len() + 2;
        // 0xa3bccf..0xa3bcfe: up to two action/target/variation triples
        // between the state-animation columns and the blocking column.
        let actions = controller
            .columns
            .get(&0x86a2_7132)
            .and_then(|column| column.checked_sub(first_action))
            .map_or(0, |columns| (columns / 3).min(2));
        if (0..actions).all(|slot| {
            !matches!(cells.get(first_action + slot * 3), Some(Cell::Int(v)) if *v as u16 != 0)
        }) {
            self.world.get_mut(id).unwrap().interactive = false;
        }
        // Forced model initialization selects its destination pose directly;
        // ordinary state changes retain one-shot playback and completion.
        if !force && let Some(animation) = transition {
            self.play_placeable_transition(id, animation);
        }
        true
    }

    fn play_placeable_transition(&mut self, id: ObjectId, animation: i64) {
        let tables = self.world.tables();
        let looping = matches!(tables.get(0, "").and_then(|t| t.cell_by_hash(0x12a0_2374, animation).cloned()), Some(Cell::Int(flags)) if flags & 1 != 0);
        let o = self.world.get_mut(id).unwrap();
        o.placeable_playback.get_or_insert_default().pending = !looping;
        if o.parked.is_some() {
            return;
        }
        let Some(types) = tables.get(93, "") else {
            return;
        };
        let Some(Cell::String(tree)) =
            types.cell_by_hash(0x70f4_63fa, i64::from(o.appearance as u16))
        else {
            return;
        };
        let tree = crate::engine::clean(tree).to_ascii_lowercase();
        if tree.is_empty() {
            return;
        }
        let Some(actions) = tables.get(0, "") else {
            return;
        };
        let Some(Cell::String(action)) = actions.cell_by_hash(0x7a39_8af9, animation) else {
            return;
        };
        let action = crate::engine::clean(action).to_ascii_lowercase();
        let resource = if tree.contains('.') {
            tree.clone()
        } else {
            format!("{tree}.anb")
        };
        let Some(node) = self
            .vfs
            .read(&resource)
            .ok()
            .and_then(|(d, _)| thedas_formats::anim::AnimTree::parse(&d).ok())
            .and_then(|t| t.action(&action).cloned())
        else {
            return;
        };
        // 0xa5e940 queries the loaded node's once flag through 0x813ca0;
        // animation-table flags are its fallback when that node is absent.
        self.world
            .get_mut(id)
            .unwrap()
            .placeable_playback
            .as_mut()
            .unwrap()
            .pending = node.once;
        if !node.once {
            return;
        }
        let Some(clip) = node.animation else { return };
        let clip = crate::engine::clean(&clip).to_ascii_lowercase();
        let Some(animation) = self
            .vfs
            .read(&clip)
            .ok()
            .and_then(|(d, _)| thedas_formats::anim::Animation::parse(&d).ok())
        else {
            return;
        };
        // The ordinary setter explicitly starts rate 1. Neither the tree's
        // default speed nor GAD field 4010 participates in that rate.
        let normalized_rate = (65_535.0 / f64::from(animation.length)) as f32;
        if !normalized_rate.is_finite() {
            return;
        }
        let clip = PlaceableClip {
            tree,
            action,
            phase: 0,
            normalized_rate,
            updated_ms: self.world.time_ms,
        };
        self.start_placeable_clip(id, clip);
    }

    fn start_placeable_clip(&mut self, id: ObjectId, clip: PlaceableClip) {
        let playback = self
            .world
            .get_mut(id)
            .unwrap()
            .placeable_playback
            .get_or_insert_default();
        // A tree contains one instance per node. Replaying the same node resets
        // it; changing action does not cancel outgoing one-shot endpoints.
        playback
            .clips
            .retain(|c| c.tree != clip.tree || c.action != clip.action);
        playback.clips.push(clip);
    }

    pub(crate) fn advance_placeable_animations(&mut self) {
        if self.game != thedas_vfs::Game::Origins {
            return;
        }
        let now = self.world.time_ms;
        let mut completed = Vec::new();
        for o in self.world.objects.values_mut() {
            let Some(playback) = o.placeable_playback.as_mut() else {
                continue;
            };
            for clip in &mut playback.clips {
                let elapsed = now.saturating_sub(clip.updated_ms);
                clip.updated_ms = now;
                if o.parked.is_some() {
                    continue;
                }
                // 0x811be0 truncates the normalized increment to a signed int,
                // then uses its low unsigned short before adding and clamping.
                let step = ((elapsed as f64 / 1000.0) * f64::from(clip.normalized_rate)).trunc();
                let step = if step >= 0.0 && step <= f64::from(i32::MAX) {
                    step as i32 as u16
                } else {
                    0
                };
                let previous = clip.phase;
                clip.phase = u32::from(previous)
                    .saturating_add(u32::from(step))
                    .min(65_535) as u16;
                if previous < 65_535 && clip.phase == 65_535 {
                    completed.push(o.id);
                }
            }
            playback.clips.retain(|c| c.phase < 65_535);
        }
        for id in completed {
            self.placeable_animation_complete(id);
        }
    }

    fn placeable_animation_complete(&mut self, id: ObjectId) {
        let o = self.world.get_mut(id).unwrap();
        if let Some(playback) = o.placeable_playback.as_mut() {
            playback.pending = false;
        }
        // 0x74d200 checks current health/appearance, not the pending byte or the
        // health at playback start. Even an outgoing node can reach this path.
        if crate::engine::origins_placeable_health(o) > 0 {
            return;
        }
        let appearance = i64::from(o.appearance as u16);
        if !matches!(self.world.tables().get(93, "").and_then(|t| t.cell_by_hash(0x1dea_a1ea, appearance).cloned()), Some(Cell::Int(v)) if v != 0)
        {
            return;
        }
        let now = self.world.time_ms;
        self.world
            .get_mut(id)
            .unwrap()
            .placeable_fade_at
            .get_or_insert(now);
        // 0x696a60 latches the first native request. Delivery rechecks the
        // independent IsDestroyable flag; a blocked delivery never retries.
        self.world.request_origins_destroy(3000, id);
    }

    pub(crate) fn set_placeable_health(&mut self, id: ObjectId, current: i32) {
        if self.game != thedas_vfs::Game::Origins
            || !self
                .world
                .get(id)
                .is_some_and(|o| o.kind == ObjectKind::Placeable)
        {
            return;
        }
        let current = i32::from(current as i16);
        if current <= 0 {
            self.clear_destroyed_placeable_walkmesh(id);
        }
        let o = self.world.get_mut(id).unwrap();
        o.fields.insert("Health".into(), current);
        if let Some(p) = o.properties.get_mut(&7) {
            p.1 = current as f32;
        }
        if current <= 0 && o.inventory.is_empty() {
            o.interactive = false;
        }
    }

    fn clear_destroyed_placeable_walkmesh(&mut self, id: ObjectId) {
        let o = self.world.get(id).unwrap();
        let row = i64::from(o.appearance as u16);
        let Some(types) = self.world.tables().get(93, "") else {
            return;
        };
        if !matches!(types.cell_by_hash(0x1dea_a1ea, row), Some(Cell::Int(v)) if *v != 0) {
            return;
        }
        let Some(Cell::String(model)) = types.cell_by_hash(0xfd84_03d5, row) else {
            return;
        };
        let model = crate::engine::clean(model);
        let physics = self
            .vfs
            .read(&format!("{model}.phy"))
            .ok()
            .and_then(|(d, _)| thedas_formats::phy::Phy::parse(&d).ok());
        let original_group = physics.as_ref().map_or(u32::MAX, |p| {
            p.actor_masks.iter().copied().find(|&m| m != 0).unwrap_or(0)
        });
        let state = o
            .fields
            .get("State")
            .or_else(|| o.fields.get("AnimContState"))
            .copied()
            .unwrap_or(0) as u16;
        let was_alive = crate::engine::origins_placeable_health(o) > 0;
        let type_one =
            matches!(types.cell_by_hash(0x5010_edc3, row), Some(Cell::Int(v)) if *v as u8 == 1);
        // 0x74d350(0) writes group 1 on every nonpositive setter, even when
        // 0x74dba0 rejects the walkmesh operation. It does not change Active.
        if physics.is_some() {
            self.world.get_mut(id).unwrap().placeable_group = Some(1);
        }
        if !was_alive || (original_group & 1 != 0 && !type_one) {
            return;
        }
        self.write_placeable_walkmesh(id, state, true);
    }

    /// 0x74dba0, operations 0 (install) and 1 (clear). Uses the original PHY
    /// group for eligibility, independently of the runtime group override.
    fn write_placeable_walkmesh(&mut self, id: ObjectId, state: u16, clear: bool) {
        let Some(o) = self.world.get(id) else { return };
        let row = i64::from(o.appearance as u16);
        let appearance = o.appearance;
        let Some(types) = self.world.tables().get(93, "") else {
            return;
        };
        let destructive =
            matches!(types.cell_by_hash(0x1dea_a1ea, row), Some(Cell::Int(v)) if *v != 0);
        if destructive && crate::engine::origins_placeable_health(o) <= 0 {
            return;
        }
        let Some(Cell::String(model)) = types.cell_by_hash(0xfd84_03d5, row) else {
            return;
        };
        let model = crate::engine::clean(model);
        let original_group = self
            .vfs
            .read(&format!("{model}.phy"))
            .ok()
            .and_then(|(d, _)| thedas_formats::phy::Phy::parse(&d).ok())
            .map_or(u32::MAX, |p| {
                p.actor_masks.iter().copied().find(|&m| m != 0).unwrap_or(0)
            });
        let type_one =
            matches!(types.cell_by_hash(0x5010_edc3, row), Some(Cell::Int(v)) if *v as u8 == 1);
        if original_group & 1 != 0 && !type_one {
            return;
        }
        let area = o.area;
        let position = o.position;
        let facing = o.facing;
        let Some(area) = area else { return };
        let Some(source) = self
            .vfs
            .read(&format!("{model}_{appearance}.pwk"))
            .ok()
            .and_then(|(d, _)| thedas_formats::pwk::Pwk::parse(&d).ok())
            .and_then(|p| p.states.get(state as usize).cloned().flatten())
        else {
            return;
        };
        let Some(grid) = self.nav_grid(area) else {
            return;
        };
        let mut changed = (*grid).clone();
        let writes = write_walkmesh_cells(&mut changed, &source, position, facing, clear);
        if !writes.is_empty() {
            self.world
                .nav_patches
                .entry(area)
                .or_default()
                .extend(writes);
            self.changed_nav_grids.insert(area, Arc::new(changed));
            // Doorway cells are derived from the walkable grid. A locked PWK
            // uses kind 8; after unlocking, its kind-7 cells must participate in
            // the closed-but-openable doorway approximation again.
            self.door_cells.remove(&area);
        }
    }
}

fn controller_animation(controller: &crate::m2da::Table, from: u16, to: u16) -> Option<i64> {
    // 0xa3ba80 uses 0xffff as a diagonal-query sentinel before bounding
    // either index. The ordinary setter passes its unnormalized request.
    let to = if to == u16::MAX { from } else { to };
    let (&from, &to) = (
        controller.order.get(usize::from(from))?,
        controller.order.get(usize::from(to))?,
    );
    let Cell::String(label) = controller.cell("Label", to)? else {
        return None;
    };
    match controller.cell(crate::engine::clean(label), from)? {
        Cell::Int(animation) if *animation != 0 => Some(*animation),
        _ => None,
    }
}

/// Operation 1 (0xa46cc0): visit EVERY source cell, transformed into the area's
/// grid. Only destination kinds 7/8 become 1; source kinds are not a mask.
#[cfg(test)]
fn clear_walkmesh_cells(
    destination: &mut NavGrid,
    source: &NavGrid,
    position: [f32; 3],
    facing: f32,
) -> BTreeMap<u32, u8> {
    write_walkmesh_cells(destination, source, position, facing, true)
}

/// Operation 0 copies source kinds only over destination kinds below 8;
/// operation 1 changes destination kinds 7/8 to 1, regardless of source kind.
fn write_walkmesh_cells(
    destination: &mut NavGrid,
    source: &NavGrid,
    position: [f32; 3],
    facing: f32,
    clear: bool,
) -> BTreeMap<u32, u8> {
    let angle = facing.to_radians();
    let (sin, cos) = (angle.dsin(), angle.dcos());
    let mut writes = BTreeMap::new();
    for x in 0..source.columns {
        for y in 0..source.rows {
            let px = source.base[0] + (x as f32 + 0.5) * source.cell_size;
            let py = source.base[1] + (y as f32 + 0.5) * source.cell_size;
            let world = [
                position[0] + px * cos - py * sin,
                position[1] + px * sin + py * cos,
            ];
            let cell = |i: usize| {
                ((world[i] - destination.base[i] - destination.cell_size * 0.5)
                    / destination.cell_size
                    + 0.5)
                    .trunc()
            };
            let (cx, cy) = (cell(0), cell(1));
            if !cx.is_finite()
                || !cy.is_finite()
                || cx >= destination.columns as f32
                || cy >= destination.rows as f32
            {
                continue;
            }
            // Native world-to-cell conversion clamps negative indices to zero
            // (0xa49410 / 0xa49440), including footprints crossing the grid edge.
            let (cx, cy) = (cx.max(0.0), cy.max(0.0));
            let index = cy as u32 * destination.columns + cx as u32;
            if let Some(v) = destination.data.get_mut(index as usize) {
                // Kind access masks the low nibble (0xa49070); writes preserve
                // destination flags in the upper nibble (0xa49550).
                let kind = *v & 0xf;
                let next = if clear {
                    matches!(kind, 7 | 8).then_some(1)
                } else {
                    (kind < 8).then(|| source.data[(y * source.columns + x) as usize] & 0xf)
                };
                let next = next.map(|kind| (*v & 0xf0) | kind);
                if let Some(next) = next.filter(|&next| next != *v) {
                    *v = next;
                    writes.insert(index, next);
                }
            }
        }
    }
    writes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forced_initialization_skips_playback_but_later_state_changes_retain_completion() {
        let (mut s, id) = playback_fixture();
        s.world.tables().insert(
            "#93",
            &["#1deaa1ea", "#9a320a51"],
            vec![(5, vec![Cell::Int(1), Cell::String("controller".into())])],
        );
        s.world.tables().insert(
            "controller",
            &["ID", "Label", "Closed", "Open"],
            vec![
                (
                    10,
                    vec![
                        Cell::Int(10),
                        Cell::String("Closed".into()),
                        Cell::Int(17),
                        Cell::Int(17),
                    ],
                ),
                (
                    20,
                    vec![
                        Cell::Int(20),
                        Cell::String("Open".into()),
                        Cell::Int(17),
                        Cell::Int(17),
                    ],
                ),
            ],
        );
        s.world
            .tables()
            .insert("#0", &["#12a02374"], vec![(17, vec![Cell::Int(0)])]);
        s.world
            .get_mut(id)
            .unwrap()
            .fields
            .insert("InitialState".into(), 1);
        s.init_placeable_state(id);
        let o = s.world.get(id).unwrap();
        assert_eq!(o.fields.get("State"), Some(&1));
        assert!(o.placeable_playback.is_none());
        assert!(
            !o.interactive,
            "initialization still applies controller action admission"
        );

        s.set_placeable_state(id, 0);
        let o = s.world.get(id).unwrap();
        assert_eq!(o.fields.get("State"), Some(&0));
        assert!(o.placeable_playback.as_ref().unwrap().pending);
        let restored: crate::World =
            serde_json::from_str(&serde_json::to_string(&s.world).unwrap()).unwrap();
        assert_eq!(
            restored.get(id).unwrap().placeable_playback,
            o.placeable_playback
        );
    }

    fn playback_fixture() -> (Sim, ObjectId) {
        let mut s = Sim::new(
            Arc::new(thedas_vfs::Vfs::new()),
            Arc::new(thedas_script::EngineTable::parse(b"").unwrap()),
            7,
        );
        s.world.tables().insert(
            "#93",
            &["#1deaa1ea"],
            vec![(5, vec![Cell::Int(1)]), (6, vec![Cell::Int(0)])],
        );
        let id = s.world.spawn(ObjectKind::Placeable, "destructible");
        let o = s.world.get_mut(id).unwrap();
        o.appearance = 5;
        o.fields.insert("Health".into(), 1);
        (s, id)
    }

    fn start(s: &mut Sim, id: ObjectId, action: &str, seconds: f32) {
        let now = s.world.time_ms;
        s.world
            .get_mut(id)
            .unwrap()
            .placeable_playback
            .get_or_insert_default()
            .pending = true;
        s.start_placeable_clip(
            id,
            PlaceableClip {
                tree: "test.anb".into(),
                action: action.into(),
                phase: 0,
                normalized_rate: 65_535.0 / seconds,
                updated_ms: now,
            },
        );
    }

    #[test]
    fn placeable_completion_uses_current_health_and_first_saved_removal_deadline() {
        let (mut s, id) = playback_fixture();
        s.set_placeable_health(id, 0);
        s.tick(100);
        assert!(s.world.origins_destroy_at.is_empty());
        start(&mut s, id, "dying", 0.2);
        s.world.get_mut(id).unwrap().active = false; // Opacity and playback are independent.
        s.tick(100);
        s.tick(100);
        assert!(s.world.origins_destroy_at.is_empty()); // Two truncated half-steps reach 65534.
        s.tick(1);
        assert_eq!(s.world.origins_destroy_at, vec![(3301, id)]);
        assert_eq!(s.world.get(id).unwrap().placeable_fade_at, Some(301));
        assert!(
            !s.world
                .get(id)
                .unwrap()
                .placeable_playback
                .as_ref()
                .unwrap()
                .pending
        );
        // A second callback neither restarts fading nor replaces the deletion time.
        s.tick(50);
        s.placeable_animation_complete(id);
        assert_eq!(s.world.origins_destroy_at, vec![(3301, id)]);
        assert_eq!(s.world.get(id).unwrap().placeable_fade_at, Some(301));
        let json = s.save_json().unwrap();
        s.load_json(&json).unwrap();
        s.tick(2949);
        assert!(s.world.get(id).is_some());
        s.tick(1);
        assert!(s.world.get(id).is_none());
    }

    #[test]
    fn placeable_removal_checks_the_delivery_flag_and_keeps_a_blocked_latch() {
        for (requested_flag, delivered_flag) in [(0, 1), (1, 0)] {
            let (mut s, id) = playback_fixture();
            s.set_placeable_health(id, 0);
            s.world
                .get_mut(id)
                .unwrap()
                .fields
                .insert("IsDestroyable".into(), requested_flag);
            s.placeable_animation_complete(id);
            assert_eq!(s.world.origins_destroy_at, vec![(3000, id)]);
            assert!(s.world.get(id).unwrap().origins_destroy_requested);
            s.world
                .get_mut(id)
                .unwrap()
                .fields
                .insert("IsDestroyable".into(), delivered_flag);
            let save = s.save_json().unwrap();
            s.load_json(&save).unwrap();
            s.tick(3000);
            assert_eq!(s.world.get(id).is_some(), delivered_flag == 0);
            assert!(s.world.origins_destroy_at.is_empty());
            if delivered_flag == 0 {
                s.world
                    .get_mut(id)
                    .unwrap()
                    .fields
                    .insert("IsDestroyable".into(), 1);
                let save = s.save_json().unwrap();
                s.load_json(&save).unwrap();
                assert!(s.world.get(id).unwrap().origins_destroy_requested);
                s.placeable_animation_complete(id);
                assert!(s.world.origins_destroy_at.is_empty());
                s.tick(3000);
                assert!(s.world.get(id).is_some(), "a consumed request cannot retry");
            }
        }
    }

    #[test]
    fn forced_lifetimes_remain_independent_of_native_removal_requests() {
        let (mut s, id) = playback_fixture();
        s.set_placeable_health(id, 0);
        s.world
            .get_mut(id)
            .unwrap()
            .fields
            .insert("IsDestroyable".into(), 0);
        s.placeable_animation_complete(id);
        s.world.queue_destroy(0, id);
        s.world.destroy_due();
        assert!(s.world.get(id).is_none());
    }

    #[test]
    fn placeable_completion_rechecks_appearance_and_healing_and_keeps_prior_requests() {
        for (health, appearance, destroy) in [(1, 5, false), (0, 6, false), (0, 65_541, true)] {
            let (mut s, id) = playback_fixture();
            s.set_placeable_health(id, 0);
            start(&mut s, id, "dying", 1.0);
            s.world.get_mut(id).unwrap().appearance = appearance;
            s.set_placeable_health(id, health);
            s.world.request_origins_destroy(7000, id);
            s.tick(1000);
            assert_eq!(
                s.world.get(id).unwrap().placeable_fade_at.is_some(),
                destroy
            );
            assert_eq!(s.world.origins_destroy_at, vec![(7000, id)]);
        }
    }

    #[test]
    fn placeable_outgoing_nodes_complete_after_replacement_and_reload() {
        let (mut s, id) = playback_fixture();
        start(&mut s, id, "old", 0.2);
        s.tick(100);
        start(&mut s, id, "new", 0.5);
        assert_eq!(
            s.world
                .get(id)
                .unwrap()
                .placeable_playback
                .as_ref()
                .unwrap()
                .clips
                .len(),
            2
        );
        let json = s.save_json().unwrap();
        s.load_json(&json).unwrap();
        s.set_placeable_health(id, 0);
        // Clearing the outstanding byte (e.g. a looping replacement) does not
        // guard the older one-shot node's eventual completion callback.
        s.world
            .get_mut(id)
            .unwrap()
            .placeable_playback
            .as_mut()
            .unwrap()
            .pending = false;
        s.tick(100);
        s.tick(1);
        assert_eq!(s.world.origins_destroy_at, vec![(3201, id)]);
        start(&mut s, id, "new", 0.5); // Same node resets rather than duplicates.
        assert_eq!(
            s.world
                .get(id)
                .unwrap()
                .placeable_playback
                .as_ref()
                .unwrap()
                .clips
                .len(),
            1
        );
        s.tick(500);
        assert_eq!(s.world.origins_destroy_at, vec![(3201, id)]);
    }

    #[test]
    fn placeable_zero_and_missing_transitions_do_not_cancel_outgoing_completion() {
        let (mut s, id) = playback_fixture();
        s.world.tables().insert(
            "#93",
            &["#1deaa1ea", "#9a320a51", "#70f463fa"],
            vec![(
                5,
                vec![
                    Cell::Int(1),
                    Cell::String("controller".into()),
                    Cell::String("absent.anb".into()),
                ],
            )],
        );
        s.world.tables().insert(
            "controller",
            &["Label", "A", "B", "C"],
            vec![
                (
                    10,
                    vec![
                        Cell::String("A".into()),
                        Cell::Int(0),
                        Cell::Int(17),
                        Cell::Int(0),
                    ],
                ),
                (
                    20,
                    vec![
                        Cell::String("B".into()),
                        Cell::Int(0),
                        Cell::Int(17),
                        Cell::Int(0),
                    ],
                ),
                (
                    30,
                    vec![
                        Cell::String("C".into()),
                        Cell::Int(0),
                        Cell::Int(0),
                        Cell::Int(0),
                    ],
                ),
            ],
        );
        s.world.tables().insert(
            "#0",
            &["#12a02374", "#7a398af9"],
            vec![(17, vec![Cell::Int(1), Cell::String("loop".into())])],
        );
        start(&mut s, id, "outgoing", 1.0);
        s.set_placeable_health(id, 0);
        s.set_placeable_state(id, 1); // Missing node uses the table's looping fallback.
        assert!(
            !s.world
                .get(id)
                .unwrap()
                .placeable_playback
                .as_ref()
                .unwrap()
                .pending
        );
        s.world
            .get_mut(id)
            .unwrap()
            .placeable_playback
            .as_mut()
            .unwrap()
            .pending = true;
        s.set_placeable_state(id, -1); // Animation query selects B's diagonal, then state becomes 0.
        let o = s.world.get(id).unwrap();
        assert_eq!(o.fields.get("State"), Some(&0));
        assert!(!o.placeable_playback.as_ref().unwrap().pending);
        s.world
            .get_mut(id)
            .unwrap()
            .placeable_playback
            .as_mut()
            .unwrap()
            .pending = true;
        s.set_placeable_state(id, 2); // Zero transition preserves the outstanding byte.
        assert!(
            s.world
                .get(id)
                .unwrap()
                .placeable_playback
                .as_ref()
                .unwrap()
                .pending
        );
        s.tick(1000);
        assert_eq!(s.world.origins_destroy_at, vec![(4000, id)]);
    }

    #[test]
    fn placeable_playback_preserves_native_large_step_short_and_parked_clock() {
        let (mut s, id) = playback_fixture();
        s.set_placeable_health(id, 0);
        start(&mut s, id, "dying", 1.0);
        s.tick(2000); // Increment 131070 contributes its low unsigned short 65534.
        assert!(s.world.origins_destroy_at.is_empty());
        assert_eq!(
            s.world
                .get(id)
                .unwrap()
                .placeable_playback
                .as_ref()
                .unwrap()
                .clips[0]
                .phase,
            65534
        );
        s.tick(1);
        assert_eq!(s.world.origins_destroy_at, vec![(5001, id)]);
        let (mut s, id) = playback_fixture();
        s.set_placeable_health(id, 0);
        start(&mut s, id, "dying", 1.0);
        s.game = thedas_vfs::Game::DragonAge2;
        s.tick(1000);
        assert!(s.world.origins_destroy_at.is_empty());
        s.game = thedas_vfs::Game::Origins;
        // Refresh clocks on parked ticks so their suspended time is not caught up.
        s.world.get_mut(id).unwrap().parked = Some(false);
        s.tick(1000);
        s.world.get_mut(id).unwrap().parked = None;
        s.tick(1000);
        assert_eq!(s.world.origins_destroy_at, vec![(6000, id)]);
    }

    #[test]
    #[ignore = "requires an Origins THEDAS_INSTALL"]
    fn installed_placeable_completion_uses_the_real_clip_and_preserves_door_loot() {
        let install =
            thedas_vfs::DaoInstall::open(std::env::var_os("THEDAS_INSTALL").unwrap()).unwrap();
        let vfs = Arc::new(
            install
                .global_vfs(thedas_vfs::install::TextureQuality::High)
                .unwrap(),
        );
        let table = Arc::new(
            thedas_script::EngineTable::parse(&vfs.read("script.ldf").unwrap().0).unwrap(),
        );
        let mut s = Sim::new(vfs, table, 7);
        let id = s.world.spawn(ObjectKind::Placeable, "barricade");
        let o = s.world.get_mut(id).unwrap();
        o.appearance = 227;
        o.fields.insert("Health".into(), 1);
        s.set_placeable_health(id, 0);
        assert!(s.world.origins_destroy_at.is_empty());
        s.set_placeable_state(id, 1);
        let playback = s
            .world
            .get(id)
            .unwrap()
            .placeable_playback
            .as_ref()
            .unwrap();
        assert!(playback.pending);
        assert_eq!(playback.clips.len(), 1);
        assert_eq!(playback.clips[0].action, "static_dying");
        // This real GAD's field 4010 is zero; it must not stop playback.
        s.tick(3000);
        let json = s.save_json().unwrap();
        s.load_json(&json).unwrap();
        for _ in 0..300 {
            if !s.world.origins_destroy_at.is_empty() {
                break;
            }
            s.tick(10);
        }
        assert_eq!(
            s.world.origins_destroy_at,
            vec![(s.world.time_ms + 3000, id)]
        );
        assert!((5860..5900).contains(&s.world.time_ms));
        // Ordinary doors/chests do not opt into removal, retaining their contents.
        for appearance in [1, 5] {
            let object = s.world.spawn(ObjectKind::Placeable, "loot_container");
            let item = s.world.spawn(ObjectKind::Item, "loot");
            let o = s.world.get_mut(object).unwrap();
            o.appearance = appearance;
            o.inventory.push(item);
            o.fields.insert("Health".into(), 0);
            s.set_placeable_state(object, 3);
            for _ in 0..100 {
                s.tick(100);
            }
            assert!(s.world.get(object).is_some());
            assert_eq!(s.world.get(object).unwrap().inventory, vec![item]);
        }
    }

    fn grid(columns: u32, rows: u32, data: Vec<u8>) -> NavGrid {
        NavGrid {
            columns,
            rows,
            cell_size: 1.0,
            clearance: 1.0,
            base: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
            data,
            heights: vec![],
            surfaces: vec![],
        }
    }

    #[test]
    fn state_initialization_respects_both_action_slots_without_enabling_scenery() {
        use thedas_script::EngineTable;
        use thedas_vfs::Vfs;
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 7);
        let tables = crate::m2da::Tables::empty();
        tables.insert(
            "#93",
            &["#9a320a51"],
            vec![(0, vec![Cell::String("controller".into())])],
        );
        // Sparse row IDs: runtime state indexes controller records by position.
        tables.insert(
            "controller",
            &[
                "ID",
                "Label",
                "s0",
                "s1",
                "s2",
                "action0",
                "next0",
                "variation0",
                "action1",
                "next1",
                "variation1",
                "#86a27132",
            ],
            [(0, 0, 9), (10, 8, 0), (99, 0, 0)]
                .into_iter()
                .map(|(id, a, b)| {
                    let mut cells = vec![Cell::Int(0); 12];
                    cells[0] = Cell::Int(id);
                    cells[5] = Cell::Int(a);
                    cells[8] = Cell::Int(b);
                    (id, cells)
                })
                .collect(),
        );
        s.world.tables = Arc::new(tables);
        for initial in 0..3 {
            for useable in [false, true] {
                let id = s.world.spawn(ObjectKind::Placeable, "controlled");
                let o = s.world.get_mut(id).unwrap();
                o.interactive = useable;
                o.fields.insert("InitialState".into(), initial);
                s.init_placeable_state(id);
                let o = s.world.get(id).unwrap();
                assert_eq!(o.fields["State"], initial);
                assert_eq!(o.interactive, useable && initial != 2);
            }
        }
    }

    #[test]
    fn walkmesh_install_copies_source_kinds_and_protects_destination_eight_and_above() {
        let source = grid(4, 1, vec![7, 1, 8, 6]);
        let mut destination = grid(4, 2, vec![1, 7, 8, 9, 0, 1, 6, 7]);
        let writes = write_walkmesh_cells(&mut destination, &source, [0.0; 3], 0.0, false);
        assert_eq!(writes, BTreeMap::from([(0, 7), (1, 1)]));
        assert_eq!(&destination.data[..4], &[7, 1, 8, 9]);
        let writes = write_walkmesh_cells(&mut destination, &source, [0.0, 1.0, 0.0], 0.0, false);
        assert_eq!(writes, BTreeMap::from([(4, 7), (6, 8), (7, 6)]));
        assert!(destination.heights.is_empty() && destination.surfaces.is_empty());
        // Several source cells can map to one destination cell. The first
        // protected kind encountered in column-then-row order wins.
        let mut small = grid(2, 2, vec![7, 8, 9, 1]);
        small.cell_size = 0.2;
        let mut destination = grid(1, 1, vec![1]);
        write_walkmesh_cells(&mut destination, &small, [0.0; 3], 0.0, false);
        assert_eq!(destination.data, [9]);
    }

    #[test]
    fn walkmesh_writes_preserve_destination_flags_and_clamp_negative_grid_edges() {
        let source = grid(4, 1, vec![0xf8, 0xa1, 0x17, 0x28]);
        let mut destination = grid(4, 1, vec![0x21, 0x47, 0x68, 0x89]);
        write_walkmesh_cells(&mut destination, &source, [0.0; 3], 0.0, false);
        assert_eq!(destination.data, [0x28, 0x41, 0x68, 0x89]);
        write_walkmesh_cells(&mut destination, &source, [0.0; 3], 0.0, true);
        assert_eq!(destination.data, [0x21, 0x41, 0x61, 0x89]);

        let source = grid(2, 1, vec![8, 7]);
        let mut destination = grid(1, 1, vec![0x31]);
        write_walkmesh_cells(&mut destination, &source, [-2.0, -2.0, 0.0], 0.0, false);
        assert_eq!(destination.data, [0x38]);
    }

    #[test]
    fn legacy_initialization_keeps_explicit_states_and_new_saves_keep_later_changes() {
        use thedas_script::EngineTable;
        use thedas_vfs::{Game, Vfs};
        for game in [Game::Origins, Game::DragonAge2] {
            let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 7);
            s.game = game;
            let untouched = s.world.spawn(ObjectKind::Placeable, "initial_open");
            let changed = s.world.spawn(ObjectKind::Placeable, "saved_closed");
            let creature = s.world.spawn(ObjectKind::Creature, "not_placeable");
            for id in [untouched, changed, creature] {
                s.world
                    .get_mut(id)
                    .unwrap()
                    .fields
                    .extend([("InitialState".into(), 2), ("AnimContState".into(), 0)]);
            }
            s.world
                .get_mut(changed)
                .unwrap()
                .fields
                .insert("State".into(), 1);
            let mut save = s.save_game();
            save.revision = 9;
            s.load_game(save).unwrap();
            assert_eq!(
                s.world.get(untouched).unwrap().fields.get("State"),
                if game == Game::Origins {
                    Some(&2)
                } else {
                    None
                }
            );
            assert_eq!(s.world.get(changed).unwrap().fields["State"], 1);
            assert!(!s.world.get(creature).unwrap().fields.contains_key("State"));
            s.world
                .get_mut(untouched)
                .unwrap()
                .fields
                .insert("State".into(), 0);
            let json = s.save_json().unwrap();
            s.load_json(&json).unwrap();
            assert_eq!(s.world.get(untouched).unwrap().fields["State"], 0);
            assert!(s.world.queue.is_empty());
        }
    }

    #[test]
    fn walkmesh_clear_transforms_cells_and_keeps_other_kinds() {
        let source = grid(3, 1, vec![0, 0, 0]);
        let mut destination = grid(4, 4, vec![7, 8, 9, 1, 8, 6, 7, 1, 7, 8, 7, 1, 1, 1, 1, 1]);
        let writes = clear_walkmesh_cells(&mut destination, &source, [0.0; 3], 0.0);
        assert_eq!(writes, BTreeMap::from([(0, 1), (1, 1)]));
        assert_eq!(&destination.data[..4], &[1, 1, 9, 1]);
        // A quarter turn of the same three local cells hits column 2, rows 0..2.
        let writes = clear_walkmesh_cells(&mut destination, &source, [3.0, 0.0, 0.0], 90.0);
        assert_eq!(writes, BTreeMap::from([(6, 1), (10, 1)]));
        assert!(clear_walkmesh_cells(&mut destination, &source, [100.0; 3], 0.0).is_empty());
        assert_eq!(destination.data[5], 6);
    }

    #[test]
    #[ignore = "requires an Origins THEDAS_INSTALL"]
    fn installed_destructible_walkmeshes_clear_once_and_survive_reload() {
        use thedas_script::EngineTable;
        let install =
            thedas_vfs::DaoInstall::open(std::env::var_os("THEDAS_INSTALL").unwrap()).unwrap();
        let vfs = Arc::new(
            install
                .global_vfs(thedas_vfs::install::TextureQuality::High)
                .unwrap(),
        );
        let table = Arc::new(EngineTable::parse(&vfs.read("script.ldf").unwrap().0).unwrap());
        let mut s = Sim::new(vfs, table, 7);
        for appearance in [227, 367, 368, 404] {
            let area = s.world.spawn(ObjectKind::Area, "walkmesh_area");
            let untouched = s.world.spawn(ObjectKind::Area, "other_instance");
            let layout = format!("fixture_{appearance}");
            for id in [area, untouched] {
                s.area_layouts.insert(id, layout.clone());
            }
            let base = Arc::new(grid(100, 100, vec![7; 10_000]));
            s.nav_grids.insert(layout.clone(), Some(base.clone()));
            let id = s.world.spawn(ObjectKind::Placeable, "barricade");
            let o = s.world.get_mut(id).unwrap();
            o.appearance = appearance;
            o.area = Some(area);
            o.position = [20.0, 20.0, 0.0];
            o.fields.insert("Health".into(), 5);
            o.fields.insert("State".into(), 65_536); // Native state indexes use u16.
            let door = s.is_door(id);
            // Populate the sight cache before destruction, so a stale cache cannot
            // keep the old physics group blocking after the health write.
            if !door {
                assert!(!s.placeable_collision(area).unwrap().is_empty());
            }
            s.set_placeable_health(id, 1);
            assert!(s.world.nav_patches.is_empty());
            assert_eq!(s.world.get(id).unwrap().placeable_group, None);
            s.set_placeable_health(id, 0);
            assert_eq!(s.world.get(id).unwrap().placeable_group, Some(1));
            assert!(s.placeable_collision(area).unwrap().is_empty());
            assert!(s.closed_doors(area).is_empty());
            assert!(s.door_sight(area).solid.is_empty());
            let patches = s
                .world
                .nav_patches
                .get(&area)
                .expect("real PWK must change grid cells");
            assert!(!patches.is_empty());
            let index = *patches.keys().next().unwrap() as usize;
            assert_eq!(s.nav_grid(area).unwrap().data[index], 1);
            assert_eq!(s.nav_grid(untouched).unwrap().data, base.data);
            // A later grid owner can write the cell again. Repeated dead health
            // writes must not replay the first old-state removal over its write.
            let mut replaced = (*s.nav_grid(area).unwrap()).clone();
            replaced.data[index] = 8;
            s.changed_nav_grids.insert(area, Arc::new(replaced));
            s.world
                .nav_patches
                .get_mut(&area)
                .unwrap()
                .insert(index as u32, 8);
            let json = s.save_json().unwrap();
            s.load_json(&json).unwrap();
            s.area_layouts.insert(area, layout.clone());
            s.area_layouts.insert(untouched, layout);
            s.set_placeable_health(id, -1);
            assert_eq!(s.nav_grid(area).unwrap().data[index], 8);
            assert_eq!(s.nav_grid(untouched).unwrap().data, base.data);
            assert_eq!(s.world.get(id).unwrap().placeable_group, Some(1));
            assert!(s.world.origins_destroy_at.is_empty()); // Health alone never starts removal.
            s.world.nav_patches.clear();
            s.changed_nav_grids.clear();
            // Table lookup truncates appearance, but the PWK loader formats the
            // original ASCII Appearance integer. No such suffixed asset exists.
            let o = s.world.get_mut(id).unwrap();
            o.appearance += 65_536;
            o.fields.insert("Health".into(), 5);
            s.set_placeable_health(id, 0);
            assert_eq!(s.world.get(id).unwrap().placeable_group, Some(1));
            assert!(s.world.nav_patches.is_empty());
        }
    }
}
