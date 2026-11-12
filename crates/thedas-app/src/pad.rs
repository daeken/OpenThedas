//! Game controller input, independent of any device: the pad's state, the default mapping
//! from buttons and sticks to the game's actions (a data table, [`BINDINGS`] and
//! [`STICKS`]), stick dead zones, D-pad repeat, and the [`Controller`] that turns a stream of
//! pad states into [`Order`]s for the app to carry out through the same paths the mouse and
//! keyboard use.
//!
//! **A deliberate addition, not parity.** The PC release of Dragon Age: Origins has no
//! controller support: DAOrigins.exe imports DirectInput 8 only (no XInput), and its default
//! key bindings (`keymap.gda`) bind no pad input. The engine it shares with the console
//! versions still names the pad's inputs (`Gamepad::BottomAction` for A, `RightAction` B,
//! `LeftAction` X, `TopAction` Y, `LeftShoulder`, `LeftTrigger`, `Start`, `Back`,
//! `DPadUp`, `LeftThumb`, `ThumbLX`..., registered as bindable inputs near 0x88c200) and
//! the console's input actions (`MoveCharacterForward`, `ControlCameraYaw`,
//! `ControlCameraZoom`, `RadialTargetMode`, `ConversationRadialSelectX/Y`,
//! `AnalogScrollValue`, `GUILeftCursorButton`..., registered near 0x895eb0), and every
//! interface movie carries the console's focus-navigation library. Those names are quoted
//! beside the bindings they correspond to. The default layout itself follows the Xbox 360
//! version's published controls (as supplied by the project owner), not anything bound in
//! the PC executable. Dead zones, repeat rates and speeds are chosen, not traced.

use glam::Vec2;

/// A pad button (an Xbox layout; the exe's input names in [`Button::exe_name`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Button {
    A,
    B,
    X,
    Y,
    LB,
    RB,
    LT,
    RT,
    /// Back (View on the Xbox One and Series pads, the Elite 2 included).
    Back,
    /// Start (Menu on the Xbox One and Series pads).
    Start,
    /// The left stick clicked.
    LS,
    /// The right stick clicked.
    RS,
    Up,
    Down,
    Left,
    Right,
}

impl Button {
    pub const ALL: [Button; 16] = [
        Button::A,
        Button::B,
        Button::X,
        Button::Y,
        Button::LB,
        Button::RB,
        Button::LT,
        Button::RT,
        Button::Back,
        Button::Start,
        Button::LS,
        Button::RS,
        Button::Up,
        Button::Down,
        Button::Left,
        Button::Right,
    ];

    fn bit(self) -> u32 {
        1 << (self as u32)
    }

    /// The executable's name for the input (its `Gamepad::` input table).
    pub fn exe_name(self) -> &'static str {
        match self {
            Button::A => "Gamepad::BottomAction",
            Button::B => "Gamepad::RightAction",
            Button::X => "Gamepad::LeftAction",
            Button::Y => "Gamepad::TopAction",
            Button::LB => "Gamepad::LeftShoulder",
            Button::RB => "Gamepad::RightShoulder",
            Button::LT => "Gamepad::LeftTrigger",
            Button::RT => "Gamepad::RightTrigger",
            Button::Back => "Gamepad::Back",
            Button::Start => "Gamepad::Start",
            Button::LS => "Gamepad::LeftThumb",
            Button::RS => "Gamepad::RightThumb",
            Button::Up => "Gamepad::DPadUp",
            Button::Down => "Gamepad::DPadDown",
            Button::Left => "Gamepad::DPadLeft",
            Button::Right => "Gamepad::DPadRight",
        }
    }

    /// A button by name, as `--steps` writes it (`a`, `lb`, `rt`, `view`, `menu`, `up`...).
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "a" => Button::A,
            "b" => Button::B,
            "x" => Button::X,
            "y" => Button::Y,
            "lb" => Button::LB,
            "rb" => Button::RB,
            "lt" => Button::LT,
            "rt" => Button::RT,
            "back" | "view" | "select" => Button::Back,
            "start" | "menu" => Button::Start,
            "ls" | "l3" => Button::LS,
            "rs" | "r3" => Button::RS,
            "up" | "dpad-up" => Button::Up,
            "down" | "dpad-down" => Button::Down,
            "left" | "dpad-left" => Button::Left,
            "right" | "dpad-right" => Button::Right,
            _ => return None,
        })
    }
}

/// One of the sticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stick {
    Left,
    Right,
}

/// What the pad is doing: buttons held and the sticks' raw deflection (-1..1 each way, +y
/// pushed up), before any dead zone.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PadState {
    buttons: u32,
    pub left: Vec2,
    pub right: Vec2,
}

impl PadState {
    pub fn held(&self, b: Button) -> bool {
        self.buttons & b.bit() != 0
    }

    pub fn set(&mut self, b: Button, down: bool) {
        if down {
            self.buttons |= b.bit();
        } else {
            self.buttons &= !b.bit();
        }
    }

    pub fn stick(&self, s: Stick) -> Vec2 {
        match s {
            Stick::Left => self.left,
            Stick::Right => self.right,
        }
    }

