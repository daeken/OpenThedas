//! Routes physical mouse buttons and distinguishes a ground click from a camera drag.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

/// A gesture keeps its mapping until both buttons are up, even if Options changes it.
#[derive(Default)]
pub struct Buttons {
    held: u8,
    swapped: bool,
}

impl Buttons {
    pub fn edge(&mut self, button: Button, down: bool, swap: bool) -> Option<Button> {
        let bit = match button {
            Button::Left => 1,
            Button::Right => 2,
        };
        if (self.held & bit != 0) == down {
            return None;
        }
        if down {
            if self.held == 0 {
                self.swapped = swap;
            }
            self.held |= bit;
        } else {
            self.held &= !bit;
        }
        Some(match (button, self.swapped) {
            (b, false) => b,
            (Button::Left, true) => Button::Right,
            (Button::Right, true) => Button::Left,
        })
    }
}

/// Provisional: the original's drag threshold has not been traced.
const DRAG_PIXELS: f64 = 4.0;

#[derive(Default)]
pub struct RightClick {
    at: Option<(f64, f64)>,
    travel: f64,
    dragged: bool,
}

impl RightClick {
    pub fn press(&mut self, at: (f64, f64)) {
        *self = Self {
            at: Some(at),
            ..Default::default()
        };
    }

    pub fn cursor_to(&mut self, at: (f64, f64)) {
        if let Some(start) = self.at {
            self.dragged |= (at.0 - start.0).hypot(at.1 - start.1) > DRAG_PIXELS;
        }
    }

    /// Relative device motion still detects dragging if the cursor is confined.
    pub fn motion(&mut self, delta: (f64, f64)) {
        self.travel += delta.0.hypot(delta.1);
        self.dragged |= self.travel > DRAG_PIXELS;
    }

    pub fn release(&mut self) -> Option<(f64, f64)> {
        let press = std::mem::take(self);
        press.at.filter(|_| !press.dragged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swapping_changes_both_edges_and_waits_until_neither_button_is_held() {
        use Button::{Left, Right};
        let mut buttons = Buttons::default();
        assert_eq!(buttons.edge(Left, false, true), None);
        assert_eq!(buttons.edge(Left, true, false), Some(Left));
        assert_eq!(buttons.edge(Left, true, true), None);
        assert_eq!(buttons.edge(Right, true, true), Some(Right));
        assert_eq!(buttons.edge(Left, false, true), Some(Left));
        assert_eq!(buttons.edge(Right, false, true), Some(Right));
        assert_eq!(buttons.edge(Left, true, true), Some(Right));
        assert_eq!(buttons.edge(Left, false, false), Some(Right));
        assert_eq!(buttons.edge(Right, true, true), Some(Left));
        assert_eq!(buttons.edge(Right, false, true), Some(Left));
        assert_eq!(buttons.edge(Right, true, false), Some(Right));
        assert_eq!(buttons.edge(Right, false, false), Some(Right));
    }

    #[test]
    fn a_tap_orders_once_and_an_unpaired_release_never_orders() {
        let mut button = RightClick::default();
        assert_eq!(button.release(), None);
        button.press((100.0, 200.0));
        button.motion((1.0, 1.0));
        button.cursor_to((101.0, 201.0));
        assert_eq!(button.release(), Some((100.0, 200.0)));
        assert_eq!(button.release(), None);
    }

    #[test]
    fn a_drag_cannot_become_an_order_by_returning_to_the_press() {
        let mut button = RightClick::default();
        button.press((100.0, 200.0));
        button.cursor_to((120.0, 200.0));
        button.cursor_to((100.0, 200.0));
        assert_eq!(button.release(), None);
        // A confined cursor still reports a drag through raw device motion.
        button.press((100.0, 200.0));
        for _ in 0..5 {
            button.motion((1.0, 0.0));
        }
        assert_eq!(button.release(), None);
        button.press((100.0, 200.0));
        assert!(button.release().is_some(), "the next tap works");
    }
}
