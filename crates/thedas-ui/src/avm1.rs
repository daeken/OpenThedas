//! A few hand-assembled AVM1 (ActionScript 2) actions injected into movies so the engine can
//! talk to them the way Scaleform let the game.
//!
//! - Scaleform games call movie functions by path (`GFxMovie::Invoke("_global.GUISystem.Start")`);
//!   Ruffle only lets the host call functions registered with `ExternalInterface.addCallback`.
//!   The injected code registers one such callback, [`INVOKE_CALLBACK`], taking an object path,
//!   a method name and an argument array, and applies the method to the object.
//! - The movies' `ExternalCommands` read engine answers by sending an `fscommand` and returning
//!   `ExternalCommands.fsc_retval`, which the game set from its fscommand handler. Ruffle's
//!   fscommand handler cannot touch the movie, so each such function is wrapped: the original
//!   sends the fscommand (the host computes and keeps the answer), then the wrapper returns
//!   `ExternalInterface.call(RETVAL_CALL)`.
//! - Flash Player (and Ruffle after it) hands an empty string passed through
//!   `ExternalInterface` to an SWF older than version 9 as `"null"`; Scaleform passed the
//!   game's values directly, and the movies test for empty text (conversation.gfx's
//!   continue-only reply). The host sends empty strings as [`EMPTY`], and every way a value
//!   comes in (`GetValue`, `GetString`, wrapped fscommands, [`INVOKE_CALLBACK`] arguments)
//!   turns it back into `""`.
//! - Scaleform passed a mouse button (and double clicks, as the button + 4) to the movies'
//!   press handlers; Flash and Ruffle pass none. `GUI.Element`'s event handler is wrapped to
//!   take the button from [`BUTTON_VAR`] when none is given, which the host sets around a
//!   right click or a double click it delivers as an ordinary click.
//! - Ruffle runs the movies' `onMouseUp` handlers before it releases the button under the
//!   mouse; Scaleform released it first. The movies' pickers close on any mouse up
//!   (`MovieUtils.HideMovie` of their list), so under Ruffle the list was gone before its
//!   entry saw the release and no choice was ever made (tactics.gfx's presets and behaviours).
//!   `MovieUtils.HideMovie` is wrapped to put off hiding while the host delivers a release
//!   ([`HIDE_LATER`] is an array then); the host hides what was put off afterwards
//!   ([`HIDE_FLUSH`]), unless `MovieUtils.ShowMovie` showed it again meanwhile. Provisional: the order is inferred from the movies working in the game.

/// The callback name the injected code registers.
pub const INVOKE_CALLBACK: &str = "thedas_invoke";

/// An empty string on its way into a movie (a zero-width space, which Flash's empty-string
/// conversion leaves alone); the injected code turns it back into `""`.
pub const EMPTY: &str = "\u{200B}";

/// `ExternalCommands` functions returning an engine value straight from
/// `ExternalInterface.call`; wrapped to turn [`EMPTY`] back into `""`.
pub const VALUE_GETTERS: &[&str] = &["GetValue", "GetString"];

/// The function the host calls once, after starting a movie, to wrap [`PRE_CALLS`] (the
/// movies define those classes after the first frame's actions run).
pub const INSTALL: &str = "_global.thedas_install";

/// The global the host sets to the Scaleform mouse button of the click it is delivering
/// (see [`MOUSE_RIGHT`]); undefined for an ordinary left click.
pub const BUTTON_VAR: &str = "_global.thedas_button";
/// Scaleform's `Mouse.LEFT`, `Mouse.RIGHT`, `Mouse.MIDDLE`; a double click is the button + 4.
/// Provisional: Scaleform's values, as the movies' button tests imply.
pub const MOUSE_LEFT: f64 = 1.0;
pub const MOUSE_RIGHT: f64 = 2.0;
pub const MOUSE_MIDDLE: f64 = 3.0;
pub const DOUBLE_CLICK: f64 = 4.0;

/// While an array, the clips `MovieUtils.HideMovie` was asked to hide (the host sets it around
/// a mouse release).
pub const HIDE_LATER: &str = "_global.thedas_later";
/// Hides the clips put off in [`HIDE_LATER`] and ends the putting off.
pub const HIDE_FLUSH: &str = "_global.thedas_flush";

/// Runs the post-load functions put off by `GUI.Element.RegisterPostLoadFunction`'s wrapper
/// (the host calls it after each frame and each mouse event). Ruffle runs a new element's
/// `onLoad` as `attachMovie` makes it, so an element was already loaded (`m_bLoaded`, its
/// post-load functions called) when the movie registers functions for it right after making
/// it, and they never ran: an open dropdown's list background was never grown to the list
/// (`ExpandBackground`) nor its entries fitted (`FitButtons`), leaving the list see-through
/// over the controls below. Scaleform ran `onLoad` after the movie's code; the wrapper puts
/// such a function off until then. Provisional: the order is inferred from the movies.
pub const POST_LOAD_RUN: &str = "_global.thedas_run_after";

/// The scaling-grid sprites the movie made (`_global.thedas_s9`, see
/// `thedas_formats::gfx::Scale9`): `[id, x scale, y scale, ...]`, one triple each.
pub const SCALE9_SCALES: &str = "_global.thedas_s9_scales";
/// Lays out one piece of a scaling-grid sprite: (sprite index, piece, x, y, x scale, y scale).
pub const SCALE9_SET: &str = "_global.thedas_s9_set";

/// The engine-image slots the movie made (`_global.thedas_slots`, see
/// `thedas_formats::gfx::Slot`): each one's image name.
pub const SLOT_NAMES: &str = "_global.thedas_slot_names";
/// Loads an image into a slot: (slot index, URL, x, y).
pub const SLOT_LOAD: &str = "_global.thedas_slot_load";

/// The callback name for setting a movie variable (`object path`, `member`, `value`).
pub const SET_CALLBACK: &str = "thedas_set";

/// The `ExternalInterface.call` name wrappers use to fetch the last fscommand's answer.
pub const RETVAL_CALL: &str = "thedas_retval";

/// `ExternalCommands` functions that return `fsc_retval` (from the movies' bytecode).
pub const RETURNING_FSCOMMANDS: &[&str] = &[
    "GetNumMembers",
    "ListMemberNames",
    "ReplaceDesignerTags",
    "GetNum2DARows",
    "Get2DAValue",
    "Get2DAValueByColumnName",
    "Get2DAValueByIndex",
    "Get2DAValueByRowIndexColumnHash",
    "Get2DAValueByRowIndexColumnName",
    "GetNumM2DARows",
    "GetM2DAValue",
    "GetM2DAValueByIndex",
    "GetTlkString",
    "FormatFloat",
];

/// `DAExternalCommands` functions that send an fscommand and return what the game set in
/// `_root.fsc_retval` (and `fsc_retval2`) meanwhile (tactics.gfx's pickers). They are wrapped
/// like [`RETURNING_FSCOMMANDS`] (once the movie defines them, see [`INSTALL`]): the
/// wrapper returns the game's whole answer, which for `ListTacticTargets` and
/// `ListTacticConditions` is the `{Targets, IDs}` / `{Conditions, IDs}` object the originals
/// build from the two variables.
pub const DA_RETURNING_FSCOMMANDS: &[&str] = &[
    "ListTacticTargets",
    "ListTacticBaseConditions",
    "ListTacticConditions",
    "ListTacticActions",
    "ListTacticAbilities",
    "ListTacticModalAbilities",
    "ListTacticItems",
    "GetObjectNameByID",
];

/// Functions the game answers by setting a movie variable while the function runs (Scaleform's
/// `GFxMovie::SetVariable` from inside an `ExternalInterface` handler, which Ruffle's handlers
/// cannot do). Each is wrapped to set the variable from `ExternalInterface.call(call name, first
/// argument)` before the original runs, so the original finds it set.
///
/// - `SharedLibrary.Ability.LoadFromToken(token)` calls `GetAbilityAttributes` and reads
///   `_root.ReturnedAbilityAttributes` (its register 1 is `_root`): id, use type, icon, GUI
///   type, prerequisite, display state.
pub const PRE_CALLS: &[PreCall] = &[PreCall {
    object: &["_global", "SharedLibrary", "Ability"],
    function: "LoadFromToken",
    target: &["_root"],
    variable: "ReturnedAbilityAttributes",
    call: "GetAbilityAttributes",
}];

/// One of [`PRE_CALLS`].
pub struct PreCall {
    /// The path of the object holding the function.
    pub object: &'static [&'static str],
    pub function: &'static str,
    /// The path of the object holding the variable.
    pub target: &'static [&'static str],
    pub variable: &'static str,
    /// The `ExternalInterface` call answering it.
    pub call: &'static str,
}

