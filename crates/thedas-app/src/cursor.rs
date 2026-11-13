//! The mouse cursor: the game's own cursor images and which one shows.
//!
//! DAOrigins.exe loads every `.cur` under `packages/core/data/cursors` by name at start
//! (0x4158c0) and sets one by name each frame. Its rule while exploring (0x70f600) runs in
//! this order: the camera being panned; the interface having the mouse (then the movie's own
//! cursor, which the movies ask for with the `SetCursor` fscommand: `standard`, `ibeam` and
//! their `_pressed` forms); an ability being aimed (`target_creature`, `target_circle`,
//! `target_cone`, `_invalid` over a bad target); the object under the mouse (0x70cda0); else
//! `standard`, or `invalid_move` off the walkable ground.
//!
//! For the object under the mouse (0x70cda0): a hostile creature shows `attack`; one with a
//! conversation `converse`; a lootable body `chest`; a party member `standard`; a usable
//! placeable the cursor its action names in `placeable_interactions.gda` (M2DA 100, the
//! column hashed 0x4f6f860f: `open`, `close`, `unlock`, `use`, `dialog`, `chest`,
//! `area_transition`, ...). Any of those but `standard` takes `_pressed` while the action
//! button is held (the exe's `Pre_PerformAction` control).
//!
//! Provisional: the left button stands in for `Pre_PerformAction` (its binding is not in
//! keymap.gda and is not traced); the camera's pan cursors, `examine` for a creature with
//! +0xc94 set, `chest` over a body, `disabled_unlock` for a lock the party cannot open,
//! `invalid_move` and the cone and lock-on aiming cursors are not shown (UI-159). While the
//! right button turns the camera the cursor is hidden, as before.

use std::collections::HashMap;
use std::path::Path;

use winit::event_loop::ActiveEventLoop;
use winit::window::{CustomCursor, Window};

/// What the mouse is over in the world, as far as the cursor cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Over {
    /// A creature the leader would attack.
    Hostile,
    /// A creature the leader would talk to.
    Talker,
    /// A living creature a click does nothing to, or a party member.
    Creature,
    /// A usable placeable, with the cursor its action names (empty when it names none).
    Placeable(String),
}

/// An ability waiting for its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aim {
    /// The ability is aimed at a place on the ground, not at a creature.
    pub place: bool,
}

/// The cursor's name. `ui` is what the movies last asked for; `covered` says the interface
/// has the mouse; `over` is what the mouse is over in the world (only read when the
/// interface does not have it); `pressed` is the action button held.
pub fn name(
    ui: &str,
    covered: bool,
    aim: Option<Aim>,
    over: Option<&Over>,
    pressed: bool,
) -> String {
    let ui = if ui.is_empty() { "standard" } else { ui };
    if covered {
        return match aim {
            Some(_) => "standard_targeting".to_string(),
            None => ui.to_string(),
        };
    }
    let base = match (aim, over) {
        (Some(Aim { place: true }), _) => "target_circle",
        (Some(_), Some(Over::Hostile | Over::Talker | Over::Creature)) => "target_creature",
        (Some(_), _) => "target_creature_invalid",
        (None, Some(Over::Hostile)) => "attack",
        (None, Some(Over::Talker)) => "converse",
        (None, Some(Over::Placeable(c))) if !c.is_empty() => c.as_str(),
        // Nothing to act on: the movies' cursor (they put it back to `standard` themselves).
        (None, _) => return ui.to_string(),
    };
    if pressed && base != "standard" {
        format!("{base}_pressed")
    } else {
        base.to_string()
    }
}

/// The game's cursors, ready for the window.
pub struct Cursors {
    by_name: HashMap<String, CustomCursor>,
    /// The cursor the window shows now (empty: the system's).
    shown: String,
}