    pub fn set_stick(&mut self, s: Stick, v: Vec2) {
        match s {
            Stick::Left => self.left = v,
            Stick::Right => self.right = v,
        }
    }

    /// Two sources at once (the device and a client's steps): any button either holds, and
    /// the larger deflection of each stick.
    pub fn merged(&self, other: &PadState) -> PadState {
        let pick = |a: Vec2, b: Vec2| {
            if b.length_squared() > a.length_squared() {
                b
            } else {
                a
            }
        };
        PadState {
            buttons: self.buttons | other.buttons,
            left: pick(self.left, other.left),
            right: pick(self.right, other.right),
        }
    }

    pub fn is_idle(&self) -> bool {
        self.buttons == 0 && self.left == Vec2::ZERO && self.right == Vec2::ZERO
    }
}

/// Deflection under this (of 1) is noise: about the 7849/32767 XInput recommends for the
/// left stick.
pub const DEAD_ZONE: f32 = 0.24;
/// Deflection past this counts as full.
pub const FULL: f32 = 0.95;

/// A stick's deflection with a radial dead zone, rescaled so it rises smoothly from 0 at
/// the dead zone's edge to 1 at [`FULL`].
pub fn dead_zone(v: Vec2) -> Vec2 {
    let len = v.length();
    if !len.is_finite() || len <= DEAD_ZONE {
        return Vec2::ZERO;
    }
    let scaled = ((len - DEAD_ZONE) / (FULL - DEAD_ZONE)).min(1.0);
    v / len * scaled
}

/// Where the player's input goes: decides which bindings apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Context {
    /// Exploring or fighting, with only the HUD up.
    Field,
    /// The quickbar picker LT opens over the field (the game paused, a pointer on screen).
    Picker,
    /// A screen or popup over the world: inventory, journal, save and load, store, character
    /// creation, dialog boxes. The pad moves a focus between their buttons (see
    /// [`crate::focusnav`]).
    Screen,
    /// A conversation's lines and replies.
    Conversation,
    /// A cutscene playing.
    Cutscene,
}

/// When a binding fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Press,
    /// Pressed, then again after [`REPEAT_DELAY_MS`] and every [`REPEAT_MS`] while held.
    Repeat,
    Release,
    /// Let go without another button joining it in a chord meanwhile (LB alone changes the
    /// leader; LB with RB selects the whole party).
    Tap,
}

/// What a button does. Each is something the mouse or keyboard already does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Act on the target (attack a hostile, talk, use a door or container), or on the
    /// nearest sensible one in front; an ability waiting for its target is aimed at it. As a
    /// left click on it.
    Interact,
    /// The leader uses quickbar slot n (from 0), on the target if it needs one. As keys 1-0.
    Quickslot(i32),
    /// The previous or next party member leads (as F1-F4).
    PrevMember,
    NextMember,
    /// Select the whole party (the exe's `ToggleFullPartySelect`).
    WholeParty,
    /// The quickbar picker (the 360 version's radial menu): pause, show a pointer on the
    /// quickbar.
    Picker,
    /// Make the nearest sensible thing in front the target (or keep the one held).
    FocusTarget,
    /// Cycle the target among those in view, left to right (or back).
    NextTarget,
    PrevTarget,
    /// The nearest hostile in front becomes the target.
    NearestHostile,
    /// Drop the target, and an ability waiting for one (as a right click).
    ClearTarget,
    /// Swing the camera behind the leader.
    CenterCamera,
    /// The player's screens (inventory first; LB/RB move between them).
    Hub,
    /// Pause or carry on (as Space).
    Pause,
    /// The game menu (as Escape): Start, the 360's "Pause Menu".
    GameMenu,
    /// The pointer's left button goes down or up (a click, or a drag while held).
    PointerDown,
    PointerUp,
    /// The right button goes down or up (an item's actions; in the picker, drops an
    /// ability waiting for its target), or a double click (equip from the backpack).
    RightDown,
    RightUp,
    DoubleClick,
    /// Nudge the pointer one step.
    PointerStep(i8, i8),
    /// Move the focus to the next button that way on the screen (see [`crate::focusnav`]).
    Focus(i8, i8),
    /// The left stick drives a free pointer on screens, or moves the focus again.
    TogglePointer,
    /// Close the screen (as its key again, or Escape).
    CloseScreen,
    /// The previous or next of the player's screens.
    PrevPage,
    NextPage,
    /// Close the quickbar picker.
    ClosePicker,
    /// Move the conversation's reply choice up or down.
    PrevReply,
    NextReply,
    /// Choose the reply picked (or skip or continue the line).
    ChooseReply,
    /// Skip the line or the cutscene.
    Skip,
}

/// One row of the mapping: in `context`, `button` (with `with` held, if given) does
/// `action` on `edge`. `exe` names the original's input action it corresponds to, where one
/// does.
#[derive(Clone, Copy, Debug)]
pub struct Binding {
    pub context: Context,
    pub button: Button,
    pub with: Option<Button>,
    pub edge: Edge,
    pub action: Action,
    pub exe: &'static str,
}

const fn bind(context: Context, button: Button, action: Action, exe: &'static str) -> Binding {
    Binding {
        context,
        button,
        with: None,
        edge: Edge::Press,
        action,
        exe,
    }
}