const SET_MEMBER: u8 = 0x4f;
const SET_VARIABLE: u8 = 0x1d;
const DEFINE_LOCAL: u8 = 0x3c;
const STRICT_EQUALS: u8 = 0x66;
const EQUALS2: u8 = 0x49;
const NOT: u8 = 0x12;
const LESS2: u8 = 0x48;
const INCREMENT: u8 = 0x50;
const IF: u8 = 0x9d;
const JUMP: u8 = 0x99;

const GET_VARIABLE: u8 = 0x1c;
const GET_MEMBER: u8 = 0x4e;
const CALL_METHOD: u8 = 0x52;
const RETURN: u8 = 0x3e;
const POP: u8 = 0x17;
const PUSH: u8 = 0x96;
const DEFINE_FUNCTION: u8 = 0x9b;
const NEW_OBJECT: u8 = 0x40;
const ADD2: u8 = 0x47;
const INIT_ARRAY: u8 = 0x42;

enum Push<'a> {
    Str(&'a str),
    Int(i32),
    Null,
    Undefined,
    Bool(bool),
    Register(u8),
}

fn get_path(out: &mut Vec<u8>, path: &[&str]) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    push(out, &[Push::Str(first)]);
    out.push(GET_VARIABLE);
    for p in rest {
        push(out, &[Push::Str(p)]);
        out.push(GET_MEMBER);
    }
}

fn function(params: &[&str], body: &[u8]) -> Vec<u8> {
    let mut header = vec![0];
    header.extend_from_slice(&(params.len() as u16).to_le_bytes());
    for p in params {
        header.extend_from_slice(p.as_bytes());
        header.push(0);
    }
    header.extend_from_slice(&(body.len() as u16).to_le_bytes());
    let mut out = Vec::new();
    record(&mut out, DEFINE_FUNCTION, &header);
    out.extend_from_slice(body);
    out
}

/// A jump record (`If` or `Jump`) by `offset` bytes from its end.
fn branch(out: &mut Vec<u8>, code: u8, offset: usize, back: bool) {
    let o = offset as i16;
    record(out, code, &(if back { -o } else { o }).to_le_bytes());
}

/// Calls `_global.thedas_unempty` on the value on top of the stack.
fn unempty_top(out: &mut Vec<u8>) {
    push(out, &[Push::Int(1)]);
    get_path(out, &["_global"]);
    push(out, &[Push::Str("thedas_unempty")]);
    out.push(CALL_METHOD);
}

/// `function (v) { if (v === EMPTY) return ""; return v; }`
fn unempty_function() -> Vec<u8> {
    let mut then = Vec::new();
    push(&mut then, &[Push::Str("")]);
    then.push(RETURN);
    let mut body = Vec::new();
    get_path(&mut body, &["v"]);
    push(&mut body, &[Push::Str(EMPTY)]);
    body.extend_from_slice(&[STRICT_EQUALS, NOT]);
    branch(&mut body, IF, then.len(), false);
    body.extend(then);
    get_path(&mut body, &["v"]);
    body.push(RETURN);
    function(&["v"], &body)
}

/// `function (o) { return function () { o.apply(_global.ExternalCommands, arguments);
/// return _global.thedas_unempty(flash.external.ExternalInterface.call(RETVAL_CALL)); }; }`
fn wrap_maker() -> Vec<u8> {
    let mut inner = Vec::new();
    get_path(&mut inner, &["arguments"]);
    get_path(&mut inner, &["_global", "ExternalCommands"]);
    push(&mut inner, &[Push::Int(2)]);
    get_path(&mut inner, &["o"]);
    push(&mut inner, &[Push::Str("apply")]);
    inner.extend_from_slice(&[CALL_METHOD, POP]);
    push(&mut inner, &[Push::Str(RETVAL_CALL), Push::Int(1)]);
    get_path(&mut inner, &["flash", "external", "ExternalInterface"]);
    push(&mut inner, &[Push::Str("call")]);
    inner.push(CALL_METHOD);
    unempty_top(&mut inner);
    inner.push(RETURN);
    let mut outer = function(&[], &inner);
    outer.push(RETURN);
    function(&["o"], &outer)
}

/// `function (o) { return function (ev, b) { if (b == undefined) b = _global.thedas_button;
/// return o.call(this, ev, b); }; }`: `GUI.Element.prototype.fnEventHandler`'s wrapper.
fn button_wrap_maker() -> Vec<u8> {
    let mut then = Vec::new();
    push(&mut then, &[Push::Str("b")]);
    get_path(&mut then, &["_global", "thedas_button"]);
    then.push(SET_VARIABLE);
    let mut inner = Vec::new();
    get_path(&mut inner, &["b"]);
    push(&mut inner, &[Push::Undefined]);
    inner.extend_from_slice(&[EQUALS2, NOT]);
    branch(&mut inner, IF, then.len(), false);
    inner.extend(then);
    get_path(&mut inner, &["b"]);
    get_path(&mut inner, &["ev"]);
    get_path(&mut inner, &["this"]);
    push(&mut inner, &[Push::Int(3)]);
    get_path(&mut inner, &["o"]);
    push(&mut inner, &[Push::Str("call")]);
    inner.extend_from_slice(&[CALL_METHOD, RETURN]);
    let mut outer = function(&["ev", "b"], &inner);
    outer.push(RETURN);
    function(&["o"], &outer)
}

/// `function (o) { return function (mc) { if (_global.thedas_later != undefined) {
/// _global.thedas_later.push(mc); return; } return o.apply(this, arguments); }; }`:
/// `MovieUtils.HideMovie`'s wrapper (see [`HIDE_LATER`]).
fn hide_wrap_maker() -> Vec<u8> {
    let mut then = Vec::new();
    get_path(&mut then, &["mc"]);
    push(&mut then, &[Push::Str(PENDING), Push::Int(1)]);
    then.push(SET_MEMBER);
    get_path(&mut then, &["mc"]);
    push(&mut then, &[Push::Int(1)]);
    get_path(&mut then, &["_global", "thedas_later"]);
    push(&mut then, &[Push::Str("push")]);
    then.extend_from_slice(&[CALL_METHOD, POP]);
    push(&mut then, &[Push::Undefined]);
    then.push(RETURN);
    let mut inner = Vec::new();
    get_path(&mut inner, &["_global", "thedas_later"]);
    push(&mut inner, &[Push::Undefined]);
    inner.extend_from_slice(&[EQUALS2]);
    // Skip the putting off when it is undefined.
    branch(&mut inner, IF, then.len(), false);
    inner.extend(then);
    get_path(&mut inner, &["arguments"]);
    get_path(&mut inner, &["this"]);
    push(&mut inner, &[Push::Int(2)]);
    get_path(&mut inner, &["o"]);
    push(&mut inner, &[Push::Str("apply")]);
    inner.extend_from_slice(&[CALL_METHOD, RETURN]);
    let mut outer = function(&["mc"], &inner);
    outer.push(RETURN);
    function(&["o"], &outer)
}

/// `function (o) { return function (f) {
/// if (_global.GUI.Element.cm_mcLastCreatedElement.m_bLoaded) { _global.thedas_after.push(f);
/// return; } return o.apply(this, arguments); }; }`: see [`POST_LOAD_RUN`].
fn post_load_wrap_maker() -> Vec<u8> {
    let mut then = Vec::new();
    get_path(&mut then, &["f"]);
    push(&mut then, &[Push::Int(1)]);
    get_path(&mut then, &["_global", "thedas_after"]);
    push(&mut then, &[Push::Str("push")]);
    then.extend_from_slice(&[CALL_METHOD, POP]);
    push(&mut then, &[Push::Undefined]);
    then.push(RETURN);
    let mut inner = Vec::new();
    get_path(
        &mut inner,
        &[
            "_global",
            "GUI",
            "Element",
            "cm_mcLastCreatedElement",
            "m_bLoaded",
        ],
    );
    inner.push(NOT);
    branch(&mut inner, IF, then.len(), false);
    inner.extend(then);
    get_path(&mut inner, &["arguments"]);
    get_path(&mut inner, &["this"]);
    push(&mut inner, &[Push::Int(2)]);
    get_path(&mut inner, &["o"]);
    push(&mut inner, &[Push::Str("apply")]);
    inner.extend_from_slice(&[CALL_METHOD, RETURN]);
    let mut outer = function(&["f"], &inner);
    outer.push(RETURN);
    function(&["o"], &outer)
}

