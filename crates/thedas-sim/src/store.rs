//! Merchants: a store's stock (`.utm` `StoreList`), its prices and buying and selling.
//!
//! Provisional, inferred from the data and `script.ldf`: an item's value is its template's
//! `Cost` (coppers) times its stack size; a store sells at `MarkUp` percent of the value
//! and buys at `MarkDown` percent, and refuses to pay more than its maximum buy price
//! (`-1` for no limit). A stock entry marked `Infinite` never runs out. The party's money is
//! the hero's.

use thedas_formats::Gff3;
use thedas_script::vm::ObjectId;

use crate::sim::{Sim, int, list_of, resref_field};

/// Why a purchase or sale did not happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TradeError {
    NoStore,
    NotForSale,
    NotYours,
    Plot,
    TooPoor { price: i32, money: i32 },
}

impl std::fmt::Display for TradeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TradeError::NoStore => write!(f, "no such store"),
            TradeError::NotForSale => write!(f, "the store does not have that item"),
            TradeError::NotYours => write!(f, "the party does not have that item"),
            TradeError::Plot => write!(f, "plot items cannot be sold"),
            TradeError::TooPoor { price, money } => {
                write!(f, "costs {price} coppers, the buyer has {money}")
            }
        }
    }
}

impl Sim {
    /// Fills a store from its template's `StoreList` (each entry's `ItemList`).
    pub(crate) fn load_store_items(&mut self, store: ObjectId, t: &Gff3) {
        let Some(r) = t.structs.first() else { return };
        let mut stock = Vec::new();
        for &page in list_of(r, "StoreList") {
            let Some(p) = t.structs.get(page as usize) else {
                continue;
            };
            for &e in list_of(p, "ItemList") {
                let Some(e) = t.structs.get(e as usize) else {
                    continue;
                };
                if let Some(res) = resref_field(e, "InventoryRes") {
                    stock.push((
                        res,
                        int(e, "StackSize").unwrap_or(1).max(1),
                        int(e, "Infinite").unwrap_or(0) != 0,
                    ));
                }
            }
        }
        for (res, stack, infinite) in stock {
            let item = self.create_item(&res);
            if let Some(o) = self.world.get_mut(item) {
                o.fields.insert("StackSize".into(), stack);
                if infinite {
                    o.fields.insert("Infinite".into(), 1);
                }
            }
            if let Some(s) = self.world.get_mut(store) {
                s.inventory.push(item);
            }
        }
    }

    /// An item's value in coppers (`GetItemValue`): its cost times its stack.
    pub fn item_value(&self, item: ObjectId) -> i32 {
        self.world.get(item).map_or(0, |o| {
            let cost = o.fields.get("Cost").copied().unwrap_or(0).max(0);
            let stack = o.fields.get("StackSize").copied().unwrap_or(1).max(1);
            cost.saturating_mul(stack)
        })
    }

    fn store_field(&self, store: ObjectId, name: &str, default: i32) -> i32 {
        self.world
            .get(store)
            .and_then(|s| s.fields.get(name).copied())
            .unwrap_or(default)
    }

    /// What the store asks for an item.
    pub fn buy_price(&self, store: ObjectId, item: ObjectId) -> i32 {
        let up = self.store_field(store, "MarkUp", 100).max(0) as i64;
        (self.item_value(item) as i64 * up / 100).clamp(0, i32::MAX as i64) as i32
    }

    /// What the store pays for an item.
    pub fn sell_price(&self, store: ObjectId, item: ObjectId) -> i32 {
        let down = self.store_field(store, "MarkDown", 100).max(0) as i64;
        let p = (self.item_value(item) as i64 * down / 100).clamp(0, i32::MAX as i64) as i32;
        match self.store_field(store, "MaxBuyPrice", -1) {
            max if max >= 0 => p.min(max),
            _ => p,
        }
    }

    /// The hero buys an item from a store. Returns the item the party now has (a copy for
    /// stock that never runs out).
    pub fn store_buy(&mut self, store: ObjectId, item: ObjectId) -> Result<ObjectId, TradeError> {
        let s = self.world.get(store).ok_or(TradeError::NoStore)?;
        if !s.inventory.contains(&item) {
            return Err(TradeError::NotForSale);
        }
        let hero = self.world.hero.ok_or(TradeError::NoStore)?;
        let price = self.buy_price(store, item);
        let money = self.world.get(hero).map_or(0, |h| h.money);
        if money < price {
            return Err(TradeError::TooPoor { price, money });
        }
        let infinite = self
            .world
            .get(item)
            .is_some_and(|o| o.fields.get("Infinite").copied().unwrap_or(0) != 0);
        let bought = if infinite {
            let (template, mut fields) = self
                .world
                .get(item)
                .map(|o| (o.template.clone(), o.fields.clone()))
                .unwrap_or_default();
            fields.remove("Infinite");
            let copy = self.create_item(&template);
            if let Some(o) = self.world.get_mut(copy) {
                o.fields = fields;
            }
            copy
        } else {
            item
        };
        self.give_item(bought, hero);
        if let Some(h) = self.world.get_mut(hero) {
            h.money -= price;
        }
        let till = self.store_field(store, "StoreMoney", -1);
        if till >= 0 {
            if let Some(s) = self.world.get_mut(store) {
                s.fields
                    .insert("StoreMoney".into(), till.saturating_add(price));
            }
        }
        Ok(bought)
    }