const fn chord(
    context: Context,
    with: Button,
    button: Button,
    action: Action,
    exe: &'static str,
) -> Binding {
    Binding {
        context,
        button,
        with: Some(with),
        edge: Edge::Press,
        action,
        exe,
    }
}

const fn repeat(context: Context, button: Button, action: Action, exe: &'static str) -> Binding {
    Binding {
        context,
        button,
        with: None,
        edge: Edge::Repeat,
        action,
        exe,
    }
}

const fn tap(context: Context, button: Button, action: Action, exe: &'static str) -> Binding {
    Binding {
        context,
        button,
        with: None,
        edge: Edge::Tap,
        action,
        exe,
    }
}

const fn release(context: Context, button: Button, action: Action, exe: &'static str) -> Binding {
    Binding {
        context,
        button,
        with: None,
        edge: Edge::Release,
        action,
        exe,
    }
}

use Action as Do;
use Button::*;
use Context::*;

/// The default mapping: the Xbox 360 version's controls (A acts, X/Y/B the battle-menu
/// shortcuts with RT holding a second set, LB/RB the party, LT the radial menu, the D-pad
/// the target, Back the character screens, Start the game menu), with a pointer for the PC's
/// mouse-driven screens. A binding with `with` wins over the same button without it.
pub const BINDINGS: &[Binding] = &[
    // Exploring and fighting.
    bind(Field, A, Do::Interact, "Pre_PerformAction"),
    bind(Field, X, Do::Quickslot(0), "ActivateQuickslot1"),
    bind(Field, Y, Do::Quickslot(1), "ActivateQuickslot2"),
    bind(Field, B, Do::Quickslot(2), "ActivateQuickslot3"),
    chord(Field, RT, X, Do::Quickslot(3), "ActivateQuickslot4"),
    chord(Field, RT, Y, Do::Quickslot(4), "ActivateQuickslot5"),
    chord(Field, RT, B, Do::Quickslot(5), "ActivateQuickslot6"),
    tap(Field, LB, Do::PrevMember, "SelectPreviousCharacterMessage"),
    tap(Field, RB, Do::NextMember, "SelectNextCharacterMessage"),
    chord(Field, RB, LB, Do::WholeParty, "ToggleFullPartySelect"),
    chord(Field, LB, RB, Do::WholeParty, "ToggleFullPartySelect"),
    bind(Field, LT, Do::Picker, "RadialTargetMode"),
    bind(Field, LS, Do::FocusTarget, "EnableAutoTarget"),
    bind(Field, RS, Do::CenterCamera, ""),
    bind(Field, Right, Do::NextTarget, "RadialTargetingXAxis"),
    bind(Field, Left, Do::PrevTarget, "RadialTargetingXAxis"),
    bind(Field, Up, Do::NearestHostile, "SelectionFilterHostile"),
    bind(Field, Down, Do::ClearTarget, ""),
    bind(Field, Back, Do::Hub, "ToggleInventory"),
    bind(Field, Start, Do::GameMenu, "ToggleMainMenu"),
    // The quickbar picker: a pointer over the paused game.
    bind(Picker, A, Do::PointerDown, "GUILeftCursorButton"),
    release(Picker, A, Do::PointerUp, "GUILeftCursorButton"),
    bind(Picker, Y, Do::RightDown, "GUIRightCursorButton"),
    release(Picker, Y, Do::RightUp, "GUIRightCursorButton"),
    bind(Picker, B, Do::ClosePicker, ""),
    bind(Picker, LT, Do::ClosePicker, "RadialTargetMode"),
    repeat(Picker, Left, Do::PointerStep(-1, 0), ""),
    repeat(Picker, Right, Do::PointerStep(1, 0), ""),
    repeat(Picker, Up, Do::PointerStep(0, -1), ""),
    repeat(Picker, Down, Do::PointerStep(0, 1), ""),
    bind(Picker, Start, Do::Pause, "TogglePause"),
    // Screens and popups: the focus moves between their buttons, the pointer resting on the
    // one focused (LS frees the pointer for the left stick).
    bind(Screen, A, Do::PointerDown, "GUILeftCursorButton"),
    release(Screen, A, Do::PointerUp, "GUILeftCursorButton"),
    bind(Screen, X, Do::DoubleClick, "GUILeftCursorButton"),
    bind(Screen, Y, Do::RightDown, "GUIRightCursorButton"),
    release(Screen, Y, Do::RightUp, "GUIRightCursorButton"),
    bind(Screen, B, Do::CloseScreen, "ToggleMainMenu"),
    bind(Screen, Start, Do::CloseScreen, "ToggleMainMenu"),
    bind(Screen, Back, Do::CloseScreen, "ToggleInventory"),
    bind(Screen, LB, Do::PrevPage, ""),
    bind(Screen, RB, Do::NextPage, ""),
    bind(Screen, LS, Do::TogglePointer, ""),
    repeat(Screen, Left, Do::Focus(-1, 0), ""),
    repeat(Screen, Right, Do::Focus(1, 0), ""),
    repeat(Screen, Up, Do::Focus(0, -1), ""),
    repeat(Screen, Down, Do::Focus(0, 1), ""),
    // Conversations.
    repeat(Conversation, Up, Do::PrevReply, "ConversationRadialSelectY"),
    repeat(
        Conversation,
        Down,
        Do::NextReply,
        "ConversationRadialSelectY",
    ),
    bind(Conversation, A, Do::ChooseReply, "SelectResponseMessage"),
    bind(Conversation, X, Do::Skip, ""),
    // Cutscenes.
    bind(Cutscene, A, Do::Skip, ""),
    bind(Cutscene, B, Do::Skip, ""),
    bind(Cutscene, X, Do::Skip, ""),
    bind(Cutscene, Start, Do::Skip, ""),
];