/// `function () { var a = _global.thedas_after; while (a.length) { var f = a.shift(); f(); } }`
/// (see [`POST_LOAD_RUN`]).
fn run_after_function() -> Vec<u8> {
    let mut body = Vec::new();
    push(&mut body, &[Push::Str("a")]);
    get_path(&mut body, &["_global", "thedas_after"]);
    body.push(DEFINE_LOCAL);
    let mut test = Vec::new();
    get_path(&mut test, &["a", "length"]);
    test.push(NOT);
    let mut step = Vec::new();
    push(&mut step, &[Push::Str("f"), Push::Int(0)]);
    get_path(&mut step, &["a"]);
    push(&mut step, &[Push::Str("shift")]);
    step.extend_from_slice(&[CALL_METHOD, DEFINE_LOCAL]);
    push(&mut step, &[Push::Int(0)]);
    get_path(&mut step, &["f"]);
    push(&mut step, &[Push::Undefined]);
    step.extend_from_slice(&[CALL_METHOD, POP]);
    // Each branch record is 5 bytes.
    let back = test.len() + 5 + step.len() + 5;
    body.extend_from_slice(&test);
    branch(&mut body, IF, step.len() + 5, false);
    body.extend(step);
    branch(&mut body, JUMP, back, true);
    function(&[], &body)
}

/// `function () { var a = _global.thedas_s9; var o = new Array(); var i = 0;
/// while (i < a.length) { var c = a[i]; o.push(c.thedas_s9id); o.push(c._xscale);
/// o.push(c._yscale); i++; } return o; }` (see [`SCALE9_SCALES`]).
fn scale9_scales_function() -> Vec<u8> {
    members_function("thedas_s9", &["thedas_s9id", "_xscale", "_yscale"])
}

/// `function () { var a = _global[list]; var o = new Array(); var i = 0;
/// while (i < a.length) { var c = a[i]; o.push(c[m]) for each member m; i++; } return o; }`:
/// the members of each sprite registered in `_global[list]`, flattened.
fn members_function(list: &str, members: &[&str]) -> Vec<u8> {
    let mut body = Vec::new();
    push(&mut body, &[Push::Str("a")]);
    get_path(&mut body, &["_global", list]);
    body.push(DEFINE_LOCAL);
    push(
        &mut body,
        &[Push::Str("o"), Push::Int(0), Push::Str("Array")],
    );
    body.extend_from_slice(&[NEW_OBJECT, DEFINE_LOCAL]);
    push(&mut body, &[Push::Str("i"), Push::Int(0)]);
    body.push(DEFINE_LOCAL);
    let mut test = Vec::new();
    get_path(&mut test, &["i"]);
    get_path(&mut test, &["a", "length"]);
    test.extend_from_slice(&[LESS2, NOT]);
    let mut step = Vec::new();
    push(&mut step, &[Push::Str("c")]);
    get_path(&mut step, &["a"]);
    get_path(&mut step, &["i"]);
    step.extend_from_slice(&[GET_MEMBER, DEFINE_LOCAL]);
    for m in members {
        get_path(&mut step, &["c", m]);
        push(&mut step, &[Push::Int(1)]);
        get_path(&mut step, &["o"]);
        push(&mut step, &[Push::Str("push")]);
        step.extend_from_slice(&[CALL_METHOD, POP]);
    }
    push(&mut step, &[Push::Str("i")]);
    get_path(&mut step, &["i"]);
    step.extend_from_slice(&[INCREMENT, SET_VARIABLE]);
    // Each branch record is 5 bytes.
    let back = test.len() + 5 + step.len() + 5;
    body.extend_from_slice(&test);
    branch(&mut body, IF, step.len() + 5, false);
    body.extend(step);
    branch(&mut body, JUMP, back, true);
    get_path(&mut body, &["o"]);
    body.push(RETURN);
    function(&[], &body)
}

/// `function (i, u, x, y) { var c = _global.thedas_slots[i].createEmptyMovieClip(
/// "thedas_img", 1); c._x = x; c._y = y; c.loadMovie(u); }` (see [`SLOT_LOAD`]).
fn slot_load_function() -> Vec<u8> {
    let mut body = Vec::new();
    push(
        &mut body,
        &[
            Push::Str("c"),
            Push::Int(1),
            Push::Str("thedas_img"),
            Push::Int(2),
        ],
    );
    get_path(&mut body, &["_global", "thedas_slots"]);
    get_path(&mut body, &["i"]);
    body.push(GET_MEMBER);
    push(&mut body, &[Push::Str("createEmptyMovieClip")]);
    body.extend_from_slice(&[CALL_METHOD, DEFINE_LOCAL]);
    for (m, v) in [("_x", "x"), ("_y", "y")] {
        get_path(&mut body, &["c"]);
        push(&mut body, &[Push::Str(m)]);
        get_path(&mut body, &[v]);
        body.push(SET_MEMBER);
    }
    get_path(&mut body, &["u"]);
    push(&mut body, &[Push::Int(1)]);
    get_path(&mut body, &["c"]);
    push(&mut body, &[Push::Str("loadMovie")]);
    body.extend_from_slice(&[CALL_METHOD, POP]);
    function(&["i", "u", "x", "y"], &body)
}

/// `function (i, k, x, y, a, b) { var p = _global.thedas_s9[i]["thedas_s9_" + k];
/// p._x = x; p._y = y; p._xscale = a; p._yscale = b; }` (see [`SCALE9_SET`]).
fn scale9_set_function() -> Vec<u8> {
    let mut body = Vec::new();
    push(&mut body, &[Push::Str("p")]);
    get_path(&mut body, &["_global", "thedas_s9"]);
    get_path(&mut body, &["i"]);
    body.push(GET_MEMBER);
    push(&mut body, &[Push::Str("thedas_s9_")]);
    get_path(&mut body, &["k"]);
    body.extend_from_slice(&[ADD2, GET_MEMBER, DEFINE_LOCAL]);
    for (m, v) in [("_x", "x"), ("_y", "y"), ("_xscale", "a"), ("_yscale", "b")] {
        get_path(&mut body, &["p"]);
        push(&mut body, &[Push::Str(m)]);
        get_path(&mut body, &[v]);
        body.push(SET_MEMBER);
    }
    function(&["i", "k", "x", "y", "a", "b"], &body)
}

/// Text fields showing inline images (`<img src='ico_gold' .../>` in HTML text, which
/// Scaleform drew and Ruffle leaves out): the coins in money values, rune slots, the
/// character record's line breaks. `SharedLibrary.TextField.prototype.ProcessText`, which
/// every `SetText` passes its text through, is wrapped to note a field whose text has one
/// (its text as `thedas_html`); the host takes them ([`IMG_TAKE`]), lays the images out
/// over the field and leaves room for them in its text (see `UiMovie::layout_inline_images`).
pub const IMG_TAKE: &str = "_global.thedas_img_take";
/// Measures HTML in a field's format: (field, html, wrap) to `[width, height]`.
pub const IMG_MEASURE: &str = "_global.thedas_img_measure";
/// Sets a field's HTML text: (field, html).
pub const IMG_SET: &str = "_global.thedas_img_set";
/// Places image `n` of a field: (field, n, linkage name or "" to clear, x, y) to the
/// image's `[width, height]`.
pub const IMG_PUT: &str = "_global.thedas_img_put";

/// `function (o) { return function (t) { var h = o.apply(this, arguments);
/// if (h.indexOf("<img") != -1) { this.thedas_html = h; if (_global.thedas_imgtf ==
/// undefined) _global.thedas_imgtf = new Array(); _global.thedas_imgtf.push(this); }
/// return h; }; }`: `SharedLibrary.TextField.prototype.ProcessText`'s wrapper (see
/// [`IMG_TAKE`]).
fn process_text_wrap_maker() -> Vec<u8> {
    let mut a = Asm::default();
    let c = &mut a.code;
    push(c, &[Push::Str("h")]);
    get_path(c, &["arguments"]);
    get_path(c, &["this"]);
    push(c, &[Push::Int(2)]);
    get_path(c, &["o"]);
    push(c, &[Push::Str("apply")]);
    c.extend_from_slice(&[CALL_METHOD, DEFINE_LOCAL]);
    push(c, &[Push::Str("<img"), Push::Int(1)]);
    get_path(c, &["h"]);
    push(c, &[Push::Str("indexOf")]);
    c.push(CALL_METHOD);
    push(c, &[Push::Int(-1)]);
    c.push(EQUALS2);
    a.branch(IF, "done");
    let c = &mut a.code;
    get_path(c, &["this"]);
    push(c, &[Push::Str("thedas_html")]);
    get_path(c, &["h"]);
    c.push(SET_MEMBER);
    get_path(c, &["_global", "thedas_imgtf"]);
    push(c, &[Push::Undefined]);
    c.extend_from_slice(&[EQUALS2, NOT]);
    a.branch(IF, "have");
    let c = &mut a.code;
    get_path(c, &["_global"]);
    push(
        c,
        &[Push::Str("thedas_imgtf"), Push::Int(0), Push::Str("Array")],
    );
    c.extend_from_slice(&[NEW_OBJECT, SET_MEMBER]);
    a.label("have");
    let c = &mut a.code;
    get_path(c, &["this"]);
    push(c, &[Push::Int(1)]);
    get_path(c, &["_global", "thedas_imgtf"]);
    push(c, &[Push::Str("push")]);
    c.extend_from_slice(&[CALL_METHOD, POP]);
    a.label("done");
    get_path(&mut a.code, &["h"]);
    a.code.push(RETURN);
    let mut outer = function(&["t"], &a.finish());
    outer.push(RETURN);
    function(&["o"], &outer)
}

