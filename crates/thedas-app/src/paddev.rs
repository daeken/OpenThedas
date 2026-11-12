//! The game controller device, through `gilrs`: on macOS it reads pads through IOKit's HID
//! manager and maps them with the SDL controller database it bundles, which knows the Xbox
//! Elite Series 2 by USB and by Bluetooth. Pads come and go while the game runs; the one
//! used last drives the game. Connections are always logged; `THEDAS_PAD_LOG=1` also logs
//! every button and axis event as gilrs reports it.
//!
//! A deliberate addition: the PC original has no controller support (see [`crate::pad`]).

use gilrs::{Axis, Button as G, EventType, GamepadId, Gilrs};
use glam::Vec2;

use crate::pad::{Button, PadState};

/// A trigger pulled this far (of 1) counts as pressed.
pub const TRIGGER_PRESS: f32 = 0.3;

pub struct Pads {
    gilrs: Gilrs,
    /// The pad that drives the game: the one used last.
    active: Option<GamepadId>,
    /// Log every event (`THEDAS_PAD_LOG`).
    raw: bool,
}

/// The pad buttons as gilrs names them (gilrs's `LeftTrigger` is the bumper,
/// `LeftTrigger2` the trigger; `Select` is Back/View, `Start` is Start/Menu).
const BUTTONS: [(G, Button); 14] = [
    (G::South, Button::A),
    (G::East, Button::B),
    (G::West, Button::X),
    (G::North, Button::Y),
    (G::LeftTrigger, Button::LB),
    (G::RightTrigger, Button::RB),
    (G::Select, Button::Back),
    (G::Start, Button::Start),
    (G::LeftThumb, Button::LS),
    (G::RightThumb, Button::RS),
    (G::DPadUp, Button::Up),
    (G::DPadDown, Button::Down),
    (G::DPadLeft, Button::Left),
    (G::DPadRight, Button::Right),
];

impl Pads {
    /// Starts watching for pads; `None` (logged) when the platform has no support.
    pub fn new() -> Option<Self> {
        let raw = std::env::var_os("THEDAS_PAD_LOG").is_some_and(|v| v != "0" && !v.is_empty());
        let gilrs = match Gilrs::new() {
            Ok(g) => g,
            Err(gilrs::Error::NotImplemented(g)) => {
                log::warn!("pad: controllers are not supported on this platform");
                g
            }
            Err(e) => {
                log::warn!("pad: no controller support: {e}");
                return None;
            }
        };
        let mut pads = Self {
            gilrs,
            active: None,
            raw,
        };
        let present: Vec<GamepadId> = pads.gilrs.gamepads().map(|(id, _)| id).collect();
        for id in present {
            pads.log_connected(id, "present");
            pads.active.get_or_insert(id);
        }
        if pads.active.is_none() {
            log::info!("pad: no controller yet (connect one at any time)");
        }
        Some(pads)
    }

    fn log_connected(&self, id: GamepadId, how: &str) {
        let g = self.gilrs.gamepad(id);
        let uuid: String = g.uuid().iter().map(|b| format!("{b:02x}")).collect();
        log::info!(
            "pad {id}: {how}: {:?} (os name {:?}, uuid {uuid}, vendor {:04x?} product {:04x?}, mapping {:?}, power {:?})",
            g.name(),
            g.os_name(),
            g.vendor_id(),
            g.product_id(),
            g.mapping_source(),
            g.power_info()
        );
    }

