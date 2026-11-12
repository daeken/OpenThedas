//! Containers the player loots: a placeable's `OpenInventory` opens its inventory on the
//! inventory screen (inventory.gfx's container panel, `GUIInventory.ShowContainer`), where
//! the player takes items one at a time (`Container.AcquireItem <item>`) or all of them
//! (`SelectedChar.AcquireAllItems <container>`) and closes it
//! (`Container.InventoryClosedMessage`).
//!
//! Provisional, from inventory.gfx's container panel: the messages' arguments are read from
//! the movie; taken items go to the hero (the party shares one backpack); the container
//! takes nothing back.

use thedas_script::vm::ObjectId;

use crate::sim::Sim;

/// Take one item (its id) from the open container.
pub const MSG_ACQUIRE: &str = "Container.AcquireItem";
/// Take everything from a container (its id).
pub const MSG_ACQUIRE_ALL: &str = "SelectedChar.AcquireAllItems";
/// The container panel closed.
pub const MSG_CLOSED: &str = "Container.InventoryClosedMessage";

impl Sim {
    /// `OpenInventory(container, ...)`: the container's inventory opens for looting.
    pub fn open_container(&mut self, container: ObjectId) {
        self.world.container = self.world.get(container).map(|_| container);
    }

    /// Closes the container panel.
    pub fn close_container(&mut self) {
        self.world.container = None;
    }

    /// Moves an item of the open container to the party.
    pub fn take_from_container(&mut self, item: ObjectId) -> bool {
        let (Some(c), Some(hero)) = (self.world.container, self.world.hero) else {
            return false;
        };
        if !self
            .world
            .get(c)
            .is_some_and(|o| o.inventory.contains(&item))
        {
            return false;
        }
        self.give_item(item, hero)
    }

    /// The container panel's messages, as player input (see `MSG_*`).
    pub(crate) fn container_message(&mut self, name: &str, args: &[String]) -> bool {
        let id = || {
            args.first()
                .and_then(|a| a.trim().parse::<f64>().ok())
                .filter(|v| *v >= 0.0 && v.fract() == 0.0)
                .map(|v| v as ObjectId)
        };
        match name {
            MSG_ACQUIRE => id().is_some_and(|i| self.take_from_container(i)),
            MSG_ACQUIRE_ALL => {
                let Some(c) = self
                    .world
                    .container
                    .filter(|&c| id().is_none_or(|i| i == c))
                else {
                    return false;
                };
                let items = self
                    .world
                    .get(c)
                    .map(|o| o.inventory.clone())
                    .unwrap_or_default();
                for i in items {
                    self.take_from_container(i);
                }
                true
            }
            MSG_CLOSED => {
                self.close_container();
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use super::*;
    use crate::world::ObjectKind;

    #[test]
    fn a_container_is_looted_one_item_or_all_and_closed() {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        let chest = s.world.spawn(ObjectKind::Placeable, "chest");
        let items: Vec<ObjectId> = ["armour", "helm", "boots"]
            .iter()
            .map(|t| s.world.spawn(ObjectKind::Item, t))
            .collect();
        s.world.get_mut(chest).unwrap().inventory = items.clone();
        let stray = s.world.spawn(ObjectKind::Item, "stray");
        // Nothing to take while no container is open.
        assert!(!s.container_message(MSG_ACQUIRE, &[items[0].to_string()]));
        s.open_container(chest);
        assert_eq!(s.world.container, Some(chest));
        assert!(!s.container_message(MSG_ACQUIRE, &[stray.to_string()]));
        assert!(s.container_message(MSG_ACQUIRE, &[items[0].to_string()]));
        assert_eq!(s.world.get(hero).unwrap().inventory, vec![items[0]]);
        assert!(s.container_message(MSG_ACQUIRE_ALL, &[chest.to_string()]));
        assert!(s.world.get(chest).unwrap().inventory.is_empty());
        assert_eq!(s.world.get(hero).unwrap().inventory.len(), 3);
        assert!(s.container_message(MSG_CLOSED, &[]));
        assert_eq!(s.world.container, None);
    }
}