/// `function () { var a = _global.thedas_imgtf; _global.thedas_imgtf = new Array();
/// if (_global.thedas_imgall == undefined) _global.thedas_imgall = new Array();
/// var o = new Array(); for (var i = 0; i < a.length; i++) { var c = a[i];
/// if (c.thedas_imgk == undefined) { c.thedas_imgk = _global.thedas_imgall.length;
/// _global.thedas_imgall.push(c); } o.push(c.thedas_imgk); o.push(c.thedas_html); }
/// return o; }` (see [`IMG_TAKE`]).
fn img_take_function() -> Vec<u8> {
    let mut a = Asm::default();
    let c = &mut a.code;
    push(c, &[Push::Str("a")]);
    get_path(c, &["_global", "thedas_imgtf"]);
    c.push(DEFINE_LOCAL);
    get_path(c, &["_global"]);
    push(
        c,
        &[Push::Str("thedas_imgtf"), Push::Int(0), Push::Str("Array")],
    );
    c.extend_from_slice(&[NEW_OBJECT, SET_MEMBER]);
    get_path(c, &["_global", "thedas_imgall"]);
    push(c, &[Push::Undefined]);
    c.extend_from_slice(&[EQUALS2, NOT]);
    a.branch(IF, "all");
    let c = &mut a.code;
    get_path(c, &["_global"]);
    push(
        c,
        &[Push::Str("thedas_imgall"), Push::Int(0), Push::Str("Array")],
    );
    c.extend_from_slice(&[NEW_OBJECT, SET_MEMBER]);
    a.label("all");
    let c = &mut a.code;
    push(c, &[Push::Str("o"), Push::Int(0), Push::Str("Array")]);
    c.extend_from_slice(&[NEW_OBJECT, DEFINE_LOCAL]);
    push(c, &[Push::Str("i"), Push::Int(0)]);
    c.push(DEFINE_LOCAL);
    a.label("loop");
    let c = &mut a.code;
    get_path(c, &["i"]);
    get_path(c, &["a", "length"]);
    c.extend_from_slice(&[LESS2, NOT]);
    a.branch(IF, "end");
    let c = &mut a.code;
    push(c, &[Push::Str("c")]);
    get_path(c, &["a"]);
    get_path(c, &["i"]);
    c.extend_from_slice(&[GET_MEMBER, DEFINE_LOCAL]);
    get_path(c, &["c", "thedas_imgk"]);
    push(c, &[Push::Undefined]);
    c.extend_from_slice(&[EQUALS2, NOT]);
    a.branch(IF, "known");
    let c = &mut a.code;
    get_path(c, &["c"]);
    push(c, &[Push::Str("thedas_imgk")]);
    get_path(c, &["_global", "thedas_imgall", "length"]);
    c.push(SET_MEMBER);
    get_path(c, &["c"]);
    push(c, &[Push::Int(1)]);
    get_path(c, &["_global", "thedas_imgall"]);
    push(c, &[Push::Str("push")]);
    c.extend_from_slice(&[CALL_METHOD, POP]);
    a.label("known");
    let c = &mut a.code;
    for m in ["thedas_imgk", "thedas_html"] {
        get_path(c, &["c", m]);
        push(c, &[Push::Int(1)]);
        get_path(c, &["o"]);
        push(c, &[Push::Str("push")]);
        c.extend_from_slice(&[CALL_METHOD, POP]);
    }
    push(c, &[Push::Str("i")]);
    get_path(c, &["i"]);
    c.extend_from_slice(&[INCREMENT, SET_VARIABLE]);
    a.branch(JUMP, "loop");
    a.label("end");
    get_path(&mut a.code, &["o"]);
    a.code.push(RETURN);
    function(&[], &a.finish())
}

/// `function (k, h, w) { var t = _global.thedas_imgall[k].TextField; var m =
/// _root.thedas_meas; if (m == undefined) { _root.createTextField("thedas_meas", 1048000,
/// -10000, -10000, 100, 100); m = _root.thedas_meas; m._visible = false; }
/// m.autoSize = "left"; m.embedFonts = t.embedFonts; m.multiline = t.multiline;
/// m.wordWrap = w && t.wordWrap; m._width = t._width; m.html = true;
/// m.setNewTextFormat(t.getNewTextFormat()); m.htmlText = h;
/// return [m.textWidth, m.textHeight]; }` (see [`IMG_MEASURE`]).
fn img_measure_function() -> Vec<u8> {
    let mut a = Asm::default();
    let c = &mut a.code;
    push(c, &[Push::Str("t")]);
    get_path(c, &["_global", "thedas_imgall"]);
    get_path(c, &["k"]);
    c.push(GET_MEMBER);
    push(c, &[Push::Str("TextField")]);
    c.extend_from_slice(&[GET_MEMBER, DEFINE_LOCAL]);
    push(c, &[Push::Str("m")]);
    get_path(c, &["_root", "thedas_meas"]);
    c.push(DEFINE_LOCAL);
    get_path(c, &["m"]);
    push(c, &[Push::Undefined]);
    c.extend_from_slice(&[EQUALS2, NOT]);
    a.branch(IF, "have");
    let c = &mut a.code;
    push(
        c,
        &[
            Push::Int(100),
            Push::Int(100),
            Push::Int(-10000),
            Push::Int(-10000),
            Push::Int(1048000),
            Push::Str("thedas_meas"),
            Push::Int(6),
        ],
    );
    get_path(c, &["_root"]);
    push(c, &[Push::Str("createTextField")]);
    c.extend_from_slice(&[CALL_METHOD, POP]);
    push(c, &[Push::Str("m")]);
    get_path(c, &["_root", "thedas_meas"]);
    c.push(SET_VARIABLE);
    get_path(c, &["m"]);
    push(c, &[Push::Str("_visible"), Push::Bool(false)]);
    c.push(SET_MEMBER);
    a.label("have");
    let c = &mut a.code;
    get_path(c, &["m"]);
    push(c, &[Push::Str("autoSize"), Push::Str("left")]);
    c.push(SET_MEMBER);
    for m in ["embedFonts", "multiline", "_width"] {
        get_path(c, &["m"]);
        push(c, &[Push::Str(m)]);
        get_path(c, &["t", m]);
        c.push(SET_MEMBER);
    }
    get_path(c, &["m"]);
    push(c, &[Push::Str("wordWrap")]);
    get_path(c, &["w"]);
    get_path(c, &["t", "wordWrap"]);
    c.push(0x10); // And
    c.push(SET_MEMBER);
    get_path(c, &["m"]);
    push(c, &[Push::Str("html"), Push::Bool(true)]);
    c.push(SET_MEMBER);
    push(c, &[Push::Int(0)]);
    get_path(c, &["t"]);
    push(c, &[Push::Str("getNewTextFormat")]);
    c.push(CALL_METHOD);
    push(c, &[Push::Int(1)]);
    get_path(c, &["m"]);
    push(c, &[Push::Str("setNewTextFormat")]);
    c.extend_from_slice(&[CALL_METHOD, POP]);
    get_path(c, &["m"]);
    push(c, &[Push::Str("htmlText")]);
    get_path(c, &["h"]);
    c.push(SET_MEMBER);
    get_path(c, &["m", "textHeight"]);
    get_path(c, &["m", "textWidth"]);
    push(c, &[Push::Int(2)]);
    c.extend_from_slice(&[INIT_ARRAY, RETURN]);
    function(&["k", "h", "w"], &a.finish())
}

/// `function (k, h) { _global.thedas_imgall[k].TextField.htmlText = h; }`
fn img_set_function() -> Vec<u8> {
    let mut c = Vec::new();
    get_path(&mut c, &["_global", "thedas_imgall"]);
    get_path(&mut c, &["k"]);
    c.push(GET_MEMBER);
    push(&mut c, &[Push::Str("TextField")]);
    c.push(GET_MEMBER);
    push(&mut c, &[Push::Str("htmlText")]);
    get_path(&mut c, &["h"]);
    c.push(SET_MEMBER);
    function(&["k", "h"], &c)
}

