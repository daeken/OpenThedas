//! Key names, as the game shows the keys bound to its actions: `keymap.gda` binds each
//! action (`RunCharacterForward_0`, its primary binding, and `_1`, its secondary) to an input
//! (`Keyboard::Button_W`, `Shift+Keyboard::Button_A`, `Mouse::WheelUp`), and `inputmap.gda`
//! names each input with a talk-table string by its row: the keyboard's keys from row 1 in
//! DirectInput's key names' alphabetical order (`0` to `9`, `A`, `Abnt_C1`, ...), the mouse's
//! inputs from row 1000. A modifier is written before the key (`Shift + A`).
//!
//! Text in the talk table names keys with designer tags (`<MoveForwardKey/>`), which the
//! game's `ReplaceDesignerTags` (DAOrigins.exe 0x51d620, a switch on the tag's CRC-32)
//! replaces with the key bound to an action, or a mouse button's name ([`TAGS`]).
//!
//! Provisional: the first of `keymap.gda`'s binding columns is taken (every column binds
//! the same keys for the actions here), and the mouse buttons are taken to be unswapped.

/// DirectInput's keyboard key names in `inputmap.gda`'s order (row 1 on).
const KEYBOARD: &[&str] = &[
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "A",
    "Abnt_C1",
    "Abnt_C2",
    "Add",
    "Apostrophe",
    "Apps",
    "AT",
    "AX",
    "B",
    "Back",
    "Backslash",
    "C",
    "Calculator",
    "Capital",
    "Colon",
    "Comma",
    "Convert",
    "D",
    "Decimal",
    "Delete",
    "Divide",
    "Down",
    "E",
    "End",
    "Equals",
    "Escape",
    "F",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
    "F13",
    "F14",
    "F15",
    "G",
    "Grave",
    "H",
    "Home",
    "I",
    "Insert",
    "J",
    "K",
    "Kana",
    "Kanji",
    "L",
    "LBracket",
    "Left",
    "LWin",
    "M",
    "Mail",
    "Mediaselect",
    "Mediastop",
    "Minus",
    "Multiply",
    "Mute",
    "Mycomputer",
    "N",
    "Next",
    "Nexttrack",
    "Noconvert",
    "Numlock",
    "NUMPAD_0",
    "NUMPAD_1",
    "NUMPAD_2",
    "NUMPAD_3",
    "NUMPAD_4",
    "NUMPAD_5",
    "NUMPAD_6",
    "NUMPAD_7",
    "NUMPAD_8",
    "NUMPAD_9",
    "NUMPAD_COMMA",
    "NUMPAD_ENTER",
    "NUMPAD_EQUALS",
    "O",
    "OEM_102",
    "P",
    "Pause",
    "Period",
    "Playpause",
    "Power",
    "Prevtrack",
    "Prior",
    "Q",
    "R",
    "RBracket",
    "Return",
    "Right",
    "RWin",
    "S",
    "Scroll",
    "Semicolon",
    "Slash",
    "Sleep",
    "Space",
    "Stop",
    "Subtract",
    "Sysrq",
    "T",
    "Tab",
    "U",
    "Underline",
    "Unlabeled",
    "Up",
    "V",
    "Volumedown",
    "Volumeup",
    "W",
    "Wake",
    "Webback",
    "Webfavorites",
    "Webforward",
    "Webhome",
    "Webrefresh",
    "Websearch",
    "Webstop",
    "X",
    "Y",
    "Yen",
    "Z",
];

/// The mouse's inputs in `inputmap.gda`'s order (row 1000 on).
const MOUSE: &[&str] = &[
    "LeftClick",
    "RightClick",
    "MiddleClick",
    "Button3",
    "Button4",
    "Button5",
    "Button6",
    "Button7",
    "XAxisDelta",
    "YAxisDelta",
    "WheelUp",
    "WheelDown",
];

/// What a designer tag stands for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tag {
    /// The key bound to an action (its primary binding).
    Action(&'static str),
    /// A talk-table string (the mouse buttons' names, for unswapped buttons).
    Text(u32),
}