/// What a stick does in a context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StickRole {
    /// Walk the leader, relative to the camera (`MoveCharacterForward` and the rest).
    Move,
    /// Turn the camera round the leader (x, `ControlCameraYaw`) and zoom (y,
    /// `ControlCameraZoom`).
    Camera,
    /// Move the pointer (`CursorPosition`).
    Pointer,
    /// Pushed past [`STICK_AS_DPAD`], move the focus as the D-pad does; a free pointer
    /// instead once LS has freed it ([`Action::TogglePointer`]).
    Focus,
    /// Up and down step the conversation's reply choice, as the D-pad does
    /// (`ConversationRadialSelectY`).
    Replies,
    /// Up and down scroll the list or text under the pointer, as the mouse wheel does (the
    /// console fed the movies' `AnalogScrollValue` from `Gamepad_ThumbRY`).
    Scroll,
}

/// The sticks' roles, by context. Unlisted, a stick does nothing.
pub const STICKS: &[(Context, Stick, StickRole)] = &[
    (Field, Stick::Left, StickRole::Move),
    (Field, Stick::Right, StickRole::Camera),
    (Picker, Stick::Left, StickRole::Pointer),
    (Picker, Stick::Right, StickRole::Camera),
    (Screen, Stick::Left, StickRole::Focus),
    (Screen, Stick::Right, StickRole::Scroll),
    (Conversation, Stick::Left, StickRole::Replies),
    (Conversation, Stick::Right, StickRole::Camera),
];

/// A held D-pad direction (or stick pushed as one) repeats after this long...
pub const REPEAT_DELAY_MS: f64 = 400.0;
/// ...then this often.
pub const REPEAT_MS: f64 = 110.0;
/// A stick pushed this far counts as a D-pad press (for [`StickRole::Replies`]).
pub const STICK_AS_DPAD: f32 = 0.5;
/// Walking is ordered again this often while the stick is held...
pub const MOVE_EVERY_MS: f64 = 250.0;
/// ...or at once when its direction turns this far (radians).
pub const MOVE_TURN: f32 = 0.35;
/// The pointer's top speed, pixels a second (it rises with the square of the deflection).
pub const POINTER_SPEED: f32 = 1100.0;
/// The scroll at full deflection (wheel lines a second).
pub const SCROLL_SPEED: f32 = 8.0;
/// The camera's turn at full deflection (radians a second) and zoom (wheel steps a second).
pub const TURN_SPEED: f32 = 2.4;
/// Provisional for zoom (the PC original has no pad): the whole range (far to near, 20 wheel
/// steps) in about 2 s; Page Up and Page Down held use it too.
pub const ZOOM_SPEED: f32 = 10.0;

/// What the app is to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Order {
    /// A button's action.
    Act(Action),
    /// Walk the leader this way (camera-relative: +y away from the camera, +x to its
    /// right; length 0..1), or stop where it stands (zero).
    Walk(Vec2),
    /// Turn the camera (radians of yaw, anticlockwise) and zoom (steps, + in).
    Camera { yaw: f32, zoom: f32 },
    /// Move the pointer (pixels).
    Pointer(Vec2),
    /// Turn the mouse wheel (whole lines, up positive).
    Scroll(f32),
}

/// Turns pad states into [`Order`]s: edges, chords, repeats and the sticks.
#[derive(Debug, Default)]
pub struct Controller {
    last: PadState,
    /// The context each held button was pressed in (its release goes there only).
    pressed_in: [Option<Context>; 16],
    /// How long each held button has been held, and when it next repeats.
    held_ms: [f64; 16],
    next_repeat: [f64; 16],
    /// The left stick as a D-pad (for replies): which way, how long.
    stick_dir: i8,
    stick_ms: f64,
    stick_next: f64,
    /// Walking: the direction last ordered and how long ago.
    walking: Option<Vec2>,
    walk_ms: f64,
    /// Buttons that joined a chord since they went down (their taps are off).
    chorded: [bool; 16],
    /// Wheel lines owed by the right stick, not yet whole.
    scroll: f32,
    /// LS freed the pointer: the left stick moves it on screens instead of the focus.
    pub free_pointer: bool,
    /// The left stick as a D-pad (for the focus): which way, how long.
    focus_dir: (i8, i8),
    focus_ms: f64,
    focus_next: f64,
}