/// `function (k, n, s, x, y) { var c = _global.thedas_imgall[k]; var t = c.TextField;
/// var m = c.createEmptyMovieClip("thedas_img" + n, 10000 + n); if (s != "")
/// m.attachBitmap(flash.display.BitmapData.loadBitmap(s), 1); m._x = t._x + x;
/// m._y = t._y + y; return [m._width, m._height]; }` (see [`IMG_PUT`]).
fn img_put_function() -> Vec<u8> {
    let mut a = Asm::default();
    let c = &mut a.code;
    push(c, &[Push::Str("c")]);
    get_path(c, &["_global", "thedas_imgall"]);
    get_path(c, &["k"]);
    c.extend_from_slice(&[GET_MEMBER, DEFINE_LOCAL]);
    push(c, &[Push::Str("t")]);
    get_path(c, &["c", "TextField"]);
    c.push(DEFINE_LOCAL);
    push(c, &[Push::Str("m"), Push::Int(10000)]);
    get_path(c, &["n"]);
    c.push(ADD2);
    push(c, &[Push::Str("thedas_img")]);
    get_path(c, &["n"]);
    c.push(ADD2);
    push(c, &[Push::Int(2)]);
    get_path(c, &["c"]);
    push(c, &[Push::Str("createEmptyMovieClip")]);
    c.extend_from_slice(&[CALL_METHOD, DEFINE_LOCAL]);
    get_path(c, &["s"]);
    push(c, &[Push::Str("")]);
    c.push(EQUALS2);
    a.branch(IF, "place");
    let c = &mut a.code;
    push(c, &[Push::Int(1)]);
    get_path(c, &["s"]);
    push(c, &[Push::Int(1)]);
    get_path(c, &["flash", "display", "BitmapData"]);
    push(c, &[Push::Str("loadBitmap")]);
    c.push(CALL_METHOD);
    push(c, &[Push::Int(2)]);
    get_path(c, &["m"]);
    push(c, &[Push::Str("attachBitmap")]);
    c.extend_from_slice(&[CALL_METHOD, POP]);
    a.label("place");
    let c = &mut a.code;
    for (m, v) in [("_x", "x"), ("_y", "y")] {
        get_path(c, &["m"]);
        push(c, &[Push::Str(m)]);
        get_path(c, &["t", m]);
        get_path(c, &[v]);
        c.push(ADD2);
        c.push(SET_MEMBER);
    }
    get_path(c, &["m", "_height"]);
    get_path(c, &["m", "_width"]);
    push(c, &[Push::Int(2)]);
    c.extend_from_slice(&[INIT_ARRAY, RETURN]);
    function(&["k", "n", "s", "x", "y"], &a.finish())
}

/// Code with forward and backward branches by label.
#[derive(Default)]
struct Asm {
    code: Vec<u8>,
    labels: std::collections::HashMap<&'static str, usize>,
    fixups: Vec<(usize, &'static str)>,
}

impl Asm {
    fn label(&mut self, l: &'static str) {
        self.labels.insert(l, self.code.len());
    }

    /// An `If` or `Jump` to `l`.
    fn branch(&mut self, op: u8, l: &'static str) {
        record(&mut self.code, op, &[0, 0]);
        self.fixups.push((self.code.len(), l));
    }