    /// Takes the pads' events and returns the active pad's state.
    pub fn poll(&mut self) -> PadState {
        while let Some(ev) = self.gilrs.next_event() {
            match ev.event {
                EventType::Connected => {
                    self.log_connected(ev.id, "connected");
                    self.active.get_or_insert(ev.id);
                }
                EventType::Disconnected => {
                    log::info!("pad {}: disconnected", ev.id);
                    if self.active == Some(ev.id) {
                        self.active = self.gilrs.gamepads().map(|(id, _)| id).next();
                    }
                }
                EventType::Dropped | EventType::ForceFeedbackEffectCompleted => {}
                other => {
                    if self.raw {
                        log::info!("pad {}: {other:?}", ev.id);
                    }
                    if self.active != Some(ev.id) {
                        log::info!("pad {}: now driving the game", ev.id);
                        self.active = Some(ev.id);
                    }
                }
            }
        }
        let Some(g) = self.active.and_then(|id| self.gilrs.connected_gamepad(id)) else {
            return PadState::default();
        };
        let mut s = PadState::default();
        for (gb, b) in BUTTONS {
            s.set(b, g.is_pressed(gb));
        }
        let pulled = |b: G| g.button_data(b).is_some_and(|d| d.value() >= TRIGGER_PRESS);
        s.set(Button::LT, pulled(G::LeftTrigger2));
        s.set(Button::RT, pulled(G::RightTrigger2));
        // gilrs's sticks are up-positive, as PadState's.
        s.left = Vec2::new(g.value(Axis::LeftStickX), g.value(Axis::LeftStickY));
        s.right = Vec2::new(g.value(Axis::RightStickX), g.value(Axis::RightStickY));
        s
    }
}

/// `--pad-probe SECS`: lists the pads gilrs sees (name, uuid, ids, mapping source, power and
/// which inputs the mapping gives), then logs every event for `secs` seconds. No window.
pub fn probe(secs: f64) {
    let mut gilrs = match Gilrs::new() {
        Ok(g) => g,
        Err(e) => {
            println!("pad probe: no controller support: {e}");
            return;
        }
    };
    let describe = |gilrs: &Gilrs, id: GamepadId| {
        let g = gilrs.gamepad(id);
        let uuid: String = g.uuid().iter().map(|b| format!("{b:02x}")).collect();
        println!(
            "pad {id}: {:?} (os name {:?}, map name {:?}), uuid {uuid}, vendor {:04x?} product {:04x?}, mapping {:?}, power {:?}",
            g.name(),
            g.os_name(),
            g.map_name(),
            g.vendor_id(),
            g.product_id(),
            g.mapping_source(),
            g.power_info()
        );
        let all = [
            G::South,
            G::East,
            G::West,
            G::North,
            G::LeftTrigger,
            G::RightTrigger,
            G::LeftTrigger2,
            G::RightTrigger2,
            G::Select,
            G::Start,
            G::Mode,
            G::LeftThumb,
            G::RightThumb,
            G::DPadUp,
            G::DPadDown,
            G::DPadLeft,
            G::DPadRight,
            G::C,
            G::Z,
        ];
        for b in all {
            println!("  button {b:?}: {:?}", g.button_code(b));
        }
        for a in [
            Axis::LeftStickX,
            Axis::LeftStickY,
            Axis::RightStickX,
            Axis::RightStickY,
            Axis::LeftZ,
            Axis::RightZ,
            Axis::DPadX,
            Axis::DPadY,
        ] {
            println!("  axis {a:?}: {:?}", g.axis_code(a));
        }
    };
    // The HID manager reports pads already plugged in as they are found.
    let start = std::time::Instant::now();
    let mut listed = false;
    while start.elapsed().as_secs_f64() < secs.max(0.5) {
        if !listed && start.elapsed().as_secs_f64() > 0.5 {
            listed = true;
            let ids: Vec<GamepadId> = gilrs.gamepads().map(|(id, _)| id).collect();
            if ids.is_empty() {
                println!("pad probe: no controller seen");
            }
            for id in ids {
                describe(&gilrs, id);
            }
            println!("pad probe: press buttons; events for {secs:.0} s");
        }
        while let Some(ev) = gilrs.next_event_blocking(Some(std::time::Duration::from_millis(50))) {
            match ev.event {
                EventType::Connected if listed => describe(&gilrs, ev.id),
                e => println!(
                    "pad {} at {:.2}s: {e:?}",
                    ev.id,
                    start.elapsed().as_secs_f64()
                ),
            }
        }
    }
}