impl Cursors {
    /// Reads every `.cur` in the install's cursor folder. A file that cannot be read or
    /// decoded is left out (and said so); with none, the system cursor stays.
    pub fn load(event_loop: &ActiveEventLoop, install: &Path) -> Cursors {
        let mut by_name = HashMap::new();
        let dir = thedas_vfs::join_ci(install, "packages/core/data/cursors");
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("cur"))
            {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let made = std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|d| thedas_formats::cur::Cursor::parse(&d).map_err(|e| e.to_string()))
                .and_then(|c| {
                    CustomCursor::from_rgba(c.rgba, c.width, c.height, c.hotspot.0, c.hotspot.1)
                        .map_err(|e| e.to_string())
                });
            match made {
                Ok(source) => {
                    by_name.insert(
                        stem.to_ascii_lowercase(),
                        event_loop.create_custom_cursor(source),
                    );
                }
                Err(e) => log::warn!("cursor {}: {e}", path.display()),
            }
        }
        log::info!("cursors: {} from {}", by_name.len(), dir.display());
        Cursors {
            by_name,
            shown: String::new(),
        }
    }

    /// The cursor that stands for `name`: itself, else without `_pressed`, else `standard`.
    /// `None` when not even that was loaded.
    fn resolve(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        let plain = name.strip_suffix("_pressed").unwrap_or(&name);
        [name.as_str(), plain, "standard"]
            .into_iter()
            .find_map(|n| self.by_name.get_key_value(n).map(|(k, _)| k.as_str()))
    }

    /// Shows the cursor named (see [`Cursors::resolve`]); nothing when it already shows.
    pub fn show(&mut self, window: &Window, name: &str) {
        let Some(found) = self.resolve(name).map(str::to_string) else {
            return;
        };
        if found == self.shown {
            return;
        }
        if let Some(c) = self.by_name.get(&found) {
            window.set_cursor(c.clone());
            self.shown = found;
        }
    }

    /// The cursor the window shows (empty before the first is set, or with none loaded).
    pub fn shown(&self) -> &str {
        &self.shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_follows_what_the_mouse_is_over() {
        let door = Over::Placeable("open".into());
        let none = Over::Placeable(String::new());
        // Over the interface the movies' cursor shows, whatever is behind it.
        assert_eq!(
            name("ibeam", true, None, Some(&Over::Hostile), false),
            "ibeam"
        );
        assert_eq!(name("", true, None, None, true), "standard");
        // In the world: by what a click would do.
        assert_eq!(
            name("standard", false, None, Some(&Over::Hostile), false),
            "attack"
        );
        assert_eq!(
            name("standard", false, None, Some(&Over::Talker), false),
            "converse"
        );
        assert_eq!(name("standard", false, None, Some(&door), false), "open");
        assert_eq!(
            name("standard", false, None, Some(&Over::Creature), false),
            "standard"
        );
        assert_eq!(
            name("standard", false, None, Some(&none), false),
            "standard"
        );
        assert_eq!(name("standard", false, None, None, false), "standard");
        // The action button held presses every cursor but the standard one.
        assert_eq!(
            name("standard", false, None, Some(&Over::Hostile), true),
            "attack_pressed"
        );
        assert_eq!(
            name("standard", false, None, Some(&door), true),
            "open_pressed"
        );
        assert_eq!(
            name("standard", false, None, Some(&Over::Creature), true),
            "standard"
        );
        // The movies press their own.
        assert_eq!(
            name("standard_pressed", false, None, None, true),
            "standard_pressed"
        );
    }

    #[test]
    fn an_ability_being_aimed_has_its_own_cursors() {
        let at_creature = Some(Aim { place: false });
        let at_place = Some(Aim { place: true });
        assert_eq!(
            name("standard", true, at_creature, None, false),
            "standard_targeting"
        );
        assert_eq!(
            name("standard", false, at_creature, Some(&Over::Hostile), false),
            "target_creature"
        );
        assert_eq!(
            name("standard", false, at_creature, None, false),
            "target_creature_invalid"
        );
        assert_eq!(
            name(
                "standard",
                false,
                at_creature,
                Some(&Over::Placeable("open".into())),
                true
            ),
            "target_creature_invalid_pressed"
        );
        assert_eq!(
            name("standard", false, at_place, None, true),
            "target_circle_pressed"
        );
    }
}