impl Controller {
    /// The orders for the pad's state `now`, `dt_ms` after the last, in `context`.
    pub fn update(&mut self, now: &PadState, context: Context, dt_ms: f64) -> Vec<Order> {
        let mut out = Vec::new();
        let dt_ms = dt_ms.max(0.0);
        for b in Button::ALL {
            let i = b as usize;
            let (was, is) = (self.last.held(b), now.held(b));
            if is && !was {
                self.pressed_in[i] = Some(context);
                self.held_ms[i] = 0.0;
                self.next_repeat[i] = REPEAT_DELAY_MS;
                if let Some(row) = BINDINGS.iter().find(|r| {
                    r.context == context
                        && r.button == b
                        && r.edge == Edge::Press
                        && r.with.is_some_and(|w| now.held(w))
                }) {
                    self.chorded[i] = true;
                    if let Some(w) = row.with {
                        self.chorded[w as usize] = true;
                    }
                }
                if let Some(a) = action_for(context, b, now, Edge::Press) {
                    push_once(&mut out, Order::Act(a));
                }
            } else if is && was {
                self.held_ms[i] += dt_ms;
                if self.pressed_in[i] == Some(context) {
                    while self.held_ms[i] >= self.next_repeat[i] {
                        self.next_repeat[i] += REPEAT_MS;
                        if let Some(a) = action_for(context, b, now, Edge::Repeat) {
                            out.push(Order::Act(a));
                        }
                    }
                }
            } else if !is && was {
                if self.pressed_in[i] == Some(context) {
                    if let Some(a) = action_for(context, b, now, Edge::Release) {
                        out.push(Order::Act(a));
                    }
                    if !self.chorded[i] {
                        if let Some(a) = action_for(context, b, now, Edge::Tap) {
                            out.push(Order::Act(a));
                        }
                    }
                }
                self.pressed_in[i] = None;
                self.chorded[i] = false;
            }
        }
        for (ctx, stick, role) in STICKS {
            if *ctx != context {
                continue;
            }
            let v = dead_zone(now.stick(*stick));
            match role {
                StickRole::Move => {}
                StickRole::Camera if v != Vec2::ZERO => {
                    let s = (dt_ms / 1000.0) as f32;
                    out.push(Order::Camera {
                        yaw: -v.x * TURN_SPEED * s,
                        zoom: v.y * ZOOM_SPEED * s,
                    });
                }
                StickRole::Pointer | StickRole::Focus
                    if v != Vec2::ZERO && (*role == StickRole::Pointer || self.free_pointer) =>
                {
                    let s = (dt_ms / 1000.0) as f32 * POINTER_SPEED * v.length();
                    out.push(Order::Pointer(Vec2::new(v.x, -v.y) * s));
                }
                StickRole::Focus if !self.free_pointer => {
                    self.stick_focus(now.stick(*stick), dt_ms, &mut out)
                }
                StickRole::Replies => self.stick_replies(now.stick(*stick), dt_ms, &mut out),
                StickRole::Scroll => {
                    self.scroll += v.y * SCROLL_SPEED * (dt_ms / 1000.0) as f32;
                    let whole = self.scroll.trunc();
                    if whole != 0.0 {
                        self.scroll -= whole;
                        out.push(Order::Scroll(whole));
                    }
                    if v == Vec2::ZERO {
                        self.scroll = 0.0;
                    }
                }
                _ => {}
            }
        }
        if !STICKS.contains(&(context, Stick::Left, StickRole::Replies)) {
            self.stick_dir = 0;
        }
        if self.free_pointer || !STICKS.contains(&(context, Stick::Left, StickRole::Focus)) {
            self.focus_dir = (0, 0);
        }
        self.walk(now, context, dt_ms, &mut out);
        self.last = *now;
        out
    }

    /// The left stick moving the focus: past [`STICK_AS_DPAD`] along its stronger axis as a
    /// D-pad press, repeating while held.
    fn stick_focus(&mut self, v: Vec2, dt_ms: f64, out: &mut Vec<Order>) {
        let dir = if v.length() < STICK_AS_DPAD {
            (0, 0)
        } else if v.x.abs() > v.y.abs() {
            (v.x.signum() as i8, 0)
        } else {
            // Up on the stick is up the screen (smaller y).
            (0, -v.y.signum() as i8)
        };
        if dir != self.focus_dir {
            self.focus_dir = dir;
            self.focus_ms = 0.0;
            self.focus_next = REPEAT_DELAY_MS;
            if dir != (0, 0) {
                out.push(Order::Act(Action::Focus(dir.0, dir.1)));
            }
        } else if dir != (0, 0) {
            self.focus_ms += dt_ms;
            while self.focus_ms >= self.focus_next {
                self.focus_next += REPEAT_MS;
                out.push(Order::Act(Action::Focus(dir.0, dir.1)));
            }
        }
    }