    fn finish(mut self) -> Vec<u8> {
        for (end, l) in std::mem::take(&mut self.fixups) {
            let target = self.labels.get(l).copied().unwrap_or(end);
            let off = (target as i64 - end as i64) as i16;
            self.code[end - 2..end].copy_from_slice(&off.to_le_bytes());
        }
        self.code
    }
}

const ENUMERATE2: u8 = 0x55;
const TYPEOF: u8 = 0x44;
const GREATER: u8 = 0x67;
const STORE_REGISTER: u8 = 0x87;

/// Scaleform's `Mouse.getTopMostEntity(testAll)`, which Ruffle lacks (it returned undefined,
/// so a radial menu's release never found the button under the mouse and every inventory
/// right-click menu cancelled itself at once): `function (t) { return
/// _global.thedas_top(_root, t); }`.
fn top_most_entity_function() -> Vec<u8> {
    let mut body = Vec::new();
    get_path(&mut body, &["t"]);
    get_path(&mut body, &["_root"]);
    push(&mut body, &[Push::Int(2)]);
    get_path(&mut body, &["_global"]);
    push(&mut body, &[Push::Str("thedas_top")]);
    body.extend_from_slice(&[CALL_METHOD, RETURN]);
    function(&["t"], &body)
}

/// `function (c, t)`: the deepest clip under the mouse below `c`, trying the visible
/// children the mouse is over from the topmost (highest depth) down, depth first; with `t`
/// false,
/// only a clip with a mouse handler (`onPress`, `onRelease`, `onRollOver`, `onDragOver`,
/// `onReleaseOutside`) counts, as Scaleform's `testAll` false. Provisional: Scaleform's
/// own search is not traced; text fields and buttons are not considered.
fn top_function() -> Vec<u8> {
    let mut a = Asm::default();
    let c = &mut a.code;
    push(c, &[Push::Str("lim"), Push::Int(i32::MAX)]);
    c.push(DEFINE_LOCAL);
    push(c, &[Push::Str("x")]);
    get_path(c, &["_root", "_xmouse"]);
    c.push(DEFINE_LOCAL);
    push(c, &[Push::Str("y")]);
    get_path(c, &["_root", "_ymouse"]);
    c.push(DEFINE_LOCAL);
    // Each pass takes the topmost child under the mouse below the last one tried.
    a.label("pass");
    let c = &mut a.code;
    push(c, &[Push::Str("b"), Push::Undefined]);
    c.push(DEFINE_LOCAL);
    push(c, &[Push::Str("d"), Push::Int(i32::MIN)]);
    c.push(DEFINE_LOCAL);
    get_path(c, &["c"]);
    c.push(ENUMERATE2);
    a.label("next");
    // var k = the next name (null after the last).
    record(&mut a.code, STORE_REGISTER, &[0]);
    a.code.push(POP);
    push(&mut a.code, &[Push::Register(0), Push::Null]);
    a.code.push(EQUALS2);
    a.branch(IF, "done");
    let c = &mut a.code;
    push(c, &[Push::Str("o")]);
    get_path(c, &["c"]);
    push(c, &[Push::Register(0)]);
    c.extend_from_slice(&[GET_MEMBER, DEFINE_LOCAL]);
    get_path(c, &["o"]);
    c.push(TYPEOF);
    push(c, &[Push::Str("movieclip")]);
    c.extend_from_slice(&[STRICT_EQUALS, NOT]);
    a.branch(IF, "next");
    let c = &mut a.code;
    get_path(c, &["o", "_parent"]);
    get_path(c, &["c"]);
    c.extend_from_slice(&[EQUALS2, NOT]);
    a.branch(IF, "next");
    let c = &mut a.code;
    get_path(c, &["o", "_visible"]);
    c.push(NOT);
    a.branch(IF, "next");
    let c = &mut a.code;
    push(c, &[Push::Bool(true)]);
    get_path(c, &["y"]);
    get_path(c, &["x"]);
    push(c, &[Push::Int(3)]);
    get_path(c, &["o"]);
    push(c, &[Push::Str("hitTest")]);
    c.extend_from_slice(&[CALL_METHOD, NOT]);
    a.branch(IF, "next");
    let c = &mut a.code;
    push(c, &[Push::Str("g"), Push::Int(0)]);
    get_path(c, &["o"]);
    push(c, &[Push::Str("getDepth")]);
    c.extend_from_slice(&[CALL_METHOD, DEFINE_LOCAL]);
    get_path(c, &["g"]);
    get_path(c, &["d"]);
    c.extend_from_slice(&[GREATER, NOT]);
    a.branch(IF, "next");
    let c = &mut a.code;
    get_path(c, &["g"]);
    get_path(c, &["lim"]);
    c.extend_from_slice(&[LESS2, NOT]);
    a.branch(IF, "next");
    let c = &mut a.code;
    push(c, &[Push::Str("d")]);
    get_path(c, &["g"]);
    c.push(SET_VARIABLE);
    push(c, &[Push::Str("b")]);
    get_path(c, &["o"]);
    c.push(SET_VARIABLE);
    a.branch(JUMP, "next");
    a.label("done");
    let c = &mut a.code;
    get_path(c, &["b"]);
    push(c, &[Push::Undefined]);
    c.push(EQUALS2);
    a.branch(IF, "none");
    // var e = _global.thedas_top(b, t); if (e != undefined) return e;
    let c = &mut a.code;
    push(c, &[Push::Str("e")]);
    get_path(c, &["t"]);
    get_path(c, &["b"]);
    push(c, &[Push::Int(2)]);
    get_path(c, &["_global"]);
    push(c, &[Push::Str("thedas_top")]);
    c.extend_from_slice(&[CALL_METHOD, DEFINE_LOCAL]);
    get_path(c, &["e"]);
    push(c, &[Push::Undefined]);
    c.push(EQUALS2);
    a.branch(IF, "self");
    get_path(&mut a.code, &["e"]);
    a.code.push(RETURN);
    a.label("self");
    get_path(&mut a.code, &["t"]);
    a.branch(IF, "found");
    for h in [
        "onPress",
        "onRelease",
        "onRollOver",
        "onDragOver",
        "onReleaseOutside",
    ] {
        get_path(&mut a.code, &["b", h]);
        a.branch(IF, "found");
    }
    // Nothing in it takes the mouse: the next child down.
    push(&mut a.code, &[Push::Str("lim")]);
    get_path(&mut a.code, &["d"]);
    a.code.push(SET_VARIABLE);
    a.branch(JUMP, "pass");
    a.label("found");
    get_path(&mut a.code, &["b"]);
    a.code.push(RETURN);
    a.label("none");
    push(&mut a.code, &[Push::Undefined]);
    a.code.push(RETURN);
    function(&["c", "t"], &a.finish())
}

/// `function (l) { if (l.thedas_proxy == undefined && !l.thedas_is_proxy) { var p = new
/// Object(); p.thedas_is_proxy = 1; p.l = l; p.onMouseDown = function () {
/// this.l.onMouseDown(1, Mouse.getTopMostEntity(false)); }; (onMouseUp likewise);
/// p.onMouseMove = function () { this.l.onMouseMove(); }; p.onMouseWheel = function (d, t)
/// { this.l.onMouseWheel(d, t); }; l.thedas_proxy = p; } return l.thedas_is_proxy ? l :
/// l.thedas_proxy; }`: a mouse listener as Scaleform called it. Scaleform passed the
/// button and the entity under the mouse to `onMouseDown`/`onMouseUp`; Ruffle passes
/// nothing, and `GUI.Element`'s listener takes a missing button for a right or middle one
/// and presses the entity under the mouse itself, twice over Ruffle's own press. The host
/// delivers every button as the left one (see [`BUTTON_VAR`]), so the left (1) is passed.
fn listener_proxy_function() -> Vec<u8> {
    let forward = |m: &str, top: bool| {
        let mut b = Vec::new();
        if top {
            push(&mut b, &[Push::Bool(false), Push::Int(1)]);
            get_path(&mut b, &["Mouse"]);
            push(&mut b, &[Push::Str("getTopMostEntity")]);
            b.push(CALL_METHOD);
            push(&mut b, &[Push::Int(1), Push::Int(2)]);
        } else {
            push(&mut b, &[Push::Int(0)]);
        }
        get_path(&mut b, &["this", "l"]);
        push(&mut b, &[Push::Str(m)]);
        b.extend_from_slice(&[CALL_METHOD, POP]);
        function(&[], &b)
    };
    let mut wheel = Vec::new();
    get_path(&mut wheel, &["t"]);
    get_path(&mut wheel, &["d"]);
    push(&mut wheel, &[Push::Int(2)]);
    get_path(&mut wheel, &["this", "l"]);
    push(&mut wheel, &[Push::Str("onMouseWheel")]);
    wheel.extend_from_slice(&[CALL_METHOD, POP]);
    let mut a = Asm::default();
    get_path(&mut a.code, &["l", "thedas_is_proxy"]);
    a.branch(IF, "proxy");
    get_path(&mut a.code, &["l", "thedas_proxy"]);
    push(&mut a.code, &[Push::Undefined]);
    a.code.extend_from_slice(&[EQUALS2, NOT]);
    a.branch(IF, "made");
    let c = &mut a.code;
    push(c, &[Push::Str("p"), Push::Int(0), Push::Str("Object")]);
    c.extend_from_slice(&[NEW_OBJECT, DEFINE_LOCAL]);
    get_path(c, &["p"]);
    push(c, &[Push::Str("thedas_is_proxy"), Push::Int(1)]);
    c.push(SET_MEMBER);
    get_path(c, &["p"]);
    push(c, &[Push::Str("l")]);
    get_path(c, &["l"]);
    c.push(SET_MEMBER);
    for (m, top) in [
        ("onMouseDown", true),
        ("onMouseUp", true),
        ("onMouseMove", false),
    ] {
        get_path(c, &["p"]);
        push(c, &[Push::Str(m)]);
        c.extend(forward(m, top));
        c.push(SET_MEMBER);
    }
    get_path(c, &["p"]);
    push(c, &[Push::Str("onMouseWheel")]);
    c.extend(function(&["d", "t"], &wheel));
    c.push(SET_MEMBER);
    get_path(c, &["l"]);
    push(c, &[Push::Str("thedas_proxy")]);
    get_path(c, &["p"]);
    c.push(SET_MEMBER);
    a.label("made");
    get_path(&mut a.code, &["l", "thedas_proxy"]);
    a.code.push(RETURN);
    a.label("proxy");
    get_path(&mut a.code, &["l"]);
    a.code.push(RETURN);
    function(&["l"], &a.finish())
}

/// `function (o) { return function (l) { return o.call(Mouse,
/// _global.thedas_listener(l)); }; }`: `Mouse.addListener`'s and `Mouse.removeListener`'s
/// wrapper (see [`listener_proxy_function`]).
fn listener_wrap_maker() -> Vec<u8> {
    let mut inner = Vec::new();
    get_path(&mut inner, &["l"]);
    push(&mut inner, &[Push::Int(1)]);
    get_path(&mut inner, &["_global"]);
    push(&mut inner, &[Push::Str("thedas_listener")]);
    inner.push(CALL_METHOD);
    get_path(&mut inner, &["Mouse"]);
    push(&mut inner, &[Push::Int(2)]);
    get_path(&mut inner, &["o"]);
    push(&mut inner, &[Push::Str("call")]);
    inner.extend_from_slice(&[CALL_METHOD, RETURN]);
    let mut outer = function(&["l"], &inner);
    outer.push(RETURN);
    function(&["o"], &outer)
}

/// `function () { var a = Mouse._listeners; for (var i = 0; i < a.length; i++) a[i] =
/// _global.thedas_listener(a[i]); }`: the listeners added before the wrappers, as proxies.
fn proxy_listeners_function() -> Vec<u8> {
    let mut a = Asm::default();
    let c = &mut a.code;
    push(c, &[Push::Str("a")]);
    get_path(c, &["Mouse", "_listeners"]);
    c.push(DEFINE_LOCAL);
    push(c, &[Push::Str("i"), Push::Int(0)]);
    c.push(DEFINE_LOCAL);
    a.label("loop");
    let c = &mut a.code;
    get_path(c, &["i"]);
    get_path(c, &["a", "length"]);
    c.extend_from_slice(&[LESS2, NOT]);
    a.branch(IF, "end");
    let c = &mut a.code;
    get_path(c, &["a"]);
    get_path(c, &["i"]);
    get_path(c, &["a"]);
    get_path(c, &["i"]);
    c.push(GET_MEMBER);
    push(c, &[Push::Int(1)]);
    get_path(c, &["_global"]);
    push(c, &[Push::Str("thedas_listener")]);
    c.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    push(c, &[Push::Str("i")]);
    get_path(c, &["i"]);
    c.extend_from_slice(&[INCREMENT, SET_VARIABLE]);
    a.branch(JUMP, "loop");
    a.label("end");
    function(&[], &a.finish())
}

/// The member marking a clip whose hiding is put off (cleared by a `ShowMovie` meanwhile).
const PENDING: &str = "thedas_hide_pending";

/// `function (o) { return function (mc) { mc.thedas_hide_pending = 0;
/// return o.apply(this, arguments); }; }`: `MovieUtils.ShowMovie`'s wrapper. A clip shown
/// again after a hide was put off stays shown (tactics.gfx's action picker hides its ability
/// list and shows it again in the click that opens it).
fn show_wrap_maker() -> Vec<u8> {
    let mut inner = Vec::new();
    get_path(&mut inner, &["mc"]);
    push(&mut inner, &[Push::Str(PENDING), Push::Int(0)]);
    inner.push(SET_MEMBER);
    get_path(&mut inner, &["arguments"]);
    get_path(&mut inner, &["this"]);
    push(&mut inner, &[Push::Int(2)]);
    get_path(&mut inner, &["o"]);
    push(&mut inner, &[Push::Str("apply")]);
    inner.extend_from_slice(&[CALL_METHOD, RETURN]);
    let mut outer = function(&["mc"], &inner);
    outer.push(RETURN);
    function(&["o"], &outer)
}

/// `function () { var a = _global.thedas_later; _global.thedas_later = undefined;
/// while (a.length) { var m = a.pop(); if (m.thedas_hide_pending) {
/// m.thedas_hide_pending = 0; _global.MovieUtils.HideMovie(m); } } }` (see [`HIDE_FLUSH`]).
fn flush_function() -> Vec<u8> {
    let mut body = Vec::new();
    push(&mut body, &[Push::Str("a")]);
    get_path(&mut body, &["_global", "thedas_later"]);
    body.push(DEFINE_LOCAL);
    get_path(&mut body, &["_global"]);
    push(&mut body, &[Push::Str("thedas_later"), Push::Undefined]);
    body.push(SET_MEMBER);
    let mut test = Vec::new();
    get_path(&mut test, &["a", "length"]);
    test.push(NOT);
    let mut step = Vec::new();
    push(&mut step, &[Push::Str("m"), Push::Int(0)]);
    get_path(&mut step, &["a"]);
    push(&mut step, &[Push::Str("pop")]);
    step.extend_from_slice(&[CALL_METHOD, DEFINE_LOCAL]);
    let mut hide = Vec::new();
    get_path(&mut hide, &["m"]);
    push(&mut hide, &[Push::Str(PENDING), Push::Int(0)]);
    hide.push(SET_MEMBER);
    get_path(&mut hide, &["m"]);
    push(&mut hide, &[Push::Int(1)]);
    get_path(&mut hide, &["_global", "MovieUtils"]);
    push(&mut hide, &[Push::Str("HideMovie")]);
    hide.extend_from_slice(&[CALL_METHOD, POP]);
    get_path(&mut step, &["m", PENDING]);
    step.push(NOT);
    branch(&mut step, IF, hide.len(), false);
    step.extend(hide);
    // Each branch record is 5 bytes.
    let back = test.len() + 5 + step.len() + 5;
    body.extend_from_slice(&test);
    branch(&mut body, IF, step.len() + 5, false);
    body.extend(step);
    branch(&mut body, JUMP, back, true);
    function(&[], &body)
}

/// `function (o) { return function () {
/// return _global.thedas_unempty(o.apply(_global.ExternalCommands, arguments)); }; }`
fn getter_wrap_maker() -> Vec<u8> {
    let mut inner = Vec::new();
    get_path(&mut inner, &["arguments"]);
    get_path(&mut inner, &["_global", "ExternalCommands"]);
    push(&mut inner, &[Push::Int(2)]);
    get_path(&mut inner, &["o"]);
    push(&mut inner, &[Push::Str("apply")]);
    inner.push(CALL_METHOD);
    unempty_top(&mut inner);
    inner.push(RETURN);
    let mut outer = function(&[], &inner);
    outer.push(RETURN);
    function(&["o"], &outer)
}

/// `function (o, t, m, c) { return function () {
/// t[m] = flash.external.ExternalInterface.call(c, arguments[0]);
/// return o.apply(this, arguments); }; }`
fn prewrap_maker() -> Vec<u8> {
    let mut inner = Vec::new();
    get_path(&mut inner, &["t"]);
    get_path(&mut inner, &["m"]);
    get_path(&mut inner, &["arguments"]);
    push(&mut inner, &[Push::Int(0)]);
    inner.push(GET_MEMBER);
    get_path(&mut inner, &["c"]);
    push(&mut inner, &[Push::Int(2)]);
    get_path(&mut inner, &["flash", "external", "ExternalInterface"]);
    push(&mut inner, &[Push::Str("call")]);
    inner.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    get_path(&mut inner, &["arguments"]);
    get_path(&mut inner, &["this"]);
    push(&mut inner, &[Push::Int(2)]);
    get_path(&mut inner, &["o"]);
    push(&mut inner, &[Push::Str("apply")]);
    inner.extend_from_slice(&[CALL_METHOD, RETURN]);
    let mut outer = function(&[], &inner);
    outer.push(RETURN);
    function(&["o", "t", "m", "c"], &outer)
}

fn record(out: &mut Vec<u8>, code: u8, body: &[u8]) {
    out.push(code);
    out.extend_from_slice(&(body.len() as u16).to_le_bytes());
    out.extend_from_slice(body);
}

fn push(out: &mut Vec<u8>, values: &[Push]) {
    let mut b = Vec::new();
    for v in values {
        match v {
            Push::Str(s) => {
                b.push(0);
                b.extend_from_slice(s.as_bytes());
                b.push(0);
            }
            Push::Int(i) => {
                b.push(7);
                b.extend_from_slice(&i.to_le_bytes());
            }
            Push::Null => b.push(2),
            Push::Undefined => b.push(3),
            Push::Bool(v) => {
                b.push(5);
                b.push(*v as u8);
            }
            Push::Register(r) => {
                b.push(4);
                b.push(*r);
            }
        }
    }
    record(out, PUSH, &b);
}

/// `function (p, m, a) { for (var i = 0; i < a.length; i++) a[i] = _global.thedas_unempty(a[i]);
/// var o = eval(p); return o[m].apply(o, a); }`
fn invoke_function() -> Vec<u8> {
    invoke_body()
}

fn invoke_body() -> Vec<u8> {
    let mut body = Vec::new();
    push(&mut body, &[Push::Str("i"), Push::Int(0)]);
    body.push(DEFINE_LOCAL);
    let mut test = Vec::new();
    get_path(&mut test, &["i"]);
    get_path(&mut test, &["a", "length"]);
    test.extend_from_slice(&[LESS2, NOT]);
    let mut step = Vec::new();
    get_path(&mut step, &["a"]);
    get_path(&mut step, &["i"]);
    get_path(&mut step, &["a"]);
    get_path(&mut step, &["i"]);
    step.push(GET_MEMBER);
    unempty_top(&mut step);
    step.push(SET_MEMBER);
    push(&mut step, &[Push::Str("i")]);
    get_path(&mut step, &["i"]);
    step.extend_from_slice(&[INCREMENT, SET_VARIABLE]);
    // Each branch record is 5 bytes.
    let back = test.len() + 5 + step.len() + 5;
    body.extend_from_slice(&test);
    branch(&mut body, IF, step.len() + 5, false);
    body.extend(step);
    branch(&mut body, JUMP, back, true);
    push(&mut body, &[Push::Str("a")]);
    body.push(GET_VARIABLE);
    push(&mut body, &[Push::Str("p")]);
    body.extend_from_slice(&[GET_VARIABLE, GET_VARIABLE]);
    push(&mut body, &[Push::Int(2), Push::Str("p")]);
    body.extend_from_slice(&[GET_VARIABLE, GET_VARIABLE]);
    push(&mut body, &[Push::Str("m")]);
    body.extend_from_slice(&[GET_VARIABLE, GET_MEMBER]);
    push(&mut body, &[Push::Str("apply")]);
    body.extend_from_slice(&[CALL_METHOD, RETURN]);
    function(&["p", "m", "a"], &body)
}

/// The body of a `DoAction` tag registering [`INVOKE_CALLBACK`]
/// (`flash.external.ExternalInterface.addCallback("thedas_invoke", null, function ...)`) and
/// wrapping [`RETURNING_FSCOMMANDS`].
pub fn register_invoke() -> Vec<u8> {
    let mut out = Vec::new();
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_unempty")]);
    out.extend(unempty_function());
    out.push(SET_MEMBER);
    // _global.thedas_wrap = wrap_maker
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_wrap")]);
    out.extend(wrap_maker());
    out.push(SET_MEMBER);
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_getter_wrap")]);
    out.extend(getter_wrap_maker());
    out.push(SET_MEMBER);
    for f in VALUE_GETTERS {
        get_path(&mut out, &["_global", "ExternalCommands"]);
        push(&mut out, &[Push::Str(f)]);
        get_path(&mut out, &["_global", "ExternalCommands", f]);
        push(&mut out, &[Push::Int(1)]);
        get_path(&mut out, &["_global"]);
        push(&mut out, &[Push::Str("thedas_getter_wrap")]);
        out.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    }
    for f in RETURNING_FSCOMMANDS {
        // _global.ExternalCommands[f] = _global.thedas_wrap(_global.ExternalCommands[f])
        get_path(&mut out, &["_global", "ExternalCommands"]);
        push(&mut out, &[Push::Str(f)]);
        get_path(&mut out, &["_global", "ExternalCommands", f]);
        push(&mut out, &[Push::Int(1)]);
        get_path(&mut out, &["_global"]);
        push(&mut out, &[Push::Str("thedas_wrap")]);
        out.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    }
    // _global.thedas_prewrap = prewrap_maker; _global.thedas_install wraps the PRE_CALLS
    // functions (the host calls it once the movie's classes exist, see `INSTALL`).
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_prewrap")]);
    out.extend(prewrap_maker());
    out.push(SET_MEMBER);
    let mut install = Vec::new();
    for f in DA_RETURNING_FSCOMMANDS {
        // _global.DAExternalCommands[f] = _global.thedas_wrap(_global.DAExternalCommands[f])
        get_path(&mut install, &["_global", "DAExternalCommands"]);
        push(&mut install, &[Push::Str(f)]);
        get_path(&mut install, &["_global", "DAExternalCommands", f]);
        push(&mut install, &[Push::Int(1)]);
        get_path(&mut install, &["_global"]);
        push(&mut install, &[Push::Str("thedas_wrap")]);
        install.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    }
    for p in PRE_CALLS {
        get_path(&mut install, p.object);
        push(
            &mut install,
            &[
                Push::Str(p.function),
                Push::Str(p.call),
                Push::Str(p.variable),
            ],
        );
        get_path(&mut install, p.target);
        let mut func = p.object.to_vec();
        func.push(p.function);
        get_path(&mut install, &func);
        push(&mut install, &[Push::Int(4)]);
        get_path(&mut install, &["_global"]);
        push(&mut install, &[Push::Str("thedas_prewrap")]);
        install.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    }
    // GUI.Element.prototype.fnEventHandler = thedas_button_wrap(...), for elements set up
    // from now on (see BUTTON_VAR).
    let element = ["_global", "GUI", "Element", "prototype"];
    get_path(&mut install, &element);
    push(&mut install, &[Push::Str("fnEventHandler")]);
    let mut handler = element.to_vec();
    handler.push("fnEventHandler");
    get_path(&mut install, &handler);
    push(&mut install, &[Push::Int(1)]);
    get_path(&mut install, &["_global"]);
    push(&mut install, &[Push::Str("thedas_button_wrap")]);
    install.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_button_wrap")]);
    out.extend(button_wrap_maker());
    out.push(SET_MEMBER);
    // _global.MovieUtils.HideMovie = thedas_hide_wrap(_global.MovieUtils.HideMovie) (see
    // HIDE_LATER).
    get_path(&mut install, &["_global", "MovieUtils"]);
    push(&mut install, &[Push::Str("HideMovie")]);
    get_path(&mut install, &["_global", "MovieUtils", "HideMovie"]);
    push(&mut install, &[Push::Int(1)]);
    get_path(&mut install, &["_global"]);
    push(&mut install, &[Push::Str("thedas_hide_wrap")]);
    install.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_hide_wrap")]);
    out.extend(hide_wrap_maker());
    out.push(SET_MEMBER);
    // _global.MovieUtils.ShowMovie = thedas_show_wrap(_global.MovieUtils.ShowMovie).
    get_path(&mut install, &["_global", "MovieUtils"]);
    push(&mut install, &[Push::Str("ShowMovie")]);
    get_path(&mut install, &["_global", "MovieUtils", "ShowMovie"]);
    push(&mut install, &[Push::Int(1)]);
    get_path(&mut install, &["_global"]);
    push(&mut install, &[Push::Str("thedas_show_wrap")]);
    install.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_show_wrap")]);
    out.extend(show_wrap_maker());
    out.push(SET_MEMBER);
    // _global.thedas_after = new Array(); GUI.Element.RegisterPostLoadFunction wrapped (see
    // POST_LOAD_RUN).
    get_path(&mut out, &["_global"]);
    push(
        &mut out,
        &[Push::Str("thedas_after"), Push::Int(0), Push::Str("Array")],
    );
    out.extend_from_slice(&[NEW_OBJECT, SET_MEMBER]);
    // Mouse.getTopMostEntity, which the movies use as Scaleform's (see
    // `top_most_entity_function`).
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_top")]);
    out.extend(top_function());
    out.push(SET_MEMBER);
    get_path(&mut out, &["Mouse"]);
    push(&mut out, &[Push::Str("getTopMostEntity")]);
    out.extend(top_most_entity_function());
    out.push(SET_MEMBER);
    // Mouse listeners called as Scaleform called them (see `listener_proxy_function`):
    // those added from now on, and (here and again at INSTALL) those added before.
    for (name, f) in [
        ("thedas_listener", listener_proxy_function()),
        ("thedas_listener_wrap", listener_wrap_maker()),
        ("thedas_proxy_listeners", proxy_listeners_function()),
    ] {
        get_path(&mut out, &["_global"]);
        push(&mut out, &[Push::Str(name)]);
        out.extend(f);
        out.push(SET_MEMBER);
    }
    for m in ["addListener", "removeListener"] {
        get_path(&mut out, &["Mouse"]);
        push(&mut out, &[Push::Str(m)]);
        get_path(&mut out, &["Mouse", m]);
        push(&mut out, &[Push::Int(1)]);
        get_path(&mut out, &["_global"]);
        push(&mut out, &[Push::Str("thedas_listener_wrap")]);
        out.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    }
    for code in [&mut out, &mut install] {
        push(code, &[Push::Int(0)]);
        get_path(code, &["_global"]);
        push(code, &[Push::Str("thedas_proxy_listeners")]);
        code.extend_from_slice(&[CALL_METHOD, POP]);
    }
    for (name, f) in [
        ("thedas_s9_scales", scale9_scales_function()),
        ("thedas_s9_set", scale9_set_function()),
        (
            "thedas_slot_names",
            members_function("thedas_slots", &["thedas_image"]),
        ),
        ("thedas_slot_load", slot_load_function()),
    ] {
        get_path(&mut out, &["_global"]);
        push(&mut out, &[Push::Str(name)]);
        out.extend(f);
        out.push(SET_MEMBER);
    }
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_run_after")]);
    out.extend(run_after_function());
    out.push(SET_MEMBER);
    // Inline images in HTML text (see IMG_TAKE).
    for (name, f) in [
        ("thedas_text_wrap", process_text_wrap_maker()),
        ("thedas_img_take", img_take_function()),
        ("thedas_img_measure", img_measure_function()),
        ("thedas_img_set", img_set_function()),
        ("thedas_img_put", img_put_function()),
    ] {
        get_path(&mut out, &["_global"]);
        push(&mut out, &[Push::Str(name)]);
        out.extend(f);
        out.push(SET_MEMBER);
    }
    let text = ["_global", "SharedLibrary", "TextField", "prototype"];
    get_path(&mut install, &text);
    push(&mut install, &[Push::Str("ProcessText")]);
    get_path(
        &mut install,
        &[
            "_global",
            "SharedLibrary",
            "TextField",
            "prototype",
            "ProcessText",
        ],
    );
    push(&mut install, &[Push::Int(1)]);
    get_path(&mut install, &["_global"]);
    push(&mut install, &[Push::Str("thedas_text_wrap")]);
    install.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_post_load_wrap")]);
    out.extend(post_load_wrap_maker());
    out.push(SET_MEMBER);
    let element = ["_global", "GUI", "Element"];
    get_path(&mut install, &element);
    push(&mut install, &[Push::Str("RegisterPostLoadFunction")]);
    get_path(
        &mut install,
        &["_global", "GUI", "Element", "RegisterPostLoadFunction"],
    );
    push(&mut install, &[Push::Int(1)]);
    get_path(&mut install, &["_global"]);
    push(&mut install, &[Push::Str("thedas_post_load_wrap")]);
    install.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_flush")]);
    out.extend(flush_function());
    out.push(SET_MEMBER);
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_install")]);
    out.extend(function(&[], &install));
    out.push(SET_MEMBER);
    for (name, f) in [
        (INVOKE_CALLBACK, invoke_function()),
        (SET_CALLBACK, set_function()),
    ] {
        out.extend(f);
        push(
            &mut out,
            &[
                Push::Null,
                Push::Str(name),
                Push::Int(3),
                Push::Str("flash"),
            ],
        );
        out.push(GET_VARIABLE);
        push(&mut out, &[Push::Str("external")]);
        out.push(GET_MEMBER);
        push(&mut out, &[Push::Str("ExternalInterface")]);
        out.push(GET_MEMBER);
        push(&mut out, &[Push::Str("addCallback")]);
        out.extend_from_slice(&[CALL_METHOD, POP]);
    }
    out.push(0);
    out
}

/// `function (p, m, v) { eval(p)[m] = v; }`: how the engine sets movie variables
/// (Scaleform's `GFxMovie::SetVariable`).
fn set_function() -> Vec<u8> {
    let mut body = Vec::new();
    push(&mut body, &[Push::Str("p")]);
    body.extend_from_slice(&[GET_VARIABLE, GET_VARIABLE]);
    push(&mut body, &[Push::Str("m")]);
    body.push(GET_VARIABLE);
    push(&mut body, &[Push::Str("v")]);
    body.extend_from_slice(&[GET_VARIABLE, SET_MEMBER]);
    function(&["p", "m", "v"], &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_length_covers_body() {
        let f = invoke_function();
        assert_eq!(f[0], DEFINE_FUNCTION);
        let len = u16::from_le_bytes([f[1], f[2]]) as usize;
        let header = &f[3..3 + len];
        let code = u16::from_le_bytes([header[len - 2], header[len - 1]]) as usize;
        assert_eq!(3 + len + code, f.len());
        assert_eq!(*f.last().unwrap(), RETURN);
        assert_eq!(*register_invoke().last().unwrap(), 0);
    }
}
