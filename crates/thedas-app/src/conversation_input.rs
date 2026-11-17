//! A conversation only takes pointer input whose press began on that screen.

#[derive(Default)]
pub(crate) struct ConversationPress {
    pub(crate) up: bool,
    held: bool,
    eligible: bool,
}

impl ConversationPress {
    /// Opening, closing or replacing the screen invalidates an earlier press.
    pub(crate) fn screen(&mut self, up: bool, replaced: bool) {
        if self.up != up || replaced {
            self.eligible = false;
        }
        self.up = up;
    }

    /// Whether this edge may reach the movies. Other screens retain their input rules.
    pub(crate) fn button(&mut self, down: bool) -> bool {
        if down && !self.held {
            self.eligible = self.up;
        }
        let accept = !self.up || self.eligible;
        self.held = down;
        if !down {
            self.eligible = false;
        }
        accept
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_talk_press_cannot_release_into_the_conversation() {
        let mut press = ConversationPress::default();
        assert!(press.button(true));
        press.screen(true, false);
        assert!(!press.button(false));
        // A subsequent deliberate click works normally.
        assert!(press.button(true));
        assert!(press.button(false));
    }

    #[test]
    fn a_repeated_down_does_not_turn_a_carried_press_into_a_new_click() {
        let mut press = ConversationPress::default();
        assert!(press.button(true));
        press.screen(true, false);
        assert!(!press.button(true));
        assert!(!press.button(false));
        assert!(press.button(true));
    }

    #[test]
    fn replacing_a_conversation_invalidates_its_held_press() {
        let mut press = ConversationPress::default();
        press.screen(true, false);
        assert!(press.button(true));
        press.screen(true, true);
        assert!(!press.button(false));
        assert!(press.button(true));
        press.screen(false, false);
        press.screen(true, false);
        assert!(!press.button(false));
    }

    #[test]
    fn an_unmatched_release_is_ignored_only_on_the_conversation() {
        let mut press = ConversationPress::default();
        assert!(press.button(false));
        press.screen(true, false);
        assert!(!press.button(false));
        assert!(press.button(true));
        // An ordinary frame on the same screen preserves the press.
        press.screen(true, false);
        assert!(press.button(false));
    }
}