    /// The left stick stepping replies: up or down past [`STICK_AS_DPAD`] as a D-pad press,
    /// repeating while held.
    fn stick_replies(&mut self, v: Vec2, dt_ms: f64, out: &mut Vec<Order>) {
        let dir = if v.y >= STICK_AS_DPAD {
            -1
        } else if v.y <= -STICK_AS_DPAD {
            1
        } else {
            0
        };
        let step = |d: i8| {
            Order::Act(if d < 0 {
                Action::PrevReply
            } else {
                Action::NextReply
            })
        };
        if dir != self.stick_dir {
            self.stick_dir = dir;
            self.stick_ms = 0.0;
            self.stick_next = REPEAT_DELAY_MS;
            if dir != 0 {
                out.push(step(dir));
            }
        } else if dir != 0 {
            self.stick_ms += dt_ms;
            while self.stick_ms >= self.stick_next {
                self.stick_next += REPEAT_MS;
                out.push(step(dir));
            }
        }
    }

    /// Walking with the left stick: ordered when it is pushed, again every
    /// [`MOVE_EVERY_MS`] or when it turns, and a stop when it is let go (or the context
    /// changes).
    fn walk(&mut self, now: &PadState, context: Context, dt_ms: f64, out: &mut Vec<Order>) {
        let role = STICKS
            .iter()
            .find(|(c, s, _)| *c == context && *s == Stick::Left)
            .map(|(_, _, r)| *r);
        let v = if role == Some(StickRole::Move) {
            dead_zone(now.left)
        } else {
            Vec2::ZERO
        };
        match (self.walking, v == Vec2::ZERO) {
            (None, true) => {}
            (Some(_), true) => {
                self.walking = None;
                if context == Context::Field {
                    out.push(Order::Walk(Vec2::ZERO));
                }
            }
            (last, false) => {
                self.walk_ms += dt_ms;
                let turned = last.is_none_or(|l| l.angle_to(v).abs() > MOVE_TURN);
                if turned || self.walk_ms >= MOVE_EVERY_MS {
                    self.walk_ms = 0.0;
                    self.walking = Some(v);
                    out.push(Order::Walk(v));
                }
            }
        }
    }
}

/// The action `button` does on `edge` in `context`, with the other buttons as `state` holds
/// them: a chord whose other button is held wins. A [`Edge::Repeat`] binding also fires on
/// the press.
pub fn action_for(
    context: Context,
    button: Button,
    state: &PadState,
    edge: Edge,
) -> Option<Action> {
    let fits = |b: &&Binding| {
        b.context == context
            && b.button == button
            && match edge {
                Edge::Press => matches!(b.edge, Edge::Press | Edge::Repeat),
                e => b.edge == e,
            }
    };
    let rows: Vec<&Binding> = BINDINGS.iter().filter(fits).collect();
    rows.iter()
        .find(|b| b.with.is_some_and(|w| state.held(w)))
        .or_else(|| rows.iter().find(|b| b.with.is_none()))
        .map(|b| b.action)
}

/// Adds `o` unless it is already there (both bumpers going down together are one chord).
fn push_once(out: &mut Vec<Order>, o: Order) {
    if !out.contains(&o) {
        out.push(o);
    }
}

/// The camera-relative direction `v` (+y away from a camera looking along `yaw`, radians
/// anticlockwise from +X; +x to its right) as a world direction on the ground.
pub fn world_direction(v: Vec2, yaw: f32) -> Vec2 {
    let forward = Vec2::new(yaw.cos(), yaw.sin());
    let right = Vec2::new(yaw.sin(), -yaw.cos());
    forward * v.y + right * v.x
}