/// The designer tags DAOrigins.exe's `ReplaceDesignerTags` knows (0x51d6a7 on), with what
/// each becomes.
pub const TAGS: &[(&str, Tag)] = &[
    ("AreaMapKey", Tag::Action("ToggleMap")),
    ("CameraLeft", Tag::Action("MoveCameraLeft")),
    ("CameraRight", Tag::Action("MoveCameraRight")),
    ("CharacterRecordKey", Tag::Action("ToggleCharacterRecord")),
    ("HighlightKey", Tag::Action("HighlightObjects")),
    ("HoldPositionsKey", Tag::Action("ToggleHoldParty")),
    ("InventoryKey", Tag::Action("ToggleInventory")),
    ("JournalKey", Tag::Action("ToggleJournal")),
    ("MainMenuKey", Tag::Action("ToggleMainMenu")),
    ("MoveBackwardKey", Tag::Action("RunCharacterBackward")),
    ("MoveForwardKey", Tag::Action("RunCharacterForward")),
    ("PartySelectKey", Tag::Action("ToggleFullPartySelect")),
    ("PauseKey", Tag::Action("TogglePause")),
    ("QuickLoadKey", Tag::Action("QuickLoad")),
    ("QuickSaveKey", Tag::Action("QuickSave")),
    ("RunWalkKey", Tag::Action("ToggleRunWalk")),
    ("ScreenshotKey", Tag::Action("Screenshot")),
    ("SelectChar1", Tag::Action("SelectPartyMember1")),
    ("SelectChar2", Tag::Action("SelectPartyMember2")),
    ("SelectChar3", Tag::Action("SelectPartyMember3")),
    ("SelectChar4", Tag::Action("SelectPartyMember4")),
    ("SwapWeaponsKey", Tag::Action("SwapWeaponSets")),
    ("TacticsKey", Tag::Action("ToggleTactics")),
    ("ZoomInMouse", Tag::Action("ZoomIn")),
    ("ZoomOutMouse", Tag::Action("ZoomOut")),
    // left-click, right-click, left-clicking, right-clicking, Left-clicking, Left-click,
    // Right-click, right.
    ("LeftClickDefault", Tag::Text(399132)),
    ("RightClickDefault", Tag::Text(399131)),
    ("LeftClickingDefault", Tag::Text(399134)),
    ("RightClickingDefault", Tag::Text(399133)),
    ("LeftClickingDefaultCapped", Tag::Text(399136)),
    ("LeftClickDefaultCapped", Tag::Text(330711)),
    ("RightClickDefaultCapped", Tag::Text(330712)),
    ("RightButtonDefault", Tag::Text(399137)),
];

/// "No key mapped": an action with no binding.
pub const NO_KEY: u32 = 399143;

/// The talk-table strings for the modifiers written before a key.
const MODIFIERS: [(&str, &str); 3] = [
    ("Control+", "Control + "),
    ("Alt+", "Alt + "),
    ("Shift+", "Shift + "),
];

/// `inputmap.gda`'s row for an input (`Keyboard::Button_W`, `Mouse::WheelUp`).
pub fn input_row(input: &str) -> Option<i64> {
    if let Some(key) = input.strip_prefix("Keyboard::Button_") {
        return KEYBOARD
            .iter()
            .position(|k| *k == key)
            .map(|i| i as i64 + 1);
    }
    if let Some(m) = input.strip_prefix("Mouse::") {
        return MOUSE.iter().position(|k| *k == m).map(|i| i as i64 + 1000);
    }
    None
}

/// The name of a binding (`Shift+Keyboard::Button_A`): its modifiers, then the input's name
/// (`name` answers an `inputmap.gda` row).
pub fn binding_name(binding: &str, name: impl Fn(i64) -> Option<String>) -> Option<String> {
    let mut rest = binding.trim().trim_end_matches('\0');
    let mut out = String::new();
    'outer: loop {
        for (prefix, written) in MODIFIERS {
            if let Some(r) = rest.strip_prefix(prefix) {
                out.push_str(written);
                rest = r;
                continue 'outer;
            }
        }
        break;
    }
    if rest.is_empty() {
        return None;
    }
    out.push_str(&name(input_row(rest)?)?);
    Some(out)
}

/// Replaces the key tags in `text` (see [`TAGS`]): `action` answers the key bound to an
/// action, `string` a talk-table string. Other tags are left for the caller.
pub fn replace_key_tags(
    text: &str,
    action: impl Fn(&str) -> Option<String>,
    string: impl Fn(u32) -> Option<String>,
) -> String {
    if !text.contains("/>") {
        return text.to_string();
    }
    let mut out = text.to_string();
    for (tag, what) in TAGS {
        let full = format!("<{tag}/>");
        if !out.contains(&full) {
            continue;
        }
        let with = match what {
            Tag::Action(a) => action(a).or_else(|| string(NO_KEY)),
            Tag::Text(s) => string(*s),
        }
        .unwrap_or_default();
        out = out.replace(&full, &with);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_have_inputmap_rows() {
        assert_eq!(input_row("Keyboard::Button_0"), Some(1));
        assert_eq!(input_row("Keyboard::Button_A"), Some(11));
        assert_eq!(input_row("Keyboard::Button_W"), Some(126));
        assert_eq!(input_row("Keyboard::Button_Z"), Some(138));
        assert_eq!(input_row("Mouse::WheelDown"), Some(1011));
        assert_eq!(input_row("Gamepad::Start"), None);
        assert_eq!(KEYBOARD.len(), 138);
    }

    #[test]
    fn bindings_name_their_modifiers_and_key() {
        let name = |r: i64| Some(format!("row{r}"));
        assert_eq!(
            binding_name("Shift+Keyboard::Button_A\0", name).as_deref(),
            Some("Shift + row11")
        );
        assert_eq!(binding_name("", name), None);
    }

    #[test]
    fn key_tags_become_keys_and_click_names() {
        let action = |a: &str| (a == "RunCharacterForward").then(|| "W".to_string());
        let string = |s: u32| Some(format!("s{s}"));
        assert_eq!(
            replace_key_tags(
                "Use <MoveForwardKey/> or <RightClickDefault/>; <PauseKey/>. <FirstName/>",
                action,
                string
            ),
            "Use W or s399131; s399143. <FirstName/>"
        );
    }
}