    /// The party sells an item (carried by the hero or a follower) to a store. Returns the
    /// price paid.
    pub fn store_sell(&mut self, store: ObjectId, item: ObjectId) -> Result<i32, TradeError> {
        if self.world.get(store).is_none() {
            return Err(TradeError::NoStore);
        }
        let hero = self.world.hero.ok_or(TradeError::NoStore)?;
        let mut party = vec![hero];
        party.extend(
            self.world
                .get(hero)
                .map(|h| h.followers.clone())
                .unwrap_or_default(),
        );
        if !party.iter().any(|p| {
            self.world
                .get(*p)
                .is_some_and(|o| o.inventory.contains(&item))
        }) {
            return Err(TradeError::NotYours);
        }
        if self.world.get(item).is_some_and(|o| o.plot) {
            return Err(TradeError::Plot);
        }
        let mut price = self.sell_price(store, item);
        let till = self.store_field(store, "StoreMoney", -1);
        if till >= 0 {
            price = price.min(till);
        }
        self.give_item(item, store);
        if let Some(h) = self.world.get_mut(hero) {
            h.money = h.money.saturating_add(price);
        }
        if till >= 0 {
            if let Some(s) = self.world.get_mut(store) {
                s.fields.insert("StoreMoney".into(), till - price);
            }
        }
        Ok(price)
    }

    /// The store screen closed (its close button): the store is no longer open.
    pub fn close_store(&mut self) {
        self.world.store = None;
    }

    /// store.gfx's trades, as player input: `Store.BuyItem` (buyer, item, quantity) and
    /// `Store.SellItem` (seller, item, quantity) with the open store. Provisional, from the
    /// movie's `BuyItem`/`SellItem`: a stack is traded whole (the quantity is not used).
    pub(crate) fn store_message(&mut self, name: &str, args: &[String]) -> bool {
        let Some(store) = self.world.store else {
            return false;
        };
        let item = args
            .get(1)
            .and_then(|a| a.trim().parse::<f64>().ok())
            .filter(|v| v.fract() == 0.0 && *v >= 0.0 && *v < u32::MAX as f64)
            .map(|v| v as ObjectId);
        let Some(item) = item else { return false };
        match name {
            MSG_BUY => self.store_buy(store, item).is_ok(),
            MSG_SELL => self.store_sell(store, item).is_ok(),
            _ => false,
        }
    }
}

/// store.gfx's purchase: buyer, item, quantity.
pub const MSG_BUY: &str = "Store.BuyItem";
/// store.gfx's sale: seller, item, quantity.
pub const MSG_SELL: &str = "Store.SellItem";

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use crate::world::ObjectKind;

    use super::*;

    fn setup() -> (Sim, ObjectId, ObjectId, ObjectId) {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        let store = s.world.spawn(ObjectKind::Store, "store");
        let item = s.world.spawn(ObjectKind::Item, "helm");
        {
            let st = s.world.get_mut(store).unwrap();
            st.fields.insert("MarkUp".into(), 150);
            st.fields.insert("MarkDown".into(), 25);
            st.inventory.push(item);
        }
        s.world
            .get_mut(item)
            .unwrap()
            .fields
            .insert("Cost".into(), 1000);
        (s, hero, store, item)
    }

    #[test]
    fn buying_and_selling_move_items_and_money() {
        let (mut s, hero, store, item) = setup();
        assert_eq!(s.buy_price(store, item), 1500);
        assert_eq!(
            s.store_buy(store, item),
            Err(TradeError::TooPoor {
                price: 1500,
                money: 0
            })
        );
        s.world.get_mut(hero).unwrap().money = 2000;
        assert_eq!(s.store_buy(store, item), Ok(item));
        assert_eq!(s.world.get(hero).unwrap().money, 500);
        assert!(s.world.get(hero).unwrap().inventory.contains(&item));
        assert!(!s.world.get(store).unwrap().inventory.contains(&item));
        assert_eq!(s.store_sell(store, item), Ok(250));
        assert_eq!(s.world.get(hero).unwrap().money, 750);
        assert_eq!(s.store_sell(store, item), Err(TradeError::NotYours));
    }

    #[test]
    fn infinite_stock_sells_copies_and_limits_apply() {
        let (mut s, hero, store, item) = setup();
        s.world
            .get_mut(item)
            .unwrap()
            .fields
            .insert("Infinite".into(), 1);
        s.world.get_mut(hero).unwrap().money = 10_000;
        let a = s.store_buy(store, item).unwrap();
        assert_ne!(a, item);
        assert!(s.world.get(store).unwrap().inventory.contains(&item));
        s.world
            .get_mut(store)
            .unwrap()
            .fields
            .insert("MaxBuyPrice".into(), 100);
        assert_eq!(s.store_sell(store, a), Ok(100));
        let plot = s.world.spawn(ObjectKind::Item, "key");
        s.world.get_mut(plot).unwrap().plot = true;
        s.world.get_mut(hero).unwrap().inventory.push(plot);
        assert_eq!(s.store_sell(store, plot), Err(TradeError::Plot));
    }

    #[test]
    fn the_store_screens_trades_go_through_the_open_store() {
        let (mut s, hero, store, item) = setup();
        let args =
            |who: ObjectId, what: ObjectId| vec![who.to_string(), what.to_string(), "1".into()];
        s.world.get_mut(hero).unwrap().money = 2000;
        assert!(!s.gui_message(MSG_BUY, &args(hero, item)), "no store open");
        s.world.store = Some(store);
        assert!(s.gui_message(MSG_BUY, &args(hero, item)));
        assert!(s.world.get(hero).unwrap().inventory.contains(&item));
        assert!(!s.gui_message(MSG_BUY, &args(hero, item)), "sold out");
        assert!(s.gui_message(MSG_SELL, &args(hero, item)));
        assert_eq!(s.world.get(hero).unwrap().money, 750);
        s.close_store();
        assert_eq!(s.world.store, None);
    }
}