/// The mapping as text, one line a binding, for the log and the docs.
pub fn describe() -> Vec<String> {
    BINDINGS
        .iter()
        .map(|b| {
            let with = b.with.map(|w| format!("{w:?}+")).unwrap_or_default();
            let edge = match b.edge {
                Edge::Press => "",
                Edge::Tap => " (tapped)",
                Edge::Repeat => " (repeats)",
                Edge::Release => " (released)",
            };
            let exe = if b.exe.is_empty() {
                String::new()
            } else {
                format!(" (the exe's {})", b.exe)
            };
            format!(
                "{:?}: {with}{:?} [{}]{edge} -> {:?}{exe}",
                b.context,
                b.button,
                b.button.exe_name(),
                b.action
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(s: &mut PadState, b: Button) -> PadState {
        s.set(b, true);
        *s
    }

    fn acts(orders: &[Order]) -> Vec<Action> {
        orders
            .iter()
            .filter_map(|o| match o {
                Order::Act(a) => Some(*a),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn dead_zone_cuts_noise_and_rescales() {
        assert_eq!(dead_zone(Vec2::new(0.2, 0.1)), Vec2::ZERO);
        assert_eq!(dead_zone(Vec2::new(f32::NAN, 0.0)), Vec2::ZERO);
        let full = dead_zone(Vec2::new(0.0, 1.0));
        assert!((full.length() - 1.0).abs() < 1e-6 && full.y > 0.0);
        let half = dead_zone(Vec2::new(0.6, 0.0));
        assert!(half.x > 0.0 && half.x < 0.6, "{half:?}");
        // Direction is kept.
        let d = dead_zone(Vec2::new(-0.7, 0.7));
        assert!((d.x + d.y).abs() < 1e-6 && d.x < 0.0);
    }

    #[test]
    fn buttons_fire_on_press_once_by_context() {
        let mut c = Controller::default();
        let mut s = PadState::default();
        let o = c.update(&press(&mut s, A), Field, 16.0);
        assert_eq!(acts(&o), vec![Action::Interact]);
        // Held: nothing more (A does not repeat).
        assert!(acts(&c.update(&s, Field, 2000.0)).is_empty());
        s.set(A, false);
        assert!(acts(&c.update(&s, Field, 16.0)).is_empty());
        // The same button elsewhere.
        assert_eq!(
            acts(&c.update(&press(&mut s, A), Conversation, 16.0)),
            vec![Action::ChooseReply]
        );
    }

    #[test]
    fn right_trigger_shifts_the_shortcuts_to_the_second_set() {
        let mut c = Controller::default();
        let mut s = PadState::default();
        assert_eq!(
            acts(&c.update(&press(&mut s, X), Field, 16.0)),
            vec![Action::Quickslot(0)]
        );
        s.set(X, false);
        c.update(&s, Field, 16.0);
        press(&mut s, RT);
        assert!(acts(&c.update(&s, Field, 16.0)).is_empty());
        for (b, slot) in [(X, 3), (Y, 4), (B, 5)] {
            assert_eq!(
                acts(&c.update(&press(&mut s, b), Field, 16.0)),
                vec![Action::Quickslot(slot)]
            );
            s.set(b, false);
            c.update(&s, Field, 16.0);
        }
    }

    #[test]
    fn both_bumpers_select_the_whole_party() {
        let mut c = Controller::default();
        let mut s = PadState::default();
        // Alone, a bumper changes the leader when it is let go.
        assert!(acts(&c.update(&press(&mut s, RB), Field, 16.0)).is_empty());
        s.set(RB, false);
        assert_eq!(acts(&c.update(&s, Field, 16.0)), vec![Action::NextMember]);
        // The other joining makes it a chord: no leader change after.
        press(&mut s, RB);
        c.update(&s, Field, 16.0);
        assert_eq!(
            acts(&c.update(&press(&mut s, LB), Field, 16.0)),
            vec![Action::WholeParty]
        );
        s.set(RB, false);
        assert!(acts(&c.update(&s, Field, 16.0)).is_empty());
        s.set(LB, false);
        assert!(acts(&c.update(&s, Field, 16.0)).is_empty());
        // Both going down in the same frame: one chord.
        let mut c = Controller::default();
        let mut s = PadState::default();
        s.set(LB, true);
        s.set(RB, true);
        assert_eq!(acts(&c.update(&s, Field, 16.0)), vec![Action::WholeParty]);
    }

    #[test]
    fn dpad_repeats_after_a_delay() {
        let mut c = Controller::default();
        let mut s = PadState::default();
        assert_eq!(
            acts(&c.update(&press(&mut s, Down), Conversation, 16.0)),
            vec![Action::NextReply]
        );
        assert!(acts(&c.update(&s, Conversation, REPEAT_DELAY_MS - 20.0)).is_empty());
        assert_eq!(acts(&c.update(&s, Conversation, 20.0)).len(), 1);
        assert_eq!(acts(&c.update(&s, Conversation, REPEAT_MS * 3.0)).len(), 3);
        // In the field the D-pad changes the target and does not repeat.
        let mut c = Controller::default();
        let mut s = PadState::default();
        assert_eq!(
            acts(&c.update(&press(&mut s, Right), Field, 16.0)),
            vec![Action::NextTarget]
        );
        assert!(acts(&c.update(&s, Field, 5000.0)).is_empty());
    }

    #[test]
    fn a_release_goes_only_where_its_press_went() {
        let mut c = Controller::default();
        let mut s = PadState::default();
        assert_eq!(
            acts(&c.update(&press(&mut s, A), Screen, 16.0)),
            vec![Action::PointerDown]
        );
        s.set(A, false);
        assert_eq!(acts(&c.update(&s, Screen, 16.0)), vec![Action::PointerUp]);
        // Pressed in the field (it opened a screen), released on the screen: no stray
        // pointer release.
        press(&mut s, A);
        c.update(&s, Field, 16.0);
        s.set(A, false);
        assert!(acts(&c.update(&s, Screen, 16.0)).is_empty());
    }

    #[test]
    fn the_left_stick_walks_and_stops() {
        let mut c = Controller::default();
        let mut s = PadState {
            left: Vec2::new(0.0, 1.0),
            ..Default::default()
        };
        let o = c.update(&s, Field, 16.0);
        assert!(matches!(o.as_slice(), [Order::Walk(v)] if v.y > 0.9));
        // Held the same way: ordered again only after a while.
        assert!(c.update(&s, Field, 100.0).is_empty());
        assert_eq!(c.update(&s, Field, MOVE_EVERY_MS).len(), 1);
        // Turned: at once.
        s.left = Vec2::new(1.0, 0.0);
        assert!(matches!(c.update(&s, Field, 16.0).as_slice(), [Order::Walk(v)] if v.x > 0.9));
        // Let go: a stop.
        s.left = Vec2::new(0.05, 0.0);
        assert_eq!(c.update(&s, Field, 16.0), vec![Order::Walk(Vec2::ZERO)]);
        assert!(c.update(&s, Field, 16.0).is_empty());
        // On a screen the left stick moves the focus, as the D-pad does, repeating while
        // held; up on the stick is up the screen.
        s.left = Vec2::new(1.0, 0.0);
        assert_eq!(
            c.update(&s, Screen, 16.0),
            vec![Order::Act(Action::Focus(1, 0))]
        );
        assert!(c.update(&s, Screen, 100.0).is_empty());
        assert_eq!(
            c.update(&s, Screen, REPEAT_DELAY_MS),
            vec![Order::Act(Action::Focus(1, 0))]
        );
        s.left = Vec2::new(0.1, 0.8);
        assert_eq!(
            c.update(&s, Screen, 16.0),
            vec![Order::Act(Action::Focus(0, -1))]
        );
        s.left = Vec2::ZERO;
        assert!(c.update(&s, Screen, 16.0).is_empty());
        // Freed (LS), it moves the pointer.
        c.free_pointer = true;
        s.left = Vec2::new(1.0, 0.0);
        let o = c.update(&s, Screen, 100.0);
        assert!(matches!(o.as_slice(), [Order::Pointer(p)] if p.x > 50.0 && p.y == 0.0));
    }

    #[test]
    fn the_right_stick_turns_and_zooms() {
        let mut c = Controller::default();
        let s = PadState {
            right: Vec2::new(1.0, 1.0) / 2f32.sqrt(),
            ..Default::default()
        };
        match c.update(&s, Field, 1000.0).as_slice() {
            [Order::Camera { yaw, zoom }] => assert!(*yaw < 0.0 && *zoom > 0.0),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn the_right_stick_scrolls_screens_in_whole_lines() {
        let mut c = Controller::default();
        let s = PadState {
            right: Vec2::new(0.0, -1.0),
            ..Default::default()
        };
        // Under a line: nothing yet; past it, whole lines down.
        assert!(c.update(&s, Screen, 50.0).is_empty());
        let o = c.update(&s, Screen, 400.0);
        assert!(matches!(o.as_slice(), [Order::Scroll(l)] if *l <= -1.0 && l.fract() == 0.0));
        // In the field the right stick is the camera's.
        assert!(matches!(
            c.update(&s, Field, 100.0).as_slice(),
            [Order::Camera { .. }]
        ));
    }

    #[test]
    fn the_left_stick_steps_replies_with_repeat() {
        let mut c = Controller::default();
        let mut s = PadState {
            left: Vec2::new(0.0, -0.9),
            ..Default::default()
        };
        assert_eq!(
            acts(&c.update(&s, Conversation, 16.0)),
            vec![Action::NextReply]
        );
        assert!(acts(&c.update(&s, Conversation, 100.0)).is_empty());
        assert_eq!(acts(&c.update(&s, Conversation, REPEAT_DELAY_MS)).len(), 1);
        s.left = Vec2::new(0.0, 0.9);
        assert_eq!(
            acts(&c.update(&s, Conversation, 16.0)),
            vec![Action::PrevReply]
        );
        // No walking in a conversation.
        assert!(
            !c.update(&s, Conversation, 16.0)
                .iter()
                .any(|o| matches!(o, Order::Walk(_)))
        );
    }

    #[test]
    fn camera_relative_directions() {
        // Camera looking along +X: forward is +X, right is -Y.
        let f = world_direction(Vec2::new(0.0, 1.0), 0.0);
        assert!((f - Vec2::X).length() < 1e-6);
        let r = world_direction(Vec2::new(1.0, 0.0), 0.0);
        assert!((r + Vec2::Y).length() < 1e-6);
        // Looking along +Y: forward +Y, right +X.
        let r = world_direction(Vec2::new(1.0, 0.0), std::f32::consts::FRAC_PI_2);
        assert!((r - Vec2::X).length() < 1e-6);
    }

    #[test]
    fn the_table_is_consistent() {
        // Every button parses by name and every context has A.
        for b in Button::ALL {
            assert!(b.exe_name().starts_with("Gamepad::"));
        }
        assert_eq!(Button::parse("View"), Some(Back));
        assert_eq!(Button::parse("menu"), Some(Start));
        assert_eq!(Button::parse("dpad-up"), Some(Up));
        assert_eq!(Button::parse("z"), None);
        for ctx in [Field, Picker, Screen, Conversation, Cutscene] {
            assert!(action_for(ctx, A, &PadState::default(), Edge::Press).is_some());
        }
        // No two plain rows for one button, context and edge.
        for (i, a) in BINDINGS.iter().enumerate() {
            for b in &BINDINGS[i + 1..] {
                assert!(
                    !(a.context == b.context
                        && a.button == b.button
                        && a.with == b.with
                        && a.edge == b.edge),
                    "{a:?} {b:?}"
                );
            }
        }
        assert_eq!(describe().len(), BINDINGS.len());
    }

    #[test]
    fn merged_states_take_either_source() {
        let mut a = PadState::default();
        a.set(A, true);
        let b = PadState {
            left: Vec2::new(0.0, 1.0),
            ..Default::default()
        };
        let m = a.merged(&b);
        assert!(m.held(A) && m.left.y == 1.0 && !m.is_idle());
        assert!(PadState::default().is_idle());
    }
}
